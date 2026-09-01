//! `_notices` 的**两个**平台专属 tool（`NTF-009`）。
//!
//! > **这两个之外不派发任何 `_notices` 的 tool。** 这不是"暂时没做"，是 `NTF-009`。
//!
//! 两处**结构性**的落法，都不是靠 tool 体里判一下：
//!
//! ```text
//! 行级限定    两个 schema 里**都没有 user 这个字段** —— 「看别人的」表达不出来（NTF-010）
//! 不派发通用  [`register`] 只注册这两个；有一条枚举本文件源码的测试盯着（I-Y）
//! ```
//!
//! ⚠️ `_notices` 也**不参与表专属 tool 的派发**（`TBL-011`、`MCP-005`），
//! 也**建不了自由看板**（`BRD-004`）——后者由 RP-05 的看板定义那一侧拒掉。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Error, Result};
use xops_mcp::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};

use crate::notice::{Notice, NoticeId};
use crate::service::Notices;

/// 事件类型。
pub mod kinds {
    pub const NOTICE_CREATED: &str = "notice.created";
    pub const NOTICE_READ: &str = "notice.read";
}

fn describe(notice: &Notice) -> Value {
    json!({
        "notice": notice.id.to_string(),
        "project": notice.project.map(|project| project.to_string()),
        "kind": notice.kind.as_str(),
        "subject": notice.subject,
        "text": notice.text,
        "createdAt": notice.created_at.as_millis(),
    })
}

/// 查我的未读。
pub struct MyUnread {
    spec: ToolSpec,
    notices: Arc<Notices>,
}

impl MyUnread {
    /// # Errors
    /// 声明不合形状。
    pub fn new(notices: Arc<Notices>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("notice.unread")
                .summary(
                    "查我的未读通知。**跨项目一起给**——「我在 N 个项目里的待办」\
                     在一个地方看得到（NTF-014）",
                )
                // **没有 user 字段。** 「看别人的」在这个 schema 里表达不出来（NTF-010）。
                .input(Schema::new().field(Field::optional(
                    "limit",
                    FieldType::Integer,
                    "最多几条",
                )))
                // 平台级：它不针对某个项目，因为一个人的待办本来就横跨项目。
                .requires(Requirement::Platform)
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::NOTICE_CREATED)
                .build()?,
            notices,
        })
    }
}

impl Tool for MyUnread {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let limit = context
            .arg("limit")
            .and_then(Value::as_i64)
            .and_then(|limit| usize::try_from(limit).ok())
            .unwrap_or(100)
            .min(1_000);
        // 令牌持有人本人，不是参数里的谁。
        let mine = self.notices.unread(context.identity.user.id, limit)?;
        Ok(json!({"notices": mine.iter().map(describe).collect::<Vec<_>>()}))
    }
}

/// 标记已读。
pub struct MarkRead {
    spec: ToolSpec,
    notices: Arc<Notices>,
}

impl MarkRead {
    /// # Errors
    /// 声明不合形状。
    pub fn new(notices: Arc<Notices>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("notice.read")
                .summary(
                    "把一条通知标记为已读。**只能改自己那一行、只能改 readAt 这一列**，\
                     且照样追加事件（NTF-011 / I-N）",
                )
                .input(Schema::new().field(Field::required(
                    "notice",
                    FieldType::Id,
                    "哪一条。**不是自己的那一条与不存在完全一致**",
                )))
                .requires(Requirement::Platform)
                .idempotency(Idempotency::Keyed)
                .audits(kinds::NOTICE_READ)
                .build()?,
            notices,
        })
    }
}

impl Tool for MarkRead {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let notice = NoticeId::from_id(context.id("notice")?);
        let updated = self.notices.mark_read(context.identity.user.id, notice)?;
        let read_at = updated
            .read_at
            .ok_or_else(|| Error::internal("标记完了却没有 readAt"))?;
        Ok(json!({"notice": updated.id.to_string(), "readAt": read_at.as_millis()}))
    }
}

/// 通知域的 tool 名字。**恰好两个。**
pub const NOTICE_TOOLS: [&str; 2] = ["notice.unread", "notice.read"];

/// 注册通知域。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, notices: &Arc<Notices>) -> Result<()> {
    registry.register(Arc::new(MyUnread::new(Arc::clone(notices))?))?;
    registry.register(Arc::new(MarkRead::new(Arc::clone(notices))?))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 恰好两个而且都不收user() {
        let source = include_str!("tools.rs");
        let body = source.split("#[cfg(test)]").next().unwrap();
        let registered = body.matches("registry.register(").count();
        assert_eq!(registered, 2, "NTF-009：只有两个平台专属 tool");
        assert_eq!(NOTICE_TOOLS.len(), 2);
        // 「看别人的」表达不出来 —— schema 里没有 user 这个字段。
        assert!(
            !body.contains(&format!("{}(\"user\"", "Field::required")),
            "NTF-010：读写硬限定为令牌持有人"
        );
        assert!(!body.contains(&format!("{}(\"user\"", "Field::optional")));
    }
}
