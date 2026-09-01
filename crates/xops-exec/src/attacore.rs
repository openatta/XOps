//! `attacored` 客户端。
//!
//! 协议是 **NDJSON over Unix socket**：一行一个 JSON-RPC 2.0 对象，`\n` 分隔，
//! 不用 `Content-Length` 头。
//!
//! `EXE-016`（跨执行的会话隔离）在这里兑现：**一次执行一个会话，用完即弃**。
//! 会话 id 记进过程记录，所以"第一次执行在会话里留下的痕迹，第二次读不到"
//! 是可以被实测的——两次执行的会话 id 不同，这件事在 trace 上看得见。
//!
//! ⚠️ **socket 路径与令牌绝不进派工单。** AttaCore 自己的文档把话说死了：
//! "把 socket 或 token 暴露出去，等同于暴露模型凭据本身"。`EXE-015`（模型凭据只在
//! 执行方够不到的地方）在裸跑下靠的就是这一条——凭据在 attacored 那一侧，
//! 而执行方手里没有通往它的路径。

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::engine::{Cancel, Completed, Engine};
use crate::failure::FailureKind;
use crate::worksheet::Worksheet;

/// 读一帧最多等多久。
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// 接 `attacored` 的引擎。
pub struct AttaCoreEngine {
    socket: PathBuf,
    next_id: AtomicU64,
    /// 只用来做健康检查的那条连接的互斥——真正跑一次执行时每次新开一条。
    probe: Mutex<()>,
}

impl std::fmt::Debug for AttaCoreEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttaCoreEngine")
            .field("socket", &self.socket)
            .finish()
    }
}

impl AttaCoreEngine {
    #[must_use]
    pub fn at(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            next_id: AtomicU64::new(1),
            probe: Mutex::new(()),
        }
    }

    /// ⚠️ **不要自己拼 socket 路径去猜**——AttaCore 的文档明说 daemon 换启动参数
    /// 重启时路径会变，要读它的实例记录。这里只接受调用方给的路径。
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn connect(&self) -> std::result::Result<Connection, (FailureKind, String)> {
        let stream = UnixStream::connect(&self.socket).map_err(|error| {
            (
                FailureKind::Engine,
                format!("连不上 attacored（{}）：{error}", self.socket.display()),
            )
        })?;
        stream.set_read_timeout(Some(READ_TIMEOUT)).ok();
        let reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|error| (FailureKind::Engine, format!("连接复制不了：{error}")))?,
        );
        Ok(Connection { stream, reader })
    }
}

struct Connection {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Connection {
    fn send(&mut self, id: u64, method: &str, params: Value) -> std::io::Result<()> {
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stream, "{frame}")?;
        self.stream.flush()
    }

    /// 读到 id 对上的那一帧为止。**中间的通知都当过程记录收下**（`EXE-022`）。
    fn wait(&mut self, id: u64, trace: &mut String, cancel: &Cancel) -> Option<Value> {
        loop {
            if cancel.requested() {
                return None;
            }
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None,
                Ok(_) => {}
                Err(_) => return None,
            }
            let Ok(frame) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if frame.get("id").and_then(Value::as_u64) == Some(id) {
                return Some(frame);
            }
            // 通知：session.event 这类。原样收进 trace。
            trace.push_str(line.trim());
            trace.push('\n');
        }
    }
}

impl Engine for AttaCoreEngine {
    fn healthy(&self) -> bool {
        let _guard = self
            .probe
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Ok(mut connection) = self.connect() else {
            return false;
        };
        let id = self.id();
        if connection.send(id, "daemon.ping", json!({})).is_err() {
            return false;
        }
        let mut trace = String::new();
        connection.wait(id, &mut trace, &Cancel::new()).is_some()
    }

