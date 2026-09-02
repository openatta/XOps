//! RP-11 的验收。
//!
//! **全程对着 RP-07 的桩引擎跑**——那是 `EXE-014` 那条接缝的另一半：
//! 换成真实现之后，本包一行都不用改。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use xops_audit::AuditLog;
use xops_core::{Id, SystemClock, TableName, Timestamp};
use xops_dispatch::dispatch::Outcome;
use xops_dispatch::event::{Event, EventKind, Trigger, Whitelist};
use xops_dispatch::{Dispatcher, WorkspaceSource, looks_like_credential, provenance};
use xops_exec::{Behaviour, ExecContract, IsolationLevel, Runtime, StubEngine};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_skill::declaration::{Declaration, Input, InputType, OutputShape};
use xops_skill::{Ownership, SkillId, Skills};
use xops_store::{MemoryStore, Store, WriteEngine};
use xops_table::TableId;
use xops_task::policy::{OnComplete, Overlap, VersionPolicy};
use xops_task::task::Kind;
use xops_task::{DEFAULT_TOKEN_BUDGET, Task, TaskId, Tasks};

/// 一个数自己被问过几次的工作区来源。
struct CountingWorkspaces {
    calls: AtomicUsize,
}

/// 一份假的工作区。真的那份是析构即销毁的，所以这条缝交的是**要被攥住的东西**。
struct FakeHeld(std::path::PathBuf);

impl xops_dispatch::PreparedWorkspace for FakeHeld {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl WorkspaceSource for CountingWorkspaces {
    fn prepare(
        &self,
        _project: ProjectId,
        _revision: Option<&str>,
    ) -> xops_core::Result<Arc<dyn xops_dispatch::PreparedWorkspace>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(FakeHeld(std::path::PathBuf::from(
            "/tmp/xops-fake-workspace",
        ))))
    }
}

struct Fixture {
    dispatcher: Arc<Dispatcher>,
    tasks: Arc<Tasks>,
    skills: Arc<Skills>,
    directory: Arc<Directory>,
    engine: Arc<StubEngine>,
    exec: Arc<dyn ExecContract>,
    workspaces: Arc<CountingWorkspaces>,
}

fn fixture() -> Fixture {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let clock = Arc::new(SystemClock);
    let write = Arc::new(WriteEngine::new(Arc::clone(&store), clock.clone()));
    let relations: Arc<dyn xops_store::Relations> = Arc::new(xops_store::MemoryRelations::new());
    let mut audit = AuditLog::new(
        Arc::clone(&write),
        Arc::clone(&store),
        Arc::clone(&relations),
    )
    .unwrap();
    for table in xops_identity::directory::platform_tables().unwrap() {
        audit = audit.watching(table);
    }
    for table in [
        xops_skill::SKILLS_TABLE,
        xops_skill::VERSIONS_TABLE,
        xops_task::TASKS_TABLE,
    ] {
        audit = audit.watching(TableName::new(table).unwrap());
    }
    let audit = Arc::new(audit);
    let directory = Arc::new(Directory::new(
        Arc::clone(&write),
        Arc::clone(&store),
        Arc::clone(&audit),
        clock.clone(),
    ));
    let skills = Arc::new(Skills::new(
        Arc::clone(&write),
        Arc::clone(&store),
        Arc::clone(&audit),
        Arc::clone(&directory),
        clock.clone(),
    ));
    let tasks = Arc::new(
        Tasks::new(
            Arc::clone(&write),
            Arc::clone(&store),
            Arc::clone(&audit),
            Arc::clone(&directory),
            Arc::clone(&skills),
            clock.clone(),
        )
        .with_subscription_check(Arc::new(Whitelist)),
    );
    // `EXE-031`：派工单要带上"产出行往哪张表交"，所以分发层拿得到表目录。
    let catalog =
        Arc::new(xops_table::engine::Catalog::open(Arc::clone(&store), clock.clone()).unwrap());
    let tables = Arc::new(xops_table::Tables::new(
        Arc::clone(&write),
        catalog,
        Arc::clone(&audit),
        Arc::clone(&directory),
        clock.clone(),
        Arc::clone(&store),
    ));

    let engine = Arc::new(StubEngine::new());
    let exec: Arc<dyn ExecContract> = Arc::new(Runtime::new(
        Arc::clone(&engine) as Arc<dyn xops_exec::Engine>,
        clock.clone(),
        IsolationLevel::Bare,
    ));
    let workspaces = Arc::new(CountingWorkspaces {
        calls: AtomicUsize::new(0),
    });
    let dispatcher = Arc::new(
        Dispatcher::new(
            Arc::clone(&tasks),
            Arc::clone(&skills),
            Arc::clone(&exec),
            Arc::clone(&audit),
            Arc::clone(&store),
            clock,
            Arc::clone(&tables),
        )
        .with_workspaces(Arc::clone(&workspaces) as Arc<dyn WorkspaceSource>),
    );
    Fixture {
        dispatcher,
        tasks,
        skills,
        directory,
        engine,
        exec,
        workspaces,
    }
}

