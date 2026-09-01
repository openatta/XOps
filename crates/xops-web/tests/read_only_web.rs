//! RP-05 的验收。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_audit::AuditLog;
use xops_core::{SystemClock, TableName};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_mcp::McpServer;
use xops_read::{Direction, ReadModel};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};
use xops_table::engine::Catalog;
use xops_table::table::{Protection, TableId};
use xops_table::{Column, ColumnType, Tables, WrittenBy};
use xops_web::{Assets, Request, Sessions, WebServer};

struct Fixture {
    label: &'static str,
    web: Arc<WebServer>,
    mcp: Arc<McpServer>,
    directory: Arc<Directory>,
    model: Arc<ReadModel>,
    tables: Arc<Tables>,
    sessions: Arc<Sessions>,
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
            catalog,
            Arc::clone(&audit),
            Arc::clone(&directory),
            clock.clone(),
            Arc::clone(&store),
        ));
        let model = Arc::new(ReadModel::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            Arc::clone(&directory),
            Arc::clone(&tables),
            clock.clone(),
        ));
        let sessions = Arc::new(Sessions::new(Arc::clone(&store), clock));
        let web = Arc::new(WebServer::new(
            Arc::clone(&model),
            Arc::clone(&directory),
            Arc::clone(&sessions),
            Assets::none(),
        ));
        let mut mcp = McpServer::new(
            Arc::clone(&directory),
            Arc::clone(&audit),
            Arc::clone(&store),
        );
        xops_mcp::tools::identity::register_identity(mcp.registry_mut(), &directory).unwrap();
        xops_read::tools::register(mcp.registry_mut(), &model).unwrap();
        Fixture {
            label,
            web,
            mcp: Arc::new(mcp),
            directory,
            model,
            tables,
            sessions,
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

    fn get(&self, session: Option<&str>, path: &str) -> xops_web::Response {
        self.web.handle(&Request {
            method: "GET".into(),
            path: path.into(),
            session: session.map(str::to_owned),
            body: Vec::new(),
        })
    }

    fn body(&self, response: &xops_web::Response) -> Value {
        serde_json::from_slice(&response.body).unwrap_or(Value::Null)
    }
}

fn setup(fixture: &Fixture) -> (UserId, String, ProjectId, TableId) {
    let alice = fixture.user("alice");
    let session = fixture.sessions.issue(alice).unwrap();
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
            vec![
                Column::new("title", ColumnType::Text { max_len: 64 }, true).unwrap(),
                Column::new(
                    "state",
                    ColumnType::Enum {
                        values: vec!["新建".into(), "已修".into()],
                    },
                    false,
                )
                .unwrap(),
                Column::new("body", ColumnType::LongText { max_len: 4096 }, false).unwrap(),
            ],
        )
        .unwrap();
    (alice, session, project, bugs)
}

// ——————————————————————————————— G2 第 ① 道 ———————————————————————————————

#[test]
fn 枚举路由表证明不存在写路由() {
    for route in xops_web::ROUTES {
        assert!(
            !route.writes_business_objects,
            "{} {} —— 不是'有但不给 Web 用'，是不存在",
            route.method, route.path
        );
    }
    let writes: Vec<&str> = xops_web::ROUTES
        .iter()
        .filter(|route| route.kind != xops_web::Kind::Credential && route.method != "GET")
        .map(|route| route.path)
        .collect();
    assert!(writes.is_empty(), "只读面上出现了非 GET 路由：{writes:?}");
}

#[test]
fn 带着会话去写也没有地方可发() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, session, project, _) = setup(&fixture);
        // 试着往看得见的那些路径上 POST / PUT / DELETE。
        for (method, path) in [
            ("POST", format!("/api/projects/{project}/boards")),
            ("PUT", format!("/api/projects/{project}/boards")),
            ("DELETE", format!("/api/projects/{project}/boards")),
            ("POST", "/api/me".to_owned()),
        ] {
            let response = fixture.web.handle(&Request {
                method: method.into(),
                path,
                session: Some(session.clone()),
                body: b"{}".to_vec(),
            });
            assert_eq!(response.status, 404, "{label}：{method} 不该有地方可发");
        }
    }
}

// ——————————————————————————————— I-L 两套凭据互不通用 ———————————————————————————————

