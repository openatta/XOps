//! RP-19 的验收。
//!
//! ⚠️ **两条验收在这个仓里跑不了，见文件末尾的 `跑不了的两条`。**
//! 它们不是被略过，是被写出来：`XFG-024`（真实的 `xforge approve --provider xops`）
//! 与断网测试都要一个跑起来的 XOps 服务与一份装好的 XForge——
//! **本仓没有可执行入口，XForge 0.8.1 也还没发布。**

use std::sync::Arc;

use serde_json::json;
use xops_audit::AuditLog;
use xops_core::{Id, Role, SystemClock, TableName, Timestamp};
use xops_flow::definition::{Criteria, Evaluation, Filter, Node, Start, State, Step, Writers};
use xops_flow::{Definition, FlowId, Flows};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_repo::credential::{Sealer, Secret};
use xops_repo::{Deps, Repos};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};
use xops_table::engine::Catalog;
use xops_table::table::{Protection, TableId};
use xops_table::{Column, ColumnType, Tables, WrittenBy};
use xops_xforge::XForge;
use xops_xforge::registration::{PolicyBinding, Registration};
use xops_xforge::scaffold::{self, Piece, Sources};
use xops_xforge::spec::{PollReply, Revision, SubmitArgs};

struct Fixture {
    label: &'static str,
    xforge: Arc<XForge>,
    flows: Arc<Flows>,
    tables: Arc<Tables>,
    repos: Arc<Repos>,
    directory: Arc<Directory>,
}

fn build(
    label: &'static str,
    store: Arc<dyn Store>,
    relations: Arc<dyn xops_store::Relations>,
) -> Fixture {
    let clock = Arc::new(SystemClock);
    let catalog = Arc::new(Catalog::open(Arc::clone(&store), clock.clone()).unwrap());
    let engine = Arc::new(
        WriteEngine::new(Arc::clone(&store), clock.clone())
            .with_pre_write(Arc::clone(&catalog) as Arc<dyn xops_store::PreWrite>)
            .with_schema_check(Arc::clone(&catalog) as Arc<dyn xops_store::SchemaCheck>),
    );
    let mut audit = AuditLog::new(Arc::clone(&engine), Arc::clone(&store)).unwrap();
    for table in xops_identity::directory::platform_tables().unwrap() {
        audit = audit.watching(table);
    }
    for table in [
        xops_table::CATALOG_TABLE,
        xops_flow::FLOWS_TABLE,
        xops_repo::BINDINGS_TABLE,
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
    let flows = Arc::new(
        Flows::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            Arc::clone(&directory),
            Arc::clone(&tables),
            Arc::clone(&relations),
            clock.clone(),
        )
        .unwrap(),
    );
    let repos = Arc::new(Repos::new(
        Deps {
            engine: Arc::clone(&engine),
            store: Arc::clone(&store),
            audit: Arc::clone(&audit),
            directory: Arc::clone(&directory),
            clock: clock.clone(),
        },
        Arc::new(Sealer::from_key(&[5u8; 32]).unwrap()),
        Arc::new(AlwaysReadOnly) as Arc<dyn xops_repo::GitPlatform>,
        std::env::temp_dir().join(format!("xops-xforge-test-{}", Id::generate())),
    ));
    let xforge = Arc::new(XForge::new(
        Arc::clone(&repos),
        Arc::clone(&flows),
        Arc::clone(&tables),
        Arc::clone(&directory),
    ));
    Fixture {
        label,
        xforge,
        flows,
        tables,
        repos,
        directory,
    }
}

/// 绑仓时会真推一次 dry-run（`RPO-002`）。测试里换一个"推不进去"的探针。
struct AlwaysReadOnly;

impl xops_repo::GitPlatform for AlwaysReadOnly {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn auth_header(&self, _secret: &Secret) -> String {
        String::new()
    }

    fn probe_write_access(
        &self,
        _remote: &str,
        _secret: &Secret,
    ) -> xops_core::Result<xops_repo::WriteProbe> {
        Ok(xops_repo::WriteProbe::ReadOnly)
    }

