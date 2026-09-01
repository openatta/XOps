//! RP-10 的验收。

use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use xops_audit::AuditLog;
use xops_core::{Id, SystemClock, TableName, Timestamp};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_skill::declaration::{Declaration, Input, InputType, OutputShape};
use xops_skill::{Ownership, SkillId, Skills};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};
use xops_table::TableId;
use xops_task::policy::{OnComplete, Overlap, VersionPolicy};
use xops_task::task::Kind;
use xops_task::{DEFAULT_TOKEN_BUDGET, Task, TaskId, Tasks, TerminationStep};

struct Fixture {
    label: &'static str,
    tasks: Arc<Tasks>,
    skills: Arc<Skills>,
    directory: Arc<Directory>,
}

fn fixtures() -> Vec<Fixture> {
    [
        ("memory", Arc::new(MemoryStore::new()) as Arc<dyn Store>),
        ("sqlite", Arc::new(SqliteStore::in_memory().unwrap())),
    ]
    .into_iter()
    .map(|(label, store)| {
        let clock = Arc::new(SystemClock);
        let engine = Arc::new(WriteEngine::new(Arc::clone(&store), clock.clone()));
        let relations: Arc<dyn xops_store::Relations> =
            Arc::new(xops_store::MemoryRelations::new());
        let mut audit = AuditLog::new(
            Arc::clone(&engine),
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
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            clock.clone(),
        ));
        let skills = Arc::new(Skills::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            Arc::clone(&directory),
            clock.clone(),
        ));
        let tasks = Arc::new(Tasks::new(
            engine,
            store,
            audit,
            Arc::clone(&directory),
            Arc::clone(&skills),
            clock,
        ));
        Fixture {
            label,
            tasks,
            skills,
            directory,
        }
    })
    .collect()
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

    /// 建一个已发布的技能，要一个必填参数 `target`。
    fn published_skill(&self, actor: UserId, project: ProjectId) -> SkillId {
        let declaration = Declaration {
            inputs: vec![Input {
                name: "target".into(),
                ty: InputType::Text,
                required: true,
                description: "看哪儿".into(),
            }],
            output: OutputShape::Report,
            needs_repository: false,
            network: vec![],
            max_duration_millis: 60_000,
        };
        let resolved = self
            .skills
            .create(
                actor,
                project,
                "查缺陷",
                Ownership::Public,
                "看看",
                declaration,
            )
            .unwrap();
        self.skills
            .record_successful_test(actor, resolved.skill.id, 1, Id::generate())
            .unwrap();
        self.skills.publish(actor, resolved.skill.id, 1).unwrap();
        resolved.skill.id
    }

    fn task(&self, project: ProjectId, skill: SkillId, actor: UserId) -> Task {
        Task {
            id: TaskId::generate(),
            project,
            name: "每天查一遍".into(),
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
}

fn setup(fixture: &Fixture) -> (UserId, ProjectId, SkillId) {
    let alice = fixture.user("alice");
    let project = fixture
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap()
        .id;
    let skill = fixture.published_skill(alice, project);
    (alice, project, skill)
}

// ——————————————————————————————— 引用技能 ———————————————————————————————

#[test]
fn 引用草稿技能被拒绝() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, _) = setup(&fixture);
        // 另建一个没发布的。
        let draft = fixture
            .skills
            .create(
                alice,
                project,
                "草稿",
                Ownership::Public,
                "还没好",
                Declaration {
                    inputs: vec![],
                    output: OutputShape::Report,
                    needs_repository: false,
                    network: vec![],
                    max_duration_millis: 1_000,
                },
            )
            .unwrap();
        let mut task = fixture.task(project, draft.skill.id, alice);
        task.inputs = json!({});
        let error = fixture.tasks.create(alice, task).unwrap_err();
        assert!(
            error.message().contains("已发布"),
            "{label}：{}",
            error.message()
        );
    }
}