impl Fixture {
    fn user(&self, account: &str) -> UserId {
        self.directory
            .provision(
                ExternalAccount {
                    provider: ProviderId::new("builtin").unwrap(),
                    account: account.into(),
                },
                account,
                None,
            )
            .unwrap()
            .id
    }

    fn skill(&self, actor: UserId, project: ProjectId, needs_repository: bool) -> SkillId {
        let declaration = Declaration {
            inputs: vec![Input {
                name: "target".into(),
                ty: InputType::Text,
                required: true,
                description: "看哪儿".into(),
            }],
            output: OutputShape::Report,
            needs_repository,
            network: vec![],
            max_duration_millis: 5_000,
        };
        let resolved = self
            .skills
            .create(
                actor,
                project,
                "查缺陷",
                Ownership::Public,
                "看看有没有崩的",
                declaration,
            )
            .unwrap();
        self.skills
            .record_successful_test(actor, resolved.skill.id, 1, Id::generate())
            .unwrap();
        self.skills.publish(actor, resolved.skill.id, 1).unwrap();
        resolved.skill.id
    }

    fn task(&self, actor: UserId, project: ProjectId, skill: SkillId) -> Task {
        Task {
            id: TaskId::generate(),
            project,
            name: "查一遍".into(),
            ownership: Ownership::Public,
            kind: Kind::Normal,
            skill,
            version_policy: VersionPolicy::Pinned { version: 1 },
            inputs: json!({"target": "src/"}),
            writes: vec![TableId::user("bugs").unwrap()],
            subscriptions: vec![],
            token_budget: DEFAULT_TOKEN_BUDGET,
            overlap: Overlap::default(),
            on_complete: OnComplete::None,
            enabled: true,
            created_by: actor,
            created_at: Timestamp::from_millis(0),
        }
    }

    fn manual(&self, project: ProjectId, who: UserId) -> Event {
        Event {
            kind: EventKind::Manual,
            project,
            external_id: None,
            triggered_by: Trigger::Person { user: who },
            revision: None,
            at: Timestamp::from_millis(0),
            payload: json!({}),
        }
    }
}

fn setup(fixture: &Fixture, needs_repository: bool) -> (UserId, ProjectId, Task) {
    let alice = fixture.user("alice");
    let project = fixture
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap()
        .id;
    let skill = fixture.skill(alice, project, needs_repository);
    let task = fixture
        .tasks
        .create(alice, fixture.task(alice, project, skill))
        .unwrap();
    (alice, project, task)
}

fn wait_done(fixture: &Fixture, run: &str) {
    let id = xops_exec::worksheet::RunId::from_id(Id::parse(run).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.exec.status(id).unwrap().finished() {
        assert!(Instant::now() < deadline, "执行没收摊");
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ——————————————————————————————— 事件白名单 ———————————————————————————————

/// 等一个条件成立，最多等两秒。
///
/// 用在"提交是非阻塞的"那几条上：被测的性质是**结果**，
/// 而结果由另一个线程写下——不等就是在测调度器。
fn wait_until(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    ready()
}

#[test]
fn 订阅某张表被写入会在创建时被拒() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);
    let mut subscriber = fixture.task(alice, project, task.skill);
    subscriber.subscriptions.push("table-written".into());
    let error = fixture.tasks.create(alice, subscriber).unwrap_err();
    assert!(
        error.message().contains("不受深度限制的回路"),
        "TRG-004：{}",
        error.message()
    );
}

#[test]
fn 后两类不能自己声明订阅() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);
    for kind in ["flow-node-activated", "upstream-task-completed"] {
        let mut subscriber = fixture.task(alice, project, task.skill);
        subscriber.subscriptions.push(kind.into());
        let error = fixture.tasks.create(alice, subscriber).unwrap_err();
        assert!(
            error.message().contains("唯一订阅途径"),
            "TRG-003：{kind} —— {}",
            error.message()
        );
    }
}

#[test]
fn 分发层没有第六种事件源() {
    assert_eq!(EventKind::all().len(), 5, "TRG-001");
    let source = include_str!("../src/event.rs");
    let production = source.split("#[cfg(test)]").next().unwrap_or_default();
    for forbidden in ["TableWritten", "RowInserted", "TableChanged"] {
        assert!(
            !production.contains(forbidden),
            "白名单里出现了 {forbidden}"
        );
    }
}

// ——————————————————————————————— 三条共同纪律 ———————————————————————————————

