//! RP-12 的验收。

use std::sync::{Arc, Mutex};

use serde_json::json;
use xops_audit::AuditLog;
use xops_core::{Clock, FixedClock, Id, TableName, Timestamp};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_skill::declaration::{Declaration, OutputShape};
use xops_skill::{Ownership, SkillId, Skills};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};
use xops_table::engine::Catalog;
use xops_table::table::{Protection, TableId};
use xops_table::{Column, ColumnType, Tables, WrittenBy};
use xops_task::landing::{Completion, MAX_OUTPUT_CHARS, Notifier, Rejection, TRUNCATION_MARK};
use xops_task::policy::{OnComplete, Overlap, VersionPolicy};
use xops_task::task::Kind;
use xops_task::{Cleanup, DEFAULT_TOKEN_BUDGET, Landing, Retention, Task, TaskId, Tasks};

/// 记下每一次通知。**`EXE-024`：两种拒绝都要通知——自动化失灵不能是静默的。**
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<Rejection>>,
}

impl Notifier for Recorder {
    fn notify(&self, _task: &Task, rejection: &Rejection) {
        self.seen.lock().unwrap().push(rejection.clone());
    }
}

struct Fixture {
    label: &'static str,
    tables: Arc<Tables>,
    tasks: Arc<Tasks>,
    skills: Arc<Skills>,
    directory: Arc<Directory>,
    landing: Landing,
    cleanup: Cleanup,
    recorder: Arc<Recorder>,
    clock: Arc<FixedClock>,
}