    fn run(
        &self,
        worksheet: &Worksheet,
        cancel: &Cancel,
    ) -> std::result::Result<Completed, (FailureKind, String)> {
        let mut trace = String::new();
        let mut connection = self.connect()?;

        // 一次执行一个会话（EXE-016）。
        let create = self.id();
        let project_root = worksheet
            .capabilities
            .workspace
            .as_ref()
            .map_or(Value::Null, |path| json!(path.display().to_string()));
        connection
            .send(
                create,
                "session.create",
                json!({ "project_root": project_root }),
            )
            .map_err(|error| {
                (
                    FailureKind::Engine,
                    format!("发不出 session.create：{error}"),
                )
            })?;
        let created = connection.wait(create, &mut trace, cancel).ok_or((
            FailureKind::Engine,
            "attacored 没有回 session.create".to_owned(),
        ))?;
        if let Some(error) = created.get("error") {
            return Err((FailureKind::Engine, format!("session.create 失败：{error}")));
        }
        let session = created
            .pointer("/result/session_id")
            .or_else(|| created.pointer("/result/id"))
            .and_then(Value::as_str)
            .ok_or((FailureKind::Engine, "session.create 没给会话 id".to_owned()))?
            .to_owned();
        // 会话 id 进 trace —— "两次执行不共用会话"因此是可实测的。
        trace.push_str(&format!("session={session}\n"));

        let turn = self.id();
        connection
            .send(
                turn,
                "session.run_turn",
                json!({
                    "session_id": session,
                    "message": prompt(worksheet),
                }),
            )
            .map_err(|error| {
                (
                    FailureKind::Engine,
                    format!("发不出 session.run_turn：{error}"),
                )
            })?;

        let Some(response) = connection.wait(turn, &mut trace, cancel) else {
            // 被取消或连接断了：**先把会话打断**，别留下孤儿继续烧额度（EXE-019）。
            let interrupt = self.id();
            let _ = connection.send(
                interrupt,
                "session.interrupt",
                json!({"session_id": session}),
            );
            return Err((
                if cancel.requested() {
                    FailureKind::Timeout
                } else {
                    FailureKind::Engine
                },
                trace,
            ));
        };
        if let Some(error) = response.get("error") {
            return Err((classify(error), format!("{trace}\n{error}")));
        }

        let output = response
            .pointer("/result/text")
            .or_else(|| response.pointer("/result/message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let tokens_used = response
            .pointer("/result/usage/total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if tokens_used > worksheet.limits.token_budget {
            return Err((FailureKind::TokenBudget, trace));
        }
        Ok(Completed {
            output,
            trace,
            tokens_used,
        })
    }
}

/// 派工单 → 一次对话的开场白。
///
/// **不含任何凭据、不含 socket 路径、不含到 XOps 的网络路径**（`EXE-010`、`EXE-004`）。
fn prompt(worksheet: &Worksheet) -> String {
    let mut text = String::new();
    text.push_str(&worksheet.instruction);
    if !worksheet.inputs.is_empty() {
        text.push_str("\n\n## 输入\n\n");
        text.push_str(&worksheet.inputs);
    }
    text
}

/// 引擎回的错误归到哪一类。
fn classify(error: &Value) -> FailureKind {
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_uppercase();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if code.contains("UNAUTHORIZED")
        || message.contains("credential")
        || message.contains("api key")
    {
        FailureKind::Credential
    } else if code.contains("PROJECT") || message.contains("workspace") {
        FailureKind::Workspace
    } else if message.contains("timeout") {
        FailureKind::Timeout
    } else if message.contains("rate") || message.contains("upstream") || message.contains("model")
    {
        FailureKind::ModelService
    } else {
        FailureKind::Engine
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worksheet::{Capabilities, Limits, RunId};

    fn worksheet() -> Worksheet {
        Worksheet {
            run: RunId::generate(),
            instruction: "看一眼".into(),
            skill: "查缺陷".into(),
            skill_version: "v1".into(),
            inputs: "上下文".into(),
            revision: None,
            capabilities: Capabilities::default(),
            limits: Limits::default(),
        }
    }

    #[test]
    fn 开场白里没有凭据也没有socket() {
        let engine = AttaCoreEngine::at("/tmp/attacore-test.sock");
        let text = prompt(&worksheet());
        assert!(text.contains("看一眼"));
        assert!(text.contains("上下文"));
        assert!(
            !text.contains("sock"),
            "socket 路径等同于模型凭据本身，不能进派工单"
        );
        assert!(!text.contains(&engine.socket().display().to_string()));
    }

    #[test]
    fn 连不上时是引擎错误不是别的() {
        let engine = AttaCoreEngine::at("/tmp/绝对不存在的-attacore.sock");
        assert!(!engine.healthy());
        let error = engine.run(&worksheet(), &Cancel::new()).unwrap_err();
        assert_eq!(error.0, FailureKind::Engine);
    }

    #[test]
    fn 引擎错误分得开类() {
        assert_eq!(
            classify(&json!({"code": "UNAUTHORIZED"})),
            FailureKind::Credential
        );
        assert_eq!(
            classify(&json!({"code": "PROJECT_REQUIRED"})),
            FailureKind::Workspace
        );
        assert_eq!(
            classify(&json!({"message": "upstream model error"})),
            FailureKind::ModelService
        );
        assert_eq!(
            classify(&json!({"message": "something else"})),
            FailureKind::Engine
        );
    }
}
