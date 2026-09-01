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
use xops_table::{Column, ColumnType, Filter, MAX_SCAN, Query, Tables, WrittenBy};

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

// ——————————————————————————————— 查询面 ———————————————————————————————
//
// 这一组盯的是一件具体的事：**"扫前 N 行再过滤"会安静地给出错误答案。**
// 行 ID 是时间有序的，扫描按行 ID 升序，所以截断留下的是**最老的 N 条**——
// 于是一个"最新在前"的看板会稳定显示最老的那一批，而没有任何地方报错。

/// 往表里塞 `count` 行，第 `i` 行带 `title = 第i` 与 `flag = 偶/奇`。
fn fill(fixture: &Fixture, owner: UserId, project: ProjectId, table: &TableId, count: usize) {
    for index in 0..count {
        fixture
            .tables
            .insert(
                &WrittenBy::Person { user: owner },
                Some(project),
                table,
                json!({
                    "title": format!("第{index}"),
                    "flag": if index % 2 == 0 { "偶" } else { "奇" },
                }),
            )
            .unwrap();
    }
}

fn flagged(fixture: &Fixture, owner: UserId) -> (ProjectId, TableId) {
    let project = fixture.project(owner, "acme");
    let name = TableId::user("notes").unwrap();
    fixture
        .tables
        .create(
            owner,
            project,
            name.clone(),
            Protection::Normal,
            vec![
                Column::new("title", ColumnType::Text { max_len: 64 }, true).unwrap(),
                Column::new("flag", ColumnType::Text { max_len: 8 }, false).unwrap(),
            ],
        )
        .unwrap();
    (project, name)
}

#[test]
fn 游标翻得完整张表而不是停在第一页() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let (project, notes) = flagged(&fixture, alice);
        fill(&fixture, alice, project, &notes, 25);

        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = fixture
                .tables
                .query(Some(project), &notes, &Query::first(4).after(cursor))
                .unwrap();
            if page.is_empty() {
                break;
            }
            seen.extend(page.rows.iter().map(|(_, values)| values["title"].clone()));
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen.len(), 25, "{label}：25 行要一行不少地翻完");
        assert_eq!(seen[0], json!("第0"), "按写入序");
        assert_eq!(seen[24], json!("第24"));
    }
}

#[test]
fn 命中在第一页之后的行也取得回来() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let (project, notes) = flagged(&fixture, alice);
        fill(&fixture, alice, project, &notes, 40);

        let hit = fixture
            .tables
            .query_all(
                Some(project),
                &notes,
                &[Filter::equals("flag", "奇")],
                MAX_SCAN,
            )
            .unwrap();
        assert_eq!(hit.len(), 20, "{label}：命中不该被第一页截住");

        // 而"扫前 N 行再过滤"看到的只有前 N 行里的那些 —— 这就是那个坑。
        let truncated: Vec<_> = fixture
            .tables
            .rows(Some(project), &notes, 4)
            .unwrap()
            .into_iter()
            .filter(|(_, values)| values["flag"] == json!("奇"))
            .collect();
        assert_eq!(
            truncated.len(),
            2,
            "{label}：它只看得到最老的四行里的那两条"
        );
    }
}

#[test]
fn 扫不动的时候明确失败而不是截断() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let (project, notes) = flagged(&fixture, alice);
        fill(&fixture, alice, project, &notes, 30);

        // 上限之内：全部命中。
        assert_eq!(
            fixture
                .tables
                .query_all(Some(project), &notes, &[], 100)
                .unwrap()
                .len(),
            30,
            "{label}"
        );

        // 超过上限：**报错，不是给一个短了的答案**。
        let error = fixture
            .tables
            .query_all(Some(project), &notes, &[], 10)
            .unwrap_err();
        assert!(
            error.message().contains("这里不截断"),
            "{label}：截断会安静地给出错误答案，实际是 {error}"
        );
    }
}

