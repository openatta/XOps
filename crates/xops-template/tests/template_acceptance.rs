//! RP-18 的验收。
//!
//! 最要紧的一条是**这个包什么都不新增**：建表走 RP-04、建流程走 RP-14、
//! 装插件走 RP-16。最后一个测试枚举本包源码来证明这件事。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_audit::AuditLog;
use xops_core::{Id, Role, SystemClock, TableName};
use xops_flow::instance::Subject;
use xops_flow::{Definition, Flows};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_repo::Sealer;
use xops_script::plugin::State;
use xops_script::service::{Deps, Plugins};
use xops_script::{Position, TransitionInput, Verdict as PluginVerdict, evaluate_transition};
use xops_settle::protection::{Origin, check_for};
use xops_settle::{Evaluator, Verdict, WriterCheck, Written};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};
use xops_table::engine::Catalog;
use xops_table::table::{Protection, TableId};
use xops_table::{Column, ColumnType, Tables, WrittenBy};
use xops_template::template::{ColumnSpec, TableSpec};
use xops_template::{Template, Templates};

struct Fixture {
    label: &'static str,
    templates: Arc<Templates>,
    tables: Arc<Tables>,
    flows: Arc<Flows>,
    plugins: Arc<Plugins>,
    directory: Arc<Directory>,
    evaluator: Evaluator,
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
    let mut audit = AuditLog::new(Arc::clone(&engine), Arc::clone(&store), Arc::clone(&relations)).unwrap();
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
        Arc::clone(&catalog),
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
    let plugins = Arc::new(Plugins::new(Deps {
        tables: Arc::clone(&tables),
        store: Arc::clone(&store),
        audit: Arc::clone(&audit),
        directory: Arc::clone(&directory),
        sealer: Arc::new(Sealer::from_key(&[9u8; 32]).unwrap()),
        net: Arc::new(xops_script::net::Denied),
        clock: clock.clone(),
    }));
    let evaluator = Evaluator::new(
        Arc::clone(&flows),
        Arc::new(WriterCheck::new(
            Arc::clone(&directory),
            Arc::clone(&tables),
        )),
    );
    let templates = Arc::new(Templates::new(
        Arc::clone(&tables),
        Arc::clone(&flows),
        Arc::clone(&plugins),
        Arc::clone(&directory),
    ));
    Fixture {
        label,
        templates,
        tables,
        flows,
        plugins,
        directory,
        evaluator,
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
}

fn scene(fixture: &Fixture) -> Scene {
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
    Scene {
        owner,
        member,
        project,
    }
}

// ——————————————————————————————— 一步实例化 ———————————————————————————————

#[test]
fn 一步建表建流程装插件() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let done = fixture
            .templates
            .instantiate(scene.owner, scene.project, "bugs")
            .unwrap();
        assert_eq!(done.tables.len(), 2, "{}", fixture.label);
        assert!(done.flow.is_some());
        assert_eq!(done.plugins, vec![("bugs".to_owned(), 1)]);

        // 三样都真的在。
        for name in ["bugs", "bug-events"] {
            assert!(
                fixture
                    .tables
                    .describe(scene.owner, scene.project, &TableId::user(name).unwrap())
                    .is_ok(),
                "{}：{name}",
                fixture.label
            );
        }
        assert_eq!(
            fixture
                .flows
                .list(scene.owner, scene.project)
                .unwrap()
                .len(),
            1
        );
        let plugin = fixture.plugins.resolve(scene.project, "bugs", 1).unwrap();
        assert_eq!(plugin.state, State::Installed);
    }
}

#[test]
fn 模板带的插件走的是正常的候选与安装流程() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        fixture
            .templates
            .instantiate(scene.owner, scene.project, "bugs")
            .unwrap();
        let plugin = fixture
            .plugins
            .read(scene.member, scene.project, "bugs", 1)
            .unwrap();
        // I-K：用例真的跑过，而且过了。
        assert!(plugin.cases_all_passed(), "{}", fixture.label);
        assert!(!plugin.cases.is_empty());
        // PLG-011：装它的人记下来了，来源也记下来了。
        assert_eq!(plugin.installed_by, Some(scene.owner));
        assert_eq!(plugin.generated_by.as_deref(), Some("template:bugs"));
        // PLG-010：源码与能力声明对成员可读。
        assert!(plugin.source.contains("function decide"));
    }
}

#[test]
fn 装插件那一步要维护者所以成员实例化不了() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let error = fixture
            .templates
            .instantiate(scene.member, scene.project, "bugs")
            .unwrap_err();
        assert!(
            !format!("{error}").is_empty(),
            "{}：不为模板开一条更松的路",
            fixture.label
        );
        // 而且**一张表都没建出来**——预检在动手之前就失败了。
        assert!(
            fixture
                .tables
                .describe(scene.owner, scene.project, &TableId::user("bugs").unwrap())
                .is_err(),
            "{}",
            fixture.label
        );
    }
}

