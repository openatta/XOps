//! stdio 传输：换行分隔的 JSON-RPC。

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;

use xops_core::{Error, Result};

use crate::McpServer;

/// 令牌从哪个环境变量取。**不从参数里取**——`I-B`。
pub const TOKEN_ENV: &str = "XOPS_TOKEN";

/// 跑一条 stdio 会话，直到输入结束。
///
/// # Errors
/// 读写失败。
pub fn serve(
    server: &Arc<McpServer>,
    credential: Option<&str>,
    input: impl Read,
    mut output: impl Write,
) -> Result<()> {
    let reader = BufReader::new(input);
    for line in reader.lines() {
        let line = line.map_err(|error| Error::unavailable(format!("读不到输入：{error}")))?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str(&line) {
            Ok(request) => server.handle(credential, &request),
            Err(error) => Some(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": crate::errors::rpc::PARSE_ERROR, "message": error.to_string()},
            })),
        };
        if let Some(response) = response {
            writeln!(output, "{response}")
                .map_err(|error| Error::unavailable(format!("写不出去：{error}")))?;
        }
    }
    output
        .flush()
        .map_err(|error| Error::unavailable(format!("写不出去：{error}")))
}