#[test]
fn 触发非阻塞() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);
    fixture.engine.behaves(Behaviour::Hang);

    let started = Instant::now();
    let record = fixture
        .dispatcher
        .trigger(&task, &fixture.manual(project, alice))
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "TRG-007：非阻塞"
    );
    let Outcome::Accepted { run } = record.outcome else {
        panic!("该被接受")
    };
    // 触发的产出是"进了队列"，不是"跑完了"。
    let id = xops_exec::worksheet::RunId::from_id(Id::parse(&run).unwrap());
    assert_eq!(fixture.exec.status(id).unwrap(), xops_exec::Status::Running);
    fixture.exec.cancel(id).unwrap();
}

#[test]
fn 同一个外部事件不产生第二次执行() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);
    let mut event = fixture.manual(project, alice);
    event.external_id = Some("delivery-1".into());

    let first = fixture.dispatcher.trigger(&task, &event).unwrap();
    let second = fixture.dispatcher.trigger(&task, &event).unwrap();
    let (Outcome::Accepted { run: first }, Outcome::Duplicate { run: second }) =
        (first.outcome, second.outcome)
    else {
        panic!("第二次该是重复，不是第二次执行");
    };
    assert_eq!(first, second, "TRG-013：返回同一次执行");
    // ⚠️ 提交是非阻塞的（`TRG-007`）：引擎在另一个线程上被调到。
    // 直接断言 `seen()` 是一个竞态——它挂在"那个线程还没跑起来"上，
    // 而不是挂在被测的性质上。**等它到 1，再断言不会变成 2。**
    assert!(
        wait_until(|| !fixture.engine.seen().is_empty()),
        "引擎该被调到一次"
    );
    assert_eq!(fixture.engine.seen().len(), 1, "引擎只被调过一次");
}

#[test]
fn 被拒绝与被跳过的触发都查得到() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);

    // ① 被跳过：让上一次挂着，重叠策略默认跳过。
    fixture.engine.behaves(Behaviour::Hang);
    let first = fixture
        .dispatcher
        .trigger(&task, &fixture.manual(project, alice))
        .unwrap();
    let skipped = fixture
        .dispatcher
        .trigger(&task, &fixture.manual(project, alice))
        .unwrap();
    assert!(
        matches!(skipped.outcome, Outcome::Skipped { .. }),
        "{:?}",
        skipped.outcome
    );

    // ② 被拒绝：停用之后再触发。
    if let Outcome::Accepted { run } = &first.outcome {
        fixture
            .exec
            .cancel(xops_exec::worksheet::RunId::from_id(
                Id::parse(run).unwrap(),
            ))
            .unwrap();
    }
    let disabled = fixture.tasks.set_enabled(alice, task.id, false).unwrap();
    let rejected = fixture
        .dispatcher
        .trigger(&disabled, &fixture.manual(project, alice))
        .unwrap();
    assert!(matches!(rejected.outcome, Outcome::Rejected { .. }));

    // 两条都在触发历史里。**一个静默被跳过的任务，会让人以为它在跑。**
    let history = fixture.dispatcher.trigger_history(task.id).unwrap();
    assert!(
        history
            .iter()
            .any(|record| matches!(record.outcome, Outcome::Skipped { .. }))
    );
    assert!(
        history
            .iter()
            .any(|record| matches!(record.outcome, Outcome::Rejected { .. }))
    );
}

#[test]
fn 触发前三项检查都拒绝并留痕() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);

    // 已停用。
    let disabled = fixture.tasks.set_enabled(alice, task.id, false).unwrap();
    let record = fixture
        .dispatcher
        .trigger(&disabled, &fixture.manual(project, alice))
        .unwrap();
    assert!(matches!(record.outcome, Outcome::Rejected { .. }));

    // 不允许这种触发方式：没订阅 git，却来了一个 git 事件。
    let enabled = fixture.tasks.set_enabled(alice, task.id, true).unwrap();
    let mut git = fixture.manual(project, alice);
    git.kind = EventKind::Git;
    let record = fixture.dispatcher.trigger(&enabled, &git).unwrap();
    assert!(
        matches!(record.outcome, Outcome::Rejected { .. }),
        "TRG-008"
    );

    assert_eq!(
        fixture.dispatcher.trigger_history(task.id).unwrap().len(),
        2,
        "两次都留痕"
    );
}

// ——————————————————————————————— 派工单 ———————————————————————————————

#[test]
fn 派工单的每个字段都对得上某条声明() {
    // **不扩权**：字段多了一个而对照表没加一行，这条就红。
    let fields = provenance();
    assert_eq!(fields.len(), 8, "TSK-015");
    let names: Vec<&str> = fields.iter().map(|(name, _)| *name).collect();
    for expected in [
        "run",
        "instruction",
        "skill",
        "skill_version",
        "inputs",
        "revision",
        "capabilities",
        "limits",
    ] {
        assert!(names.contains(&expected), "少了 {expected}");
    }
}

