//! RP-03 的验收。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use xops_audit::AuditLog;
use xops_core::{Actor, Result, Role, SystemClock};
use xops_identity::{Action, Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_mcp::registry::{CallContext, Idempotency, Requirement, Tool, ToolSpec};
use xops_mcp::{
    Capabilities, Field, FieldType, McpServer, MyPendingNodes, NoPendingNodes, Schema, WhoAmI,
};
use xops_store::{MemoryStore, Store, WriteEngine};

/// 一个有副作用、支持幂等键的测试 tool。数自己被真正调了几次。
struct Counter {
    spec: ToolSpec,
    calls: Arc<AtomicUsize>,
    seen_actor: Arc<std::sync::Mutex<Option<Actor>>>,
}

impl Counter {
    fn new(calls: Arc<AtomicUsize>, seen_actor: Arc<std::sync::Mutex<Option<Actor>>>) -> Self {
        Self {
            spec: ToolSpec::builder("test.count")
                .summary("数数")
                .input(
                    Schema::new()
                        .field(Field::required("project", FieldType::Id, "项目"))
                        // 故意声明一个叫 actor 的字段：就算 schema 里真的有它，
                        // 写入署的名也必须来自令牌（TOK-007 / I-B）。
                        .field(Field::optional(
                            "actor",
                            FieldType::Text { max_len: 32 },
                            "诱饵",
                        )),
                )
                .requires(Requirement::InProject(Action::WriteTable))
                .idempotency(Idempotency::Keyed)
                .audits(xops_audit::kinds::CALL_REJECTED)
                .build()
                .unwrap(),
            calls,
            seen_actor,
        }
    }
}

impl Tool for Counter {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        *self.seen_actor.lock().unwrap() = Some(context.actor());
        Ok(json!({"calls": self.calls.fetch_add(1, Ordering::SeqCst) + 1}))
    }
}

/// 只有所有者能调的 tool，用来验能力发现的裁剪。
struct OwnerOnly {
    spec: ToolSpec,
}

impl OwnerOnly {
    fn new() -> Self {
        Self {
            spec: ToolSpec::builder("test.owner-only")
                .summary("只有所有者能调")
                .input(Schema::new().field(Field::required("project", FieldType::Id, "项目")))
                .requires(Requirement::InProject(Action::ManageMember))
                .idempotency(Idempotency::ReadOnly)
                .audits(xops_audit::kinds::CALL_REJECTED)
                .build()
                .unwrap(),
        }
    }
}

impl Tool for OwnerOnly {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, _context: &CallContext<'_>) -> Result<Value> {
        Ok(json!({"ok": true}))
    }
}

struct Harness {
    server: Arc<McpServer>,
    directory: Arc<Directory>,
    calls: Arc<AtomicUsize>,
    seen_actor: Arc<std::sync::Mutex<Option<Actor>>>,
}

fn harness() -> Harness {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let clock = Arc::new(SystemClock);
    let engine = Arc::new(WriteEngine::new(Arc::clone(&store), clock.clone()));
    let relations: Arc<dyn xops_store::Relations> =
        Arc::new(xops_store::MemoryRelations::new());
    let mut audit = AuditLog::new(Arc::clone(&engine), Arc::clone(&store), Arc::clone(&relations)).unwrap();
    for table in xops_identity::directory::platform_tables().unwrap() {
        audit = audit.watching(table);
    }
    let audit = Arc::new(audit);
    let directory = Arc::new(Directory::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&audit),
        clock,
    ));

    let calls = Arc::new(AtomicUsize::new(0));
    let seen_actor = Arc::new(std::sync::Mutex::new(None));
    let mut server = McpServer::new(Arc::clone(&directory), audit, store);
    let registry = server.registry_mut();
    registry.register(Arc::new(WhoAmI::new().unwrap())).unwrap();
    registry
        .register(Arc::new(Capabilities::new(Arc::clone(&directory)).unwrap()))
        .unwrap();
    registry
        .register(Arc::new(
            MyPendingNodes::new(Arc::new(NoPendingNodes)).unwrap(),
        ))
        .unwrap();
    registry
        .register(Arc::new(Counter::new(
            Arc::clone(&calls),
            Arc::clone(&seen_actor),
        )))
        .unwrap();
    registry.register(Arc::new(OwnerOnly::new())).unwrap();

    Harness {
        server: Arc::new(server),
        directory,
        calls,
        seen_actor,
    }
}

