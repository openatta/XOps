//! HTTP 传输：`POST /mcp`，一次请求一份 JSON-RPC。
//!
//! 手写的、阻塞式的、只认 `Content-Length` 的 HTTP/1.1。**这是刻意的**：
//! 这个服务面只需要一条路由，而它必须与 Web 那一侧分开（RP-03 / RP-05）——
//! 为一条路由引一整套异步栈进来，换回的是"两个服务面共用一个路由层"这个正好不该有的东西。
//!
//! 不做的事，明写出来：**不支持 chunked、不支持 SSE、不支持 HTTP/2、不做限流**
//! （`MCP-015`：限流交给部署侧的反向代理，这是明写的不做）。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::thread;

use xops_core::{Error, Result};

use crate::McpServer;

/// 唯一的路径。
pub const PATH: &str = "/mcp";
/// 请求体上限。
pub const MAX_BODY: usize = 4 * 1024 * 1024;
/// 请求头上限。
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// 监听并一直服务下去。
///
/// # Errors
/// 端口占用或监听失败。
pub fn serve(server: Arc<McpServer>, address: impl ToSocketAddrs) -> Result<()> {
    let listener = listen(address)?;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let server = Arc::clone(&server);
        // 一连接一线程。单实例部署下，连接数的量级是"几个 agent 客户端"。
        thread::spawn(move || {
            let _ = handle_connection(&server, stream);
        });
    }
    Ok(())
}

/// 绑定端口。测试要拿到实际端口，所以单独一步。
///
/// # Errors
/// 绑不上。
pub fn listen(address: impl ToSocketAddrs) -> Result<TcpListener> {
    TcpListener::bind(address).map_err(|error| Error::unavailable(format!("监听失败：{error}")))
}

/// 服务一个已经绑好的监听器。
///
/// # Errors
/// 不会返回，除非监听器坏了。
pub fn serve_listener(server: Arc<McpServer>, listener: &TcpListener) -> Result<()> {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let server = Arc::clone(&server);
        thread::spawn(move || {
            let _ = handle_connection(&server, stream);
        });
    }
    Ok(())
}

fn handle_connection(server: &McpServer, mut stream: TcpStream) -> std::io::Result<()> {
    let response = match read_request(&mut stream) {
        Ok(request) => dispatch(server, &request),
        Err(status) => status,
    };
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

struct Request {
    method: String,
    path: String,
    credential: Option<String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> std::result::Result<Request, String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|_| response(500, "{}"))?);
    let mut head = String::new();
    let mut consumed = 0;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|_| response(400, "{}"))?;
        if read == 0 {
            return Err(response(400, "{}"));
        }
        consumed += read;
        if consumed > MAX_HEADER_BYTES {
            return Err(problem(431, "请求头太大"));
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        head.push_str(&line);
    }

    let mut lines = head.lines();
    let request_line = lines.next().ok_or_else(|| response(400, "{}"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();

    let mut length = 0usize;
    let mut credential = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().unwrap_or(0),
            "transfer-encoding" if value.eq_ignore_ascii_case("chunked") => chunked = true,
            "authorization" => {
                credential = value.strip_prefix("Bearer ").map(str::to_owned);
            }
            _ => {}
        }
    }
    if chunked {
        return Err(problem(411, "只认 Content-Length，不支持 chunked"));
    }
    if length > MAX_BODY {
        return Err(problem(413, "请求体太大"));
    }
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|_| response(400, "{}"))?;
    Ok(Request {
        method,
        path,
        credential,
        body,
    })
}

fn dispatch(server: &McpServer, request: &Request) -> String {
    if request.path != PATH {
        return problem(404, "只有 /mcp");
    }
    if request.method != "POST" {
        // MCP 的 Streamable HTTP 允许 GET 开 SSE；这里不做流式，所以只认 POST。
        return problem(405, "只认 POST");
    }
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&request.body) else {
        return response(
            400,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": crate::errors::rpc::PARSE_ERROR, "message": "请求体不是 JSON"},
            })
            .to_string(),
        );
    };
    match server.handle(request.credential.as_deref(), &payload) {
        Some(value) => response(200, &value.to_string()),
        // 通知没有响应体。
        None => response(202, ""),
    }
}

fn problem(status: u16, message: &str) -> String {
    response(status, &serde_json::json!({"error": message}).to_string())
}

fn response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