#[test]
fn 中途失败不留下半套东西() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        // 第二张表的列名是非法的 —— 第一张已经建出来了，第二张失败。
        let broken = Template {
            name: "broken".into(),
            summary: "第二张表会失败".into(),
            tables: vec![
                TableSpec {
                    name: "first".into(),
                    protection: Protection::Normal,
                    columns: vec![ColumnSpec {
                        name: "title".into(),
                        ty: ColumnType::Text { max_len: 64 },
                        required: false,
                    }],
                },
                TableSpec {
                    name: "second".into(),
                    protection: Protection::Normal,
                    columns: vec![ColumnSpec {
                        // `writtenBy` 是平台自动补的列位，声明它会被拒（TBL-014）。
                        name: "writtenBy".into(),
                        ty: ColumnType::Text { max_len: 64 },
                        required: false,
                    }],
                },
            ],
            flow: None,
            plugins: vec![],
        };
        assert!(
            fixture
                .templates
                .instantiate_template(scene.owner, scene.project, &broken)
                .is_err(),
            "{}",
            fixture.label
        );
        // ③ 兜底：第一张表被撤掉了。
        assert!(
            fixture
                .tables
                .describe(scene.owner, scene.project, &TableId::user("first").unwrap())
                .is_err(),
            "{}：不留下半套",
            fixture.label
        );
    }
}

#[test]
fn 撞名明确失败而不是覆盖() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        fixture
            .templates
            .instantiate(scene.owner, scene.project, "bugs")
            .unwrap();
        let error = fixture
            .templates
            .instantiate(scene.owner, scene.project, "bugs")
            .unwrap_err();
        assert!(format!("{error}").contains("不覆盖"), "{}", fixture.label);
    }
}

#[test]
fn 三套互不干扰() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        for name in ["bugs", "issues", "approvals"] {
            fixture
                .templates
                .instantiate(scene.owner, scene.project, name)
                .unwrap_or_else(|error| panic!("{}：{name} 装不上：{error}", fixture.label));
        }
        let user_tables: Vec<String> = fixture
            .tables
            .list(scene.owner, scene.project)
            .unwrap()
            .into_iter()
            .filter(|schema| !schema.name.is_system())
            .map(|schema| schema.name.to_string())
            .collect();
        assert_eq!(
            user_tables.len(),
            5,
            "{}：bugs·bug-events·issues·issue-events·approvals，实际是 {user_tables:?}",
            fixture.label
        );
        assert_eq!(
            fixture
                .flows
                .list(scene.owner, scene.project)
                .unwrap()
                .len(),
            3
        );
    }
}

// ——————————————————————————————— 实例化之后就是普通对象 ———————————————————————————————

#[test]
fn 实例化之后加得了列换得了插件() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        fixture
            .templates
            .instantiate(scene.owner, scene.project, "bugs")
            .unwrap();
        // 加一列 —— 普通表。
        fixture
            .tables
            .add_column(
                scene.owner,
                scene.project,
                &TableId::user("bugs").unwrap(),
                Column::new("severity", ColumnType::Text { max_len: 16 }, false).unwrap(),
            )
            .unwrap();
        // 换插件 —— **仍需维护者**（TPL-004）。
        let replacement = xops_script::generate(
            scene.project,
            "bugs",
            2,
            Position::Transition,
            "decide",
            "function decide() { return { verdict: 'fail', writes: [] }; }",
            xops_script::Capabilities::none(),
            // **没有用例就装不上**（PLG-006）：空的用例集不算"全过"。
            vec![xops_script::plugin::Case {
                name: "一律不结算".into(),
                input: json!({}),
                expected: json!({"verdict": "fail", "writes": []}),
            }],
            None,
            None,
        )
        .unwrap();
        let candidate = fixture
            .plugins
            .record_candidate(scene.member, replacement)
            .unwrap();
        assert!(
            fixture
                .plugins
                .install(
                    scene.member,
                    scene.project,
                    "bugs",
                    2,
                    &candidate.capabilities.disclose()
                )
                .is_err(),
            "{}：换插件仍需维护者",
            fixture.label
        );
        fixture
            .plugins
            .install(
                scene.owner,
                scene.project,
                "bugs",
                2,
                &candidate.capabilities.disclose(),
            )
            .unwrap();
        // 老版本原样还在 —— 引用它的节点用固定版本。
        assert!(fixture.plugins.resolve(scene.project, "bugs", 1).is_ok());
    }
}

// ——————————————————————————————— 缺陷 ID ———————————————————————————————