    fn verify_webhook(&self, _secret: &str, _body: &[u8], _signature: &str) -> bool {
        false
    }
}

fn fixtures() -> Vec<Fixture> {
    // ⚠️ **关系投影跟着各自的后端走。** 两档都给内存投影的话，
    // SQLite 那个实现在整个测试套里一次都不会被跑到。
    let sqlite = Arc::new(SqliteStore::in_memory().unwrap());
    let sqlite_relations = sqlite.relations();
    vec![
        build(
            "memory",
            Arc::new(MemoryStore::new()),
            Arc::new(xops_store::MemoryRelations::new()),
        ),
        build("sqlite", sqlite, sqlite_relations),
    ]
}

struct Scene {
    owner: UserId,
    member: UserId,
    project: ProjectId,
    flow: FlowId,
    settlement: TableId,
}

fn scene(fixture: &Fixture, bind_repo: bool, register: bool) -> Scene {
    let user = |account: &str| {
        fixture
            .directory
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
    };
    let owner = user("owner");
    let member = user("member");
    let project = fixture
        .directory
        .create_project(owner, Slug::new("acme").unwrap(), "Acme")
        .unwrap()
        .id;
    fixture
        .directory
        .set_member(owner, project, member, Role::Member)
        .unwrap();
    fixture
        .tables
        .ensure_system_tables(project, "acme")
        .unwrap();

    let settlement = TableId::user("approvals").unwrap();
    fixture
        .tables
        .create(
            owner,
            project,
            settlement.clone(),
            Protection::Normal,
            vec![
                Column::new("decision", ColumnType::Text { max_len: 16 }, false).unwrap(),
                Column::new("reason", ColumnType::Text { max_len: 500 }, false).unwrap(),
            ],
        )
        .unwrap();

    let definition = fixture
        .flows
        .define(
            owner,
            Definition {
                flow: FlowId::generate(),
                project,
                version: 0,
                name: "审批".into(),
                settlement_table: settlement.clone(),
                subject_table: None,
                start: Start::Explicit,
                status_columns: vec![],
                steps: vec![Step::Single {
                    node: Node {
                        name: "审批".into(),
                        pass: Criteria {
                            filters: vec![Filter::Equals {
                                column: "decision".into(),
                                value: json!("批准"),
                            }],
                        },
                        quorum: 1,
                        reject: Some(Criteria {
                            filters: vec![Filter::Equals {
                                column: "decision".into(),
                                value: json!("驳回"),
                            }],
                        }),
                        writers: Writers {
                            roles: vec![Role::Member, Role::Maintainer, Role::Owner],
                            roster: None,
                            task: None,
                        },
                        separation_of_duties: false,
                        evaluation: Evaluation::ByCriteria,
                    },
                }],
                state: State::Published,
                created_by: owner,
                created_at: Timestamp::from_millis(0),
            },
        )
        .unwrap();

    if bind_repo {
        fixture
            .repos
            .bind(
                owner,
                project,
                "https://github.com/openatta/XOps.git",
                Secret::new("ro-token"),
            )
            .unwrap();
    }
    if register {
        fixture
            .xforge
            .register(
                owner,
                project,
                &Registration {
                    provider_id: "xops".into(),
                    policies: vec![PolicyBinding {
                        policy_id: "release-approval".into(),
                        flow: definition.flow,
                        flow_version: definition.version,
                        decision_column: "decision".into(),
                        reason_column: "reason".into(),
                        approve_value: "批准".into(),
                        reject_value: "驳回".into(),
                        roles: vec!["maintainer".into(), "owner".into(), "member".into()],
                    }],
                },
            )
            .unwrap();
    }
    Scene {
        owner,
        member,
        project,
        flow: definition.flow,
        settlement,
    }
}

fn args(digest: &str) -> SubmitArgs {
    SubmitArgs {
        change: "CH-1".into(),
        flow: "release".into(),
        stage: "review".into(),
        transition: "approve".into(),
        policy_id: "release-approval".into(),
        revision: Revision {
            state_revision: "s1".into(),
            content_revision: "c1".into(),
            policy_snapshot_digest: "p1".into(),
            git_base: "base".into(),
            git_head: "head".into(),
        },
        governing_digest: digest.into(),
        roles: vec!["maintainer".into()],
        reason: "请审".into(),
    }
}

