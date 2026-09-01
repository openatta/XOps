//! RP-16 的验收。
//!
//! 分三组，对应 `G11` 的两半再加一条流程侧：
//!
//! ```text
//! 能力    **必须失败在「函数不存在」而不是「被拒绝」** —— 这是本包的核心验收
//! 载体    死循环 · 异常 · 跨调用无状态 · 内存上限 · 故障只花一次调用
//! 治理    谁能装 · 披露不可跳过 · 改能力 = 新版本 · 源码成员可读 · 配置读不出来
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;
use xops_audit::AuditLog;
use xops_core::{Role, SystemClock, TableName};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_repo::Sealer;
use xops_script::capability::{Capabilities, Position};
use xops_script::carrier::{Grant, Outcome, invoke};
use xops_script::net::Denied;
use xops_script::plugin::{Case, State};
use xops_script::service::{Deps, Plugins};
use xops_script::{Plugin, generate};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};
use xops_table::engine::Catalog;
use xops_table::table::{Protection, TableId};
use xops_table::{Column, ColumnType, Tables};

struct Fixture {
    label: &'static str,
    plugins: Arc<Plugins>,
    directory: Arc<Directory>,
    tables: Arc<Tables>,
}

fn fixtures() -> Vec<Fixture> {
    [
        ("memory", Arc::new(MemoryStore::new()) as Arc<dyn Store>),
        ("sqlite", Arc::new(SqliteStore::in_memory().unwrap())),
    ]
    .into_iter()
    .map(|(label, store)| {
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
        audit = audit.watching(TableName::new(xops_table::CATALOG_TABLE).unwrap());
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
        let plugins = Arc::new(Plugins::new(Deps {
            tables: Arc::clone(&tables),
            store: Arc::clone(&store),
            audit,
            directory: Arc::clone(&directory),
            sealer: Arc::new(Sealer::from_key(&[7u8; 32]).unwrap()),
            net: Arc::new(Denied),
            clock,
        }));
        Fixture {
            label,
            plugins,
            directory,
            tables,
        }
    })
    .collect()
}

struct Scene {
    owner: UserId,
    maintainer: UserId,
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
    let maintainer = user("maintainer");
    let member = user("member");
    let project = fixture
        .directory
        .create_project(owner, Slug::new("acme").unwrap(), "Acme")
        .unwrap()
        .id;
    fixture
        .directory
        .set_member(owner, project, maintainer, Role::Maintainer)
        .unwrap();
    fixture
        .directory
        .set_member(owner, project, member, Role::Member)
        .unwrap();
    fixture
        .tables
        .ensure_system_tables(project, "acme")
        .unwrap();
    fixture
        .tables
        .create(
            owner,
            project,
            TableId::user("bugs").unwrap(),
            Protection::Normal,
            vec![Column::new("status", ColumnType::Text { max_len: 16 }, false).unwrap()],
        )
        .unwrap();
    Scene {
        owner,
        maintainer,
        member,
        project,
    }
}

/// 一个什么都不干、只回 `{ok:true}` 的插件。
const TRIVIAL: &str = "function run() { return { ok: true }; }";

fn candidate(
    fixture: &Fixture,
    scene: &Scene,
    name: &str,
    version: u32,
    position: Position,
    capabilities: Capabilities,
) -> Plugin {
    let generated = generate(
        scene.project,
        name,
        version,
        position,
        "run",
        TRIVIAL,
        capabilities,
        vec![Case {
            name: "跑得起来".into(),
            input: json!({}),
            expected: json!({"ok": true}),
        }],
        None,
        Some("RUN-1 / skill@3".into()),
    )
    .unwrap();
    fixture
        .plugins
        .record_candidate(scene.member, generated)
        .unwrap()
}

fn install(fixture: &Fixture, scene: &Scene, plugin: &Plugin) -> Plugin {
    let disclosure = plugin.capabilities.disclose();
    fixture
        .plugins
        .install(
            scene.maintainer,
            scene.project,
            &plugin.name,
            plugin.version,
            &disclosure,
        )
        .unwrap()
}

// ——————————————————————————————— 能力 ———————————————————————————————

#[test]
fn 未声明的能力失败在函数不存在而不是被拒绝() {
    // 核心验收：**探针问的是 `typeof`，不是「调用它会不会报错」**。
    for (what, probe) in [
        ("出网", "fetch"),
        ("读表", "globalThis.xops"),
        ("触发任务", "globalThis.trigger"),
        ("文件", "require"),
        ("进程", "process"),
    ] {
        let source = format!("function run() {{ return {{ kind: typeof ({probe}) }}; }}");
        let outcome = invoke(&source, "run", &json!({}), Position::Output, &Grant::none()).unwrap();
        assert_eq!(
            outcome.value().unwrap()["kind"],
            json!("undefined"),
            "{what}：它不该是「被拒绝」，它该是不存在"
        );
    }
}