#[test]
fn 缺陷id是项目短名加序号而且insert之后改不了() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        fixture
            .templates
            .instantiate(scene.owner, scene.project, "bugs")
            .unwrap();
        let bugs = TableId::user("bugs").unwrap();
        let by = WrittenBy::Person { user: scene.member };

        let first = fixture
            .tables
            .insert(
                &by,
                Some(scene.project),
                &bugs,
                json!({"title": "登录页白屏", "status": "新建"}),
            )
            .unwrap();
        let second = fixture
            .tables
            .insert(
                &by,
                Some(scene.project),
                &bugs,
                json!({"title": "导出乱码", "status": "新建"}),
            )
            .unwrap();

        let read = |row| {
            fixture
                .tables
                .get(Some(scene.project), &bugs, row)
                .unwrap()
                .unwrap()
        };
        assert_eq!(
            read(first)["ref"],
            json!("acme-1"),
            "{}：TPL-005",
            fixture.label
        );
        assert_eq!(read(second)["ref"], json!("acme-2"), "序号连续");

        // **insert 时生成一次、之后 update 写不了它**（TBL-020）。
        assert!(
            fixture
                .tables
                .update(
                    &by,
                    Some(scene.project),
                    &bugs,
                    first,
                    json!({"ref": "acme-999"})
                )
                .is_err(),
            "{}",
            fixture.label
        );
        assert_eq!(read(first)["ref"], json!("acme-1"), "还是原来那个");
    }
}

// ——————————————————————————————— bugs 全程 ———————————————————————————————

#[test]
fn 一条bug从插入到状态被流转插件写成新状态() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture);
        fixture
            .templates
            .instantiate(scene.owner, scene.project, "bugs")
            .unwrap();
        let bugs = TableId::user("bugs").unwrap();
        let events = TableId::user("bug-events").unwrap();

        // ① 插一条 bug。
        let bug = fixture
            .tables
            .insert(
                &WrittenBy::Person { user: scene.member },
                Some(scene.project),
                &bugs,
                json!({"title": "登录页白屏", "status": "新建"}),
            )
            .unwrap();

        // ② 随行自动发起。
        let definition: Definition = fixture
            .flows
            .list(scene.owner, scene.project)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let mut instance = fixture
            .flows
            .start_automatically(
                &definition,
                scene.member,
                Subject {
                    kind: "row".into(),
                    id: bug.to_string(),
                    revision: None,
                },
            )
            .unwrap()
            .expect("随行发起要真的发起一个实例");

        // ③ 有人往结算表写一行表态。`_instance` 由平台代填。
        let settlement_values = json!({
            "decision": "确认",
            "reason": "复现了",
            "_instance": instance.id.to_string(),
        });
        let settlement_row = fixture
            .tables
            .insert(
                &WrittenBy::Person { user: scene.member },
                Some(scene.project),
                &events,
                settlement_values.clone(),
            )
            .unwrap();

        // ④ 七条判定里 ②～⑦ 由平台先判完（FLW-028）。
        let node = definition.node(0, "分诊").unwrap();
        let written = Written {
            values: settlement_values,
            written_by: WrittenBy::Person { user: scene.member },
            row: settlement_row.to_string(),
        };
        let verdict = fixture
            .evaluator
            .judge(&definition, &instance, node, &written)
            .unwrap();
        assert_eq!(verdict, Verdict::Settle, "{label}：②～⑦ 都过");

        // ⑤ 「满足筛选」那一半交给流转插件。输入由平台预取（PLG-002）。
        let plugin = fixture.plugins.resolve(scene.project, "bugs", 1).unwrap();
        let related: Vec<Value> = fixture
            .tables
            .rows(Some(scene.project), &events, 50)
            .unwrap()
            .into_iter()
            .map(|(_, values)| values)
            .collect();
        let settled = evaluate_transition(
            &plugin,
            &TransitionInput {
                instance: json!({"subjectRow": bug.to_string()}),
                row: json!({"decision": "确认"}),
                related: json!(related),
            },
            &events,
            Some(&bugs),
        )
        .unwrap();
        assert_eq!(settled.verdict, PluginVerdict::Pass, "{label}");
        assert_eq!(settled.writes.len(), 1, "{label}：交回一行给平台代写");
        assert_eq!(settled.writes[0].table, bugs);

        // ⑥ **由平台代写**——插件自己写不了任何表（I-R）。
        let write = &settled.writes[0];
        fixture
            .tables
            .update(
                &WrittenBy::Plugin {
                    plugin: "bugs".into(),
                    version: "1".into(),
                    installed_by: scene.owner,
                    instance: instance.id.as_id(),
                },
                Some(scene.project),
                &bugs,
                bug,
                write.values.clone(),
            )
            .unwrap();

        // ⑦ 驱动状态机。
        fixture
            .evaluator
            .apply(
                &mut instance,
                node,
                &verdict,
                &written,
                xops_core::Timestamp::from_millis(1),
            )
            .unwrap();

        // 链路走完：状态变了，实例到了终态。
        let after = fixture
            .tables
            .get(Some(scene.project), &bugs, bug)
            .unwrap()
            .unwrap();
        assert_eq!(after["status"], json!("已确认"), "{label}：流转插件写的");
        assert_eq!(
            fixture.flows.instance(instance.id).unwrap().state,
            xops_flow::instance::InstanceState::Approved,
            "{label}"
        );

        // 而且**用户自己改不了状态列**（FLW-036 / I-P）。
        assert!(
            check_for(
                &definition,
                Origin::User,
                &json!({"status": "已关闭"}),
                false
            )
            .is_err(),
            "{label}：任何成员都能直接改状态的话，整条流程就白搭了"
        );
        assert!(
            check_for(
                &definition,
                Origin::Platform,
                &json!({"status": "已关闭"}),
                false
            )
            .is_ok(),
            "{label}：平台与流转插件写得了"
        );
    }
}

