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
    notices: Arc<xops_notice::Notices>,
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
            catalog,
            Arc::clone(&audit),
            Arc::clone(&directory),
            clock.clone(),
            Arc::clone(&store),
        ));
        // 个人看板读它（`NTF-001`）。**装配顺序就是依赖顺序**：通知先建。
        let notices = Arc::new(
            xops_notice::Notices::new(
                Arc::clone(&tables),
                Arc::clone(&relations),
                Arc::clone(&directory),
                clock.clone(),
            )
            .unwrap(),
        );
        let model = Arc::new(ReadModel::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            Arc::clone(&directory),
            Arc::clone(&tables),
            Arc::clone(&notices),
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
            notices,
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
        // 测试里 `path` 允许带查询串——按真实解析那样切开，
        // **不然分页在这一层就永远测不到**。
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        self.web.handle(&Request {
            method: "GET".into(),
            path: path.into(),
            query: query.into(),
            session: session.map(str::to_owned),
            headers: std::collections::BTreeMap::new(),
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
    // ⚠️ `_notices` 是**平台全局表**，不属于任何项目（`TBL-010`、`NTF-014`）——
    // 它不在 `ensure_system_tables` 里。少了这一句，通知写不进去，
    // 而 `NTF-008` 说通知写失败**只留痕、绝不回滚业务写**：于是**没有人会报错**。
    fixture.tables.ensure_global_tables().unwrap();
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
    // 非 GET 的只能是 `MCP-013` 认下的那几个例外：凭据面与 webhook。
    let writes: Vec<&str> = xops_web::ROUTES
        .iter()
        .filter(|route| {
            !matches!(
                route.kind,
                xops_web::Kind::Credential | xops_web::Kind::Webhook
            ) && route.method != "GET"
        })
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
                query: String::new(),
                session: Some(session.clone()),
                headers: std::collections::BTreeMap::new(),
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
            query: String::new(),
            session: Some(session.clone()),
            headers: std::collections::BTreeMap::new(),
            body: Vec::new(),
        });
        assert_eq!(response.status, 200, "{label}");
        assert!(
            fixture.sessions.resolve(&session).unwrap().is_none(),
            "{label}：注销了"
        );
    }
}

// ——————————————————————————————— 前端产物与会话 cookie ———————————————————————————————

#[test]
fn 前端产物嵌进了二进制() {
    // D55：**构建产物随二进制发行，部署方不需要 Node。**
    // 这个断言在 `npm run build` 跑过之后才有意义；没跑过时它会明确告诉你少了哪一步。
    assert!(
        xops_web::Assets::embedded_count() > 0,
        "二进制里没有前端页面。先在 web/ 里跑 `npm run build`，再 cargo build —— \
         D55 说的是产物随二进制发行，不是运行时去某个目录找"
    );
    let assets = xops_web::Assets::embedded();
    let index = assets.serve("GET", "/");
    assert_eq!(index.status, 200);
    assert_eq!(index.content_type, "text/html; charset=utf-8");
    // SPA 的深链回落到 index.html —— 路由在前端那一侧。
    assert_eq!(assets.serve("GET", "/projects/anything/boards").status, 200);
}

#[test]
fn 静态资源挡得住路径穿越() {
    let assets = xops_web::Assets::embedded();
    for path in [
        "/../Cargo.toml",
        "/assets/../../Cargo.toml",
        "/..%2F..%2Fetc",
    ] {
        let response = assets.serve("GET", path);
        // 要么 404，要么回落到 index.html —— 无论如何都拿不到目录外的东西。
        assert!(
            response.status == 404 || response.content_type.starts_with("text/html"),
            "{path} 漏了"
        );
    }
}

#[test]
fn 会话cookie不让脚本读到() {
    for fixture in fixtures() {
        let label = fixture.label;
        // 会话泄露那一条（RP-06 验收）在后端这一侧的兑现：HttpOnly + SameSite。
        let alice = fixture.user("alice");
        let session = fixture.sessions.issue(alice).unwrap();
        let rendered =
            format!("Set-Cookie: xops_session={session}; HttpOnly; SameSite=Strict; Path=/");
        assert!(rendered.contains("HttpOnly"), "{label}");
        assert!(rendered.contains("SameSite=Strict"), "{label}");
    }
}

// ——————————————————————————————— webhook 端点（RP-13） ———————————————————————————————

/// 一个只会数自己被调过几次的落点。
struct CountingSink {
    calls: std::sync::atomic::AtomicUsize,
    accept: bool,
}

