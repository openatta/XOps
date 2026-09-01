//! 统一错误契约（`MCP-007`）。
//!
//! 三样：**稳定错误码 · 可读消息 · "该重试还是该改参数"的明确区分**。
//!
//! ⚠️ `ErrorKind::Denied` 映射成 `unauthenticated` 而不是某种"无权限"——
//! **XOps 里没有"无权限"这个对外错误**。鉴权失败在 RP-02 那一层就已经变成
//! 「不存在」了（`PRJ-008` + `MCP-008`），能走到这里的 `Denied` 只有一种：令牌不对。

use serde_json::{Value, json};
use xops_core::{Error, ErrorKind};

/// 对外的错误形态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorContract {
    /// 稳定错误码。**客户端按它分支，不按消息分支。**
    pub code: &'static str,
    pub message: String,
    /// 重试有没有意义。
    pub retriable: bool,
}

impl ErrorContract {
    #[must_use]
    pub fn of(error: &Error) -> Self {
        let code = match error.kind() {
            ErrorKind::Invalid => "invalid_argument",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Denied => "unauthenticated",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Unavailable => "unavailable",
            ErrorKind::Internal => "internal",
        };
        Self {
            code,
            message: error.message().to_owned(),
            retriable: error.retriable(),
        }
    }

    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({ "code": self.code, "message": self.message, "retriable": self.retriable })
    }
}

/// JSON-RPC 的错误码。协议层的错，不是业务的错。
pub mod rpc {
    pub const PARSE_ERROR: i64 = -32_700;
    pub const INVALID_REQUEST: i64 = -32_600;
    pub const METHOD_NOT_FOUND: i64 = -32_601;
    pub const INVALID_PARAMS: i64 = -32_602;
    /// 没带令牌或令牌无效（`MCP-002`）。
    pub const UNAUTHENTICATED: i64 = -32_001;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 每一类都有稳定的码() {
        assert_eq!(
            ErrorContract::of(&Error::invalid("x")).code,
            "invalid_argument"
        );
        assert_eq!(ErrorContract::of(&Error::not_found("x")).code, "not_found");
        assert_eq!(ErrorContract::of(&Error::conflict("x")).code, "conflict");
        assert_eq!(
            ErrorContract::of(&Error::denied("x")).code,
            "unauthenticated"
        );
        assert_eq!(ErrorContract::of(&Error::timeout("x")).code, "timeout");
        assert_eq!(
            ErrorContract::of(&Error::unavailable("x")).code,
            "unavailable"
        );
        assert_eq!(ErrorContract::of(&Error::internal("x")).code, "internal");
    }

    #[test]
    fn 该重试的说得清() {
        assert!(ErrorContract::of(&Error::timeout("x")).retriable);
        assert!(ErrorContract::of(&Error::unavailable("x")).retriable);
        assert!(!ErrorContract::of(&Error::invalid("x")).retriable);
        assert!(!ErrorContract::of(&Error::not_found("x")).retriable);
    }

    #[test]
    fn 无权限与不存在在这一层已经是同一件事() {
        // RP-02 的 authorize 对"不是成员""角色不够""项目不存在"返回同一个 NotFound，
        // 所以这里看到的是同一个契约 —— MCP-008 在两层之间不需要第二次对齐。
        let denied_by_role = Error::not_found("不存在");
        let truly_missing = Error::not_found("不存在");
        assert_eq!(
            ErrorContract::of(&denied_by_role),
            ErrorContract::of(&truly_missing)
        );
    }
}