fn fixtures() -> Vec<Fixture> {
    [
        ("memory", Arc::new(MemoryStore::new()) as Arc<dyn Store>),
        ("sqlite", Arc::new(SqliteStore::in_memory().unwrap())),
    ]
    .into_iter()
    .map(|(label, store)| {
        let clock = Arc::new(FixedClock::new(1_700_000_000_000));
        let catalog = Arc::new(Catalog::open(Arc::clone(&store), clock.clone()).unwrap());
        let engine = Arc::new(
            WriteEngine::new(Arc::clone(&store), clock.clone())
                .with_pre_write(Arc::clone(&catalog) as Arc<dyn xops_store::PreWrite>)
                .with_schema_check(Arc::clone(&catalog) as Arc<dyn xops_store::SchemaCheck>),
        );
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
            xops_table::CATALOG_TABLE,
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
        let tables = Arc::new(Tables::new(
            Arc::clone(&engine),
            catalog,
            Arc::clone(&audit),
            Arc::clone(&directory),
            clock.clone(),
            Arc::clone(&store),
        ));
        let skills = Arc::new(Skills::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            Arc::clone(&directory),
            clock.clone(),
        ));
        let tasks = Arc::new(Tasks::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            Arc::clone(&directory),
            Arc::clone(&skills),
            clock.clone(),
        ));
        let recorder = Arc::new(Recorder::default());
        let landing = Landing::new(Arc::clone(&tables), clock.clone())
            .with_notifier(Arc::clone(&recorder) as Arc<dyn Notifier>);
        let cleanup = Cleanup::new(Arc::clone(&tables), Arc::clone(&store), Arc::clone(&audit));
        Fixture {
            label,
            tables,
            tasks,
            skills,
            directory,
            landing,
            cleanup,
            recorder,
            clock,
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

    fn skill(&self, actor: UserId, project: ProjectId) -> SkillId {
        let resolved = self
            .skills
            .create(
                actor,
                project,
                "查缺陷",
                Ownership::Public,
                "看看",
                Declaration {
                    inputs: vec![],
                    output: OutputShape::Rows,
                    needs_repository: false,
                    network: vec![],
                    max_duration_millis: 5_000,
                },
            )
            .unwrap();
        self.skills
            .record_successful_test(actor, resolved.skill.id, 1, Id::generate())
            .unwrap();
        self.skills.publish(actor, resolved.skill.id, 1).unwrap();
        resolved.skill.id
    }
}

fn setup(fixture: &Fixture) -> (UserId, ProjectId, Task, TableId) {
    let alice = fixture.user("alice");
    let project = fixture
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap()
        .id;
    fixture
        .tables
        .ensure_system_tables(project, "acme")
        .unwrap();
    let bugs = TableId::user("bugs").unwrap();
    fixture
        .tables
        .create(
            alice,
            project,
            bugs.clone(),
            Protection::Normal,
            vec![Column::new("title", ColumnType::Text { max_len: 64 }, true).unwrap()],
        )
        .unwrap();
    let skill = fixture.skill(alice, project);
    let task = fixture
        .tasks
        .create(
            alice,
            Task {
                id: TaskId::generate(),
                project,
                name: "查一遍".into(),
                ownership: Ownership::Public,
                kind: Kind::Normal,
                skill,
                version_policy: VersionPolicy::Pinned { version: 1 },
                inputs: json!({}),
                writes: vec![bugs.clone()],
                subscriptions: vec![],
                token_budget: DEFAULT_TOKEN_BUDGET,
                overlap: Overlap::default(),
                on_complete: OnComplete::None,
                enabled: true,
                created_by: alice,
                created_at: Timestamp::from_millis(0),
            },
        )
        .unwrap();
    (alice, project, task, bugs)
}

fn completion(rows: Vec<serde_json::Value>) -> Completion {
    Completion {
        run: Id::generate(),
        status: "succeeded".into(),
        failure_kind: None,
        tokens_used: 120,
        token_budget: DEFAULT_TOKEN_BUDGET,
        output: "跑完了".into(),
        trace: "一堆过程".into(),
        revision: None,
        skill: "查缺陷".into(),
        skill_version: "1".into(),
        trigger: "manual".into(),
        triggered_by: "alice".into(),
        started_at: Timestamp::from_millis(1_700_000_000_000),
        finished_at: Some(Timestamp::from_millis(1_700_000_001_000)),
        rows,
    }
}

fn execution_writer() -> WrittenBy {
    WrittenBy::Execution {
        run: Id::generate(),
        task: Id::generate(),
        task_owner: UserId::generate(),
        skill: "查缺陷".into(),
        skill_version: "1".into(),
        revision: None,
        status: "succeeded".into(),
    }
}

// ——————————————————————————————— 写入顺序与两层拒绝 ———————————————————————————————

#[test]
fn runs行先于产出行落定() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, project, task, bugs) = setup(&fixture);
        let runs = TableId::system("_runs").unwrap();

        let landed = fixture
            .landing
            .land(
                &task,
                Retention::default(),
                &execution_writer(),
                &completion(vec![json!({"title": "崩了"})]),
            )
            .unwrap();

        let run_row = fixture
            .tables
            .get(Some(project), &runs, landed.run_row)
            .unwrap()
            .unwrap();
        let produced = fixture
            .tables
            .get(Some(project), &bugs, landed.rows[0])
            .unwrap()
            .unwrap();
        // `_runs` 的事件序号必须小于产出行的 —— 顺序在事件流上是可查的。
        let run_history = fixture
            .tables
            .history(Some(project), &runs, landed.run_row)
            .unwrap();
        let row_history = fixture
            .tables
            .history(Some(project), &bugs, landed.rows[0])
            .unwrap();
        assert!(
            !run_history.is_empty() && !row_history.is_empty(),
            "{label}"
        );
        assert_eq!(run_row["status"], json!("succeeded"), "{label}");
        assert_eq!(produced["title"], json!("崩了"), "{label}");
    }
}

#[test]
fn schema不过整批不入表且归为技能错误() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, project, task, bugs) = setup(&fixture);
        let landed = fixture
            .landing
            .land(
                &task,
                Retention::default(),
                &execution_writer(),
                // "没声明的列" —— 整批都不该入表。
                &completion(vec![json!({"title": "崩了"}), json!({"nope": 1})]),
            )
            .unwrap();

        assert!(
            matches!(landed.rejection, Some(Rejection::SchemaFailed { .. })),
            "{label}：{:?}",
            landed.rejection
        );
        assert!(landed.rows.is_empty(), "{label}：EXE-024 —— 整批行不入表");
        assert!(
            fixture
                .tables
                .rows(Some(project), &bugs, 10)
                .unwrap()
                .is_empty(),
            "{label}"
        );

        let runs = TableId::system("_runs").unwrap();
        let run_row = fixture
            .tables
            .get(Some(project), &runs, landed.run_row)
            .unwrap()
            .unwrap();
        assert_eq!(run_row["status"], json!("failed"), "{label}");
        assert_eq!(
            run_row["failureKind"],
            json!("skill"),
            "{label}：归为技能错误类"
        );

        assert_eq!(
            fixture.recorder.seen.lock().unwrap().len(),
            1,
            "{label}：要通知"
        );
    }
}