#[test]
fn 会话凭据调不动mcp() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, session, _, _) = setup(&fixture);
        let response = fixture
            .mcp
            .handle(
                Some(&session),
                &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            )
            .unwrap();
        assert_eq!(response["error"]["code"], -32_001, "{label}：I-L");
    }
}

#[test]
fn mcp令牌当不了会话() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, _, _, _) = setup(&fixture);
        let token = fixture
            .directory
            .issue_token(alice, "笔记本", None)
            .unwrap()
            .1
            .into_string();
        let response = fixture.get(Some(&token), "/api/me");
        assert_eq!(response.status, 401, "{label}：I-L —— 反过来也不通用");
        assert!(
            fixture.sessions.resolve(&token).unwrap().is_none(),
            "{label}"
        );
    }
}

#[test]
fn 没有会话什么都读不到() {
    for fixture in fixtures() {
        let label = fixture.label;
        setup(&fixture);
        assert_eq!(fixture.get(None, "/api/me").status, 401, "{label}");
        assert_eq!(fixture.get(None, "/api/projects").status, 401, "{label}");
    }
}

// ——————————————————————————————— 可见性 ———————————————————————————————

#[test]
fn 明确展示当前用户身份() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, session, _, _) = setup(&fixture);
        let response = fixture.get(Some(&session), "/api/me");
        assert_eq!(response.status, 200, "{label}");
        let body = fixture.body(&response);
        assert_eq!(body["user"], json!(alice.to_string()), "{label}");
        assert_eq!(body["account"], json!("alice"), "{label}");
    }
}

#[test]
fn 非成员看到的与项目不存在一致() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (_, _, project, _) = setup(&fixture);
        let bob = fixture.user("bob");
        let bob_session = fixture.sessions.issue(bob).unwrap();

        let outsider = fixture.get(
            Some(&bob_session),
            &format!("/api/projects/{project}/boards"),
        );
        let missing = fixture.get(
            Some(&bob_session),
            &format!("/api/projects/{}/boards", ProjectId::generate()),
        );
        assert_eq!(outsider.status, missing.status, "{label}");
        assert_eq!(outsider.body, missing.body, "{label}：逐字节一致");

        let listed = fixture.get(Some(&bob_session), "/api/projects");
        assert_eq!(
            fixture.body(&listed)["projects"],
            json!([]),
            "{label}：看不到不属于自己的项目"
        );
    }
}

// ——————————————————————————————— 看板 ———————————————————————————————

#[test]
fn 看板是一张表的一个视图() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, session, project, bugs) = setup(&fixture);
        let written = WrittenBy::Person { user: alice };
        for (title, state) in [("一", "新建"), ("二", "已修"), ("三", "新建")] {
            fixture
                .tables
                .insert(
                    &written,
                    Some(project),
                    &bugs,
                    json!({"title": title, "state": state}),
                )
                .unwrap();
        }

        let board = fixture
            .model
            .define_board(
                alice,
                project,
                xops_read::BoardSpec {
                    name: "待修的".into(),
                    table: bugs.clone(),
                    filters: vec![xops_read::Filter::Equals {
                        column: "state".into(),
                        value: json!("新建"),
                    }],
                    sort: Some("title".into()),
                    direction: Direction::Asc,
                    columns: vec!["title".into(), "state".into()],
                },
            )
            .unwrap();

        let response = fixture.get(
            Some(&session),
            &format!("/api/projects/{project}/boards/{}", board.id),
        );
        assert_eq!(response.status, 200, "{label}");
        let view = fixture.body(&response);
        assert_eq!(view["columns"], json!(["title", "state"]), "{label}");
        let rows = view["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "{label}：筛掉了已修的那条");
        assert_eq!(
            rows[0]["values"]["title"],
            json!("一"),
            "{label}：按 title 升序"
        );
        assert!(
            rows[0]["values"]["writtenBy"].is_object(),
            "{label}：看板上的来源标识读的就是 writtenBy（TBL-016）"
        );
        assert!(
            rows[0]["values"]["body"].is_null(),
            "{label}：没声明的列不该出现在视图里"
        );
    }
}

