//! RP-14 的验收。
//!
//! ⚠️ **以上全部验收，一行结算行都不用写**——那是 RP-15 的事。

use std::sync::Arc;

use serde_json::json;
use xops_audit::AuditLog;
use xops_core::{Role, SystemClock, TableName, Timestamp};
use xops_flow::definition::{Criteria, Evaluation, Filter, Node, Start, State, Step, Writers};
use xops_flow::instance::{Instance, InstanceState, NodeState, Subject};
use xops_flow::{Definition, FlowId, Flows};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};
use xops_table::engine::Catalog;
use xops_table::table::{Protection, TableId};
use xops_table::{Column, ColumnType, Tables};

struct Fixture {
    label: &'static str,
    flows: Arc<Flows>,
    tables: Arc<Tables>,
    directory: Arc<Directory>,
    relations: Arc<dyn xops_store::Relations>,
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
            Fixture {
                label,
                flows,
                tables,
                directory,
                relations,
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

fn node(name: &str, pass: Criteria) -> Node {
    Node {
        name: name.into(),
        pass,
        quorum: 1,
        reject: None,
        writers: Writers {
            roles: vec![Role::Member],
            ..Writers::default()
        },
        separation_of_duties: false,
        evaluation: Evaluation::default(),
    }
}

impl Fixture {
    fn setup(&self) -> (UserId, ProjectId) {
        let alice = self
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
        let project = self
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap()
            .id;
        for name in ["approvals", "bugs"] {
            self.tables
                .create(
                    alice,
                    project,
                    TableId::user(name).unwrap(),
                    Protection::Normal,
                    vec![
                        Column::new("decision", ColumnType::Text { max_len: 16 }, false).unwrap(),
                        Column::new("stage", ColumnType::Text { max_len: 16 }, false).unwrap(),
                    ],
                )
                .unwrap();
        }
        (alice, project)
    }

    fn definition(&self, project: ProjectId, actor: UserId, steps: Vec<Step>) -> Definition {
        Definition {
            flow: FlowId::generate(),
            project,
            version: 0,
            name: "审批".into(),
            settlement_table: TableId::user("approvals").unwrap(),
            subject_table: Some(TableId::user("bugs").unwrap()),
            start: Start::Explicit,
            status_columns: vec![],
            steps,
            state: State::Published,
            created_by: actor,
            created_at: Timestamp::from_millis(0),
        }
    }
}

fn subject() -> Subject {
    Subject {
        kind: "bug".into(),
        id: "行1".into(),
        revision: Some("abc".into()),
    }
}

// ——————————————————————————————— 校验 ———————————————————————————————

#[test]
fn 结算表等于主体表定义时就被拒() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project) = fixture.setup();
        let mut definition = fixture.definition(
            project,
            alice,
            vec![Step::Single {
                node: node("批", equals("decision", "同意")),
            }],
        );
        definition.subject_table = Some(definition.settlement_table.clone());
        let error = fixture.flows.define(alice, definition).unwrap_err();
        assert!(
            error.message().contains("FLW-004"),
            "{label}：{}",
            error.message()
        );
    }
}

#[test]
fn 无主体表却随行发起被拒() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project) = fixture.setup();
        let mut definition = fixture.definition(
            project,
            alice,
            vec![Step::Single {
                node: node("批", equals("decision", "同意")),
            }],
        );
        definition.subject_table = None;
        definition.start = Start::Automatic;
        assert!(fixture.flows.define(alice, definition).is_err(), "{label}");
    }
}

#[test]
fn 看起来不重叠但证不出互斥的一律误拒() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project) = fixture.setup();
        // 两个节点看的是不同的列 —— 人看着不会撞，但证不出互斥。
        let definition = fixture.definition(
            project,
            alice,
            vec![
                Step::Single {
                    node: node("初审", equals("stage", "初")),
                },
                Step::Single {
                    node: node("复审", equals("decision", "同意")),
                },
            ],
        );
        let error = fixture.flows.define(alice, definition).unwrap_err();
        assert!(
            error.message().contains("宁可误拒"),
            "{label}：{}",
            error.message()
        );
    }
}

#[test]
fn 校验不落库() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project) = fixture.setup();
        let definition = fixture.definition(
            project,
            alice,
            vec![Step::Single {
                node: node("批", equals("decision", "同意")),
            }],
        );
        let findings = fixture.flows.check(alice, &definition).unwrap();
        assert!(findings.is_empty(), "{label}");
        assert!(
            fixture.flows.list(alice, project).unwrap().is_empty(),
            "{label}：FLW-008 —— 校验不落库"
        );
    }
}