#[test]
fn 流转插件尝试任何io都不存在() {
    // 它**没有任何可声明项**，所以连"声明了才注入"这条路都不存在。
    let source = "function run() { return { \
                  xops: typeof (globalThis.xops), \
                  fetch: typeof (globalThis.fetch) }; }";
    let outcome = invoke(
        source,
        "run",
        &json!({}),
        Position::Transition,
        &Grant::none(),
    )
    .unwrap();
    let value = outcome.value().unwrap().clone();
    assert_eq!(value["xops"], json!("undefined"));
    assert_eq!(value["fetch"], json!("undefined"));
}

#[test]
fn 任何插件都读不到通知表() {
    let notices = TableId::system(xops_table::system::NOTICES).unwrap();
    let declared = Capabilities {
        tables: vec![notices.clone()],
        ..Capabilities::none()
    };
    // 连声明都过不去 —— **它不在可声明之列**（NTF-012）。
    assert!(declared.check(Position::Output).is_err());
    assert!(!declared.allows_table(&notices));
}

#[test]
fn 声明之外的表连不上() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let plugin = candidate(
            &fixture,
            &scene,
            "reader",
            1,
            Position::Output,
            Capabilities {
                tables: vec![TableId::user("bugs").unwrap()],
                ..Capabilities::none()
            },
        );
        install(&fixture, &scene, &plugin);
        let host = fixture
            .plugins
            .host_for(scene.project, "reader", 1)
            .unwrap();
        let grant = Grant {
            capabilities: plugin.capabilities.clone(),
            host: Some(host),
        };
        let mine = invoke(
            "function run() { return { rows: xops.readTable('bugs', 10).rows.length }; }",
            "run",
            &json!({}),
            Position::Output,
            &grant,
        )
        .unwrap();
        assert!(mine.value().is_some(), "{}：声明过的读得到", fixture.label);
        let other = invoke(
            "function run() { return xops.readTable('_flows', 10); }",
            "run",
            &json!({}),
            Position::Output,
            &grant,
        )
        .unwrap();
        assert!(
            matches!(other, Outcome::Threw(_)),
            "{}：声明之外的表够不到",
            fixture.label
        );
    }
}

// ——————————————————————————————— 治理 ———————————————————————————————

#[test]
fn 非维护者装不了候选() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let plugin = candidate(
            &fixture,
            &scene,
            "gate",
            1,
            Position::Transition,
            Capabilities::none(),
        );
        let error = fixture
            .plugins
            .install(
                scene.member,
                scene.project,
                "gate",
                1,
                &plugin.capabilities.disclose(),
            )
            .unwrap_err();
        let message = format!("{error}");
        assert!(
            message.contains("权限") || message.contains("不存在") || message.contains("不够"),
            "{}：成员装不了（PLG-008），实际是 {message}",
            fixture.label
        );
        assert!(install(&fixture, &scene, &plugin).usable(), "维护者可以");
    }
}

#[test]
fn 披露不可跳过() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let plugin = candidate(
            &fixture,
            &scene,
            "poster",
            1,
            Position::Output,
            Capabilities {
                network: vec!["hooks.example".into()],
                own_config: true,
                ..Capabilities::none()
            },
        );
        // ① 一句都不看就装。
        let error = fixture
            .plugins
            .install(scene.maintainer, scene.project, "poster", 1, &[])
            .unwrap_err();
        assert!(
            format!("{error}").contains("披露对不上"),
            "{}：不看披露直接装必须失败",
            fixture.label
        );
        // ② 只抄一半。
        let half = vec![plugin.capabilities.disclose()[0].clone()];
        assert!(
            fixture
                .plugins
                .install(scene.maintainer, scene.project, "poster", 1, &half)
                .is_err(),
            "{}：抄一半也不行",
            fixture.label
        );
        // ③ 逐条抄全才装得上。
        assert!(install(&fixture, &scene, &plugin).usable());
    }
}

