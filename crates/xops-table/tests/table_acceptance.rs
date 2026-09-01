//! RP-04 的验收。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_audit::AuditLog;
use xops_core::{Role, SystemClock, TableName};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_mcp::{Capabilities, McpServer, WhoAmI};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};
use xops_table::engine::Catalog;
use xops_table::table::{Kind, Protection, TableId};
use xops_table::{Column, ColumnType, Tables, WrittenBy};

struct Fixture {
    label: &'static str,
    tables: Arc<Tables>,
    directory: Arc<Directory>,
    server: Arc<McpServer>,
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
        let audit = Arc::new(audit.watching(TableName::new(xops_table::CATALOG_TABLE).unwrap()));
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
            clock,
            Arc::clone(&store),
        ));
        let mut server = McpServer::new(
            Arc::clone(&directory),
            Arc::clone(&audit),
            Arc::clone(&store),
        );
        let registry = server.registry_mut();
        registry.register(Arc::new(WhoAmI::new().unwrap())).unwrap();
        registry
            .register(Arc::new(Capabilities::new(Arc::clone(&directory)).unwrap()))
            .unwrap();
        xops_table::tools::register(registry, &tables).unwrap();
        Fixture {
            label,
            tables,
            directory,
            server: Arc::new(server),
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

    fn project(&self, owner: UserId, slug: &str) -> ProjectId {
        let project = self
            .directory
            .create_project(owner, Slug::new(slug).unwrap(), slug)
            .unwrap();
        self.tables.ensure_system_tables(project.id, slug).unwrap();
        project.id
    }

    fn bugs(&self, owner: UserId, project: ProjectId) -> TableId {
        let name = TableId::user("bugs").unwrap();
        self.tables
            .create(
                owner,
                project,
                name.clone(),
                Protection::Normal,
                vec![
                    Column::new("title", ColumnType::Text { max_len: 64 }, true).unwrap(),
                    Column::new("seq", ColumnType::Sequence, false).unwrap(),
                    Column::new(
                        "code",
                        ColumnType::Derived {
                            template: "{project.slug}-{seq}".into(),
                        },
                        false,
                    )
                    .unwrap(),
                ],
            )
            .unwrap();
        name
    }

    fn token(&self, user: UserId) -> String {
        self.directory
            .issue_token(user, "测试", None)
            .unwrap()
            .1
            .into_string()
    }

    fn tools_in(&self, token: &str, project: ProjectId) -> Vec<String> {
        let listed = self
            .server
            .handle(
                Some(token),
                &json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                    "params": {"_meta": {"project": project.to_string()}},
                }),
            )
            .unwrap();
        listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect()
    }

    fn call(&self, token: &str, name: &str, arguments: Value) -> Value {
        self.server
            .handle(
                Some(token),
                &json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": name, "arguments": arguments},
                }),
            )
            .unwrap()
    }
}

// ——————————————————————————————— 目录 ———————————————————————————————

#[test]
fn 建表即派发删表即停派() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let token = fixture.token(alice);
        let project = fixture.project(alice, "acme");

        assert!(
            !fixture
                .tools_in(&token, project)
                .iter()
                .any(|name| name.starts_with("row.bugs.")),
            "{label}：还没建表"
        );
        let bugs = fixture.bugs(alice, project);
        let dispatched = fixture.tools_in(&token, project);
        for action in ["insert", "update", "delete", "select"] {
            assert!(
                dispatched.contains(&format!("row.bugs.{action}")),
                "{label}：建表即派发，少了 {action}：{dispatched:?}"
            );
        }

        // 先写一行，删表之后要证明它还查得到。
        let row = fixture
            .tables
            .insert(
                &WrittenBy::Person { user: alice },
                Some(project),
                &bugs,
                json!({"title": "崩了"}),
            )
            .unwrap();

        fixture.tables.drop_table(alice, project, &bugs).unwrap();
        assert!(
            !fixture
                .tools_in(&token, project)
                .iter()
                .any(|name| name.starts_with("row.bugs.")),
            "{label}：删表即停派"
        );
        let listed = fixture.tables.list(alice, project).unwrap();
        assert!(
            !listed.iter().any(|schema| schema.name.as_str() == "bugs"),
            "{label}：列不出来了（系统表照旧在）"
        );
        assert!(
            !fixture
                .tables
                .history(Some(project), &bugs, row)
                .unwrap()
                .is_empty(),
            "{label}：TBL-026 —— 行与事件一律保留、单行历史仍可查"
        );
    }
}