// ——————————————————————————————— 实例 ———————————————————————————————

#[test]
fn 创建的同一步第一个节点随即激活() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project) = fixture.setup();
        let definition = fixture
            .flows
            .define(
                alice,
                fixture.definition(
                    project,
                    alice,
                    vec![
                        Step::Single {
                            node: node("初审", equals("stage", "初")),
                        },
                        Step::Single {
                            node: node("复审", equals("stage", "复")),
                        },
                    ],
                ),
            )
            .unwrap();

        let instance = fixture
            .flows
            .start(alice, definition.flow, definition.version, subject(), None)
            .unwrap();
        assert_eq!(instance.active().len(), 1, "{label}");
        assert_eq!(instance.active()[0].node, "初审", "{label}");
        assert_eq!(instance.state, InstanceState::Running, "{label}");
    }
}

#[test]
fn 版本冻结在途实例不受新版本影响() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project) = fixture.setup();
        let first = fixture
            .flows
            .define(
                alice,
                fixture.definition(
                    project,
                    alice,
                    vec![Step::Single {
                        node: node("初审", equals("stage", "初")),
                    }],
                ),
            )
            .unwrap();
        let instance = fixture
            .flows
            .start(alice, first.flow, first.version, subject(), None)
            .unwrap();

        // 同一条流程发布第二版：多一步。
        let mut second = first.clone();
        second.steps.push(Step::Single {
            node: node("复审", equals("stage", "复")),
        });
        let second = fixture.flows.define(alice, second).unwrap();
        assert_eq!(second.version, 2, "{label}");

        let reread = fixture.flows.status(alice, instance.id).unwrap();
        assert_eq!(reread.version, 1, "{label}：FLW-007 —— 按发起时的版本走完");
        assert_eq!(reread.nodes.len(), 1, "{label}：新版本多的那一步不在它身上");
    }
}

#[test]
fn 停用之后发不了新实例但在途的继续() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project) = fixture.setup();
        let definition = fixture
            .flows
            .define(
                alice,
                fixture.definition(
                    project,
                    alice,
                    vec![Step::Single {
                        node: node("初审", equals("stage", "初")),
                    }],
                ),
            )
            .unwrap();
        let running = fixture
            .flows
            .start(alice, definition.flow, definition.version, subject(), None)
            .unwrap();

        fixture
            .flows
            .disable(alice, definition.flow, definition.version)
            .unwrap();
        assert!(
            fixture
                .flows
                .start(alice, definition.flow, definition.version, subject(), None)
                .is_err(),
            "{label}：停用后发不了新实例"
        );
        assert_eq!(
            fixture.flows.status(alice, running.id).unwrap().state,
            InstanceState::Running,
            "{label}：在途的继续执行完"
        );
    }
}

#[test]
fn 拒绝即终态其余节点转为已作废() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project) = fixture.setup();
        let definition = fixture
            .flows
            .define(
                alice,
                fixture.definition(
                    project,
                    alice,
                    vec![
                        Step::Single {
                            node: node("初审", equals("stage", "初")),
                        },
                        Step::Single {
                            node: node("复审", equals("stage", "复")),
                        },
                    ],
                ),
            )
            .unwrap();
        let mut instance = fixture
            .flows
            .start(alice, definition.flow, definition.version, subject(), None)
            .unwrap();

        instance
            .reject("初审", &["行1".into()], Timestamp::from_millis(1))
            .unwrap();
        fixture.flows.save(&instance).unwrap();

        let reread = fixture.flows.status(alice, instance.id).unwrap();
        assert_eq!(reread.state, InstanceState::Rejected, "{label}");
        assert!(
            reread
                .nodes
                .iter()
                .filter(|node| node.step == 1)
                .all(|node| node.state == NodeState::Void),
            "{label}：不停在未激活"
        );
    }
}