#[test]
fn 不需要代码仓的技能连工作区都不给() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);
    let record = fixture
        .dispatcher
        .trigger(&task, &fixture.manual(project, alice))
        .unwrap();
    assert!(matches!(record.outcome, Outcome::Accepted { .. }));
    assert_eq!(
        fixture.workspaces.calls.load(Ordering::SeqCst),
        0,
        "EXE-006 / I-I：未声明的一律不提供 —— 连问都不该问一次"
    );
}

#[test]
fn 声明了要读仓才会去备工作区() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, true);
    fixture
        .dispatcher
        .trigger(&task, &fixture.manual(project, alice))
        .unwrap();
    assert_eq!(fixture.workspaces.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn 事件带来的修订覆盖任务定义里的那个() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);
    let mut event = fixture.manual(project, alice);
    event.revision = Some("abc123".into());

    let version = fixture.skills.versions(task.skill).unwrap().remove(0);
    let worksheet = xops_dispatch::assemble(&task, &version, &event, None, None).unwrap();
    assert_eq!(worksheet.revision.as_deref(), Some("abc123"));
}

#[test]
fn 派工单里没有凭据也没有表数据() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);
    let version = fixture.skills.versions(task.skill).unwrap().remove(0);
    let worksheet =
        xops_dispatch::assemble(&task, &version, &fixture.manual(project, alice), None, None)
            .unwrap();

    assert_eq!(
        looks_like_credential(&worksheet),
        None,
        "I-F：派工单不含任何凭据"
    );
    let rendered = serde_json::to_string(&worksheet).unwrap();
    // EXE-013 / D44：表不是数据源 —— 派工单里没有表快照，也没有到 XOps 的网络路径。
    for forbidden in ["_runs", "table", "http://", "https://", "/mcp"] {
        assert!(
            !rendered.contains(forbidden),
            "派工单里出现了 {forbidden}：{rendered}"
        );
    }
}

#[test]
fn 触发不允许覆盖输入参数() {
    // TRG-020 是双重的：手动触发 tool 的 schema 里**根本没有那个字段**，
    // 而未声明字段一律被 MCP-003 挡在更外面。这里验的是前一半。
    let source = include_str!("../src/tools.rs");
    let production = source.split("#[cfg(test)]").next().unwrap_or_default();
    let trigger_tool = production
        .split("run.trigger")
        .nth(1)
        .and_then(|rest| rest.split("build()?").next())
        .unwrap_or_default();
    for forbidden in ["\"inputs\"", "\"arguments\"", "\"params\""] {
        assert!(
            !trigger_tool.contains(forbidden),
            "手动触发的 schema 里出现了 {forbidden} —— 触发不允许覆盖输入参数"
        );
    }
}

// ——————————————————————————————— 与执行契约的接缝 ———————————————————————————————

#[test]
fn 全程对着桩引擎跑得通() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);
    let record = fixture
        .dispatcher
        .trigger(&task, &fixture.manual(project, alice))
        .unwrap();
    let Outcome::Accepted { run } = record.outcome else {
        panic!("该被接受")
    };
    wait_done(&fixture, &run);

    let outcome = fixture
        .exec
        .collect(xops_exec::worksheet::RunId::from_id(
            Id::parse(&run).unwrap(),
        ))
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, xops_exec::Status::Succeeded);
    assert_eq!(fixture.engine.seen().len(), 1);
}

#[test]
fn 重叠策略重跑会先取消上一次() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);
    fixture.engine.behaves(Behaviour::Hang);
    let mut restart = task.clone();
    restart.overlap = Overlap::Restart;
    let restart = fixture.tasks.update(alice, restart).unwrap();

    let first = fixture
        .dispatcher
        .trigger(&restart, &fixture.manual(project, alice))
        .unwrap();
    let Outcome::Accepted { run: first_run } = first.outcome else {
        panic!()
    };
    let second = fixture
        .dispatcher
        .trigger(&restart, &fixture.manual(project, alice))
        .unwrap();
    assert!(
        matches!(second.outcome, Outcome::Accepted { .. }),
        "重跑：这一次照常提交"
    );

    // 上一次被取消了。
    let id = xops_exec::worksheet::RunId::from_id(Id::parse(&first_run).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.exec.status(id).unwrap().finished() {
        assert!(Instant::now() < deadline, "上一次没被取消");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn 停用的技能让触发失败而不是静默跑一个旧版本() {
    let fixture = fixture();
    let (alice, project, task) = setup(&fixture, false);
    fixture.skills.disable(alice, task.skill, 1).unwrap();
    assert!(
        fixture
            .dispatcher
            .trigger(&task, &fixture.manual(project, alice))
            .is_err(),
        "解不出版本就该失败"
    );
}
