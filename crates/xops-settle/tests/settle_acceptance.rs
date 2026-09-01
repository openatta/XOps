//! RP-15 的验收：**七条判定各挡各的**。

use std::sync::Arc;

use serde_json::json;
use xops_audit::AuditLog;
use xops_core::{Id, Role, SystemClock, TableName, Timestamp};
use xops_flow::definition::{Criteria, Evaluation, Filter, Node, Start, State, Step, Writers};
use xops_flow::instance::Subject;
use xops_flow::{Definition, FlowId, Flows};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_settle::protection::{Origin, check};
use xops_settle::{Evaluator, Rule, Verdict, WriterCheck, Written};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};
use xops_table::engine::Catalog;
use xops_table::table::{Protection, TableId};
use xops_table::{Column, ColumnType, Tables, WrittenBy};

struct Fixture {
    label: &'static str,
    flows: Arc<Flows>,
    tables: Arc<Tables>,
    directory: Arc<Directory>,
    evaluator: Evaluator,
}

/// 两个后端，**关系投影跟着各自的后端走**。
///
/// ⚠️ 一开始这里两档都给的内存投影——那样 SQLite 那个实现在整个测试套里
/// 一次都没被跑到。**第二实现的价值在于两个都被跑**。
type Backend = (&'static str, Arc<dyn Store>, Arc<dyn xops_store::Relations>);

fn backends() -> Vec<Backend> {
    let sqlite = Arc::new(SqliteStore::in_memory().unwrap());
    let sqlite_relations = sqlite.relations();
    vec![
        (
            "memory",
            Arc::new(MemoryStore::new()) as Arc<dyn Store>,
            Arc::new(xops_store::MemoryRelations::new()) as Arc<dyn xops_store::Relations>,
        ),
        ("sqlite", sqlite as Arc<dyn Store>, sqlite_relations),
    ]
}

fn fixtures() -> Vec<Fixture> {
    backends()
        .into_iter()
        .map(|(label, store, relations)| {
            let clock = Arc::new(SystemClock);
            let catalog = Arc::new(Catalog::open(Arc::clone(&store), clock.clone()).unwrap());
            let engine = Arc::new(
                WriteEngine::new(Arc::clone(&store), clock.clone())
                    .with_pre_write(Arc::clone(&catalog) as Arc<dyn xops_store::PreWrite>)
                    .with_schema_check(Arc::clone(&catalog) as Arc<dyn xops_store::SchemaCheck>),
            );
            let mut audit = AuditLog::new(
                Arc::clone(&engine),
                Arc::clone(&store),
                Arc::clone(&relations),
            )
            .unwrap();
            for table in xops_identity::directory::platform_tables().unwrap() {
                audit = audit.watching(table);
            }
            for table in [xops_table::CATALOG_TABLE, xops_flow::FLOWS_TABLE] {
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
            let flows = Arc::new(
                Flows::new(
                    engine,
                    store,
                    audit,
                    Arc::clone(&directory),
                    Arc::clone(&tables),
                    Arc::clone(&relations),
                    clock,
                )
                .unwrap(),
            );
            let evaluator = Evaluator::new(
                Arc::clone(&flows),
                Arc::new(WriterCheck::new(
                    Arc::clone(&directory),
                    Arc::clone(&tables),
                )),
            );
            Fixture {
                label,
                flows,
                tables,
                directory,
                evaluator,
            }
        })
        .collect()
}

fn equals(column: &str, value: &str) -> Criteria {
    Criteria {
        filters: vec![Filter::Equals {
            column: column.into(),
            value: json!(value),
        }],
    }
}

struct Scene {
    alice: UserId,
    bob: UserId,
    project: ProjectId,
    definition: Definition,
    instance: xops_flow::Instance,
}

fn scene(fixture: &Fixture, separation: bool) -> Scene {
    let alice = fixture
        .directory
        .provision(
            ExternalAccount {
                provider: ProviderId::new("builtin").unwrap(),
                account: "alice".into(),
            },
            "Alice",
            None,
        )
        .unwrap()
        .id;
    let bob = fixture
        .directory
        .provision(
            ExternalAccount {
                provider: ProviderId::new("builtin").unwrap(),
                account: "bob".into(),
            },
            "Bob",
            None,
        )
        .unwrap()
        .id;
    let project = fixture
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap()
        .id;
    fixture
        .directory
        .set_member(alice, project, bob, Role::Member)
        .unwrap();

    for name in ["approvals", "bugs"] {
        fixture
            .tables
            .create(
                alice,
                project,
                TableId::user(name).unwrap(),
                Protection::Normal,
                vec![
                    Column::new("decision", ColumnType::Text { max_len: 16 }, false).unwrap(),
                    Column::new("status", ColumnType::Text { max_len: 16 }, false).unwrap(),
                ],
            )
            .unwrap();
    }

    let definition = fixture
        .flows
        .define(
            alice,
            Definition {
                flow: FlowId::generate(),
                project,
                version: 0,
                name: "审批".into(),
                settlement_table: TableId::user("approvals").unwrap(),
                subject_table: Some(TableId::user("bugs").unwrap()),
                start: Start::Explicit,
                status_columns: vec![],
                steps: vec![Step::Single {
                    node: Node {
                        name: "初审".into(),
                        pass: equals("decision", "同意"),
                        quorum: 1,
                        reject: Some(equals("decision", "拒绝")),
                        writers: Writers {
                            roles: vec![Role::Member],
                            ..Writers::default()
                        },
                        separation_of_duties: separation,
                        evaluation: Evaluation::default(),
                    },
                }],
                state: State::Published,
                created_by: alice,
                created_at: Timestamp::from_millis(0),
            },
        )
        .unwrap();

    // 发起人是 alice。
    let instance = fixture
        .flows
        .start(
            alice,
            definition.flow,
            definition.version,
            Subject {
                kind: "bug".into(),
                id: "行1".into(),
                revision: Some("abc".into()),
            },
            None,
        )
        .unwrap();
    Scene {
        alice,
        bob,
        project,
        definition,
        instance,
    }
}

fn written(values: serde_json::Value, written_by: WrittenBy) -> Written {
    Written {
        values,
        written_by,
        row: Id::generate().to_string(),
    }
}

fn node(definition: &Definition) -> &Node {
    definition.node(0, "初审").unwrap()
}

// ——————————————————————————————— 七条判定 ———————————————————————————————

#[test]
fn 七条各挡各的() {
    for rule in Rule::all() {
        assert!(!rule.why().is_empty(), "{rule:?}");
    }
    assert_eq!(Rule::all().len(), 7, "FLW-026：缺一不可");
}

#[test]
fn 满足七条就结算() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture, false);
        let row = written(
            json!({"decision": "同意", "_instance": scene.instance.id.to_string()}),
            WrittenBy::Person { user: scene.bob },
        );
        let verdict = fixture
            .evaluator
            .judge(
                &scene.definition,
                &scene.instance,
                node(&scene.definition),
                &row,
            )
            .unwrap();
        assert_eq!(verdict, Verdict::Settle, "{label}");
    }
}