// ——————————————————————————————— approvals ———————————————————————————————

#[test]
fn approvals无主体表且理由必填由插件承接() {
    for fixture in fixtures() {
        let label = fixture.label;
        let scene = scene(&fixture);
        fixture
            .templates
            .instantiate(scene.owner, scene.project, "approvals")
            .unwrap();
        let approvals = TableId::user("approvals").unwrap();
        let definition = fixture
            .flows
            .list(scene.owner, scene.project)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert!(definition.subject_table.is_none(), "{label}");
        assert!(
            definition.status_columns.is_empty(),
            "没有主体表就没有状态列"
        );

        let plugin = fixture
            .plugins
            .resolve(scene.project, "approvals", 1)
            .unwrap();
        let judge = |rows: Value| {
            evaluate_transition(
                &plugin,
                &TransitionInput {
                    instance: json!({}),
                    row: json!({}),
                    related: rows,
                },
                &approvals,
                None,
            )
            .unwrap()
            .verdict
        };

        // **空理由被拒**——不结算（TPL-006）。
        assert_eq!(
            judge(json!([{"decision": "批准", "reason": ""}])),
            PluginVerdict::Fail,
            "{label}：决策必须附带理由"
        );
        assert_eq!(
            judge(json!([{"decision": "批准", "reason": "   "}])),
            PluginVerdict::Fail,
            "{label}：全是空白也不算"
        );
        assert_eq!(
            judge(json!([{"decision": "批准", "reason": "看过了"}])),
            PluginVerdict::Pass,
            "{label}"
        );
        assert_eq!(
            judge(json!([{"decision": "驳回", "reason": "预算不够"}])),
            PluginVerdict::Reject,
            "{label}"
        );

        // 而且平台自己**不认识「理由」这个概念**：reason 列不是必填的。
        let schema = fixture
            .tables
            .describe(scene.owner, scene.project, &approvals)
            .unwrap();
        assert!(
            !schema.column("reason").unwrap().required,
            "{label}：平台的 required 只挡「没有这个键」，挡不住空串与全空白"
        );
    }
}

// ——————————————————————————————— 不新增机制 ———————————————————————————————

/// 枚举本包源码：**没有任何一处在 RP-04/RP-14/RP-15/RP-16 之外实现表、流程或插件的能力。**
#[test]
fn 本包不新增任何机制() {
    let files = [
        ("template.rs", include_str!("../src/template.rs")),
        ("catalog.rs", include_str!("../src/catalog.rs")),
        ("service.rs", include_str!("../src/service.rs")),
        ("tools.rs", include_str!("../src/tools.rs")),
    ];
    for (name, source) in files {
        let body = source.split("#[cfg(test)]").next().unwrap();
        // 建表、写行、发事件这些事，只能经别人的接口做。
        for own in [
            "WriteEngine",
            "impl Store",
            "impl PreWrite",
            "impl SchemaCheck",
        ] {
            assert!(
                !body.contains(own),
                "{name} 里出现了 {own}——那是 RP-01 的活"
            );
        }
        for own in ["rquickjs", "fn invoke(", "Runtime::new"] {
            assert!(
                !body.contains(own),
                "{name} 里出现了 {own}——那是 RP-16 的活"
            );
        }
    }
    // 依赖面就是那四个包，一个不多。
    let manifest = include_str!("../Cargo.toml");
    let deps = manifest
        .split("[dev-dependencies]")
        .next()
        .unwrap_or_default();
    for forbidden in ["xops-store", "xops-audit", "xops-exec", "xops-repo"] {
        assert!(
            !deps.contains(forbidden),
            "本包不该直接依赖 {forbidden}——它只组合表 / 流程 / 插件三样"
        );
    }
}

/// 让 `Id` 在这个文件里有个用处。
fn _unused(id: Id) -> String {
    id.to_string()
}
