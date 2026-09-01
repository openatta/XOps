//! 令牌域的 tool。
//!
//! ⚠️ **这是 `MCP-013` 四个例外里的"令牌管理面"**：它是凭据类的，
//! **不写任何项目内的业务对象**。签发与撤销都只动 `_tokens`。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_audit::kinds;
use xops_core::{Result, Timestamp};
use xops_identity::{Directory, TokenId};

use crate::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use crate::schema::{Field, FieldType, Schema};

pub struct IssueToken {
    spec: ToolSpec,
    directory: Arc<Directory>,
}

impl IssueToken {
    /// # Errors
    /// 声明不合形状。
    pub fn new(directory: Arc<Directory>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("token.issue")
                .summary("签一个令牌。**原文只呈现这一次**，之后系统只保存不可逆摘要")
                .input(
                    Schema::new()
                        .field(Field::required(
                            "label",
                            FieldType::Text { max_len: 64 },
                            "给人看的名字，用来分辨哪台机器上的哪个工具",
                        ))
                        .field(Field::optional(
                            "expiresAt",
                            FieldType::Timestamp,
                            "过期时刻。**已过期与已撤销的行为完全一致**",
                        )),
                )
                .requires(Requirement::Platform)
                // 幂等键在这里没有意义：签两次就该是两个令牌，而"返回与首次相同的结果"
                // 会把第一次的原文再吐一遍 —— 那正好破掉 TOK-002。
                .idempotency(Idempotency::NotIdempotent {
                    reason: "重复签发就该是两个令牌；返回首次结果等于把原文再呈现一次，破 TOK-002",
                })
                .audits(kinds::TOKEN_ISSUED)
                .build()?,
            directory,
        })
    }
}

impl Tool for IssueToken {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let expires_at = context
            .arg("expiresAt")
            .and_then(Value::as_i64)
            .map(Timestamp::from_millis);
        let (token, secret) = self.directory.issue_token(
            context.identity.user.id,
            context.text("label")?,
            expires_at,
        )?;
        Ok(json!({
            "token": token.id.to_string(),
            // 唯一一次。之后任何接口都读不出它（TOK-002、I-A）。
            "secret": secret.into_string(),
            "label": token.label,
            "expiresAt": token.expires_at.map(Timestamp::as_millis),
        }))
    }
}

pub struct RevokeToken {
    spec: ToolSpec,
    directory: Arc<Directory>,
}

impl RevokeToken {
    /// # Errors
    /// 声明不合形状。
    pub fn new(directory: Arc<Directory>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("token.revoke")
                .summary("撤销一个自己的令牌。**立即生效，没有延迟窗口**")
                .input(Schema::new().field(Field::required("token", FieldType::Id, "令牌标识")))
                .requires(Requirement::Platform)
                .idempotency(Idempotency::Keyed)
                .audits(kinds::TOKEN_REVOKED)
                .build()?,
            directory,
        })
    }
}

impl Tool for RevokeToken {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let token = TokenId::from_id(context.id("token")?);
        self.directory
            .revoke_token(context.identity.user.id, token)?;
        Ok(json!({"revoked": token.to_string()}))
    }
}

pub struct ListTokens {
    spec: ToolSpec,
    directory: Arc<Directory>,
}

impl ListTokens {
    /// # Errors
    /// 声明不合形状。
    pub fn new(directory: Arc<Directory>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("token.mine")
                .summary("列出我的令牌。**只有摘要与时间，没有原文**")
                .input(Schema::new())
                .requires(Requirement::Platform)
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::TOKEN_ISSUED)
                .build()?,
            directory,
        })
    }
}

impl Tool for ListTokens {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let tokens = self.directory.tokens_of(context.identity.user.id)?;
        Ok(json!({
            "tokens": tokens
                .iter()
                .map(|token| json!({
                    "token": token.id.to_string(),
                    "label": token.label,
                    "issuedAt": token.issued_at.as_millis(),
                    "expiresAt": token.expires_at.map(Timestamp::as_millis),
                    "revokedAt": token.revoked_at.map(Timestamp::as_millis),
                    // TOK-006：供所有者识别不再使用的令牌。精度是分钟级（见 token 模块）。
                    "lastUsedAt": token.last_used_at.map(Timestamp::as_millis),
                }))
                .collect::<Vec<_>>(),
        }))
    }
}

/// 注册令牌域。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, directory: &Arc<Directory>) -> Result<()> {
    registry.register(Arc::new(IssueToken::new(Arc::clone(directory))?))?;
    registry.register(Arc::new(RevokeToken::new(Arc::clone(directory))?))?;
    registry.register(Arc::new(ListTokens::new(Arc::clone(directory))?))?;
    Ok(())
}