#[test]
fn 不带instance的行不是冲着这个节点来的() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture, false);
        let row = written(
            json!({"decision": "同意"}),
            WrittenBy::Person { user: scene.bob },
        );
        assert_eq!(
            fixture
                .evaluator
                .judge(
                    &scene.definition,
                    &scene.instance,
                    node(&scene.definition),
                    &row
                )
                .unwrap(),
            Verdict::NotSettled {
                failed: Rule::Targeted
            },
            "{label}：① 没有 _instance，两个并发实例就无从区分"
        );
    }
}

#[test]
fn 不在允许写入者集合里就不算数() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture, false);
        // carol 不是项目成员。
        let carol = fixture
            .directory
            .provision(
                ExternalAccount {
                    provider: ProviderId::new("builtin").unwrap(),
                    account: "carol".into(),
                },
                "Carol",
                None,
            )
            .unwrap()
            .id;
        let row = written(
            json!({"decision": "同意", "_instance": scene.instance.id.to_string()}),
            WrittenBy::Person { user: carol },
        );
        assert_eq!(
            fixture
                .evaluator
                .judge(
                    &scene.definition,
                    &scene.instance,
                    node(&scene.definition),
                    &row
                )
                .unwrap(),
            Verdict::NotSettled {
                failed: Rule::AllowedWriter
            },
            "{label}"
        );
    }
}

