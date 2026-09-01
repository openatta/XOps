//! 幂等键（`MCP-006`）。
//!
//! 「重复调用不产生第二次副作用，并返回与首次相同的结果」——注意后半句：
//! 它不是"第二次报错"，是**返回第一次的那个结果**。所以这里存的是响应本身，
//! 不是一个"见过了"的标记。

use std::sync::Arc;

use serde_json::Value;
use xops_core::{Error, Result};
use xops_store::Store;

/// 幂等记录的键空间。
const SPACE: &str = "mcp-idempotency";

/// 幂等键的长度上限。
pub const MAX_KEY_LEN: usize = 128;

/// 记住"这个人用这个键调过这个 tool，结果是这个"。
pub struct Idempotency {
    store: Arc<dyn Store>,
}

impl std::fmt::Debug for Idempotency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Idempotency").finish_non_exhaustive()
    }
}

impl Idempotency {
    #[must_use]
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// 查一次。
    ///
    /// # Errors
    /// 键太长，或者底层不可用。
    pub fn lookup(&self, user: &str, tool: &str, key: &str) -> Result<Option<Value>> {
        let Some(bytes) = self.store.get(SPACE, &Self::key(user, tool, key)?)? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| Error::internal(format!("幂等记录读不回来：{error}")))
    }

    /// 记下来。
    ///
    /// # Errors
    /// 键太长，或者底层不可用。
    pub fn remember(&self, user: &str, tool: &str, key: &str, result: &Value) -> Result<()> {
        let bytes = serde_json::to_vec(result)
            .map_err(|error| Error::internal(format!("幂等记录存不下：{error}")))?;
        self.store.put(SPACE, &Self::key(user, tool, key)?, &bytes)
    }

    /// 键**按人分区**：幂等键是调用方自己取的字符串，不同的人用了同一个字符串
    /// 是完全正常的事，混在一起就成了跨用户的信息泄露。
    fn key(user: &str, tool: &str, key: &str) -> Result<Vec<u8>> {
        if key.is_empty() || key.len() > MAX_KEY_LEN {
            return Err(Error::invalid(format!("幂等键要 1–{MAX_KEY_LEN} 字节")));
        }
        Ok(format!("{user}\u{0}{tool}\u{0}{key}").into_bytes())
    }
}