#[test]
fn 版本策略默认钉死跟随最新必须明确选() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, skill) = setup(&fixture);
        let task = fixture.task(project, skill, alice);
        assert!(
            matches!(task.version_policy, VersionPolicy::Pinned { .. }),
            "{label}：默认钉死"
        );
        let created = fixture.tasks.create(alice, task).unwrap();
        assert!(
            matches!(created.version_policy, VersionPolicy::Pinned { version: 1 }),
            "{label}"
        );
    }
}

#[test]
fn 输入不满足契约时指明缺哪个参数() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, skill) = setup(&fixture);
        let mut task = fixture.task(project, skill, alice);
        task.inputs = json!({});
        let error = fixture.tasks.create(alice, task).unwrap_err();
        assert!(
            error.message().contains("target"),
            "{label}：TSK-003 要指明缺哪个参数 —— {}",
            error.message()
        );
    }
}

#[test]
fn 多给一个没声明的参数也不收() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, skill) = setup(&fixture);
        let mut task = fixture.task(project, skill, alice);
        task.inputs = json!({"target": "src/", "别的": 1});
        assert!(fixture.tasks.create(alice, task).is_err(), "{label}");
    }
}

// ——————————————————————————————— 策略 ———————————————————————————————

#[test]
fn 未声明的表写不了() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, skill) = setup(&fixture);
        let created = fixture
            .tasks
            .create(alice, fixture.task(project, skill, alice))
            .unwrap();
        assert!(
            created.may_write(&TableId::user("bugs").unwrap()),
            "{label}"
        );
        assert!(
            !created.may_write(&TableId::user("issues").unwrap()),
            "{label}：TSK-004"
        );
    }
}

#[test]
fn 重叠策略默认跳过且三选一() {
    assert_eq!(Overlap::default(), Overlap::Skip);
    let all = [Overlap::Skip, Overlap::Queue, Overlap::Restart];
    assert_eq!(all.len(), 3, "TSK-008：三选一");
}

#[test]
fn 停用之后连手动都不响应而且没有删除这条路() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, skill) = setup(&fixture);
        let created = fixture
            .tasks
            .create(alice, fixture.task(project, skill, alice))
            .unwrap();
        let disabled = fixture.tasks.set_enabled(alice, created.id, false).unwrap();
        assert!(!disabled.responds_to_triggers(), "{label}");
        // 停用之后仍然读得到 —— 执行记录不能因为任务没了就丢。
        assert!(
            fixture.tasks.read(alice, created.id).is_ok(),
            "{label}：不提供删除"
        );
    }
}

#[test]
fn 不存在删除任务这条路() {
    // 枚举，不是靠读。
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service.rs")).unwrap();
    let code: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("pub fn delete") && !code.contains("WriteOp::Delete"),
        "TSK-009：不提供删除——执行记录不能因为任务没了就丢"
    );
}

#[test]
fn 项目级额度与限流不存在() {
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service.rs")).unwrap();
    let code = source.to_ascii_lowercase();
    for forbidden in ["quota", "rate_limit", "ratelimit", "project_budget"] {
        assert!(
            !code.contains(forbidden),
            "TSK-007：项目级额度与限流不做，出现了 {forbidden}"
        );
    }
}

// ——————————————————————————————— onComplete 深度 1 ———————————————————————————————

#[test]
fn oncomplete深度硬限制一() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, skill) = setup(&fixture);
        // 第三层的那个：自己没挂东西。
        let leaf = fixture
            .tasks
            .create(alice, fixture.task(project, skill, alice))
            .unwrap();
        // 第二层：挂着 leaf。
        let mut middle = fixture.task(project, skill, alice);
        middle.on_complete = OnComplete::Task { task: leaf.id };
        let middle = fixture.tasks.create(alice, middle).unwrap();

        // 想在 leaf 上再挂一个 —— 它已经被 middle 挂着了。
        let mut leaf_with_hook = leaf.clone();
        leaf_with_hook.on_complete = OnComplete::Plugin {
            plugin: "gate".into(),
        };
        let error = fixture.tasks.update(alice, leaf_with_hook).unwrap_err();
        assert!(
            error.message().contains("已经被别人挂在 onComplete 上"),
            "{label}：被挂着的任务自己不能再挂 —— {}",
            error.message()
        );

        // 想挂一个自己已经挂了东西的任务 —— 同样不行。
        let mut third = fixture.task(project, skill, alice);
        third.on_complete = OnComplete::Task { task: middle.id };
        let error = fixture.tasks.create(alice, third).unwrap_err();
        assert!(
            error.message().contains("必须为空"),
            "{label}：一层是输出后处理，两层就是任务编排 DAG —— {}",
            error.message()
        );
    }
}