#[test]
fn 移出名单之后写入这一刻就不算数了() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture, false);
        let row = written(
            json!({"decision": "同意", "_instance": scene.instance.id.to_string()}),
            WrittenBy::Person { user: scene.bob },
        );
        // 先能过。
        assert_eq!(
            fixture
                .evaluator
                .judge(
                    &scene.definition,
                    &scene.instance,
                    node(&scene.definition),
                    &row
                )
                .unwrap(),
            Verdict::Settle,
            "{label}"
        );
        // 把 bob 移出项目 —— **写入这一刻**再判就不算数了（FLW-029）。
        fixture
            .directory
            .remove_member(scene.alice, scene.project, scene.bob)
            .unwrap();
        assert_eq!(
            fixture
                .evaluator
                .judge(
                    &scene.definition,
                    &scene.instance,
                    node(&scene.definition),
                    &row
                )
                .unwrap(),
            Verdict::NotSettled {
                failed: Rule::AllowedWriter
            },
            "{label}：事件发出时在名单里，跑了几分钟被移出 —— ② 正好挡住这一情形"
        );
    }
}

#[test]
fn 职责分离挡闭环自批() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture, true);
        // alice 是发起人，她自己来批。
        let row = written(
            json!({"decision": "同意", "_instance": scene.instance.id.to_string()}),
            WrittenBy::Person { user: scene.alice },
        );
        assert_eq!(
            fixture
                .evaluator
                .judge(
                    &scene.definition,
                    &scene.instance,
                    node(&scene.definition),
                    &row
                )
                .unwrap(),
            Verdict::NotSettled {
                failed: Rule::SeparationOfDuties
            },
            "{label}：审批唯一的价值（多一个人）当场归零"
        );
        // 换个人就过。
        let row = written(
            json!({"decision": "同意", "_instance": scene.instance.id.to_string()}),
            WrittenBy::Person { user: scene.bob },
        );
        assert_eq!(
            fixture
                .evaluator
                .judge(
                    &scene.definition,
                    &scene.instance,
                    node(&scene.definition),
                    &row
                )
                .unwrap(),
            Verdict::Settle,
            "{label}"
        );
    }
}

#[test]
fn 任务写的行比的是任务所有者() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture, true);
        // 任务的所有者就是发起人 alice —— 职责分离照样挡住（I-O）。
        let row = written(
            json!({"decision": "同意", "_instance": scene.instance.id.to_string()}),
            WrittenBy::Execution {
                run: Id::generate(),
                task: Id::generate(),
                task_owner: scene.alice,
                skill: "s".into(),
                skill_version: "1".into(),
                revision: None,
                status: "succeeded".into(),
            },
        );
        assert_eq!(
            fixture
                .evaluator
                .judge(
                    &scene.definition,
                    &scene.instance,
                    node(&scene.definition),
                    &row
                )
                .unwrap(),
            Verdict::NotSettled {
                failed: Rule::SeparationOfDuties
            },
            "{label}：任务不是责任主体，人才是"
        );
    }
}

#[test]
fn 执行没正常完成的残片不算数() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture, false);
        let row = written(
            json!({"decision": "同意", "_instance": scene.instance.id.to_string()}),
            WrittenBy::Execution {
                run: Id::generate(),
                task: Id::generate(),
                task_owner: scene.bob,
                skill: "s".into(),
                skill_version: "1".into(),
                revision: None,
                status: "failed".into(),
            },
        );
        assert_eq!(
            fixture
                .evaluator
                .judge(
                    &scene.definition,
                    &scene.instance,
                    node(&scene.definition),
                    &row
                )
                .unwrap(),
            Verdict::NotSettled {
                failed: Rule::ExecutionSucceeded
            },
            "{label}：⑥ 挡的是「执行超时后部分产出仍被保留」"
        );
    }
}

#[test]
fn 修订对不上的不算数() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture, false);
        let row = written(
            json!({"decision": "同意", "_instance": scene.instance.id.to_string()}),
            WrittenBy::Execution {
                run: Id::generate(),
                task: Id::generate(),
                task_owner: scene.bob,
                skill: "s".into(),
                skill_version: "1".into(),
                // 主体修订是 abc，它读的是别的。
                revision: Some("别的修订".into()),
                status: "succeeded".into(),
            },
        );
        assert_eq!(
            fixture
                .evaluator
                .judge(
                    &scene.definition,
                    &scene.instance,
                    node(&scene.definition),
                    &row
                )
                .unwrap(),
            Verdict::NotSettled {
                failed: Rule::RevisionMatches
            },
            "{label}：⑦ 挡的是「读的是仓库当前 HEAD 而不是这次要它看的那一版」"
        );
    }
}