#[test]
fn 通知表建不了自由看板() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, _, project, _) = setup(&fixture);
        let error = fixture
            .model
            .define_board(
                alice,
                project,
                xops_read::BoardSpec {
                    name: "我的通知".into(),
                    table: TableId::system("_notices").unwrap(),
                    filters: vec![],
                    sort: None,
                    direction: Direction::Asc,
                    columns: vec![],
                },
            )
            .unwrap_err();
        assert!(
            error.message().contains("_notices"),
            "{label}：{}",
            error.message()
        );
        assert!(
            error.message().contains("平台内建的固定视图"),
            "{label}：个人看板归 RP-17，本包不得先做一个简化版顶上"
        );
    }
}

// ——————————————————————————————— BRD-006 两个视图 ———————————————————————————————

#[test]
fn 时间线是两个视图两次查询() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, session, project, bugs) = setup(&fixture);
        let written = WrittenBy::Person { user: alice };
        let row = fixture
            .tables
            .insert(
                &written,
                Some(project),
                &bugs,
                json!({"title": "崩了", "state": "新建"}),
            )
            .unwrap();
        fixture
            .tables
            .update(
                &written,
                Some(project),
                &bugs,
                row,
                json!({"state": "已修"}),
            )
            .unwrap();

        // ① 主体行的单行历史：状态怎么变的、谁改的、什么时候。
        let history = fixture.get(
            Some(&session),
            &format!("/api/projects/{project}/tables/bugs/rows/{row}/history"),
        );
        assert_eq!(history.status, 200, "{label}");
        let versions = fixture.body(&history)["versions"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(versions.len(), 2, "{label}");
        assert!(
            versions
                .iter()
                .all(|version| version["written_by"].is_object()),
            "{label}"
        );

        // ② 同实例的结算行：为什么这么变、谁表的态。**另一次查询。**
        let settlements = fixture.get(
            Some(&session),
            &format!(
                "/api/projects/{project}/tables/bugs/instances/{}/settlements",
                xops_core::Id::generate()
            ),
        );
        assert_eq!(settlements.status, 200, "{label}");
        assert_eq!(
            fixture.body(&settlements)["settlements"],
            json!([]),
            "{label}：还没有流程实例，但形状已经定了"
        );

        // 后端没有把两者 join 起来的查询 —— 路由表上就是两条。
        let joined = xops_web::ROUTES
            .iter()
            .filter(|route| route.path.contains("timeline"))
            .count();
        assert_eq!(joined, 0, "{label}：平台不做 join（BRD-006 / TBL-023）");
    }
}

// ——————————————————————————————— 长文本原文 ———————————————————————————————

#[test]
fn 长文本给得出原文且不经任何渲染() {
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, session, project, bugs) = setup(&fixture);
        // 一段刻意带着内联 HTML 的正文 —— 它可能来自被分析的代码仓。
        let hostile = "# 标题\n\n<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>";
        let row = fixture
            .tables
            .insert(
                &WrittenBy::Person { user: alice },
                Some(project),
                &bugs,
                json!({"title": "崩了", "body": hostile}),
            )
            .unwrap();

        let response = fixture.get(
            Some(&session),
            &format!("/api/projects/{project}/tables/bugs/rows/{row}/columns/body/raw"),
        );
        assert_eq!(response.status, 200, "{label}");
        assert_eq!(
            response.content_type, "text/plain; charset=utf-8",
            "{label}：BRD-010 —— 原始形式，不是 HTML"
        );
        assert_eq!(
            String::from_utf8(response.body).unwrap(),
            hostile,
            "{label}：一个字都没动"
        );
    }
}

#[test]
fn 会话面只建会话不写业务对象() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        // 预置账号那一侧要有口令才登得进来，这里直接发会话验证注销那一半。
        let session = fixture.sessions.issue(alice).unwrap();
        assert!(session.starts_with(xops_web::SESSION_PREFIX), "{label}");
        assert_eq!(
            fixture.sessions.resolve(&session).unwrap(),
            Some(alice),
            "{label}"
        );

        let response = fixture.web.handle(&Request {
            method: "DELETE".into(),
            path: "/session".into(),
            session: Some(session.clone()),
            body: Vec::new(),
        });
        assert_eq!(response.status, 200, "{label}");
        assert!(
            fixture.sessions.resolve(&session).unwrap().is_none(),
            "{label}：注销了"
        );
    }
}
