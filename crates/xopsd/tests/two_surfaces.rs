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
/// `vendor/attacore` 是上游的仓，改那边的代码是明令禁止的：改动会被上游清理掉，
/// **而一次被清理掉的修改是查不出来的**。
///
/// ⚠️ 已经踩过一次：`cargo fmt --all` 顺着 path 依赖走进了子模块，
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
        // 没有 git 或者子模块没拉下来：这条测试没什么可说的，不该因此变红。
        return;
    };
    let dirty = String::from_utf8_lossy(&output.stdout);
    assert!(
        dirty.trim().is_empty(),
        "vendor/attacore 被改过了。**那个仓只读**——需求变更走 ISSUE 提过去。\n\
         格式化用 `./scripts/fmt.sh`，不要用 `cargo fmt --all`。\n{dirty}"
    );
}

/// **每一条接缝都要在装配层被填上。**
///
/// # 为什么要有这条测试
///
/// 这个仓里每个单元自己都是对的、单元测试也绿，而好几条链**从来没有谁调用**:
/// 流程求值、定时触发、webhook、落账、保留期、实例过期——**六条**。
/// 表现全是"功能整个不工作，而且是静默的"。
///
/// ⚠️ **单元测试证明不了这件事**:它构造被测对象、注入桩、断言行为——
/// 那恰恰跳过了"这个对象在成品里被接上了吗"。**装配层是唯一知道全貌的地方**，
/// 而它是最后写的，几条线就那么留在了半空中。
///
/// 这条测试枚举全仓的"注入位"型接缝，断言每一个在 `assemble.rs` 里都有着落。
#[test]
fn 每一条注入位都在装配层被填上() {
    let assemble = include_str!("../src/assemble.rs");
    let background = include_str!("../src/background.rs");
    let wired = format!("{assemble}\n{background}");

    // (接缝, 在装配层该出现的那个标记, 没接的后果)
    let seams = [
        (
            "PreWrite / SchemaCheck",
            "with_pre_write",
            "schema 不校验、序号不补齐",
        ),
        (
            "Evaluate（流程求值）",
            "with_evaluate",
            "**整个流程引擎惰性**：行写进结算表，没有任何东西去求值",
        ),
        (
            "PluginEvaluator（流转插件）",
            "with_plugins",
            "指定了流转插件的节点求不了值",
        ),
        (
            "NotSettledNotifier（未被采纳）",
            "NotSettled",
            "写的人不知道自己白写了（FLW-027）",
        ),
        (
            "TestRunner（发起测试执行）",
            "with_test_runner",
            "**技能发布不了**：发布要测试执行，而它没有入口",
        ),
        (
            "SubscriptionCheck（订阅白名单）",
            "with_subscription_check",
            "订阅声明不校验",
        ),
        (
            // ⚠️ 这条以前数的是字符串 `GitPlatform`——那个词因为**别的原因**
            // 也在装配层里，于是它一直是绿的，而 `WorkspaceSource`
            // 全仓一处实现都没有。**数名字不如数接口。**
            "WorkspaceSource（正式触发那条）",
            ".with_workspaces(Arc::clone(&workspaces))",
            "**要读代码仓的技能跑不了**：拿不到工作区",
        ),
        (
            "WorkspaceSource（技能试跑那条）",
            "with_workspaces(Arc::clone(&workspaces)),",
            "**要读代码仓的技能发布不了**：试跑拿不到工作区，而发布要一次成功的试跑",
        ),
        (
            "WebhookSink（Git webhook）",
            "with_webhooks",
            "**webhook 事件掉进地里**：路由在、验签在，什么也不发生",
        ),
        (
            "RunNotifier（执行结束通知）",
            "with_notices",
            "执行完了没人知道（NTF-007）",
        ),
        (
            "Reaper（落账）",
            "Reaper::new",
            "**执行成功了，`_runs` 上什么也没有**",
        ),
        (
            "Ticker（定时触发）",
            "Ticker::new",
            "**定时任务永远不触发**：配置在那儿，时间也对，什么都不发生",
        ),
        (
            "Keeper（保留期）",
            "Keeper::new",
            "保留期从不生效，库只涨不减",
        ),
        (
            "expire_due（实例过期）",
            "expire_due",
            "实例永不过期（FLW-017）",
        ),
        (
            "Concurrency（并发上限）",
            "with_concurrency",
            "**一个项目能把算力吃光**（EXE-027）",
        ),
        (
            "关系投影的开机重放",
            "rebuild_index",
            "**存量库升上来时投影是空的**：审计查不到、按主体找不到",
        ),
    ];

    let mut missing = Vec::new();
    for (seam, marker, consequence) in seams {
        if !wired.contains(marker) {
            missing.push(format!("  {seam}（找不到 `{marker}`）→ {consequence}"));
        }
    }
    assert!(
        missing.is_empty(),
        "这些接缝在装配层没有着落——**每一条都是「功能整个不工作」且静默**：\n{}",
        missing.join("\n")
    );
}