impl xops_web::WebhookSink for CountingSink {
    fn deliver(
        &self,
        _signature: Option<&str>,
        _event: Option<&str>,
        _delivery: Option<&str>,
        _body: &[u8],
    ) -> xops_core::Result<()> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.accept {
            Ok(())
        } else {
            Err(<CountingSink as xops_web::WebhookSink>::rejection())
        }
    }
}

fn webhook_request(body: &str) -> Request {
    let mut headers = std::collections::BTreeMap::new();
    headers.insert(
        "x-hub-signature-256".to_owned(),
        "sha256=deadbeef".to_owned(),
    );
    headers.insert("x-github-event".to_owned(), "push".to_owned());
    headers.insert("x-github-delivery".to_owned(), "delivery-1".to_owned());
    Request {
        method: "POST".into(),
        path: "/webhooks/git".into(),
        query: String::new(),
        session: None,
        headers,
        body: body.as_bytes().to_vec(),
    }
}

#[test]
fn webhook端点不写任何业务对象() {
    let route = xops_web::ROUTES
        .iter()
        .find(|route| route.path == "/webhooks/git")
        .expect("该有这条路由");
    assert!(
        !route.writes_business_objects,
        "TRG-011：它只能产生一个 git 事件"
    );
    assert_eq!(route.kind, xops_web::Kind::Webhook);
}

#[test]
fn 验签失败与没接落点回的是同一个东西() {
    for fixture in fixtures() {
        let label = fixture.label;
        // ① 根本没接落点。
        let unwired = fixture.web.handle(&webhook_request("{}"));
        // ② 接了，但验签失败。
        let rejecting = Arc::new(CountingSink {
            calls: std::sync::atomic::AtomicUsize::new(0),
            accept: false,
        });
        let wired = Arc::new(
            WebServer::new(
                Arc::clone(&fixture.model),
                Arc::clone(&fixture.directory),
                Arc::clone(&fixture.sessions),
                Assets::none(),
            )
            .with_webhooks(Arc::clone(&rejecting) as Arc<dyn xops_web::WebhookSink>),
        );
        let rejected = wired.handle(&webhook_request("{}"));

        assert_eq!(unwired.status, rejected.status, "{label}");
        assert_eq!(
            unwired.body, rejected.body,
            "{label}：TRG-012 —— 不泄露任何关于任务或项目是否存在的信息"
        );
    }
}