#[test]
fn 表名不可复用() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice, "acme");
        let bugs = fixture.bugs(alice, project);
        fixture.tables.drop_table(alice, project, &bugs).unwrap();

        let error = fixture
            .tables
            .create(
                alice,
                project,
                TableId::user("bugs").unwrap(),
                Protection::Normal,
                vec![Column::new("title", ColumnType::Text { max_len: 8 }, true).unwrap()],
            )
            .unwrap_err();
        assert!(
            error.message().contains("不可复用"),
            "{label}：{}",
            error.message()
        );
    }
}

#[test]
fn 两个项目各建一张同名表互不相干() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let bob = fixture.user("bob");
        let first = fixture.project(alice, "first");
        let second = fixture.project(bob, "second");
        let bugs = fixture.bugs(alice, first);
        fixture.bugs(bob, second);

        fixture
            .tables
            .insert(
                &WrittenBy::Person { user: alice },
                Some(first),
                &bugs,
                json!({"title": "一"}),
            )
            .unwrap();
        assert_eq!(
            fixture.tables.rows(Some(first), &bugs, 10).unwrap().len(),
            1,
            "{label}"
        );
        assert_eq!(
            fixture.tables.rows(Some(second), &bugs, 10).unwrap().len(),
            0,
            "{label}"
        );
    }
}

#[test]
fn 加列可以改列不做() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice, "acme");
        let bugs = fixture.bugs(alice, project);

        let schema = fixture
            .tables
            .add_column(
                alice,
                project,
                &bugs,
                Column::new("state", ColumnType::Bool, false).unwrap(),
            )
            .unwrap();
        assert!(schema.column("state").is_some(), "{label}");

        let error = fixture
            .tables
            .add_column(
                alice,
                project,
                &bugs,
                Column::new("state", ColumnType::Integer, false).unwrap(),
            )
            .unwrap_err();
        assert!(
            error.message().contains("新建一张表"),
            "{label}：{}",
            error.message()
        );
    }
}

#[test]
fn 列声明覆盖不了自动补的字段() {
    for name in xops_table::AUTO_COLUMNS {
        assert!(
            Column::new(name, ColumnType::Integer, false).is_err(),
            "{name} 该被拒（TBL-014）"
        );
    }
}

#[test]
fn 系统表建好了且删不掉也改不了() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice, "acme");

        let listed = fixture.tables.list(alice, project).unwrap();
        let names: Vec<&str> = listed.iter().map(|schema| schema.name.as_str()).collect();
        for system in ["_runs", "_flows", "_flow_nodes", "_plugins"] {
            assert!(names.contains(&system), "{label}：{names:?}");
        }

        let runs = TableId::system("_runs").unwrap();
        assert!(
            fixture.tables.drop_table(alice, project, &runs).is_err(),
            "{label}"
        );
        assert!(
            fixture
                .tables
                .add_column(
                    alice,
                    project,
                    &runs,
                    Column::new("x", ColumnType::Bool, false).unwrap()
                )
                .is_err(),
            "{label}"
        );
    }
}

#[test]
fn 系统表只有平台能写() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let token = fixture.token(alice);
        let project = fixture.project(alice, "acme");
        let runs = TableId::system("_runs").unwrap();

        let error = fixture
            .tables
            .insert(
                &WrittenBy::Person { user: alice },
                Some(project),
                &runs,
                json!({}),
            )
            .unwrap_err();
        assert!(
            error.message().contains("只有平台能写"),
            "{label}：{}",
            error.message()
        );

        // 也不该有写它的 tool。
        let dispatched = fixture.tools_in(&token, project);
        assert!(
            dispatched.contains(&"row.sys-runs.select".to_owned()),
            "{dispatched:?}"
        );
        for action in ["insert", "update", "delete"] {
            assert!(
                !dispatched.contains(&format!("row.sys-runs.{action}")),
                "{label}：系统表不该派发写 tool"
            );
        }
    }
}

// ——————————————————————————————— 行与自动补 ———————————————————————————————

#[test]
fn 每一行都带得出谁写的与何时() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice, "acme");
        let bugs = fixture.bugs(alice, project);

        let row = fixture
            .tables
            .insert(
                &WrittenBy::Person { user: alice },
                Some(project),
                &bugs,
                json!({"title": "崩了"}),
            )
            .unwrap();
        let stored = fixture
            .tables
            .get(Some(project), &bugs, row)
            .unwrap()
            .unwrap();
        assert_eq!(stored["writtenBy"]["kind"], json!("person"), "{label}");
        assert_eq!(
            stored["writtenBy"]["user"],
            json!(alice.to_string()),
            "{label}"
        );
        assert!(stored["at"].is_i64(), "{label}");
    }
}