#[test]
fn 并行组全部通过才推进而且卡在哪看得出来() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project) = fixture.setup();
        let definition = fixture
            .flows
            .define(
                alice,
                fixture.definition(
                    project,
                    alice,
                    vec![
                        Step::Parallel {
                            nodes: vec![
                                node("甲", equals("decision", "甲同意")),
                                node("乙", equals("decision", "乙同意")),
                            ],
                        },
                        // 与并行组的两个都要证得出互斥 —— 同一列、不同字面值。
                        Step::Single {
                            node: node("终审", equals("decision", "终审通过")),
                        },
                    ],
                ),
            )
            .unwrap();
        let mut instance = fixture
            .flows
            .start(alice, definition.flow, definition.version, subject(), None)
            .unwrap();
        assert_eq!(instance.active().len(), 2, "{label}：并行组同时激活");

        instance
            .approve("甲", &[], Timestamp::from_millis(1))
            .unwrap();
        let activated = fixture.flows.advance(&mut instance).unwrap();
        assert!(activated.is_empty(), "{label}：还差一个");

        instance
            .approve("乙", &[], Timestamp::from_millis(2))
            .unwrap();
        let activated = fixture.flows.advance(&mut instance).unwrap();
        assert_eq!(activated, vec!["终审"], "{label}");
        assert_eq!(instance.active()[0].node, "终审", "{label}");
    }
}

#[test]
fn 取消与过期各进各的终态() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project) = fixture.setup();
        let definition = fixture
            .flows
            .define(
                alice,
                fixture.definition(
                    project,
                    alice,
                    vec![Step::Single {
                        node: node("初审", equals("stage", "初")),
                    }],
                ),
            )
            .unwrap();

        let cancelled = fixture
            .flows
            .start(alice, definition.flow, definition.version, subject(), None)
            .unwrap();
        fixture.flows.cancel(alice, cancelled.id).unwrap();
        assert_eq!(
            fixture.flows.status(alice, cancelled.id).unwrap().state,
            InstanceState::Cancelled,
            "{label}"
        );

        let expiring = fixture
            .flows
            .start(
                alice,
                definition.flow,
                definition.version,
                subject(),
                Some(Timestamp::from_millis(1)),
            )
            .unwrap();
        assert_eq!(
            fixture.flows.expire_due(Timestamp::from_millis(2)).unwrap(),
            1,
            "{label}"
        );
        assert_eq!(
            fixture.flows.status(alice, expiring.id).unwrap().state,
            InstanceState::Expired,
            "{label}"
        );
    }
}

#[test]
fn 同一张表可以被多条流程引用() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project) = fixture.setup();
        for name in ["审批一", "审批二"] {
            let mut definition = fixture.definition(
                project,
                alice,
                vec![Step::Single {
                    node: node("批", equals("stage", "初")),
                }],
            );
            definition.name = name.into();
            fixture.flows.define(alice, definition).unwrap();
        }
        assert_eq!(
            fixture
                .flows
                .referencing(project, &TableId::user("approvals").unwrap())
                .unwrap()
                .len(),
            2,
            "{label}：FLW-014 —— 结算表 ≠ 主体表是每条流程内部的约束"
        );
    }
}

#[test]
fn 跨项目待办在一个地方查得到() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, first) = fixture.setup();
        // 第二个项目。
        let second = fixture
            .directory
            .create_project(alice, Slug::new("other").unwrap(), "另一个")
            .unwrap()
            .id;
        for name in ["approvals", "bugs"] {
            fixture
                .tables
                .create(
                    alice,
                    second,
                    TableId::user(name).unwrap(),
                    Protection::Normal,
                    vec![Column::new("stage", ColumnType::Text { max_len: 16 }, false).unwrap()],
                )
                .unwrap();
        }

        for project in [first, second] {
            let definition = fixture
                .flows
                .define(
                    alice,
                    fixture.definition(
                        project,
                        alice,
                        vec![Step::Single {
                            node: node("初审", equals("stage", "初")),
                        }],
                    ),
                )
                .unwrap();
            fixture
                .flows
                .start(alice, definition.flow, definition.version, subject(), None)
                .unwrap();
        }

        let pending = fixture.flows.pending_for(alice).unwrap();
        assert_eq!(pending.len(), 2, "{label}：FLW-016 —— 跨项目聚合");
        let projects: std::collections::BTreeSet<ProjectId> = pending
            .iter()
            .map(|(instance, _)| instance.project)
            .collect();
        assert_eq!(projects.len(), 2, "{label}");
    }
}

#[test]
fn 全部验收一行结算行都没写() {
    // 这个测试文件里没有任何一次往结算表写行的调用 —— RP-14 的验收本来就该这样。
    let source = include_str!("flow_acceptance.rs");
    let code: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    // 拼出来，免得这行断言自己撞上自己。
    let needle = format!("tables.{}", "insert");
    assert!(
        !code.contains(&needle),
        "结算行归 RP-15，本包的验收不需要它"
    );
}

// ——————————————————————————————— 关系投影 ———————————————————————————————
//
// 实例这张表有三处读**都不是按行标识找的**：`find_by_subject`（XFG-011 的幂等）·
// `pending_for`（跨项目聚合）· `expire_due`（到期那批）。它们现在走一张带索引的真表。