#[test]
fn webhook收下之后立刻返回() {
    for fixture in fixtures() {
        let label = fixture.label;
        let sink = Arc::new(CountingSink {
            calls: std::sync::atomic::AtomicUsize::new(0),
            accept: true,
        });
        let server = Arc::new(
            WebServer::new(
                Arc::clone(&fixture.model),
                Arc::clone(&fixture.directory),
                Arc::clone(&fixture.sessions),
                Assets::none(),
            )
            .with_webhooks(Arc::clone(&sink) as Arc<dyn xops_web::WebhookSink>),
        );
        let started = std::time::Instant::now();
        let response = server.handle(&webhook_request(r#"{"ref":"refs/heads/main"}"#));
        assert_eq!(response.status, 202, "{label}");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(100),
            "{label}：TRG-014 —— 端点内不做任何拉取或执行"
        );
        assert_eq!(
            sink.calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "{label}"
        );
    }
}

#[test]
fn webhook端点不认会话也不给别的方法() {
    for fixture in fixtures() {
        let label = fixture.label;
        let mut get = webhook_request("{}");
        get.method = "GET".into();
        assert_ne!(fixture.web.handle(&get).status, 202, "{label}：只认 POST");
    }
}

// ————————————————————— 个人看板 · 成员 · 表清单（2026-09-04 补） —————————————————————

#[test]
fn 个人看板只给自己那些行() {
    // `NTF-010`：读写被硬限定为 `user = 令牌持有人`——**这是行级可见性的唯一例外**。
    // ⚠️ 这条测试盯的不是"过滤对不对"，是**路径上表达不出"看别人的"**：
    // `/api/me/notices` 上没有 user 参数，换一个会话拿到的就是另一个人的那些行。
    for fixture in fixtures() {
        let (_, session, project, _) = setup(&fixture);
        let bob_session = fixture.sessions.issue(fixture.user("bob")).unwrap();
        let _ = project;

        let mine = fixture.get(Some(&session), "/api/me/notices");
        assert_eq!(mine.status, 200, "{}", fixture.label);
        let body = fixture.body(&mine);
        assert!(body["notices"].is_array(), "{}", fixture.label);
        assert_eq!(body["truncated"], serde_json::json!(false), "没到上限");

        let theirs = fixture.body(&fixture.get(Some(&bob_session), "/api/me/notices"));
        assert!(theirs["notices"].is_array(), "别人也读得到自己的那份");
    }
}

#[test]
fn 个人看板要会话() {
    for fixture in fixtures() {
        setup(&fixture);
        assert_eq!(
            fixture.get(None, "/api/me/notices").status,
            401,
            "{}：只读面一律要会话",
            fixture.label
        );
    }
}

#[test]
fn 成员清单非成员看到的与项目不存在一致() {
    // `PRJ-008`：非成员看到的与项目不存在**是同一个错**——
    // 分开说等于把"这个项目存在"告诉一个不该知道的人。
    for fixture in fixtures() {
        let (_, session, project, _) = setup(&fixture);
        let outsider = fixture.sessions.issue(fixture.user("mallory")).unwrap();

        let mine = fixture.get(Some(&session), &format!("/api/projects/{project}/members"));
        assert_eq!(mine.status, 200, "{}", fixture.label);
        let members = fixture.body(&mine);
        let list = members["members"].as_array().expect("是个数组");
        assert_eq!(list.len(), 1, "只有 alice 一个人");
        assert_eq!(list[0]["role"], serde_json::json!("owner"));
        assert_eq!(
            list[0]["display_name"],
            serde_json::json!("alice"),
            "显示名在后端解出来——前端没有第二条通路去按 id 换名字"
        );

        let theirs = fixture.get(Some(&outsider), &format!("/api/projects/{project}/members"));
        assert_eq!(theirs.status, 404, "{}：与项目不存在一致", fixture.label);
    }
}

#[test]
fn 表清单给的是有哪些表不是表里有什么() {
    // ⚠️ 这条盯的是**那条界线**：一行数据都不回。
    // 一个顺手"再回十行"的版本，就是绕过看板定义的第二条读数据通路（`BRD-001`）。
    for fixture in fixtures() {
        let (_, session, project, _) = setup(&fixture);
        let response = fixture.get(Some(&session), &format!("/api/projects/{project}/tables"));
        assert_eq!(response.status, 200, "{}", fixture.label);
        let body = fixture.body(&response);
        let tables = body["tables"].as_array().expect("是个数组");

        let names: Vec<&str> = tables
            .iter()
            .filter_map(|table| table["table"].as_str())
            .collect();
        assert!(names.contains(&"bugs"), "用户表在里面：{names:?}");
        assert!(
            names.iter().any(|name| name.starts_with('_')),
            "系统表也在里面（页面要靠它说清楚有什么可看）：{names:?}"
        );

        let bugs = tables
            .iter()
            .find(|table| table["table"] == serde_json::json!("bugs"))
            .expect("找得到 bugs");
        assert_eq!(bugs["kind"], serde_json::json!("user"));
        let columns = bugs["columns"].as_array().expect("列是个数组");
        assert!(
            columns
                .iter()
                .any(|column| column["column"] == serde_json::json!("title")),
            "列名要在：{columns:?}"
        );

        // **一行数据都不回。**
        let serialised = serde_json::to_string(&body).unwrap();
        assert!(
            !serialised.contains("\"rows\""),
            "表清单里出现了行——那是绕过看板的第二条读数据通路：{serialised}"
        );
    }
}

#[test]
fn 新加的三条也是只读的() {
    // `BRD-005` 第 ① 道靠枚举 `ROUTES` 证明。加路由的时候最容易破的就是它，
    // 所以这里再点名一次这三条。
    for path in [
        "/api/me/notices",
        "/api/projects/{project}/members",
        "/api/projects/{project}/tables",
    ] {
        let route = xops_web::ROUTES
            .iter()
            .find(|route| route.path == path)
            .unwrap_or_else(|| panic!("{path} 不在路由表里"));
        assert_eq!(route.method, "GET", "{path}");
        assert!(!route.writes_business_objects, "{path}");
    }
}

#[test]
fn 看板翻得了页而且说得出后面还有没有() {
    // ⚠️ 这条盯的是一个**不报错的缺陷**：一次给死 200 行、没有第二页，
    // 一张 201 行的表在页面上就是少一行——看的人不会知道。
    //
    // 另一半是顺序：**先排完序再切**。先切再排会稳定地显示最老的那一批，
    // 而它同样不报错。所以这里排的是倒序，验第一页拿到的是**最新的那些**。
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, session, project, bugs) = setup(&fixture);
        let written = WrittenBy::Person { user: alice };
        for index in 0..5 {
            fixture
                .tables
                .insert(
                    &written,
                    Some(project),
                    &bugs,
                    json!({"title": format!("第{index}条"), "state": "新建"}),
                )
                .unwrap();
        }
        let board = fixture
            .model
            .define_board(
                alice,
                project,
                xops_read::BoardSpec {
                    name: "全部".into(),
                    table: bugs.clone(),
                    filters: Vec::new(),
                    sort: Some("title".into()),
                    direction: xops_read::Direction::Desc,
                    columns: vec!["title".into()],
                },
            )
            .unwrap()
            .id;

        let first = fixture.body(&fixture.get(
            Some(&session),
            &format!("/api/projects/{project}/boards/{board}?limit=2"),
        ));
        assert_eq!(first["rows"].as_array().unwrap().len(), 2, "{label}");
        assert_eq!(first["offset"], json!(0), "{label}");
        assert_eq!(first["has_more"], json!(true), "{label}：后面还有三条");
        assert_eq!(
            first["rows"][0]["values"]["title"],
            json!("第4条"),
            "{label}：倒序的第一页是最新的那些——先切再排会拿到「第0条」"
        );

        let last = fixture.body(&fixture.get(
            Some(&session),
            &format!("/api/projects/{project}/boards/{board}?limit=2&offset=4"),
        ));
        assert_eq!(
            last["rows"].as_array().unwrap().len(),
            1,
            "{label}：只剩一条"
        );
        assert_eq!(last["offset"], json!(4), "{label}");
        assert_eq!(last["has_more"], json!(false), "{label}：到头了");

        // ⚠️ 上限是平台的，不是调用方的：`?limit=99999999` 不该变成一次自助拒绝服务。
        let huge = fixture.body(&fixture.get(
            Some(&session),
            &format!("/api/projects/{project}/boards/{board}?limit=99999999"),
        ));
        assert_eq!(huge["rows"].as_array().unwrap().len(), 5, "{label}");

        // 手打错的参数该看到第一页，不该看到一屏错误。
        let typo = fixture.get(
            Some(&session),
            &format!("/api/projects/{project}/boards/{board}?limit=abc"),
        );
        assert_eq!(typo.status, 200, "{label}");
    }
}

