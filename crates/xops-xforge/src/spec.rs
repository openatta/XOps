//! 两个 tool 的参数与返回值形状。
//!
//! ⚠️ **这一整个文件是照抄，不是设计**（`XFG-007`～`XFG-010`）。
//! 字段名、大小写、可选性都按 XForge 的规格来——**任何"优化"都是破坏性变更**。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use xops_core::{Error, Result};

/// 被审对象的修订信息。**原样保存各字段**（`XFG-012`）：
/// 人做决定时要看清"我批的是哪一版"。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    #[serde(default)]
    pub state_revision: String,
    #[serde(default)]
    pub content_revision: String,
    #[serde(default)]
    pub policy_snapshot_digest: String,
    #[serde(default)]
    pub git_base: String,
    /// **同时作为事件载荷里的主体修订**喂给任务（`TRG-017`、`FLW-026⑦`）。
    #[serde(default)]
    pub git_head: String,
}

/// `submit_approval_request` 的参数。**与 XForge 规格完全一致**（`XFG-007`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitArgs {
    pub change: String,
    pub flow: String,
    pub stage: String,
    pub transition: String,
    pub policy_id: String,
    #[serde(default)]
    pub revision: Revision,
    pub governing_digest: String,
    #[serde(default)]
    pub roles: Vec<String>,
    /// **不可信自由文本**（`XFG-016`、`G7`）：原样保存与展示，**不解析**。
    #[serde(default)]
    pub reason: String,
}

impl SubmitArgs {
    /// 从 tool 参数里读出来。
    ///
    /// # Errors
    /// 形状不对，或者必填的空着。
    pub fn from_value(value: &Value) -> Result<Self> {
        let args: Self = serde_json::from_value(value.clone())
            .map_err(|error| Error::invalid(format!("参数形状不对：{error}")))?;
        if args.governing_digest.trim().is_empty() {
            return Err(Error::invalid("governingDigest 不能空"));
        }
        if args.policy_id.trim().is_empty() {
            return Err(Error::invalid("policyId 不能空"));
        }
        Ok(args)
    }
}

/// `submit_approval_request` 的回话。**立即返回**（`XFG-011`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitReply {
    pub governing_digest: String,
    /// 这次是新开的，还是撞上了同一个 digest 的老实例。
    pub created: bool,
    pub instance: String,
}

impl SubmitReply {
    /// 拼成回话。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "status": "submitted",
            "governingDigest": self.governing_digest,
            "requestId": self.instance,
            "created": self.created,
        })
    }
}

/// 谁批的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApproverOut {
    pub id: String,
    /// **XOps 自己的项目角色名**（`XFG-019`）。
    pub role: String,
}

/// `poll_approval` 的回话，三种（`XFG-013`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollReply {
    /// **从未提交过 → 明确的未知状态，不是报错。**
    ///
    /// XForge 会整轮重试（连接 + 提交 + 轮询），**必须对重试安全**——
    /// 报错会让调用方以为出了别的事。
    Unknown,
    /// 未决。
    Pending,
    /// 已决。
    Decided {
        /// `approve` / `reject`。
        decision: String,
        approver: ApproverOut,
        /// 原样，**不解析**（`XFG-016`）。
        reason: String,
    },
}

impl PollReply {
    /// 拼成回话。
    ///
    /// ⚠️ **首版不返回 `expiresAt`**：`Q12`（由谁设、默认多久）还没定。
    /// 它在规格里是可选的——**这一条要与 XForge 侧确认它接受缺席**。
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            Self::Unknown => json!({"status": "unknown"}),
            Self::Pending => json!({"status": "pending"}),
            Self::Decided {
                decision,
                approver,
                reason,
            } => json!({
                "status": "decided",
                "decision": decision,
                "approver": {"id": approver.id, "role": approver.role},
                "reason": reason,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 参数照抄xforge的字段名() {
        let raw = json!({
            "change": "CH-1",
            "flow": "release",
            "stage": "review",
            "transition": "approve",
            "policyId": "release-approval",
            "revision": {
                "stateRevision": "s1",
                "contentRevision": "c1",
                "policySnapshotDigest": "p1",
                "gitBase": "b1",
                "gitHead": "h1"
            },
            "governingDigest": "d1",
            "roles": ["maintainer"],
            "reason": "看过了"
        });
        let args = SubmitArgs::from_value(&raw).unwrap();
        assert_eq!(args.policy_id, "release-approval");
        assert_eq!(args.revision.git_head, "h1");
        assert_eq!(args.governing_digest, "d1");
    }

    #[test]
    fn 三种回话各是各的() {
        assert_eq!(PollReply::Unknown.to_json()["status"], json!("unknown"));
        assert_eq!(PollReply::Pending.to_json()["status"], json!("pending"));
        let decided = PollReply::Decided {
            decision: "approve".into(),
            approver: ApproverOut {
                id: "U1".into(),
                role: "maintainer".into(),
            },
            reason: "看过了".into(),
        }
        .to_json();
        assert_eq!(decided["status"], json!("decided"));
        assert_eq!(decided["approver"]["role"], json!("maintainer"));
        // **首版不返回 expiresAt**（Q12 未定）。
        assert!(decided.get("expiresAt").is_none());
    }

    #[test]
    fn 从未提交过不是报错() {
        // XForge 会整轮重试 —— 报错会让调用方以为出了别的事。
        assert_eq!(PollReply::Unknown.to_json()["status"], json!("unknown"));
    }

    #[test]
    fn 缺必填的参数当场拒() {
        assert!(SubmitArgs::from_value(&json!({"governingDigest": ""})).is_err());
    }
}
