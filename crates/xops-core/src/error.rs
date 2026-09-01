//! 错误与它的分类。
//!
//! 分类存在的理由只有一个：**让调用方知道该重试还是该改参数**（`MCP-007`）。
//! 不做错误码字符串——那是 MCP 层（RP-03）把这几类映射出去的事，不是这里的事。

use std::fmt;

/// 错误的类别。**对外映射由 RP-03 负责**，这里只回答"这是哪一类"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// 参数或状态不合法。改了参数才可能成功。
    Invalid,
    /// 目标不存在。
    NotFound,
    /// 与既有状态冲突（例如往一个已存在的事件序号上写）。
    Conflict,
    /// 权限不足。
    ///
    /// ⚠️ **对外不得原样透出**：`MCP-008` 要求"无权限"与"不存在"返回完全一致的错误，
    /// 否则错误码本身就是探测他人项目的工具。映射在 RP-03。
    Denied,
    /// 超时。求值超时属于这一类（`CON-009`）。
    Timeout,
    /// 依赖暂时不可用。
    Unavailable,
    /// 不该发生的内部状态。
    Internal,
}

impl ErrorKind {
    /// 重试有没有意义。`MCP-007` 要求把这件事说清楚。
    #[must_use]
    pub fn retriable(self) -> bool {
        matches!(self, Self::Timeout | Self::Unavailable)
    }
}

/// 一个错误：一个类别加一句给人看的话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn retriable(&self) -> bool {
        self.kind.retriable()
    }
}

macro_rules! constructor {
    ($name:ident, $kind:ident) => {
        impl Error {
            #[doc = concat!("构造一个 `ErrorKind::", stringify!($kind), "`。")]
            pub fn $name(message: impl Into<String>) -> Self {
                Self::new(ErrorKind::$kind, message)
            }
        }
    };
}

constructor!(invalid, Invalid);
constructor!(not_found, NotFound);
constructor!(conflict, Conflict);
constructor!(denied, Denied);
constructor!(timeout, Timeout);
constructor!(unavailable, Unavailable);
constructor!(internal, Internal);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for Error {}

/// 全仓统一的 `Result`。
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 只有超时与不可用值得重试() {
        assert!(ErrorKind::Timeout.retriable());
        assert!(ErrorKind::Unavailable.retriable());
        for kind in [
            ErrorKind::Invalid,
            ErrorKind::NotFound,
            ErrorKind::Conflict,
            ErrorKind::Denied,
            ErrorKind::Internal,
        ] {
            assert!(!kind.retriable(), "{kind:?} 不该被当成可重试");
        }
    }

    #[test]
    fn 错误带得住原话() {
        let error = Error::invalid("列 status 不在 schema 里");
        assert_eq!(error.kind(), ErrorKind::Invalid);
        assert_eq!(error.message(), "列 status 不在 schema 里");
        assert!(error.to_string().contains("列 status"));
    }
}