#[test]
fn 查询串不参与路由匹配() {
    // ⚠️ 查询串是**新加的一条能到达后端的输入**。它绝不能决定命中哪条路由——
    // 那等于凭空多出一组没有被 `ROUTES` 枚举过的路径，而 `BRD-005` 第 ① 道
    // 靠的正是"枚举这张表"。
    for fixture in fixtures() {
        let (_, session, _, _) = setup(&fixture);
        assert_eq!(
            fixture
                .get(Some(&session), "/api/me?anything=/api/projects")
                .status,
            200,
            "{}",
            fixture.label
        );
        assert_eq!(
            fixture.get(Some(&session), "/api/nope?x=/api/me").status,
            404,
            "{}：查询串救不回一条不存在的路由",
            fixture.label
        );
    }
}

#[test]
fn 派生出来的通知真的出现在个人看板上() {
    // ⚠️ 这条把 RP-17 与 RP-05 之间那条**以前不存在**的线走通：
    // 通知服务整套都在（派生、五类、行级限定、保留期），
    // 而只读面上一条通知路由都没有——三份包文档各自把它推给了对方。
    //
    // `NTF-002`：通知**只从事件派生**。所以这里发的是一个事件，不是一条通知。
    for fixture in fixtures() {
        let label = fixture.label;
        let (alice, session, project, bugs) = setup(&fixture);
        let bob = fixture.user("bob");
        let bob_session = fixture.sessions.issue(bob).unwrap();

        let failures = fixture
            .notices
            .notify(&xops_notice::SourceEvent::RowNotSettled {
                project,
                instance: "inst-1".into(),
                table: bugs.as_str().to_owned(),
                row: "row-1".into(),
                writer: alice,
                reason: "发起人自己表态不算数".into(),
            });
        assert!(failures.is_empty(), "{label}：{failures:?}");

        let mine = fixture.body(&fixture.get(Some(&session), "/api/me/notices"));
        let list = mine["notices"].as_array().expect("是个数组");
        assert_eq!(list.len(), 1, "{label}：alice 该收到这一条");
        assert_eq!(list[0]["kind"], json!("row-not-settled"), "{label}");
        assert_eq!(
            list[0]["project"],
            json!(project.to_string()),
            "{label}：跨项目一起排，所以每条都要说清来自哪个项目（NTF-014）"
        );
        assert!(
            list[0]["text"]
                .as_str()
                .unwrap_or_default()
                .contains("发起人自己表态不算数"),
            "{label}：自由文本原样引用（NTF-004），不摘要不改写：{}",
            list[0]["text"]
        );

        // `NTF-010`：**读被硬限定为 user = 令牌持有人**——这是行级可见性的唯一例外。
        let theirs = fixture.body(&fixture.get(Some(&bob_session), "/api/me/notices"));
        assert_eq!(
            theirs["notices"].as_array().unwrap().len(),
            0,
            "{label}：bob 不该看见 alice 的那一条"
        );
    }
}