#[test]
fn 改能力是新版本而不是给已装的加一项() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let first = candidate(
            &fixture,
            &scene,
            "poster",
            1,
            Position::Output,
            Capabilities::none(),
        );
        install(&fixture, &scene, &first);

        // **不存在「给已安装的插件加一项能力」这条路径**：
        // 能力声明只在生成候选时给得进去，而同名同版本的候选建不了第二个。
        let again = generate(
            scene.project,
            "poster",
            1,
            Position::Output,
            "run",
            TRIVIAL,
            Capabilities {
                network: vec!["evil.example".into()],
                ..Capabilities::none()
            },
            vec![],
            None,
            None,
        )
        .unwrap();
        let error = fixture
            .plugins
            .record_candidate(scene.member, again)
            .unwrap_err();
        assert!(
            format!("{error}").contains("已经有了"),
            "{}：同一个版本改不了能力",
            fixture.label
        );

        // 新版本可以，而且它**要重新走一次披露**。
        let second = candidate(
            &fixture,
            &scene,
            "poster",
            2,
            Position::Output,
            Capabilities {
                network: vec!["hooks.example".into()],
                ..Capabilities::none()
            },
        );
        assert!(
            fixture
                .plugins
                .install(
                    scene.maintainer,
                    scene.project,
                    "poster",
                    2,
                    &first.capabilities.disclose(),
                )
                .is_err(),
            "{}：拿旧版本的披露装新版本，不行",
            fixture.label
        );
        assert!(install(&fixture, &scene, &second).usable());

        // 老版本原样还在 —— **引用它的流程节点用固定版本，不跟随最新**。
        let version_one = fixture.plugins.resolve(scene.project, "poster", 1).unwrap();
        assert!(version_one.capabilities.is_empty());
    }
}

#[test]
fn 源码与能力声明对全体成员可读() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let plugin = candidate(
            &fixture,
            &scene,
            "gate",
            1,
            Position::Transition,
            Capabilities::none(),
        );
        install(&fixture, &scene, &plugin);
        // 一个既不是维护者也不是所有者的普通成员。
        let read = fixture
            .plugins
            .read(scene.member, scene.project, "gate", 1)
            .unwrap();
        assert_eq!(read.source, TRIVIAL, "{}：I-T", fixture.label);
        assert!(!read.capabilities.disclose().is_empty());
        // 候选也一样看得到（PLG-010 末句）。
        let draft = candidate(
            &fixture,
            &scene,
            "gate",
            2,
            Position::Transition,
            Capabilities::none(),
        );
        assert_eq!(draft.state, State::Candidate);
        assert!(
            fixture
                .plugins
                .read(scene.member, scene.project, "gate", 2)
                .is_ok()
        );
    }
}

#[test]
fn 用例不过的产不出候选更装不上() {
    let failing = generate(
        ProjectId::generate(),
        "gate",
        1,
        Position::Transition,
        "run",
        "function run() { return { ok: false }; }",
        Capabilities::none(),
        vec![Case {
            name: "要求 true".into(),
            input: json!({}),
            expected: json!({"ok": true}),
        }],
        None,
        None,
    );
    assert!(failing.is_err(), "I-K：三样全过才产出候选");
}

// ——————————————————————————————— 配置 ———————————————————————————————

#[test]
fn 配置任何接口都读不出原文且不在插件表里() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let plugin = candidate(
            &fixture,
            &scene,
            "poster",
            1,
            Position::Output,
            Capabilities {
                own_config: true,
                ..Capabilities::none()
            },
        );
        install(&fixture, &scene, &plugin);
        let config = BTreeMap::from([("webhook".to_owned(), "https://hooks/s3cr3t".to_owned())]);

        // ① 非所有者写不了。
        assert!(
            fixture
                .plugins
                .write_config(scene.maintainer, scene.project, "poster", &config)
                .is_err(),
            "{}：读写配置是所有者的事（PLG-008）",
            fixture.label
        );
        fixture
            .plugins
            .write_config(scene.owner, scene.project, "poster", &config)
            .unwrap();

        // ② 所有者自己也只读得到键名。
        let keys = fixture
            .plugins
            .config_keys(scene.owner, scene.project, "poster")
            .unwrap();
        assert_eq!(keys, vec!["webhook".to_owned()]);
        assert!(
            fixture
                .plugins
                .config_keys(scene.member, scene.project, "poster")
                .is_err()
        );

        // ③ 它不在 `_plugins` 表里 —— 那张表可查询，把凭据放进去等于公开。
        let rows = fixture
            .tables
            .rows(
                Some(scene.project),
                &TableId::system(xops_table::system::PLUGINS).unwrap(),
                64,
            )
            .unwrap();
        let dumped =
            serde_json::to_string(&rows.iter().map(|(_, row)| row).collect::<Vec<_>>()).unwrap();
        assert!(
            !dumped.contains("s3cr3t"),
            "{}：配置不落在 _plugins 表里（PLG-015）",
            fixture.label
        );

        // ④ 但它注入得进去 —— **只注入给声明了这项能力的那个插件自己**。
        let host = fixture
            .plugins
            .host_for(scene.project, "poster", 1)
            .unwrap();
        let got = invoke(
            "function run() { return xops.config(); }",
            "run",
            &json!({}),
            Position::Output,
            &Grant {
                capabilities: plugin.capabilities.clone(),
                host: Some(host),
            },
        )
        .unwrap();
        assert_eq!(
            got.value().unwrap()["webhook"],
            json!("https://hooks/s3cr3t"),
            "{}",
            fixture.label
        );
    }
}