#[test]
fn 参数里带的writtenby会被盖掉() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let bob = fixture.user("bob");
        let project = fixture.project(alice, "acme");
        let bugs = fixture.bugs(alice, project);

        let row = fixture
            .tables
            .insert(
                &WrittenBy::Person { user: alice },
                Some(project),
                &bugs,
                json!({"title": "崩了", "writtenBy": {"kind": "person", "user": bob.to_string()}}),
            )
            .unwrap();
        let stored = fixture
            .tables
            .get(Some(project), &bugs, row)
            .unwrap()
            .unwrap();
        assert_eq!(
            stored["writtenBy"]["user"],
            json!(alice.to_string()),
            "{label}：I-B —— writtenBy 不来自请求体"
        );
    }
}

#[test]
fn 自增序号项目内每表独立() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let bob = fixture.user("bob");
        let first = fixture.project(alice, "first");
        let second = fixture.project(bob, "second");
        let bugs = fixture.bugs(alice, first);
        fixture.bugs(bob, second);

        let mut numbers = Vec::new();
        for _ in 0..3 {
            let row = fixture
                .tables
                .insert(
                    &WrittenBy::Person { user: alice },
                    Some(first),
                    &bugs,
                    json!({"title": "崩了"}),
                )
                .unwrap();
            let stored = fixture
                .tables
                .get(Some(first), &bugs, row)
                .unwrap()
                .unwrap();
            numbers.push(stored["seq"].as_i64().unwrap());
        }
        assert_eq!(numbers, vec![1, 2, 3], "{label}");

        let row = fixture
            .tables
            .insert(
                &WrittenBy::Person { user: bob },
                Some(second),
                &bugs,
                json!({"title": "另一个"}),
            )
            .unwrap();
        let stored = fixture
            .tables
            .get(Some(second), &bugs, row)
            .unwrap()
            .unwrap();
        assert_eq!(
            stored["seq"].as_i64(),
            Some(1),
            "{label}：TBL-018 —— 项目内、每表独立，不跨项目共享计数器"
        );
    }
}

#[test]
fn 派生文本insert时生成一次之后改不了() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice, "acme");
        let bugs = fixture.bugs(alice, project);

        let row = fixture
            .tables
            .insert(
                &WrittenBy::Person { user: alice },
                Some(project),
                &bugs,
                json!({"title": "崩了"}),
            )
            .unwrap();
        let stored = fixture
            .tables
            .get(Some(project), &bugs, row)
            .unwrap()
            .unwrap();
        assert_eq!(stored["code"], json!("acme-1"), "{label}：{stored}");

        let error = fixture
            .tables
            .update(
                &WrittenBy::Person { user: alice },
                Some(project),
                &bugs,
                row,
                json!({"code": "我改的"}),
            )
            .unwrap_err();
        assert!(
            error.message().contains("改不了"),
            "{label}：{}",
            error.message()
        );

        // 改别的列不受影响，派生列照旧。
        fixture
            .tables
            .update(
                &WrittenBy::Person { user: alice },
                Some(project),
                &bugs,
                row,
                json!({"title": "还是崩了"}),
            )
            .unwrap();
        let stored = fixture
            .tables
            .get(Some(project), &bugs, row)
            .unwrap()
            .unwrap();
        assert_eq!(stored["code"], json!("acme-1"), "{label}");
        assert_eq!(stored["title"], json!("还是崩了"), "{label}");
    }
}

#[test]
fn 软删一行之后读不到但历史还在() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice, "acme");
        let bugs = fixture.bugs(alice, project);
        let written = WrittenBy::Person { user: alice };

        let row = fixture
            .tables
            .insert(&written, Some(project), &bugs, json!({"title": "崩了"}))
            .unwrap();
        fixture
            .tables
            .update(
                &written,
                Some(project),
                &bugs,
                row,
                json!({"title": "改了"}),
            )
            .unwrap();
        fixture
            .tables
            .delete(&written, Some(project), &bugs, row)
            .unwrap();

        assert!(
            fixture
                .tables
                .get(Some(project), &bugs, row)
                .unwrap()
                .is_none(),
            "{label}"
        );
        let history = fixture.tables.history(Some(project), &bugs, row).unwrap();
        assert_eq!(history.len(), 3, "{label}：插入 + 修改 + 删除");
        assert_eq!(history[0].values["title"], json!("崩了"), "{label}");
        assert_eq!(history[1].values["title"], json!("改了"), "{label}");
        assert!(
            history.iter().all(|version| version.written_by.is_some()),
            "{label}"
        );
    }
}

#[test]
fn 未声明的列写不进去必填列也少不得() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice, "acme");
        let bugs = fixture.bugs(alice, project);
        let written = WrittenBy::Person { user: alice };

        assert!(
            fixture
                .tables
                .insert(
                    &written,
                    Some(project),
                    &bugs,
                    json!({"title": "崩了", "nope": 1})
                )
                .is_err(),
            "{label}：未声明的列"
        );
        assert!(
            fixture
                .tables
                .insert(&written, Some(project), &bugs, json!({}))
                .is_err(),
            "{label}：少了必填列"
        );
    }
}