#[test]
fn 不能把自己挂在自己身上() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, skill) = setup(&fixture);
        let created = fixture
            .tasks
            .create(alice, fixture.task(project, skill, alice))
            .unwrap();
        let mut looped = created.clone();
        looped.on_complete = OnComplete::Task { task: created.id };
        assert!(fixture.tasks.update(alice, looped).is_err(), "{label}");
    }
}

// ——————————————————————————————— 造插件任务 ———————————————————————————————

#[test]
fn 造插件任务订阅事件会被拒() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, skill) = setup(&fixture);
        let mut builder = fixture.task(project, skill, alice);
        builder.kind = Kind::PluginBuilder;
        builder.subscriptions.push("git.push".into());
        let error = fixture.tasks.create(alice, builder).unwrap_err();
        assert!(error.message().contains("只能手动触发"), "{label}");
    }
}

// ——————————————————————————————— 终止时序 ———————————————————————————————

#[test]
fn 终止时序四步且runs先于产出行() {
    let order = TerminationStep::order();
    let names: Vec<&str> = order.iter().map(|step| step.why()).collect();
    assert_eq!(names.len(), 4, "TSK-006");
    assert_eq!(
        order[0],
        TerminationStep::AbortModelAndSession,
        "先停掉烧钱的那一头"
    );
    let runs_position = order
        .iter()
        .position(|step| *step == TerminationStep::WriteRunsThenRows);
    let destroy_position = order
        .iter()
        .position(|step| *step == TerminationStep::DestroySandbox);
    assert!(runs_position < destroy_position, "先落账再收摊");
}

// ——————————————————————————————— 可见性 ———————————————————————————————

#[test]
fn 别人的私有任务看不到() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, skill) = setup(&fixture);
        let bob = fixture.user("bob");
        fixture
            .directory
            .set_member(alice, project, bob, xops_core::Role::Member)
            .unwrap();

        let mut private = fixture.task(project, skill, alice);
        private.ownership = Ownership::Private { owner: alice };
        let private = fixture.tasks.create(alice, private).unwrap();

        assert!(fixture.tasks.read(alice, private.id).is_ok(), "{label}");
        let error = fixture.tasks.read(bob, private.id).unwrap_err();
        let missing = fixture.tasks.read(bob, TaskId::generate()).unwrap_err();
        assert_eq!(error.message(), missing.message(), "{label}：与不存在一致");
    }
}

#[test]
fn 技能停用之后任务解不出版本() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, skill) = setup(&fixture);
        let created = fixture
            .tasks
            .create(alice, fixture.task(project, skill, alice))
            .unwrap();
        assert_eq!(
            fixture.tasks.resolve_skill_version(&created).unwrap(),
            1,
            "{label}"
        );

        fixture.skills.disable(alice, skill, 1).unwrap();
        assert!(
            fixture.tasks.resolve_skill_version(&created).is_err(),
            "{label}：停用的技能版本跑不了"
        );
    }
}

#[test]
fn 订阅者列表只含启用着的() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, skill) = setup(&fixture);
        let mut subscriber = fixture.task(project, skill, alice);
        subscriber.subscriptions.push("git.push".into());
        let subscriber = fixture.tasks.create(alice, subscriber).unwrap();

        assert_eq!(
            fixture
                .tasks
                .subscribers(project, "git.push")
                .unwrap()
                .len(),
            1,
            "{label}"
        );
        fixture
            .tasks
            .set_enabled(alice, subscriber.id, false)
            .unwrap();
        assert!(
            fixture
                .tasks
                .subscribers(project, "git.push")
                .unwrap()
                .is_empty(),
            "{label}：停用的不响应任何触发"
        );
    }
}