#[test]
fn 实例终结之后节点就不激活了() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture, false);
        let cancelled = fixture
            .flows
            .cancel(scene.alice, scene.instance.id)
            .unwrap();
        let row = written(
            json!({"decision": "同意", "_instance": cancelled.id.to_string()}),
            WrittenBy::Person { user: scene.bob },
        );
        assert_eq!(
            fixture
                .evaluator
                .judge(&scene.definition, &cancelled, node(&scene.definition), &row)
                .unwrap(),
            Verdict::NotSettled {
                failed: Rule::NodeActive
            },
            "{label}"
        );
    }
}

#[test]
fn 命中拒绝条件就是拒绝不是不结算() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture, false);
        let row = written(
            json!({"decision": "拒绝", "_instance": scene.instance.id.to_string()}),
            WrittenBy::Person { user: scene.bob },
        );
        assert_eq!(
            fixture
                .evaluator
                .judge(
                    &scene.definition,
                    &scene.instance,
                    node(&scene.definition),
                    &row
                )
                .unwrap(),
            Verdict::Reject,
            "{label}"
        );
    }
}

#[test]
fn 不结算的行照常留在表里() {
    for fixture in fixtures() {
        let label = fixture.label;
        let mut scene = scene(&fixture, false);
        let row = written(
            json!({"decision": "看看"}),
            WrittenBy::Person { user: scene.bob },
        );
        let verdict = Verdict::NotSettled {
            failed: Rule::Targeted,
        };
        let activated = fixture
            .evaluator
            .apply(
                &mut scene.instance,
                node(&scene.definition),
                &verdict,
                &row,
                Timestamp::from_millis(1),
            )
            .unwrap();
        assert!(activated.is_empty(), "{label}");
        assert_eq!(
            scene.instance.state,
            xops_flow::InstanceState::Running,
            "{label}：FLW-027 —— 它是一条正常数据，只是不算数"
        );
    }
}

#[test]
fn 结算之后经状态机推进而不是自己改() {
    for fixture in fixtures() {
        let label = fixture.label;
        let mut scene = scene(&fixture, false);
        let row = written(
            json!({"decision": "同意", "_instance": scene.instance.id.to_string()}),
            WrittenBy::Person { user: scene.bob },
        );
        fixture
            .evaluator
            .apply(
                &mut scene.instance,
                node(&scene.definition),
                &Verdict::Settle,
                &row,
                Timestamp::from_millis(1),
            )
            .unwrap();
        assert_eq!(
            scene.instance.state,
            xops_flow::InstanceState::Approved,
            "{label}：只有一个节点，通过即实例通过"
        );
        // 落库了 —— 而落库是经 RP-14 的接口做的。
        assert_eq!(
            fixture
                .flows
                .status(scene.alice, scene.instance.id)
                .unwrap()
                .state,
            xops_flow::InstanceState::Approved,
            "{label}"
        );
    }
}

#[test]
fn 本包不碰flows与flownodes() {
    // 这条分工是那一刀能成立的全部前提。
    let source = include_str!("../src/evaluate.rs");
    let code: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in ["_flows", "_flow_nodes"] {
        assert!(!code.contains(forbidden), "本包不得自己去改 {forbidden}");
    }
}

// ——————————————————————————————— 受保护的列 ———————————————————————————————

#[test]
fn 用户写不了instance也写不了状态列() {
    let status = vec!["status".to_owned()];
    assert!(
        check(Origin::User, &status, &json!({"_instance": "x"}), false).is_err(),
        "I-P：整个流程模型的地基"
    );
    assert!(
        check(Origin::User, &status, &json!({"status": "closed"}), false).is_err(),
        "否则任何成员都能直接把它改成完成态，绕过整条流程"
    );
    assert!(
        check(
            Origin::Platform,
            &status,
            &json!({"status": "closed"}),
            false
        )
        .is_ok()
    );
}