// ——————————————————————————————— 派发出来的 tool ———————————————————————————————

#[test]
fn 派发出来的每一个都是固定形状且没有通用写tool() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let token = fixture.token(alice);
        let project = fixture.project(alice, "acme");
        fixture.bugs(alice, project);

        let listed = fixture
            .server
            .handle(
                Some(&token),
                &json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                    "params": {"_meta": {"project": project.to_string()}},
                }),
            )
            .unwrap();
        let tools = listed["result"]["tools"].as_array().unwrap();
        assert!(!tools.is_empty(), "{label}");
        for tool in tools {
            let schema = &tool["inputSchema"];
            assert_eq!(
                schema["additionalProperties"],
                json!(false),
                "{}",
                tool["name"]
            );
            // 不存在 {table, values} 这种形状：没有哪个 tool 同时收一个自由的 table 与一个自由的 values。
            assert!(
                schema["properties"]["values"].is_null(),
                "{} 收了一个自由的 values —— 那就是通用写 tool",
                tool["name"]
            );
        }

        // 表专属的写 tool 收的是**列**，不是一个任意对象。
        let insert = tools
            .iter()
            .find(|tool| tool["name"] == json!("row.bugs.insert"))
            .expect("该派发出来");
        assert!(
            insert["inputSchema"]["properties"]["title"].is_object(),
            "{label}"
        );
        assert!(
            insert["inputSchema"]["properties"]["seq"].is_null(),
            "{label}：自增序号是平台算的，不该出现在写 tool 的参数里"
        );
        assert!(
            insert["inputSchema"]["properties"]["code"].is_null(),
            "{label}：派生文本同理"
        );
    }
}

#[test]
fn 经tool写进去的行照样带得出署名() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let token = fixture.token(alice);
        let project = fixture.project(alice, "acme");
        let bugs = fixture.bugs(alice, project);

        let response = fixture.call(
            &token,
            "row.bugs.insert",
            json!({"project": project.to_string(), "title": "经 tool 写的"}),
        );
        let row = response["result"]["structuredContent"]["row"]
            .as_str()
            .unwrap();
        let row = xops_core::RowId::from_id(xops_core::Id::parse(row).unwrap());
        let stored = fixture
            .tables
            .get(Some(project), &bugs, row)
            .unwrap()
            .unwrap();
        assert_eq!(
            stored["writtenBy"]["user"],
            json!(alice.to_string()),
            "{label}"
        );
        assert_eq!(stored["code"], json!("acme-1"), "{label}");
    }
}

#[test]
fn 受保护表只有所有者能写() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let bob = fixture.user("bob");
        let bob_token = fixture.token(bob);
        let project = fixture.project(alice, "acme");
        fixture
            .directory
            .set_member(alice, project, bob, Role::Member)
            .unwrap();

        let roster = TableId::user("roster").unwrap();
        fixture
            .tables
            .create(
                alice,
                project,
                roster,
                Protection::Protected,
                vec![Column::new("who", ColumnType::Text { max_len: 32 }, true).unwrap()],
            )
            .unwrap();

        let dispatched = fixture.tools_in(&bob_token, project);
        assert!(
            !dispatched.contains(&"row.roster.insert".to_owned()),
            "{label}：受保护表成员看不到写 tool —— {dispatched:?}"
        );
        let response = fixture.call(
            &bob_token,
            "row.roster.insert",
            json!({"project": project.to_string(), "who": "bob"}),
        );
        assert_eq!(
            response["error"]["data"]["code"],
            json!("not_found"),
            "{label}：裁剪不是只藏起来"
        );
    }
}

#[test]
fn 别的项目的表专属tool不会串进来() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let token = fixture.token(alice);
        let mine = fixture.project(alice, "mine");
        let bob = fixture.user("bob");
        let theirs = fixture.project(bob, "theirs");
        fixture.bugs(bob, theirs);

        let listed = fixture.tools_in(&token, mine);
        assert!(
            !listed.iter().any(|name| name.starts_with("row.bugs.")),
            "{label}：别人项目里的表不该出现在我的能力发现里 —— {listed:?}"
        );
    }
}

#[test]
fn 目录重建之后表还在() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice, "acme");
        fixture.bugs(alice, project);

        fixture.tables.catalog().reload().unwrap();
        let listed = fixture.tables.list(alice, project).unwrap();
        assert!(
            listed.iter().any(|schema| schema.name.as_str() == "bugs"),
            "{label}：目录是从 _tables 的事件流重建的"
        );
        assert!(
            listed.iter().any(|schema| schema.kind == Kind::System),
            "{label}：系统表也在"
        );
    }
}
