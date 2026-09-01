//! 审计事件的形状与它的类型目录。
//!
//! **审计事件不是第二套事件流。** `AUD` 与 §5「表对内追加」是同一套流的两个视角：
//! 一次业务写追加的那条事件，本身就是它的审计记录——`AUD-005`（不存在"业务成功但没留痕"
//! 或"留了痕但业务没生效"的中间态）**因此是结构性成立的，不是靠两次写小心翼翼地对齐**。
//! 跨表写没有原子性（`CON-007`），任何"先写业务再写审计"的实现都做不到这条。
//!
//! 代价是：`xops_core::Event` 只带得动"谁、何时、动了哪张表的哪一行"，而 `AUD-002` 还要
//! 项目、事件类型与结构化载荷。这三样装在 payload 的**信封**里——[`AuditEnvelope`]。
//! 凡是要留痕的写，payload 一律是一个信封。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xops_core::{Error, Id, Result};

/// 事件类型。形如 `project.created` —— `<域>.<动作>`，可以更深，如 `flow.node.settled`。
///
/// `AUD-009` 要的"统一目录与扩展方式"就是这个类型加 [`catalog`]：**目录是常量清单，
/// 扩展方式是新加一个常量**，而不是各处随手写字符串。校验挡住的是后者。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventKind(String);

impl EventKind {
    pub const MAX_LEN: usize = 64;

    /// # Errors
    /// 不是 `<段>.<段>[.<段>…]` 的形状，或者用了小写字母数字与 `-` 之外的字符。
    pub fn new(kind: impl Into<String>) -> Result<Self> {
        let kind = kind.into();
        if kind.len() > Self::MAX_LEN {
            return Err(Error::invalid(format!(
                "事件类型最长 {} 字节",
                Self::MAX_LEN
            )));
        }
        let segments: Vec<&str> = kind.split('.').collect();
        let shaped = segments.len() >= 2
            && segments.iter().all(|segment| {
                !segment.is_empty()
                    && segment.starts_with(|c: char| c.is_ascii_lowercase())
                    && segment
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            });
        if !shaped {
            return Err(Error::invalid(format!(
                "事件类型要写成 <域>.<动作>，小写字母数字与 -：{kind}"
            )));
        }
        Ok(Self(kind))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 它属于哪个域（第一段）。按域查询用它。
    #[must_use]
    pub fn domain(&self) -> &str {
        self.0.split('.').next().unwrap_or(&self.0)
    }
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `AUD-011` 点名"一律留痕"的那些动作，加上本包自己的几条。
///
/// **缺一条就有一处黑箱。** 后面每个包往这里加自己的常量，不要在调用处写裸字符串——
/// [`catalog`] 是唯一一处能回答"系统里到底有哪些事件类型"的地方。
pub mod kinds {
    // 项目与成员（RP-02）
    pub const PROJECT_CREATED: &str = "project.created";
    pub const PROJECT_ARCHIVED: &str = "project.archived";
    pub const MEMBER_ADDED: &str = "member.added";
    pub const MEMBER_ROLE_CHANGED: &str = "member.role-changed";
    pub const MEMBER_REMOVED: &str = "member.removed";
    // 身份与令牌（RP-02）
    pub const USER_CREATED: &str = "user.created";
    pub const TOKEN_ISSUED: &str = "token.issued";
    pub const TOKEN_REVOKED: &str = "token.revoked";
    pub const TOKEN_USED: &str = "token.used";
    // 调用留痕（RP-03 会用到；AUD-007 失败留痕）
    pub const CALL_REJECTED: &str = "call.rejected";
    // 清理（AUD-010 / RET）
    pub const AUDIT_PRUNED: &str = "audit.pruned";

    /// 全部已知类型。新增一条常量就往这里加一行。
    pub const ALL: &[&str] = &[
        PROJECT_CREATED,
        PROJECT_ARCHIVED,
        MEMBER_ADDED,
        MEMBER_ROLE_CHANGED,
        MEMBER_REMOVED,
        USER_CREATED,
        TOKEN_ISSUED,
        TOKEN_REVOKED,
        TOKEN_USED,
        CALL_REJECTED,
        AUDIT_PRUNED,
    ];
}

/// 事件类型目录（`AUD-009`）。
///
/// # Panics
/// 目录里有不合形状的类型——那是常量写错了，该在第一次跑测试时就炸。
#[must_use]
pub fn catalog() -> Vec<EventKind> {
    kinds::ALL
        .iter()
        .map(|kind| EventKind::new(*kind).expect("目录里的类型必须合法"))
        .collect()
}

/// 这次动作成了还是没成。
///
/// `AUD-007`：**失败留痕与成功事件区分开**——失败用于排查与滥用检测，不进入业务账本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Succeeded,
    Rejected,
}

/// 一次写的 payload 信封。`AUD-002` 少的那几样都在这里。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEnvelope {
    pub kind: EventKind,
    /// 所属项目。
    ///
    /// `None` 是**平台级事件**（建用户、签令牌）——`AUD-003`：它们只有其主体本人可读，
    /// 不属于任何项目，因而不会出现在任何项目的事件流里。
    pub project: Option<Id>,
    /// 目标对象。"查询某个对象的完整历史"（`AUD-008`）按它查。
    pub target: Id,
    /// 主体本人。平台级事件靠它做可见性判定。
    pub subject: Option<Id>,
    pub outcome: Outcome,
    /// 结构化载荷，**不是一段自由文本**（`AUD-002`）。
    pub data: Value,
}