#[test]
fn 没声明读配置的插件注入不到它() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let plugin = candidate(
            &fixture,
            &scene,
            "quiet",
            1,
            Position::Output,
            Capabilities::none(),
        );
        install(&fixture, &scene, &plugin);
        fixture
            .plugins
            .write_config(
                scene.owner,
                scene.project,
                "quiet",
                &BTreeMap::from([("k".to_owned(), "v".to_owned())]),
            )
            .unwrap();
        let host = fixture.plugins.host_for(scene.project, "quiet", 1).unwrap();
        let outcome = invoke(
            "function run() { return { has: typeof (globalThis.xops) }; }",
            "run",
            &json!({}),
            Position::Output,
            &Grant {
                capabilities: plugin.capabilities.clone(),
                host: Some(host),
            },
        )
        .unwrap();
        assert_eq!(
            outcome.value().unwrap()["has"],
            json!("undefined"),
            "{}：没声明就没有那个函数",
            fixture.label
        );
    }
}

// ——————————————————————————————— 载体 ———————————————————————————————

#[test]
fn 故障只花一次调用() {
    let bad = "function run() { while (true) {} }";
    for _ in 0..3 {
        let failed = invoke(bad, "run", &json!({}), Position::Transition, &Grant::none()).unwrap();
        assert_eq!(failed, Outcome::TimedOut);
        let next = invoke(
            TRIVIAL,
            "run",
            &json!({}),
            Position::Transition,
            &Grant::none(),
        )
        .unwrap();
        assert_eq!(next.value().unwrap()["ok"], json!(true), "下一次照常");
    }
}

#[test]
fn 停用之后引用不了() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let plugin = candidate(
            &fixture,
            &scene,
            "gate",
            1,
            Position::Transition,
            Capabilities::none(),
        );
        install(&fixture, &scene, &plugin);
        assert!(fixture.plugins.resolve(scene.project, "gate", 1).is_ok());
        fixture
            .plugins
            .disable(scene.maintainer, scene.project, "gate", 1)
            .unwrap();
        assert!(
            fixture.plugins.resolve(scene.project, "gate", 1).is_err(),
            "{}",
            fixture.label
        );
        // 历史完整保留。
        let history = fixture
            .plugins
            .history(scene.member, scene.project, "gate")
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].state, State::Disabled);
    }
}

#[test]
fn 生成执行的那条run行没了插件行还自包含() {
    // RET-009 的已知悬空：`generatedBy` 指向的 `_runs` 行会到期被清理。
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let plugin = candidate(
            &fixture,
            &scene,
            "gate",
            1,
            Position::Transition,
            Capabilities::none(),
        );
        install(&fixture, &scene, &plugin);
        let read = fixture
            .plugins
            .read(scene.member, scene.project, "gate", 1)
            .unwrap();
        // 源码、能力声明、用例、结果、安装人 —— 五样一样不缺，都不用去查 `_runs`。
        assert!(!read.source.is_empty());
        assert!(!read.capabilities.disclose().is_empty());
        assert!(!read.cases.is_empty());
        assert!(read.cases_all_passed());
        assert!(read.installed_by.is_some());
        assert_eq!(read.generated_by.as_deref(), Some("RUN-1 / skill@3"));
    }
}

/// 载体里**一句注入宿主绑定的代码都不该在「没声明」那条分支上**。
///
/// 这条枚举的是源码本身：`carrier.rs` 里每一处 `globals.set` 注入绑定的地方，
/// 都必须落在一个 `if capabilities.…` 里面。
#[test]
fn 绑定的注入全都挂在声明后面() {
    let source = include_str!("../src/carrier.rs");
    let body = source.split("#[cfg(test)]").next().unwrap();
    let mut guards = 0;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("if capabilities.") || trimmed.starts_with("if !capabilities.") {
            guards += 1;
        }
        if trimmed.starts_with("globals.set(") {
            assert!(guards > 0, "有一处绑定注入不在能力判断里面：{trimmed}");
        }
    }
    assert_eq!(guards, 3, "三样能力各有一道判断，不多不少");
}
