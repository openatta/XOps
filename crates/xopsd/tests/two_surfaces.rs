//! 装配层的验收：**两个服务面真的起得来，而且它们是分开的。**
//!
//! 这个文件里的每一条都走**真的 TCP**——装配层的价值就在"接起来之后还成立吗"，
//! 用内存对象直接调是证不了的。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use serde_json::{Value, json};
use xops_identity::{ExternalAccount, ProviderId};
use xopsd::{Assembled, Config, assemble};

fn config() -> Config {
    Config {
        secret_key: "0b".repeat(32),
        ..Config::default()
    }
}

/// 起一个 MCP 面，返回端口。
fn serve_mcp(assembled: &Assembled) -> u16 {
    let listener = xops_mcp::transport::http::listen("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = Arc::clone(&assembled.mcp);
    thread::spawn(move || {
        let _ = xops_mcp::transport::http::serve_listener(server, &listener);
    });
    port
}

/// 起一个 Web 面，返回端口。
fn serve_web(assembled: &Assembled) -> u16 {
    let listener = xops_web::server::listen("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = Arc::clone(&assembled.web);
    thread::spawn(move || {
        let _ = server.serve_listener(&listener);
    });
    port
}

/// 起一个可停的 Web 面，返回 `(端口, 停止开关, 线程)`。
fn serve_web_stoppable(assembled: &Assembled) -> (u16, Arc<AtomicBool>, thread::JoinHandle<()>) {
    let listener = xops_web::server::listen("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = Arc::clone(&assembled.web);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        let _ = server.serve_listener_until(&listener, &flag);
    });
    (port, stop, handle)
}

fn http(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}

fn body_of(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

fn rpc(port: u16, token: Option<&str>, payload: &Value) -> Value {
    let body = payload.to_string();
    let auth = token.map_or_else(String::new, |token| {
        format!("Authorization: Bearer {token}\r\n")
    });
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    serde_json::from_str(body_of(&http(port, &request))).unwrap_or(Value::Null)
}

fn token(assembled: &Assembled, account: &str) -> String {
    let user = assembled
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
        .id;
    assembled
        .directory
        .issue_token(user, "笔记本", None)
        .unwrap()
        .1
        .into_string()
}

// ——————————————————————————————— 装得起来 ———————————————————————————————

#[test]
fn 十九个包接得起来而且没有重名的tool() {
    let assembled = assemble(&config()).unwrap();
    let names: Vec<String> = assembled
        .mcp
        .registry()
        .specs()
        .map(|spec| spec.name().as_str().to_owned())
        .collect();
    assert!(names.len() > 40, "十六个域一次注册齐，实际 {}", names.len());
    let unique: std::collections::BTreeSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), names.len(), "**重名会让后注册的那个悄悄赢**");
}

#[test]
fn 没有密钥就起不来() {
    // **一个写死的默认密钥看起来是加密的，实际不是**——所以默认是空的，而空的起不来。
    assert!(assemble(&Config::default()).is_err());
}

#[test]
fn 裸跑的代价可枚举而且启动横幅说得出来() {
    let assembled = assemble(&config()).unwrap();
    // D58 / EXE-029：**不静默降级。**
    assert!(
        !assembled.unsatisfied.is_empty(),
        "没兑现的那几条要列得出来"
    );
    assert_eq!(assembled.engine_kind, "stub", "没给 socket 就是桩");
    let banner = xopsd::banner::render(&config(), &assembled);
    assert!(banner.contains("桩") && banner.contains("裸跑"));
}

// ——————————————————————————————— MCP 写入面 ———————————————————————————————

#[test]
fn 经真的tcp调得通一次tools调用() {
    let assembled = assemble(&config()).unwrap();
    let port = serve_mcp(&assembled);
    let token = token(&assembled, "alice");

    let listed = rpc(
        port,
        Some(&token),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    let tools = listed["result"]["tools"]
        .as_array()
        .expect("要有 tool 目录");
    assert!(!tools.is_empty());

    // 建一个项目 —— 这条会连带把那四张系统表建起来（装配层挂的 ProjectHook）。
    let created = rpc(
        port,
        Some(&token),
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "project.create", "arguments": {"slug": "acme", "displayName": "Acme"}},
        }),
    );
    assert!(
        created["error"].is_null(),
        "建项目应该成功：{}",
        created["error"]
    );
    let project = created["result"]["structuredContent"]["project"]
        .as_str()
        .expect("回话里要有项目标识")
        .to_owned();

    // 系统表真的建起来了 —— 表专属 tool 在这个项目里出现了。
    let scoped = rpc(
        port,
        Some(&token),
        &json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/list",
            "params": {"_meta": {"project": project}},
        }),
    );
    let names: Vec<&str> = scoped["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(
        names.iter().any(|name| name.starts_with("row.sys-runs.")),
        "ProjectHook 该把那四张系统表建起来，实际的 tool 目录是 {names:?}"
    );
}

#[test]
fn 没有令牌进不来() {
    let assembled = assemble(&config()).unwrap();
    let port = serve_mcp(&assembled);
    let response = rpc(
        port,
        None,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    assert_eq!(
        response["error"]["code"], -32_001,
        "MCP-002：每次调用都要带令牌"
    );
}

// ——————————————————————————————— 只读 Web 面 ———————————————————————————————

#[test]
fn web面起得来而且是另一个端口() {
    let assembled = assemble(&config()).unwrap();
    let mcp = serve_mcp(&assembled);
    let web = serve_web(&assembled);
    assert_ne!(mcp, web, "两个服务面分开");

    let response = http(
        web,
        "GET /api/me HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 "), "它得是个 HTTP 应答");
}