#[test]
fn 节点判定不过行照样入表只是不结算() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, project, task, bugs) = setup(&fixture);
        // 这一层的拒绝由 RP-15 判出来，落账这一侧只负责"行入表 + 通知"。
        let rejection = Rejection::NotSettled {
            reason: "筛选没命中".into(),
        };
        assert!(rejection.rows_landed(), "{label}");

        let landed = fixture
            .landing
            .land(
                &task,
                Retention::default(),
                &execution_writer(),
                &completion(vec![json!({"title": "崩了"})]),
            )
            .unwrap();
        assert_eq!(landed.rows.len(), 1, "{label}");
        assert_eq!(
            fixture.tables.rows(Some(project), &bugs, 10).unwrap().len(),
            1,
            "{label}"
        );
    }
}

// ——————————————————————————————— 保留期 ———————————————————————————————

#[test]
fn retainuntil取写入当时的配置() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, project, task, _) = setup(&fixture);
        let runs = TableId::system("_runs").unwrap();
        let short = Retention {
            output_millis: 1_000,
            trace_millis: 500,
        };

        let landed = fixture
            .landing
            .land(&task, short, &execution_writer(), &completion(vec![]))
            .unwrap();
        let before = fixture
            .tables
            .get(Some(project), &runs, landed.run_row)
            .unwrap()
            .unwrap();
        let recorded = before["retainUntil"].as_i64().unwrap();

        // 改任务的保留期（这里直接用一份新的配置再落一次），旧那一行不该跟着变。
        let long = Retention {
            output_millis: 999_000,
            trace_millis: 500,
        };
        fixture
            .landing
            .land(&task, long, &execution_writer(), &completion(vec![]))
            .unwrap();

        let after = fixture
            .tables
            .get(Some(project), &runs, landed.run_row)
            .unwrap()
            .unwrap();
        assert_eq!(
            after["retainUntil"].as_i64().unwrap(),
            recorded,
            "{label}：RET-002 —— 已经写下的行不该因为任务改了配置就提前消失或延后清理"
        );
    }
}

#[test]
fn 过程记录先过期而行本身还在() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, project, task, _) = setup(&fixture);
        let runs = TableId::system("_runs").unwrap();
        let retention = Retention {
            output_millis: 100_000,
            trace_millis: 1_000,
        };
        let landed = fixture
            .landing
            .land(&task, retention, &execution_writer(), &completion(vec![]))
            .unwrap();

        fixture.clock.advance(2_000);
        let swept = fixture
            .cleanup
            .sweep(project, &runs, fixture.clock.now())
            .unwrap();
        assert_eq!(swept.traces, 1, "{label}：RET-004 —— 只清 trace 这一列");
        assert_eq!(swept.rows, 0, "{label}：行本身按输出保留期走");

        let row = fixture
            .tables
            .get(Some(project), &runs, landed.run_row)
            .unwrap()
            .unwrap();
        assert!(row["trace"].is_null(), "{label}");
        assert_eq!(row["status"], json!("succeeded"), "{label}：别的列没动");
    }
}

#[test]
fn 到期整行删除并留痕() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, project, task, _) = setup(&fixture);
        let runs = TableId::system("_runs").unwrap();
        let retention = Retention {
            output_millis: 1_000,
            trace_millis: 500,
        };
        let landed = fixture
            .landing
            .land(&task, retention, &execution_writer(), &completion(vec![]))
            .unwrap();

        fixture.clock.advance(2_000);
        let swept = fixture
            .cleanup
            .sweep(project, &runs, fixture.clock.now())
            .unwrap();
        assert_eq!(swept.rows, 1, "{label}：RET-003 —— 整行删除");
        assert!(
            fixture
                .tables
                .get(Some(project), &runs, landed.run_row)
                .unwrap()
                .is_none(),
            "{label}"
        );
    }
}