#[test]
fn rows给的是最老的那一批这件事被钉住了() {
    // ⚠️ 这条不是在夸这个行为，是在**钉住它**：`rows` 就是"前 N 行"，
    // 它的语义可以被依赖，但**不能拿它去顶"最新的 N 条"**。
    // 哪天有人想"顺手让它返回最新的"，这条会拦住——那会悄悄改掉所有调用方的语义。
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let (project, notes) = flagged(&fixture, alice);
        fill(&fixture, alice, project, &notes, 10);

        let first_three = fixture.tables.rows(Some(project), &notes, 3).unwrap();
        let titles: Vec<_> = first_three
            .iter()
            .map(|(_, values)| values["title"].clone())
            .collect();
        assert_eq!(
            titles,
            vec![json!("第0"), json!("第1"), json!("第2")],
            "{label}：行 ID 时间有序 → 前 N 行就是最老的 N 行"
        );
    }
}

#[test]
fn 软删的行不出现在任何一种读里() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let (project, notes) = flagged(&fixture, alice);
        fill(&fixture, alice, project, &notes, 6);

        let victim = fixture
            .tables
            .query(Some(project), &notes, &Query::first(1))
            .unwrap()
            .rows[0]
            .0;
        fixture
            .tables
            .delete(
                &WrittenBy::Person { user: alice },
                Some(project),
                &notes,
                victim,
            )
            .unwrap();

        assert_eq!(
            fixture
                .tables
                .query_all(Some(project), &notes, &[], MAX_SCAN)
                .unwrap()
                .len(),
            5,
            "{label}"
        );
        assert!(
            !fixture
                .tables
                .query(Some(project), &notes, &Query::first(10))
                .unwrap()
                .rows
                .iter()
                .any(|(row, _)| *row == victim),
            "{label}"
        );
    }
}

/// 那几处**曾经**"扫一个写死的上限再过滤"的读路径,现在都不是那么写的了。
///
/// 它们的失败形态是**静默的错误结果**——没有报错,只有一个短了的答案,
/// 而短掉的那一半恰好是新的那一半。这条测试盯着它们不要长回来。
#[test]
fn 读路径不再拿写死的上限去顶谓词() {
    let sites = [
        (
            "xops-read/board 与 settlements",
            include_str!("../../xops-read/src/model.rs"),
        ),
        (
            "xops-notice/unread",
            include_str!("../../xops-notice/src/service.rs"),
        ),
        (
            "xops-xforge/settling_row",
            include_str!("../../xops-xforge/src/service.rs"),
        ),
        (
            "xops-settle/in_roster",
            include_str!("../../xops-settle/src/writers.rs"),
        ),
        (
            "xops-script/plugins",
            include_str!("../../xops-script/src/service.rs"),
        ),
    ];
    for (what, source) in sites {
        let body = source.split("#[cfg(test)]").next().unwrap();
        for line in body.lines() {
            if !line.contains(".rows(") {
                continue;
            }
            // 剩下的 `.rows(` 只允许把调用方给的 limit 透下去，
            // **不允许自己写一个大数字当上限**——那就是那个坑。
            let has_big_literal = line
                .split(|c: char| !(c.is_ascii_digit() || c == '_'))
                .filter(|token| !token.is_empty())
                .filter_map(|token| token.replace('_', "").parse::<usize>().ok())
                .any(|number| number >= 500);
            assert!(
                !has_big_literal,
                "{what}：又出现了「扫前 N 行再过滤」——{line}"
            );
        }
    }
}

#[test]
fn select的游标翻得完整张表() {
    // `row.<表>.select` 现在带游标：把回话里的 `next` 传回来就是下一页。
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let (project, notes) = flagged(&fixture, alice);
        fill(&fixture, alice, project, &notes, 12);
        let token = fixture.token(alice);

        let mut titles = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..10 {
            let mut args = json!({"project": project.to_string(), "limit": 5});
            if let Some(cursor) = &after {
                args["after"] = json!(cursor);
            }
            let reply = fixture.call(&token, "row.notes.select", args);
            assert!(reply.get("error").is_none(), "{label}：{reply}");
            let page = &reply["result"]["structuredContent"];
            titles.extend(
                page["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row["values"]["title"].clone()),
            );
            after = page["next"].as_str().map(str::to_owned);
            if after.is_none() {
                break;
            }
        }
        assert_eq!(titles.len(), 12, "{label}：12 行要一行不少地翻完");
        assert_eq!(titles[0], json!("第0"));
        assert_eq!(titles[11], json!("第11"));
    }
}