impl AuditEnvelope {
    /// 一条成功的、属于某个项目的事件。
    ///
    /// # Errors
    /// 事件类型不合形状。
    pub fn project_scoped(kind: &str, project: Id, target: Id, data: Value) -> Result<Self> {
        Ok(Self {
            kind: EventKind::new(kind)?,
            project: Some(project),
            target,
            subject: None,
            outcome: Outcome::Succeeded,
            data,
        })
    }

    /// 一条成功的平台级事件，只有 `subject` 本人读得到。
    ///
    /// # Errors
    /// 事件类型不合形状。
    pub fn platform(kind: &str, subject: Id, target: Id, data: Value) -> Result<Self> {
        Ok(Self {
            kind: EventKind::new(kind)?,
            project: None,
            target,
            subject: Some(subject),
            outcome: Outcome::Succeeded,
            data,
        })
    }

    /// 标成失败留痕（`AUD-007`）。
    #[must_use]
    pub fn rejected(mut self) -> Self {
        self.outcome = Outcome::Rejected;
        self
    }

    /// 从一条事件的 payload 里读回信封。**不是信封就返回 `None`**——
    /// 表引擎写的业务行不是审计事件，它们的 payload 是行本身。
    #[must_use]
    pub fn from_payload(payload: &Value) -> Option<Self> {
        serde_json::from_value(payload.clone()).ok()
    }

    /// 装回 payload。
    ///
    /// # Errors
    /// 序列化失败。
    pub fn to_payload(&self) -> Result<Value> {
        serde_json::to_value(self)
            .map_err(|error| Error::internal(format!("审计信封装不进 payload：{error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 目录里每一条都合形状() {
        let catalog = catalog();
        assert_eq!(catalog.len(), kinds::ALL.len());
        assert!(
            catalog
                .iter()
                .any(|kind| kind.as_str() == kinds::PROJECT_CREATED)
        );
    }

    #[test]
    fn 类型要有域和动作() {
        assert!(EventKind::new("project.created").is_ok());
        assert!(EventKind::new("flow.node.settled").is_ok());
        assert!(EventKind::new("member.role-changed").is_ok());
        assert!(EventKind::new("created").is_err(), "只有一段不行");
        assert!(EventKind::new("Project.Created").is_err(), "大写不行");
        assert!(EventKind::new("project..created").is_err(), "空段不行");
        assert!(EventKind::new("1project.created").is_err(), "数字开头不行");
    }

    #[test]
    fn 域取得出来() {
        assert_eq!(
            EventKind::new("flow.node.settled").unwrap().domain(),
            "flow"
        );
    }

    #[test]
    fn 信封可往返() {
        let envelope = AuditEnvelope::project_scoped(
            kinds::MEMBER_ADDED,
            Id::from_parts(1, 1),
            Id::from_parts(2, 2),
            serde_json::json!({"role": "member"}),
        )
        .unwrap();
        let payload = envelope.to_payload().unwrap();
        assert_eq!(AuditEnvelope::from_payload(&payload).unwrap(), envelope);
    }

    #[test]
    fn 业务行的payload不会被当成信封() {
        let row = serde_json::json!({"title": "崩了", "state": "新建"});
        assert!(AuditEnvelope::from_payload(&row).is_none());
    }

    #[test]
    fn 失败留痕与成功区分得开() {
        let envelope = AuditEnvelope::platform(
            kinds::TOKEN_ISSUED,
            Id::from_parts(1, 1),
            Id::from_parts(1, 1),
            Value::Null,
        )
        .unwrap();
        assert_eq!(envelope.outcome, Outcome::Succeeded);
        assert_eq!(envelope.rejected().outcome, Outcome::Rejected);
    }
}