impl Harness {
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

    fn token(&self, user: UserId) -> String {
        self.directory
            .issue_token(user, "测试", None)
            .unwrap()
            .1
            .into_string()
    }

    fn call(&self, token: &str, name: &str, arguments: Value) -> Value {
        self.request(token, json!({"name": name, "arguments": arguments}))
    }

    fn request(&self, token: &str, params: Value) -> Value {
        self.server
            .handle(
                Some(token),
                &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params}),
            )
            .expect("有 id 就该有响应")
    }
}

// ——————————————————————————————— 认证 ———————————————————————————————

#[test]
fn 没有令牌什么都做不了() {
    let harness = harness();
    for method in ["initialize", "ping", "tools/list", "tools/call"] {
        let response = harness
            .server
            .handle(None, &json!({"jsonrpc": "2.0", "id": 1, "method": method}))
            .unwrap();
        assert_eq!(
            response["error"]["code"], -32_001,
            "{method}：MCP-002 —— 每次调用都要带令牌，握手也不例外"
        );
    }
}

#[test]
fn 无效令牌不产生任何副作用() {
    let harness = harness();
    let alice = harness.user("alice");
    let token = harness.token(alice);
    let project = harness
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap();

    let response = harness.call(
        "xops_deadbeef",
        "test.count",
        json!({"project": project.id.to_string()}),
    );
    assert_eq!(response["error"]["code"], -32_001);
    assert_eq!(harness.calls.load(Ordering::SeqCst), 0, "MCP-002");
    // 有效令牌照常。
    harness.call(
        &token,
        "test.count",
        json!({"project": project.id.to_string()}),
    );
    assert_eq!(harness.calls.load(Ordering::SeqCst), 1);
}

// ——————————————————————————————— schema ———————————————————————————————

#[test]
fn 未声明的字段被拒绝而不是静默丢弃() {
    let harness = harness();
    let alice = harness.user("alice");
    let token = harness.token(alice);
    let project = harness
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap();

    let response = harness.call(
        &token,
        "test.count",
        json!({"project": project.id.to_string(), "somethingElse": 1}),
    );
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("somethingElse"),
        "{response}"
    );
    assert_eq!(harness.calls.load(Ordering::SeqCst), 0, "被拒了就不该执行");
}

#[test]
fn 署名来自令牌哪怕参数里真有一个叫actor的字段() {
    let harness = harness();
    let alice = harness.user("alice");
    let token = harness.token(alice);
    let project = harness
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap();

    harness.call(
        &token,
        "test.count",
        json!({"project": project.id.to_string(), "actor": "root"}),
    );
    assert_eq!(
        *harness.seen_actor.lock().unwrap(),
        Some(Actor::User {
            user: alice.to_string()
        }),
        "TOK-007 / I-B —— 行为人一律由令牌解析得出"
    );
}

#[test]
fn 没有任何一个tool收任意对象() {
    let harness = harness();
    for spec in harness.server.registry().specs() {
        let rendered = spec.input().to_json_schema();
        assert_eq!(
            rendered["additionalProperties"],
            json!(false),
            "{} 收了任意字段",
            spec.name()
        );
        for (name, property) in rendered["properties"].as_object().unwrap() {
            let ty = property["type"].as_str().unwrap_or_default();
            assert!(
                ty != "object"
                    && !property
                        .get("additionalProperties")
                        .is_some_and(|v| v == true),
                "{}.{name} 是个任意对象 —— MCP-004 不允许",
                spec.name()
            );
        }
    }
}

// ——————————————————————————————— 鉴权与错误契约 ———————————————————————————————

#[test]
fn 非成员与项目不存在响应逐字节一致() {
    let harness = harness();
    let alice = harness.user("alice");
    let bob = harness.user("bob");
    let bob_token = harness.token(bob);
    let project = harness
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap();

    let outsider = harness.call(
        &bob_token,
        "test.count",
        json!({"project": project.id.to_string()}),
    );
    let missing = harness.call(
        &bob_token,
        "test.count",
        json!({"project": ProjectId::generate().to_string()}),
    );
    assert_eq!(
        serde_json::to_string(&outsider).unwrap(),
        serde_json::to_string(&missing).unwrap(),
        "PRJ-008 + MCP-008 —— 否则错误码本身就是探测工具"
    );
}

