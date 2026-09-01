//! 事件的形状。
//!
//! **事件是底层的真相，投影是它的缓存**（§4.3）。这条选择贯穿整个写入路径：
//! `I-D` 事件一经写入即不可变、`I-N` 不存在只改投影而不写事件的路径，两条都靠它成立。
//!
//! 这里只定形状。"什么是审计事件"归 RP-02，"payload 里是什么"归 RP-04 ——
//! 本 crate 不解释 payload，它对这里是一坨不透明的 JSON。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::id::Id;
use crate::time::Timestamp;

/// 表名。**串行区间按它的字典序取锁**（`CON-004`），所以它必须是可全序比较的。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TableName(String);

impl TableName {
    /// 最长长度。够用，且让键的编码有个上界。
    pub const MAX_LEN: usize = 64;

    /// # Errors
    /// 空、超长、含空白或控制字符、含键编码用的分隔符 `\0`。
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            return Err(Error::invalid("表名不能为空"));
        }
        if name.len() > Self::MAX_LEN {
            return Err(Error::invalid(format!("表名最长 {} 字节", Self::MAX_LEN)));
        }
        if name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\0')
        {
            return Err(Error::invalid("表名不能含空白或控制字符"));
        }
        Ok(Self(name))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 是不是系统表（`_runs` 这类）。系统表的 schema 固定、只有平台能写（`TBL` 域）。
    #[must_use]
    pub fn is_system(&self) -> bool {
        self.0.starts_with('_')
    }
}

impl std::fmt::Display for TableName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 行标识。平台生成，按时间可排序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RowId(Id);

impl RowId {
    #[must_use]
    pub fn generate() -> Self {
        Self(Id::generate())
    }

    #[must_use]
    pub const fn from_id(id: Id) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_id(self) -> Id {
        self.0
    }
}

impl std::fmt::Display for RowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 一次写是三种之一。**只有 `Insert` 参与流程求值**（D45），但三种都追加事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteOp {
    Insert,
    Update,
    Delete,
}

/// 谁写的。
///
/// `I-B`：**它只能来自这四个地方——令牌解析、执行标识、插件求值、平台自身，
/// 不来自请求体。** 这个类型不提供"从字符串随便造一个"的构造，就是为了让越界显眼。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Actor {
    /// 令牌解析出来的用户。
    User { user: String },
    /// 一次执行。写这一行的是容器里跑完的那次任务。
    Execution { run: Id },
    /// 插件求值交回、由平台代写的行（`CON-003`）。
    Plugin { plugin: String },
    /// 平台自身（迁移、清理、系统表维护）。
    Platform,
}

/// 一条事件。写进去就不再变（`I-D`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// 事件自己的 ID。
    pub id: Id,
    /// 它属于哪张表。
    pub table: TableName,
    /// 它动的是哪一行。
    pub row: RowId,
    /// **每张表独立**、从 1 开始、不跳号的序号。投影的水位线按它算。
    pub seq: u64,
    pub op: WriteOp,
    pub at: Timestamp,
    pub actor: Actor,
    /// 这次写的内容。`Insert` / `Update` 是行的新形态，`Delete` 是 `null`。
    ///
    /// **本 crate 不解释它**——列与类型是 RP-04 的事。
    pub payload: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 表名拒绝坏形状() {
        assert!(TableName::new("bugs").is_ok());
        assert!(TableName::new("_runs").is_ok());
        assert!(TableName::new("").is_err());
        assert!(TableName::new("有 空格").is_err());
        assert!(TableName::new("带\0分隔符").is_err());
        assert!(TableName::new("x".repeat(TableName::MAX_LEN + 1)).is_err());
    }

    #[test]
    fn 系统表看前缀() {
        assert!(TableName::new("_runs").unwrap().is_system());
        assert!(!TableName::new("runs").unwrap().is_system());
    }

    #[test]
    fn 表名按字典序全序() {
        let mut names = [
            TableName::new("bugs").unwrap(),
            TableName::new("_runs").unwrap(),
            TableName::new("approvals").unwrap(),
        ];
        names.sort();
        let ordered: Vec<&str> = names.iter().map(TableName::as_str).collect();
        assert_eq!(ordered, vec!["_runs", "approvals", "bugs"]);
    }

    #[test]
    fn 事件可往返() {
        let event = Event {
            id: Id::from_parts(1, 2),
            table: TableName::new("bugs").unwrap(),
            row: RowId::from_id(Id::from_parts(3, 4)),
            seq: 1,
            op: WriteOp::Insert,
            at: Timestamp::from_millis(1_700_000_000_000),
            actor: Actor::User { user: "u-1".into() },
            payload: serde_json::json!({"title": "崩了"}),
        };
        let text = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&text).unwrap(), event);
    }
}