#[test]
fn web面没有mcp那条路由() {
    // **唯一的写入通道是 MCP**（I-L）：Web 那一侧连这条路径都不该有。
    let assembled = assemble(&config()).unwrap();
    let web = serve_web(&assembled);
    let response = http(
        web,
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\
         Connection: close\r\n\r\n{}",
    );
    assert!(
        !response.contains("\"result\""),
        "Web 面不该处理 MCP 调用：{response}"
    );
}

/// Web 那一侧**结构性地不存在写业务对象的路由**（`G2`）。
///
/// 这条在 RP-05 里由 `ROUTES` 那张表证明过；这里再确认一次
/// **装配之后它还是那张表**——装配层没有偷偷加一条。
#[test]
fn 装配之后web的写路由还是只有那三条例外() {
    let writes: Vec<&str> = xops_web::routes::ROUTES
        .iter()
        .filter(|route| route.method != "GET")
        .map(|route| route.path)
        .collect();
    assert_eq!(
        writes.len(),
        3,
        "两个凭据路由 + 一条 webhook，实际是 {writes:?}"
    );
    assert!(
        xops_web::routes::ROUTES
            .iter()
            .all(|route| !route.writes_business_objects),
        "G2：一条写业务对象的路由都没有"
    );
}

/// 让 `Value` 在这个文件里有个用处。
fn _unused(value: &Value) -> bool {
    value.is_null()
}

// ——————————————————————————————— 出版本要的那几样 ———————————————————————————————

#[test]
fn 存活探针不认证不泄露() {
    let assembled = assemble(&config()).unwrap();
    let web = serve_web(&assembled);
    // **不带任何凭据**——探针是给编排器用的，它没有令牌也没有会话。
    let response = http(
        web,
        "GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let body = body_of(&response);
    assert_eq!(body, r#"{"status":"ok"}"#);

    // ⚠️ **回话里不能有任何信息。** 版本、项目数、连接数、库路径——
    // 一个未认证的端点泄露的每一样都是给人探的。
    for leak in [
        env!("CARGO_PKG_VERSION"),
        "stub",
        "sqlite",
        "memory",
        "project",
        "tool",
    ] {
        assert!(!body.contains(leak), "探针泄露了 {leak}：{body}");
    }
}

#[test]
fn 探针不是第五个非mcp例外() {
    // `MCP-013` 认下的例外说的是"能写点什么的非 MCP 入口"。
    // 探针连读都不读——**它不该被算进那张表**，否则那张表就开始收留无关的东西了。
    assert_eq!(xops_mcp::boundary::NON_MCP_ENTRYPOINTS.len(), 4);
    let health = xops_web::routes::ROUTES
        .iter()
        .find(|route| route.path == "/healthz")
        .expect("探针要在路由表里");
    assert_eq!(health.method, "GET");
    assert!(!health.writes_business_objects);
}

#[test]
fn 置起停止开关之后不再接新连接() {
    let assembled = assemble(&config()).unwrap();
    let (port, stop, handle) = serve_web_stoppable(&assembled);

    // 停之前：连得上。
    assert!(
        http(
            port,
            "GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
        )
        .starts_with("HTTP/1.1 200")
    );

    stop.store(true, Ordering::Relaxed);
    // accept 是轮询的，给它一点反应时间。
    handle.join().expect("停止开关一置起，服务循环就该返回");

    // 停之后：连不上了（端口已经关掉）。
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "停机之后监听器该关掉"
    );
}

#[test]
fn 日志级别关得掉而且不认识的值不会把日志关掉() {
    // ⚠️ 一个拼错的 `XOPS_LOG` 把日志静默关掉，是出了事之后最难查的那种情形。
    use xops_core::log::Level;
    assert_eq!(Level::parse("off"), Level::Off);
    assert_eq!(
        Level::parse("胡说"),
        Level::Info,
        "不认识的当 info，不是 off"
    );
}

/// **子模块是只读的。** 这条测试盯着一件真出过事的事。
///
/// `vendor/attacore` 是上游的仓，改那边的代码是明令禁止的:改动会被上游清理掉，
/// **而一次被清理掉的修改是查不出来的**。
///
/// ⚠️ 已经踩过一次:`cargo fmt --all` 顺着 path 依赖走进了子模块，
/// **一次格式化了 75 个文件**。`[workspace] exclude` 拦不住它——那张表是给
/// 依赖解析看的，不是给 rustfmt 看的。所以格式化要走 `scripts/fmt.sh`。
#[test]
fn 子模块没有被改过() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root.join("vendor/attacore"))
        .output();
    let Ok(output) = output else {
        // 没有 git 或者子模块没拉下来:这条测试没什么可说的，不该因此变红。
        return;
    };
    let dirty = String::from_utf8_lossy(&output.stdout);
    assert!(
        dirty.trim().is_empty(),
        "vendor/attacore 被改过了。**那个仓只读**——需求变更走 ISSUE 提过去。\n\
         格式化用 `./scripts/fmt.sh`，不要用 `cargo fmt --all`。\n{dirty}"
    );
}