#[test]
fn 错误契约带得出码与是否该重试() {
    let harness = harness();
    let alice = harness.user("alice");
    let token = harness.token(alice);
    let response = harness.call(&token, "test.count", json!({}));
    assert_eq!(response["error"]["data"]["code"], json!("invalid_argument"));
    assert_eq!(response["error"]["data"]["retriable"], json!(false));
}

#[test]
fn 没有这个tool也是不存在() {
    let harness = harness();
    let token = harness.token(harness.user("alice"));
    let response = harness.call(&token, "test.nonexistent", json!({}));
    assert_eq!(response["error"]["data"]["code"], json!("not_found"));
}

// ——————————————————————————————— 幂等 ———————————————————————————————

#[test]
fn 同一个幂等键不产生第二次副作用且返回一致() {
    let harness = harness();
    let alice = harness.user("alice");
    let token = harness.token(alice);
    let project = harness
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap();
    let params = json!({
        "name": "test.count",
        "arguments": {"project": project.id.to_string()},
        "_meta": {"idempotencyKey": "abc-123"},
    });

    let first = harness.request(&token, params.clone());
    let second = harness.request(&token, params);
    assert_eq!(
        harness.calls.load(Ordering::SeqCst),
        1,
        "MCP-006 —— 不产生第二次副作用"
    );
    assert_eq!(first, second, "而且返回与首次相同的结果");
}

#[test]
fn 幂等键按人分区() {
    let harness = harness();
    let alice = harness.user("alice");
    let bob = harness.user("bob");
    let alice_token = harness.token(alice);
    let bob_token = harness.token(bob);
    let project = harness
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap();
    harness
        .directory
        .set_member(alice, project.id, bob, Role::Member)
        .unwrap();
    let params = json!({
        "name": "test.count",
        "arguments": {"project": project.id.to_string()},
        "_meta": {"idempotencyKey": "same-key"},
    });

    harness.request(&alice_token, params.clone());
    harness.request(&bob_token, params);
    assert_eq!(
        harness.calls.load(Ordering::SeqCst),
        2,
        "幂等键是调用方自己取的字符串，撞名是正常的 —— 混在一起就是跨用户泄露"
    );
}

#[test]
fn 只读tool不接受幂等键() {
    let harness = harness();
    let token = harness.token(harness.user("alice"));
    let response = harness.request(
        &token,
        json!({"name": "identity.whoami", "arguments": {}, "_meta": {"idempotencyKey": "k"}}),
    );
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("不接受幂等键")
    );
}

// ——————————————————————————————— 能力发现 ———————————————————————————————

#[test]
fn 能力发现按角色裁剪且看不见的也调不动() {
    let harness = harness();
    let alice = harness.user("alice");
    let bob = harness.user("bob");
    let bob_token = harness.token(bob);
    let project = harness
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap();
    harness
        .directory
        .set_member(alice, project.id, bob, Role::Member)
        .unwrap();

    let listed = harness
        .server
        .handle(
            Some(&bob_token),
            &json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                "params": {"_meta": {"project": project.id.to_string()}},
            }),
        )
        .unwrap();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"test.count"), "成员看得到 {names:?}");
    assert!(
        !names.contains(&"test.owner-only"),
        "成员不该看到只有所有者能调的 {names:?}"
    );

    // 裁剪不是只藏起来 —— 调也调不动。
    let response = harness.call(
        &bob_token,
        "test.owner-only",
        json!({"project": project.id.to_string()}),
    );
    assert_eq!(response["error"]["data"]["code"], json!("not_found"));
}

#[test]
fn 归档项目里写的tool都不见了() {
    let harness = harness();
    let alice = harness.user("alice");
    let token = harness.token(alice);
    let project = harness
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap();
    harness
        .directory
        .archive_project(alice, project.id)
        .unwrap();

    let capabilities = harness.call(
        &token,
        "identity.capabilities",
        json!({"project": project.id.to_string()}),
    );
    let names = capabilities["result"]["structuredContent"]["tools"]
        .as_array()
        .unwrap();
    let names: Vec<&str> = names.iter().map(|name| name.as_str().unwrap()).collect();
    assert!(!names.contains(&"test.count"), "归档项目转为只读 {names:?}");
    assert!(names.contains(&"identity.whoami"), "平台级的照旧 {names:?}");
}