// ——————————————————————————————— ②③ 找不到就明确失败 ———————————————————————————————

#[test]
fn 没绑仓就明确失败绝不静默创建() {
    for fixture in fixtures() {
        let scene = scene(&fixture, false, false);
        let error = fixture
            .xforge
            .submit(scene.owner, scene.project, &args("d1"))
            .unwrap_err();
        assert!(
            format!("{error}").contains("还没绑仓"),
            "{}：XFG-002，实际是 {error}",
            fixture.label
        );
        // **一个实例都没开出来。**
        assert!(
            fixture
                .flows
                .find_by_subject(scene.project, "xforge", "d1")
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn 没登记policy就明确失败() {
    for fixture in fixtures() {
        let scene = scene(&fixture, true, false);
        let error = fixture
            .xforge
            .submit(scene.owner, scene.project, &args("d1"))
            .unwrap_err();
        assert!(
            format!("{error}").contains("还没有 XForge 登记"),
            "{}",
            fixture.label
        );
    }
}

#[test]
fn 登记了但policyid对不上也明确失败() {
    for fixture in fixtures() {
        let scene = scene(&fixture, true, true);
        let mut other = args("d1");
        other.policy_id = "nope".into();
        let error = fixture
            .xforge
            .submit(scene.owner, scene.project, &other)
            .unwrap_err();
        assert!(
            format!("{error}").contains("绝不静默创建"),
            "{}",
            fixture.label
        );
    }
}

// ——————————————————————————————— 幂等与立即返回 ———————————————————————————————

#[test]
fn 同一个digest不开第二个实例() {
    for fixture in fixtures() {
        let scene = scene(&fixture, true, true);
        let first = fixture
            .xforge
            .submit(scene.owner, scene.project, &args("d1"))
            .unwrap();
        assert!(first.created, "{}", fixture.label);
        let again = fixture
            .xforge
            .submit(scene.owner, scene.project, &args("d1"))
            .unwrap();
        assert!(!again.created, "{}：XFG-011 不得重复开单", fixture.label);
        assert_eq!(first.instance, again.instance, "一一映射");
    }
}

#[test]
fn githead原样存成主体修订() {
    for fixture in fixtures() {
        let scene = scene(&fixture, true, true);
        fixture
            .xforge
            .submit(scene.owner, scene.project, &args("d1"))
            .unwrap();
        let instance = fixture
            .flows
            .find_by_subject(scene.project, "xforge", "d1")
            .unwrap()
            .unwrap();
        assert_eq!(
            instance.subject.id, "d1",
            "{}：主体 = governingDigest",
            fixture.label
        );
        assert_eq!(
            instance.subject.revision.as_deref(),
            Some("head"),
            "XFG-012：gitHead 同时作为主体修订"
        );
    }
}

// ——————————————————————————————— poll 的三种状态 ———————————————————————————————

#[test]
fn 从未提交过是明确的未知状态不是报错() {
    for fixture in fixtures() {
        let scene = scene(&fixture, true, true);
        let reply = fixture
            .xforge
            .poll(scene.owner, scene.project, "从来没见过")
            .unwrap();
        assert_eq!(reply, PollReply::Unknown, "{}：XFG-013", fixture.label);
        assert_eq!(reply.to_json()["status"], json!("unknown"));
        // **可安全重复调用**（XFG-014）：再查一次还是一样，而且没有副作用。
        assert_eq!(
            fixture
                .xforge
                .poll(scene.owner, scene.project, "从来没见过")
                .unwrap(),
            PollReply::Unknown
        );
    }
}

#[test]
fn 未决就是pending而且立刻返回() {
    for fixture in fixtures() {
        let scene = scene(&fixture, true, true);
        fixture
            .xforge
            .submit(scene.owner, scene.project, &args("d1"))
            .unwrap();
        let started = std::time::Instant::now();
        let reply = fixture
            .xforge
            .poll(scene.owner, scene.project, "d1")
            .unwrap();
        assert_eq!(reply, PollReply::Pending, "{}", fixture.label);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "**必须立即返回，绝不阻塞**（XFG-013）"
        );
    }
}

/// 把一个实例结算掉，返回结算行的 RowId。
fn settle(fixture: &Fixture, scene: &Scene, digest: &str, written_by: WrittenBy) -> String {
    let mut instance = fixture
        .flows
        .find_by_subject(scene.project, "xforge", digest)
        .unwrap()
        .unwrap();
    let row = fixture
        .tables
        .insert(
            &written_by,
            Some(scene.project),
            &scene.settlement,
            json!({
                "decision": "批准",
                "reason": "看过了：\"没问题\" <ok>",
                "_instance": instance.id.to_string(),
            }),
        )
        .unwrap();
    instance
        .approve("审批", &[row.to_string()], Timestamp::from_millis(1))
        .unwrap();
    fixture.flows.advance(&mut instance).unwrap();
    row.to_string()
}

#[test]
fn 已决就给出decision与approver与原样的reason() {
    for fixture in fixtures() {
        let scene = scene(&fixture, true, true);
        fixture
            .xforge
            .submit(scene.member, scene.project, &args("d1"))
            .unwrap();
        settle(
            &fixture,
            &scene,
            "d1",
            WrittenBy::Person { user: scene.member },
        );
        let reply = fixture
            .xforge
            .poll(scene.owner, scene.project, "d1")
            .unwrap();
        let PollReply::Decided {
            decision,
            approver,
            reason,
        } = reply
        else {
            panic!("{}：应该已决了", fixture.label);
        };
        assert_eq!(decision, "approve");
        assert_eq!(
            approver.id,
            scene.member.to_string(),
            "XFG-004：人写的行就是他"
        );
        assert_eq!(approver.role, "member", "XFG-019：XOps 自己的角色名");
        // **原样保存与展示，不解析**（XFG-016 / G7）。
        assert_eq!(reason, "看过了：\"没问题\" <ok>");
    }
}

/// **结算表很大的时候,`poll_approval` 照样答得出来。**
///
/// 这一条对着一个真实的缺陷:早先 `settling_row` 是"扫结算表前 500 行再比对",
/// 于是结算表一过 500 行,**对一个已决实例就找不到结算行**——它会报一个
/// XForge 那边查不出原因的失败,而不是给出 `decided`。
///
/// 行标识本来就在实例自己身上(`_flow_nodes.settledBy`),**根本不该去扫表**。
#[test]
fn 结算表过了旧上限之后照样查得到批的人() {
    // 一个 fixture 就够 —— 这条测的是访问路径,不是存储实现。
    let fixture = build(
        "memory",
        Arc::new(MemoryStore::new()),
        Arc::new(xops_store::MemoryRelations::new()),
    );
    let scene = scene(&fixture, true, true);
    fixture
        .xforge
        .submit(scene.member, scene.project, &args("d1"))
        .unwrap();

    // 先把结算表填过旧的那个上限,**真正结算的那一行落在最后**。
    for index in 0..600 {
        fixture
            .tables
            .insert(
                &WrittenBy::Person { user: scene.member },
                Some(scene.project),
                &scene.settlement,
                json!({"decision": "批准", "reason": format!("噪声 {index}")}),
            )
            .unwrap();
    }
    settle(
        &fixture,
        &scene,
        "d1",
        WrittenBy::Person { user: scene.member },
    );

    let reply = fixture
        .xforge
        .poll(scene.owner, scene.project, "d1")
        .unwrap();
    let PollReply::Decided {
        approver, reason, ..
    } = reply
    else {
        panic!("结算行排在第 601 位,旧写法在这里会报「找不到结算它的那一行」");
    };
    assert_eq!(approver.id, scene.member.to_string());
    assert_eq!(
        reason, "看过了：\"没问题\" <ok>",
        "拿到的是那一行,不是噪声行"
    );
}

#[test]
fn approver三种解析都实际构造一遍() {
    for (index, (label, written_by, expected)) in [
        ("人写的行", None, "person"),
        ("私有任务写的行", Some("execution"), "task-owner"),
        ("插件判定", Some("plugin"), "plugin-installer"),
    ]
    .into_iter()
    .enumerate()
    {
        for fixture in fixtures() {
            let scene = scene(&fixture, true, true);
            let digest = format!("d{index}");
            fixture
                .xforge
                .submit(scene.member, scene.project, &args(&digest))
                .unwrap();
            let by = match written_by {
                None => WrittenBy::Person { user: scene.member },
                Some("execution") => WrittenBy::Execution {
                    run: Id::generate(),
                    task: Id::generate(),
                    task_owner: scene.member,
                    skill: "s".into(),
                    skill_version: "1".into(),
                    revision: None,
                    status: "succeeded".into(),
                },
                _ => WrittenBy::Plugin {
                    plugin: "approvals".into(),
                    version: "1".into(),
                    installed_by: scene.owner,
                    instance: Id::generate(),
                },
            };
            let expects_owner = expected == "plugin-installer";
            settle(&fixture, &scene, &digest, by);
            let reply = fixture
                .xforge
                .poll(scene.owner, scene.project, &digest)
                .unwrap();
            let PollReply::Decided { approver, .. } = reply else {
                panic!("{}：{label} 应该已决了", fixture.label);
            };
            let want = if expects_owner {
                scene.owner
            } else {
                scene.member
            };
            assert_eq!(
                approver.id,
                want.to_string(),
                "{}：{label}（XFG-004）",
                fixture.label
            );
        }
    }
}

// ——————————————————————————————— 角色自校验 ———————————————————————————————

#[test]
fn 配一个xops不会返回的角色名在登记阶段就失败() {
    for fixture in fixtures() {
        let scene = scene(&fixture, true, false);
        let error = fixture
            .xforge
            .register(
                scene.owner,
                scene.project,
                &Registration {
                    provider_id: "xops".into(),
                    policies: vec![PolicyBinding {
                        policy_id: "release-approval".into(),
                        flow: scene.flow,
                        flow_version: 1,
                        decision_column: "decision".into(),
                        reason_column: "reason".into(),
                        approve_value: "批准".into(),
                        reject_value: "驳回".into(),
                        roles: vec!["verifier".into()],
                    }],
                },
            )
            .unwrap_err();
        assert!(
            format!("{error}").contains("verifier"),
            "{}：XFG-015——不是等到「告诉人类他的批准生效了」之后",
            fixture.label
        );
    }
}

#[test]
fn 请求带来的角色与登记对不上就拒() {
    for fixture in fixtures() {
        let scene = scene(&fixture, true, true);
        // 登记里认 maintainer/owner/member，请求要 verifier。
        let mut mismatched = args("d1");
        mismatched.roles = vec!["verifier".into()];
        assert!(
            fixture
                .xforge
                .submit(scene.owner, scene.project, &mismatched)
                .is_err(),
            "{}",
            fixture.label
        );
    }
}

// ——————————————————————————————— 边界 ———————————————————————————————

/// **XOps 从不写任何仓库**（`XFG-017`、`I-G`）：枚举本包的代码路径。
#[test]
fn 本包不写任何仓库() {
    let files = [
        include_str!("../src/service.rs"),
        include_str!("../src/tools.rs"),
        include_str!("../src/spec.rs"),
        include_str!("../src/registration.rs"),
        include_str!("../src/approver.rs"),
        include_str!("../src/scaffold.rs"),
    ];
    for source in files {
        let body = source.split("#[cfg(test)]").next().unwrap();
        for forbidden in ["Command::new", "std::process", "std::fs"] {
            assert!(
                !body.contains(forbidden),
                "{forbidden}：回执由 XForge CLI 自己写"
            );
        }
    }
}

/// **本域不提供任何供 Gate 调用的查询接口**（`XFG-018`、`G6`）。
///
/// Gate 子进程会过滤掉一切凭据形状的环境变量——它**没有能力认证**。
/// 所以本包的每一个 tool 都要一个已认证的调用上下文，**一个匿名口子都没有**。
#[test]
fn 没有给gate用的免认证查询口() {
    let source = include_str!("../src/tools.rs");
    let body = source.split("#[cfg(test)]").next().unwrap();
    let requirements = body.matches(".requires(").count();
    let registered = body.matches("registry.register(").count();
    assert_eq!(registered, 4, "四个 tool");
    assert_eq!(requirements, registered, "每一个都声明了需要的角色");
    assert!(
        !body.contains("Requirement::Anonymous") && !body.contains("no_auth"),
        "XFG-018：Gate 没有能力认证，所以这里一个免认证的口子都不能有"
    );
}

/// **一处降级都没有**（`XFG-020`）。
///
/// 任何"连不上就跳过"的降级逻辑都会让变更被静默放行。
#[test]
fn 查不到就明确失败而不是当作通过() {
    let source = include_str!("../src/service.rs");
    let body = source.split("#[cfg(test)]").next().unwrap();
    // "查不到"的每一处都通向一个 Err 或一个明确的 Unknown/Pending，
    // **没有一处通向"当作批准了"**。
    assert!(
        !body.contains("unwrap_or(PollReply::Decided") && !body.contains("默认通过"),
        "XFG-020"
    );
    assert!(body.contains("明确失败，绝不静默创建"));
}

// ——————————————————————————————— 配套四样 ———————————————————————————————

#[test]
fn 我们自己的检查能检出第三样和第四样缺失() {
    // `xforge doctor` 对未被引用的扩展资源**只警告、从不阻塞**，
    // 所以 ④ 必须有一个我们自己的检查（XFG-022）。
    let complete = Sources {
        mcp_server: scaffold::mcp_server_resource(
            "xops-approvals",
            "https://xops.example/mcp",
            "XOPS_TOKEN",
        ),
        manifest: "scaffold:\n  mcpServers:\n    - xops-approvals\n".into(),
        approvals: scaffold::provider_entry("xops", "xops-approvals"),
        flows: scaffold::flow_reference("xops"),
    };
    assert!(scaffold::missing("xops", "xops-approvals", &complete).is_empty());

    let mut unreferenced = complete.clone();
    unreferenced.flows = "approvalPolicies:\n  - id: release-approval\n    providers: []\n".into();
    assert_eq!(
        scaffold::missing("xops", "xops-approvals", &unreferenced),
        vec![Piece::FlowReference],
        "provider 装好了、连得上、却没有任何一条 Flow 引用它"
    );
}

#[test]
fn 交付的四样文件都在仓里() {
    // XFG-021 / XFG-023：**这是持续交付物，不是一次性配置说明。**
    for path in [
        "../../xforge/xops-approvals.mcpserver.yaml",
        "../../xforge/manifest.snippet.yaml",
        "../../xforge/approvals.snippet.yaml",
        "../../xforge/flow.snippet.yaml",
    ] {
        let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        assert!(full.exists(), "缺一样门就不存在：{path}");
    }
}

// ——————————————————————————————— 跑不了的两条 ———————————————————————————————

/// **这两条在本仓跑不了，写出来而不是假装通过。**
///
/// ```text
/// XFG-024  真实的 `xforge approve --provider xops` 端到端
/// XFG-020  关停 XOps 之后跑 `xforge approve`，报连接失败可重试
/// ```
///
/// 两条都要**一个跑起来的 XOps 服务**加**一份装好的 XForge**：
/// 本仓还没有可执行入口（`xopsd` 未建），XForge 的契约治理版本也还没发布。
/// 本包能做到的是**让降级逻辑不存在**（见 `查不到就明确失败而不是当作通过`），
/// 而"断网时到底发生什么"由 XForge 侧的传输层决定——**那件事必须实际断网测一次**。
#[test]
fn 端到端与断网这两条要在真环境里补() {
    let pending = ["XFG-024 真实 xforge approve", "XFG-020 断网测试"];
    assert_eq!(pending.len(), 2, "它们还欠着，别忘了");
}