impl Fixture {
    /// 起一个实例，返回 `(发起人, 项目, 实例)`。
    fn running(&self) -> (UserId, ProjectId, Instance) {
        let (alice, project) = self.setup();
        let definition = self
            .flows
            .define(
                alice,
                self.definition(
                    project,
                    alice,
                    vec![Step::Single {
                        node: node("初审", equals("stage", "初")),
                    }],
                ),
            )
            .unwrap();
        let instance = self
            .flows
            .start(alice, definition.flow, definition.version, subject(), None)
            .unwrap();
        (alice, project, instance)
    }
}

/// 关系投影是**查询真正走的那条路**——清掉它，这三处就什么也找不到。
///
/// 这条不是在夸这个设计，是在**证明它确实被用上了**：
/// 如果三处仍然在扫账，清掉投影不会有任何影响，那这张表就是白建的。
#[test]
fn 三处读真的走关系投影() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, _) = fixture.running();
        assert!(
            fixture
                .flows
                .find_by_subject(project, "bug", "行1")
                .unwrap()
                .is_some(),
            "{label}"
        );
        assert!(!fixture.flows.pending_for(alice).unwrap().is_empty());

        fixture.relations.clear("flow_instances").unwrap();

        assert!(
            fixture
                .flows
                .find_by_subject(project, "bug", "行1")
                .unwrap()
                .is_none(),
            "{label}：投影清掉之后就找不到了 —— 它确实是查询走的那条路"
        );
        assert!(fixture.flows.pending_for(alice).unwrap().is_empty());
    }
}

/// **账还在，重放就全回来。** 关系投影是缓存，不是账。
#[test]
fn 投影漂了从账上重放回来() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, project, instance) = fixture.running();

        fixture.relations.clear("flow_instances").unwrap();
        assert_eq!(
            fixture.flows.rebuild_instances().unwrap(),
            1,
            "{label}：账上有一个实例"
        );

        let after = fixture
            .flows
            .find_by_subject(project, "bug", "行1")
            .unwrap()
            .unwrap();
        assert_eq!(after.id, instance.id, "{label}：一模一样地回来了");
    }
}

/// 主体不同的实例互不干扰——**幂等找的是「这一个」，不是「随便一个」**。
#[test]
fn 按主体找找的是那一个() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, first) = fixture.running();
        let definition = fixture.flows.list(alice, project).unwrap()[0].clone();
        let other = fixture
            .flows
            .start(
                alice,
                definition.flow,
                definition.version,
                Subject {
                    kind: "xforge".into(),
                    id: "另一个摘要".into(),
                    revision: None,
                },
                None,
            )
            .unwrap();

        let found = fixture
            .flows
            .find_by_subject(project, "xforge", "另一个摘要")
            .unwrap()
            .unwrap();
        assert_eq!(found.id, other.id, "{label}");
        assert_ne!(found.id, first.id, "{label}：不是随便一个");
        assert!(
            fixture
                .flows
                .find_by_subject(project, "xforge", "没见过的摘要")
                .unwrap()
                .is_none(),
            "{label}：没有的就是没有"
        );
    }
}

/// 没设过期时刻的实例**不该被到期清理扫进来**。
///
/// ⚠️ 这一条盯的是两个实现之间最容易漂的地方：SQL 里 `NULL <= x` 是 NULL（不匹配），
/// 而手写过滤很容易把缺列当成 0。
#[test]
fn 没设过期时刻的不算到期() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, project, _) = fixture.running();
        assert_eq!(
            fixture
                .flows
                .expire_due(Timestamp::from_millis(i64::MAX))
                .unwrap(),
            0,
            "{label}：null 不参与 ≤ 的比较"
        );

        let definition = fixture.flows.list(alice, project).unwrap()[0].clone();
        let doomed = fixture
            .flows
            .start(
                alice,
                definition.flow,
                definition.version,
                Subject {
                    kind: "xforge".into(),
                    id: "会过期的".into(),
                    revision: None,
                },
                Some(Timestamp::from_millis(10)),
            )
            .unwrap();
        assert_eq!(
            fixture
                .flows
                .expire_due(Timestamp::from_millis(11))
                .unwrap(),
            1,
            "{label}：设了的那个到点了"
        );
        assert_eq!(
            fixture.flows.instance(doomed.id).unwrap().state,
            InstanceState::Expired,
            "{label}"
        );
    }
}