// ——————————————————————————————— 身份域三个 tool ———————————————————————————————

#[test]
fn 三个身份tool都在且调得通() {
    let harness = harness();
    let alice = harness.user("alice");
    let token = harness.token(alice);

    let who = harness.call(&token, "identity.whoami", json!({}));
    assert_eq!(
        who["result"]["structuredContent"]["user"],
        json!(alice.to_string())
    );

    let capabilities = harness.call(&token, "identity.capabilities", json!({}));
    assert!(capabilities["result"]["structuredContent"]["tools"].is_array());

    let pending = harness.call(&token, "identity.pending-nodes", json!({}));
    assert_eq!(
        pending["result"]["structuredContent"]["nodes"],
        json!([]),
        "注册位在 RP-03，实现在 RP-14 —— 现在是空的，但形状已经定死"
    );
}

// ——————————————————————————————— 协议与传输 ———————————————————————————————

#[test]
fn 握手回得出协议版本() {
    let harness = harness();
    let token = harness.token(harness.user("alice"));
    let response = harness
        .server
        .handle(
            Some(&token),
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
        )
        .unwrap();
    assert_eq!(
        response["result"]["protocolVersion"],
        json!(xops_mcp::PROTOCOL_VERSION)
    );
    assert_eq!(response["result"]["serverInfo"]["name"], json!("xops"));
}

#[test]
fn 通知不产生响应() {
    let harness = harness();
    let token = harness.token(harness.user("alice"));
    assert!(
        harness
            .server
            .handle(Some(&token), &json!({"jsonrpc": "2.0", "method": "ping"}))
            .is_none(),
        "没有 id 就是通知"
    );
}

#[test]
fn 不是jsonrpc二点零就拒() {
    let harness = harness();
    let token = harness.token(harness.user("alice"));
    let response = harness
        .server
        .handle(
            Some(&token),
            &json!({"jsonrpc": "1.0", "id": 1, "method": "ping"}),
        )
        .unwrap();
    assert_eq!(response["error"]["code"], -32_600);
}

#[test]
fn http传输走得通() {
    let harness = harness();
    let token = harness.token(harness.user("alice"));
    let listener = xops_mcp::transport::http::listen("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = Arc::clone(&harness.server);
    std::thread::spawn(move || {
        let _ = xops_mcp::transport::http::serve_listener(server, &listener);
    });

    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "identity.whoami"}).to_string();
    let response = post(address, Some(&token), "/mcp", &body);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    // 别的路径、别的方法都不认。
    assert!(post(address, Some(&token), "/anything", &body).starts_with("HTTP/1.1 404"));
    let no_auth = post(
        address,
        None,
        "/mcp",
        &json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string(),
    );
    assert!(no_auth.contains("-32001"), "{no_auth}");
}

fn post(address: std::net::SocketAddr, token: Option<&str>, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    let auth = token.map_or_else(String::new, |token| {
        format!("Authorization: Bearer {token}\r\n")
    });
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn stdio传输走得通() {
    let harness = harness();
    let token = harness.token(harness.user("alice"));
    let input = format!(
        "{}\n{}\n",
        json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
        json!({"jsonrpc": "2.0", "method": "ping"}),
    );
    let mut output = Vec::new();
    xops_mcp::transport::stdio::serve(&harness.server, Some(&token), input.as_bytes(), &mut output)
        .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert_eq!(text.lines().count(), 1, "通知不产生响应：{text}");
}

// ——————————————————————————————— 四个例外 ———————————————————————————————

#[test]
fn 非mcp入口恰好是清单上那四个() {
    let listed: Vec<&str> = xops_mcp::NON_MCP_ENTRYPOINTS
        .iter()
        .map(|entry| entry.entrypoint)
        .collect();
    assert_eq!(
        listed,
        vec![
            "OAuth 登录回调",
            "Git webhook 端点",
            "会话面（登录与注销）",
            "令牌管理面"
        ]
    );
    assert!(
        xops_mcp::NON_MCP_ENTRYPOINTS
            .iter()
            .all(|entry| !entry.writes_business_objects)
    );
}
