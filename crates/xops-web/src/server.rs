//! 只读 HTTP 服务。
//!
//! ⚠️ **它与 MCP 的服务面不是同一个，也不共用路由层**（RP-03 / RP-05 的分工）。
//! 两边各有一小段 HTTP 解析，这份重复是**故意留的**：把它们合起来就等于让两个
//! 服务面共用一个路由层，而那正是这条分工要避免的事。
//!
//! 会话凭据从 `Authorization: Bearer <会话 id>` 或 `Cookie: xops_session=<会话 id>` 来。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use serde_json::{Value, json};
use xops_core::{Error, Id, Result, RowId};
use xops_identity::{Directory, ProjectId};
use xops_read::ReadModel;
use xops_table::TableId;

use crate::assets::Assets;
use crate::routes::{Kind, match_route};
use crate::session::Sessions;

const MAX_BODY: usize = 64 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;

/// 只读 Web 后端。
pub struct WebServer {
    model: Arc<ReadModel>,
    directory: Arc<Directory>,
    sessions: Arc<Sessions>,
    assets: Assets,
}

impl std::fmt::Debug for WebServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebServer").finish_non_exhaustive()
    }
}

/// 一次请求。
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub session: Option<String>,
    pub body: Vec<u8>,
}

/// 一次响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// 要下发的会话 id（登录成功时）。
    pub set_session: Option<String>,
}

impl Response {
    fn json(status: u16, value: &Value) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: value.to_string().into_bytes(),
            set_session: None,
        }
    }

    fn problem(status: u16, message: &str) -> Self {
        Self::json(status, &json!({"error": message}))
    }
}

impl WebServer {
    #[must_use]
    pub fn new(
        model: Arc<ReadModel>,
        directory: Arc<Directory>,
        sessions: Arc<Sessions>,
        assets: Assets,
    ) -> Self {
        Self {
            model,
            directory,
            sessions,
            assets,
        }
    }

    /// 处理一次请求。**没有传输，因而整段可以直接测。**
    #[must_use]
    pub fn handle(&self, request: &Request) -> Response {
        match self.dispatch(request) {
            Ok(response) => response,
            Err(error) => {
                let status = match error.kind() {
                    xops_core::ErrorKind::NotFound => 404,
                    xops_core::ErrorKind::Denied => 401,
                    xops_core::ErrorKind::Invalid => 400,
                    _ => 500,
                };
                Response::problem(status, error.message())
            }
        }
    }

    fn dispatch(&self, request: &Request) -> Result<Response> {
        let Some((route, captured)) = match_route(&request.method, &request.path) else {
            // 没命中 API 路由就交给静态资源（前端是个 SPA，深链要回落到 index.html）。
            return Ok(self.assets.serve(&request.method, &request.path));
        };
        if route.kind == Kind::Credential {
            return self.session_face(request);
        }
        debug_assert_eq!(route.kind, Kind::Read);

        // 只读面一律要会话。可见性完全遵循项目成员边界（BRD-011）。
        let viewer = request
            .session
            .as_deref()
            .map(|id| self.sessions.resolve(id))
            .transpose()?
            .flatten()
            .ok_or_else(|| Error::denied("请先登录"))?;

        let value = match route.path {
            "/api/me" => serde_json::to_value(self.model.me(viewer)?),
            "/api/projects" => serde_json::to_value(json!({
                "projects": self.model.projects(viewer)?,
            })),
            "/api/projects/{}/boards" => {
                let project = parse_project(&captured[0])?;
                serde_json::to_value(json!({"boards": self.model.boards(viewer, project)?}))
            }
            "/api/projects/{}/boards/{}" => {
                let board = xops_read::BoardId::from_id(Id::parse(&captured[1])?);
                serde_json::to_value(self.model.board(viewer, board, 200)?)
            }
            "/api/projects/{}/tables/{}/rows/{}/history" => {
                let project = parse_project(&captured[0])?;
                let table = parse_table(&captured[1])?;
                let row = RowId::from_id(Id::parse(&captured[2])?);
                serde_json::to_value(self.model.row_history(viewer, project, &table, row)?)
            }
            "/api/projects/{}/tables/{}/instances/{}/settlements" => {
                let project = parse_project(&captured[0])?;
                let table = parse_table(&captured[1])?;
                let instance = Id::parse(&captured[2])?;
                serde_json::to_value(json!({
                    "settlements": self.model.settlements(viewer, project, &table, instance)?,
                }))
            }
            "/api/projects/{}/tables/{}/rows/{}/columns/{}/raw" => {
                let project = parse_project(&captured[0])?;
                let table = parse_table(&captured[1])?;
                let row = RowId::from_id(Id::parse(&captured[2])?);
                let view = self
                    .model
                    .long_text(viewer, project, &table, row, &captured[3])?;
                // BRD-010：原始形式。**不是 HTML，也不经任何渲染。**
                return Ok(Response {
                    status: 200,
                    content_type: "text/plain; charset=utf-8",
                    body: view.text.into_bytes(),
                    set_session: None,
                });
            }
            other => return Err(Error::internal(format!("路由 {other} 没有实现"))),
        };
        Ok(Response::json(
            200,
            &value.map_err(|error| Error::internal(format!("装不下：{error}")))?,
        ))
    }