// ——————————————————————— 流程定义经 MCP 创建（FLW-001）———————————————————————

/// 一次 tool 调用，失败就地报出来。
fn call(port: u16, token: &str, name: &str, arguments: Value) -> Value {
    let response = rpc(
        port,
        Some(token),
        &json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }),
    );
    assert!(
        response["error"].is_null(),
        "调 {name} 失败了：{}",
        response["error"]
    );
    response["result"]["structuredContent"].clone()
}

/// 一次 tool 调用，**期望它失败**，返回那条消息。
fn call_err(port: u16, token: &str, name: &str, arguments: Value) -> String {
    let response = rpc(
        port,
        Some(token),
        &json!({
            "jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }),
    );
    assert!(
        !response["error"].is_null(),
        "调 {name} 本该失败：{response}"
    );
    response["error"]["message"]
        .as_str()
        .unwrap_or("")
        .to_owned()
}

fn 建一个项目(port: u16, token: &str, slug: &str) -> String {
    call(
        port,
        token,
        "project.create",
        json!({"slug": slug, "displayName": slug}),
    )["project"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn 流程定义经mcp创建并且真的能跑() {
    // `FLW-001`：**不存在流程设计器界面**——定义经 MCP 创建。
    // 这条以前是断的：`Flows::define` 一直都在，**只有模板实例化那一条路能到它**。
    let assembled = assemble(&config()).unwrap();
    let port = serve_mcp(&assembled);
    let alice = token(&assembled, "alice");
    let bob = token(&assembled, "bob");
    let project = 建一个项目(port, &alice, "flowdef");

    let bob_id = call(port, &bob, "identity.whoami", json!({}))["user"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        port,
        &alice,
        "member.set",
        json!({"project": project, "user": bob_id, "role": "member"}),
    );

    // 结算表：谁对它做了什么表态。
    call(
        port,
        &alice,
        "table.create",
        json!({
            "project": project, "table": "votes",
            "columns": [
                {"name": "decision", "type": "enum", "enumValues": ["yes", "no"], "required": true},
            ],
        }),
    );

    let defined = call(
        port,
        &alice,
        "flow.define",
        json!({
            "project": project,
            "name": "两道",
            "settlementTable": "votes",
            "steps": [
                [{
                    "name": "初审",
                    "pass": [{"op": "equals", "column": "decision", "value": "yes"}],
                    "reject": [{"op": "equals", "column": "decision", "value": "no"}],
                    "writerRoles": ["member", "maintainer", "owner"],
                    "separationOfDuties": true,
                }],
            ],
        }),
    );
    assert_eq!(defined["version"], 1, "版本号由平台排");
    assert_eq!(defined["state"], "published");
    let flow = defined["flow"].as_str().unwrap().to_owned();

    // 发起 → 结算 → 通过。**这条链以前只有模板能走通。**
    let instance = call(
        port,
        &alice,
        "flow.start",
        json!({
            "project": project, "flow": flow, "version": 1,
            "subjectKind": "release", "subjectId": "v1",
        }),
    )["instance"]
        .as_str()
        .unwrap()
        .to_owned();

    // ⚠️ 职责分离是**参数里显式打开的那一个**：alice 是发起人，她自己投不算数。
    call(
        port,
        &alice,
        "flow.settle",
        json!({"project": project, "instance": instance, "values": r#"{"decision":"yes"}"#}),
    );
    let after_self = call(
        port,
        &alice,
        "flow.status",
        json!({"project": project, "instance": instance}),
    );
    assert_eq!(
        after_self["state"], "running",
        "发起人自己投不算数（FLW-026③）——这说明定义里的 separationOfDuties 真的到了求值那一侧"
    );

    call(
        port,
        &bob,
        "flow.settle",
        json!({"project": project, "instance": instance, "values": r#"{"decision":"yes"}"#}),
    );
    let after_bob = call(
        port,
        &alice,
        "flow.status",
        json!({"project": project, "instance": instance}),
    );
    assert_eq!(after_bob["state"], "approved", "第二个人投了才算数");

    // 停用之后发不起新实例（`FLW-006`）。
    call(
        port,
        &alice,
        "flow.disable",
        json!({"project": project, "flow": flow, "version": 1}),
    );
    let refused = call_err(
        port,
        &alice,
        "flow.start",
        json!({
            "project": project, "flow": flow, "version": 1,
            "subjectKind": "release", "subjectId": "v2",
        }),
    );
    assert!(!refused.is_empty(), "停用之后发不起新实例");
}

#[test]
fn 流程定义的参数是逐字段声明的不是一整份json() {
    // `MCP-004`：打错一个键名要被**拒绝**，不能静默丢掉。
    // 流程定义里最怕静默丢掉的正是 `separationOfDuties`——
    // **少了它没有任何症状，只是审批不再需要第二个人。**
    let assembled = assemble(&config()).unwrap();
    let port = serve_mcp(&assembled);
    let alice = token(&assembled, "alice");
    let project = 建一个项目(port, &alice, "narrow");
    call(
        port,
        &alice,
        "table.create",
        json!({
            "project": project, "table": "votes",
            "columns": [{"name": "decision", "type": "text", "maxLen": 8, "required": true}],
        }),
    );

    let typo = call_err(
        port,
        &alice,
        "flow.define",
        json!({
            "project": project, "name": "打错了", "settlementTable": "votes",
            "steps": [[{
                "name": "初审",
                "pass": [{"op": "equals", "column": "decision", "value": "yes"}],
                "writerRoles": ["member"],
                "separationOfDudies": true,
            }]],
        }),
    );
    assert!(
        typo.contains("separationOfDudies"),
        "打错的键名要被指出来，实际是：{typo}"
    );
}

#[test]
fn 没有项目设过密钥时webhook端点不是探测器() {
    // `TRG-012`：验签失败、没有这个项目、没接落点 —— **三种回同一个东西**。
    // 平台级那把密钥拿掉之后，这条尤其要成立:一个全新部署里没有任何项目设过密钥，
    // 端点这时的回应不能与"设过但签错了"有半点差别。
    let assembled = assemble(&config()).unwrap();
    let port = serve_web(&assembled);
    let body = r#"{"ref":"refs/heads/main"}"#;
    let mut seen = std::collections::BTreeSet::new();
    for signature in ["sha256=00", "sha256=deadbeef", ""] {
        let response = http(
            port,
            &format!(
                "POST /webhooks/git HTTP/1.1\r\nHost: localhost\r\n\
                 X-Hub-Signature-256: {signature}\r\nX-GitHub-Event: push\r\n\
                 X-GitHub-Delivery: d-1\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        );
        let status = response.lines().next().unwrap_or_default().to_owned();
        seen.insert((status, body_of(&response).to_owned()));
    }
    assert_eq!(seen.len(), 1, "三种情形的回应要逐字一致，实际有 {seen:#?}");
    assert!(
        seen.iter().next().unwrap().0.contains("404"),
        "回的是「不存在」"
    );
}

#[test]
fn 配了预置账号就真的登得进来() {
    // ⚠️ 这条盯的是一处**装配层的断链**：`Directory::new` 一个身份提供方都没有，
    // 而装配层从来没接过一个 —— 于是 `POST /session` 一律回"凭证不对"，
    // **页面在、路由在、就是进不去**，日志里一个字都没有。
    // 单元测试全绿、装配也过，而那条链在运行时是断的。
    let config = Config {
        secret_key: "0a".repeat(32),
        logins: vec![("alice".to_owned(), "口令".to_owned(), "Alice".to_owned())],
        ..Config::default()
    };
    let assembled = xopsd::assemble(&config).unwrap();
    assert_eq!(assembled.logins, 1);

    let login = |account: &str, secret: &str| {
        assembled.web.handle(&xops_web::Request {
            method: "POST".into(),
            path: "/session".into(),
            query: String::new(),
            session: None,
            headers: std::collections::BTreeMap::new(),
            body: format!(r#"{{"provider":"builtin","account":"{account}","secret":"{secret}"}}"#)
                .into_bytes(),
        })
    };

    let ok = login("alice", "口令");
    assert_eq!(ok.status, 200, "配了就该登得进来");
    let session = ok.set_session.clone().expect("要下发会话");

    // ⚠️ **两种失败必须是同一个错**（`IDN-001`）：区分"账号不存在"与"口令不对"
    // 是给探测者的。
    assert_eq!(
        login("alice", "错的").status,
        login("mallory", "随便").status
    );

    // 登进来之后个人看板读得到 —— 这一条把「登录 → 个人看板」整条链走通。
    let me = assembled.web.handle(&xops_web::Request {
        method: "GET".into(),
        path: "/api/me/notices".into(),
        query: String::new(),
        session: Some(session),
        headers: std::collections::BTreeMap::new(),
        body: Vec::new(),
    });
    assert_eq!(me.status, 200, "登进来之后个人看板该读得到");
}

#[test]
fn 不配预置账号就一个人都登不进来而且横幅说出来() {
    let config = Config {
        secret_key: "0a".repeat(32),
        ..Config::default()
    };
    let assembled = xopsd::assemble(&config).unwrap();
    assert_eq!(assembled.logins, 0);
    let response = assembled.web.handle(&xops_web::Request {
        method: "POST".into(),
        path: "/session".into(),
        query: String::new(),
        session: None,
        headers: std::collections::BTreeMap::new(),
        body: r#"{"provider":"builtin","account":"alice","secret":"whatever"}"#
            .as_bytes()
            .to_vec(),
    });
    assert_ne!(response.status, 200, "没配就该进不来");
    assert!(
        xopsd::banner::render(&config, &assembled).contains("没有预置任何账号"),
        "**这一条不能悄悄发生**：页面在、路由在、就是登不进去"
    );
}