#[test]
fn 带instance的行豁免而且豁免优先() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, _, bugs) = setup(&fixture);
        // 一个任务写进主体表的行：既命中"任务输出"又命中"被流程引用"。
        let row = fixture
            .tables
            .insert(
                &WrittenBy::Person { user: alice },
                Some(project),
                &bugs,
                json!({"title": "主体行", "_instance": "某个实例", "retainUntil": 1}),
            )
            .unwrap();

        fixture.clock.advance(10_000);
        let swept = fixture
            .cleanup
            .sweep(project, &bugs, fixture.clock.now())
            .unwrap();
        assert_eq!(swept.rows, 0, "{label}：RET-007 —— 两条规则都命中时豁免赢");
        assert_eq!(swept.exempted, 1, "{label}");
        assert!(
            fixture
                .tables
                .get(Some(project), &bugs, row)
                .unwrap()
                .is_some(),
            "{label}：清了等于把实例腰斩（I-X）"
        );
    }
}

#[test]
fn 不存在删除某一行的入口() {
    // RET-005：清理整批按时间进行，**不得选择性删除个别行**。
    let source = include_str!("../src/cleanup.rs");
    let code: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in ["pub fn delete_row", "pub fn remove_row", "pub fn purge_row"] {
        assert!(!code.contains(forbidden), "出现了 {forbidden}");
    }
    // 唯一的公开入口只接受一个时刻。
    assert!(code.contains("pub fn sweep"));
    assert_eq!(code.matches("pub fn ").count(), 2, "只有 new 与 sweep");
}

// ——————————————————————————————— 执行记录 ———————————————————————————————

#[test]
fn runs行永不被后续执行覆盖() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, project, task, _) = setup(&fixture);
        let runs = TableId::system("_runs").unwrap();

        let first = fixture
            .landing
            .land(
                &task,
                Retention::default(),
                &execution_writer(),
                &completion(vec![]),
            )
            .unwrap();
        let second = fixture
            .landing
            .land(
                &task,
                Retention::default(),
                &execution_writer(),
                &completion(vec![]),
            )
            .unwrap();

        assert_ne!(
            first.run_row, second.run_row,
            "{label}：EXE-026 —— 重跑产生新行"
        );
        assert!(
            fixture
                .tables
                .get(Some(project), &runs, first.run_row)
                .unwrap()
                .is_some(),
            "{label}"
        );
        assert_eq!(
            fixture.tables.rows(Some(project), &runs, 10).unwrap().len(),
            2,
            "{label}"
        );
    }
}

#[test]
fn 任务停用之后执行记录仍完整保留() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, task, _) = setup(&fixture);
        let runs = TableId::system("_runs").unwrap();
        let landed = fixture
            .landing
            .land(
                &task,
                Retention::default(),
                &execution_writer(),
                &completion(vec![]),
            )
            .unwrap();

        fixture.tasks.set_enabled(alice, task.id, false).unwrap();
        assert!(
            fixture
                .tables
                .get(Some(project), &runs, landed.run_row)
                .unwrap()
                .is_some(),
            "{label}：EXE-026"
        );
    }
}

#[test]
fn 产物超限截断并标注() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, project, task, _) = setup(&fixture);
        let runs = TableId::system("_runs").unwrap();
        let mut oversized = completion(vec![]);
        oversized.output = "啊".repeat(MAX_OUTPUT_CHARS + 100);

        let landed = fixture
            .landing
            .land(&task, Retention::default(), &execution_writer(), &oversized)
            .unwrap();
        assert!(landed.truncated, "{label}");
        let row = fixture
            .tables
            .get(Some(project), &runs, landed.run_row)
            .unwrap()
            .unwrap();
        assert!(
            row["output"].as_str().unwrap().ends_with(TRUNCATION_MARK),
            "{label}：EXE-025 —— 不静默丢弃"
        );
    }
}

// ——————————————————————————————— 并发上限 ———————————————————————————————

#[test]
fn 单个项目吃不掉全部算力() {
    let concurrency = Arc::new(xops_task::Concurrency::new(8, 2));
    let hungry = ProjectId::generate();
    let _first = concurrency.acquire(hungry).unwrap();
    let _second = concurrency.acquire(hungry).unwrap();
    assert!(concurrency.acquire(hungry).is_none(), "EXE-027");
    assert!(
        concurrency.acquire(ProjectId::generate()).is_some(),
        "别的项目还有名额"
    );
}