    /// 凭据面：登录与注销。**`MCP-013` 认下的例外之一，不写任何业务对象。**
    fn session_face(&self, request: &Request) -> Result<Response> {
        if request.method == "DELETE" {
            if let Some(id) = request.session.as_deref() {
                self.sessions.revoke(id)?;
            }
            return Ok(Response::json(200, &json!({"ok": true})));
        }
        let payload: Value =
            serde_json::from_slice(&request.body).map_err(|_| Error::invalid("请求体不是 JSON"))?;
        let provider = payload["provider"].as_str().unwrap_or("builtin");
        let account = payload["account"].as_str().unwrap_or_default();
        let secret = payload["secret"].as_str().unwrap_or_default();
        let user = self.directory.login(provider, account, secret)?;
        let id = self.sessions.issue(user.id)?;
        Ok(Response {
            set_session: Some(id.clone()),
            ..Response::json(200, &json!({"session": id, "user": user.id.to_string()}))
        })
    }

    /// 监听并一直服务下去。
    ///
    /// # Errors
    /// 端口占用或监听失败。
    pub fn serve(self: &Arc<Self>, address: impl ToSocketAddrs) -> Result<()> {
        let listener = listen(address)?;
        self.serve_listener(&listener)
    }

    /// 服务一个已经绑好的监听器。测试要拿到实际端口，所以分成两步。
    ///
    /// # Errors
    /// 监听器坏了。
    pub fn serve_listener(self: &Arc<Self>, listener: &TcpListener) -> Result<()> {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let server = Arc::clone(self);
            thread::spawn(move || {
                let _ = server.handle_connection(stream);
            });
        }
        Ok(())
    }

    fn handle_connection(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let response = match read_request(&mut stream) {
            Ok(request) => self.handle(&request),
            Err(response) => response,
        };
        stream.write_all(&render(&response))?;
        stream.flush()
    }
}

/// 绑定端口。
///
/// # Errors
/// 绑不上。
pub fn listen(address: impl ToSocketAddrs) -> Result<TcpListener> {
    TcpListener::bind(address).map_err(|error| Error::unavailable(format!("监听失败：{error}")))
}

fn parse_project(text: &str) -> Result<ProjectId> {
    Ok(ProjectId::from_id(Id::parse(text)?))
}

fn parse_table(name: &str) -> Result<TableId> {
    if name.starts_with('_') {
        TableId::system(name)
    } else {
        TableId::user(name)
    }
}

fn read_request(stream: &mut TcpStream) -> std::result::Result<Request, Response> {
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|_| Response::problem(500, "读不到请求"))?,
    );
    let mut head = String::new();
    let mut consumed = 0;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|_| Response::problem(400, "读不到请求"))?;
        if read == 0 {
            return Err(Response::problem(400, "读不到请求"));
        }
        consumed += read;
        if consumed > MAX_HEADER_BYTES {
            return Err(Response::problem(431, "请求头太大"));
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        head.push_str(&line);
    }

    let mut lines = head.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| Response::problem(400, "读不到请求"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or("/")
        .to_owned();

    let mut length = 0usize;
    let mut session = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().unwrap_or(0),
            "authorization" => {
                session = value.strip_prefix("Bearer ").map(str::to_owned);
            }
            "cookie" if session.is_none() => {
                session = value
                    .split(';')
                    .filter_map(|part| part.trim().strip_prefix("xops_session="))
                    .map(str::to_owned)
                    .next();
            }
            _ => {}
        }
    }
    if length > MAX_BODY {
        return Err(Response::problem(413, "请求体太大"));
    }
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|_| Response::problem(400, "读不到请求体"))?;
    Ok(Request {
        method,
        path,
        session,
        body,
    })
}

fn render(response: &Response) -> Vec<u8> {
    let reason = match response.status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    };
    let cookie = response
        .set_session
        .as_ref()
        .map_or_else(String::new, |id| {
            // HttpOnly：前端 JS 读不到它，因而 XSS 也偷不走。SameSite=Strict：跨站带不过去。
            format!("Set-Cookie: xops_session={id}; HttpOnly; SameSite=Strict; Path=/\r\n")
        });
    let mut out = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{cookie}Connection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    )
    .into_bytes();
    out.extend_from_slice(&response.body);
    out
}

/// 静态资源目录的默认位置。
#[must_use]
pub fn default_assets_dir() -> PathBuf {
    PathBuf::from("web/dist")
}
