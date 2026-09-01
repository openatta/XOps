//! Web 会话。
//!
//! `I-L` / `BRD-007`：**Web 会话凭据与 MCP 令牌互不通用**。
//!
//! 实现上这条不是靠检查，是靠**两套东西根本不认识对方**：会话 id 存在自己的键空间里、
//! 带自己的前缀；MCP 令牌只认 `xops_` 开头的原文并且比对的是摘要。拿一个去换另一个，
//! 两边都查不到。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use xops_core::{Clock, Error, Result, Timestamp};
use xops_identity::UserId;
use xops_store::Store;

/// 会话记录的键空间。
const SPACE: &str = "web-session";
/// 会话 id 的前缀。**与 MCP 令牌的 `xops_` 不同**，一眼能分出来是哪一种凭据。
pub const SESSION_PREFIX: &str = "xsess_";
/// 会话有效期：12 小时。
pub const TTL_MILLIS: i64 = 12 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    user: UserId,
    issued_at: Timestamp,
    expires_at: Timestamp,
}

/// 会话面。
pub struct Sessions {
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Sessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sessions").finish_non_exhaustive()
    }
}

impl Sessions {
    #[must_use]
    pub fn new(store: Arc<dyn Store>, clock: Arc<dyn Clock>) -> Self {
        Self { store, clock }
    }

    /// 建一个会话。
    ///
    /// # Errors
    /// 取不到系统熵或底层不可用。
    pub fn issue(&self, user: UserId) -> Result<String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| Error::internal(format!("取不到系统熵：{error}")))?;
        let mut id = String::from(SESSION_PREFIX);
        for byte in bytes {
            use std::fmt::Write as _;
            let _ = write!(id, "{byte:02x}");
        }
        let now = self.clock.now();
        let record = Record {
            user,
            issued_at: now,
            expires_at: Timestamp::from_millis(now.as_millis() + TTL_MILLIS),
        };
        let encoded = serde_json::to_vec(&record)
            .map_err(|error| Error::internal(format!("会话存不下：{error}")))?;
        self.store.put(SPACE, id.as_bytes(), &encoded)?;
        Ok(id)
    }

    /// 会话 id → 谁。
    ///
    /// # Errors
    /// 底层不可用。**会话不存在或已过期时返回 `None`，不区分**——
    /// 与令牌那一侧同一条纪律。
    pub fn resolve(&self, id: &str) -> Result<Option<UserId>> {
        if !id.starts_with(SESSION_PREFIX) {
            // 拿一个 MCP 令牌来当会话用，走的就是这条 —— 它连查都不用查（I-L）。
            return Ok(None);
        }
        let Some(bytes) = self.store.get(SPACE, id.as_bytes())? else {
            return Ok(None);
        };
        let record: Record = serde_json::from_slice(&bytes)
            .map_err(|error| Error::internal(format!("会话读不回来：{error}")))?;
        if self.clock.now() >= record.expires_at {
            return Ok(None);
        }
        Ok(Some(record.user))
    }

    /// 注销。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn revoke(&self, id: &str) -> Result<()> {
        self.store.delete(SPACE, id.as_bytes())
    }
}
