//! 登记（`XFG-002`、`XFG-003`）。
//!
//! > **③ 挂在 ② 的仓绑定上，不另开一套对象。**
//!
//! 所以这里没有一张自己的表：登记序列化之后放进 `Binding.xforge` 那个位子
//! （`RPO-014` 早就留好了）。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Result, Role};
use xops_flow::FlowId;

/// XOps 会返回的角色名，**恰好这三个**（`XFG-019`）。
///
/// > **代价**：若 XForge 侧某条 policy 要求 `verifier`，校验将永远失败且无法绕过。
/// > 两条出路：约定 XForge 侧只用这三个名字，或日后在绑定上加一张三五行的映射表——
/// > **不要为此把 XOps 改成可配置角色系统。**
pub const XOPS_ROLES: [&str; 3] = ["owner", "maintainer", "member"];

/// 角色名怎么写出去。
#[must_use]
pub const fn role_name(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Maintainer => "maintainer",
        Role::Member => "member",
    }
}

/// 一条 policy 的登记。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyBinding {
    /// XForge 那边的 policyId。
    pub policy_id: String,
    pub flow: FlowId,
    pub flow_version: u32,
    /// **结果列映射**（`XFG-003`）：结算表的哪一列是 decision、哪一列是 reason。
    /// **没有它，适配层拼不出 `poll_approval` 的返回值。**
    pub decision_column: String,
    pub reason_column: String,
    /// decision 列上哪两个取值分别代表 approve / reject。
    pub approve_value: String,
    pub reject_value: String,
    /// 这条 policy 认的角色名。**自校验按它做**（`XFG-015`）。
    pub roles: Vec<String>,
}

impl PolicyBinding {
    /// 自校验：**让角色配错在"告诉人类他的批准生效了"之前就失败**（`XFG-015`）。
    ///
    /// # Errors
    /// 声明了一个 XOps 永远不会返回的角色名 · 列映射空着。
    pub fn check(&self) -> Result<()> {
        if self.policy_id.trim().is_empty() {
            return Err(Error::invalid("policyId 不能空"));
        }
        if self.decision_column.trim().is_empty() || self.reason_column.trim().is_empty() {
            return Err(Error::invalid(
                "结果列映射要声明齐——没有它，适配层拼不出 poll_approval 的返回值（XFG-003）",
            ));
        }
        if self.approve_value == self.reject_value {
            return Err(Error::invalid("approve 与 reject 不能是同一个取值"));
        }
        if self.roles.is_empty() {
            return Err(Error::invalid("这条 policy 认哪些角色，要写出来"));
        }
        for role in &self.roles {
            if !XOPS_ROLES.contains(&role.as_str()) {
                return Err(Error::invalid(format!(
                    "「{role}」不是 XOps 会返回的角色名。XOps 的角色固定为 {}——\
                     配了别的，校验将永远失败且无法绕过（XFG-019）。\
                     **不要为此把 XOps 改成可配置角色系统**",
                    XOPS_ROLES.join(" / ")
                )));
            }
        }
        Ok(())
    }

    /// 调用方带来的 `roles` 与这条登记对不对得上。
    #[must_use]
    pub fn accepts(&self, roles: &[String]) -> bool {
        roles.is_empty() || roles.iter().any(|role| self.roles.contains(role))
    }
}

/// 一个项目的全部登记。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    /// 这个 provider 在 XForge 侧的 id。**④ 的检查要用它**（`XFG-021`）。
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub policies: Vec<PolicyBinding>,
}

impl Registration {
    /// 找一条 policy。
    ///
    /// # Errors
    /// **找不到就明确失败，绝不静默创建**（`XFG-002`）。
    pub fn policy(&self, policy_id: &str) -> Result<&PolicyBinding> {
        self.policies
            .iter()
            .find(|policy| policy.policy_id == policy_id)
            .ok_or_else(|| {
                Error::not_found(format!(
                    "policyId「{policy_id}」没有登记过。**明确失败，绝不静默创建**（XFG-002）"
                ))
            })
    }

    /// # Errors
    /// 任何一条登记不合法 · provider id 空着。
    pub fn check(&self) -> Result<()> {
        if self.provider_id.trim().is_empty() {
            return Err(Error::invalid(
                "provider id 要写出来——④「某条 Flow 引用了这个 provider」的检查要用它",
            ));
        }
        for policy in &self.policies {
            policy.check()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(roles: &[&str]) -> PolicyBinding {
        PolicyBinding {
            policy_id: "release-approval".into(),
            flow: FlowId::generate(),
            flow_version: 1,
            decision_column: "decision".into(),
            reason_column: "reason".into(),
            approve_value: "批准".into(),
            reject_value: "驳回".into(),
            roles: roles.iter().map(|role| (*role).to_owned()).collect(),
        }
    }

    #[test]
    fn xops只有三个角色名() {
        assert_eq!(XOPS_ROLES.len(), 3, "XFG-019");
        assert_eq!(role_name(Role::Owner), "owner");
        assert_eq!(role_name(Role::Maintainer), "maintainer");
        assert_eq!(role_name(Role::Member), "member");
    }

    #[test]
    fn 配一个xops不会返回的角色名当场失败() {
        let error = policy(&["verifier"]).check().unwrap_err();
        assert!(
            error.message().contains("verifier"),
            "XFG-015：在告诉人类「你的批准生效了」之前就失败"
        );
        assert!(policy(&["maintainer", "owner"]).check().is_ok());
    }

    #[test]
    fn 结果列映射缺了就登记不了() {
        let mut broken = policy(&["owner"]);
        broken.reason_column = String::new();
        assert!(broken.check().is_err(), "XFG-003");
    }

    #[test]
    fn 没登记过的policy明确失败() {
        let registration = Registration {
            provider_id: "xops".into(),
            policies: vec![policy(&["owner"])],
        };
        assert!(registration.policy("release-approval").is_ok());
        let error = registration.policy("nope").unwrap_err();
        assert!(error.message().contains("绝不静默创建"), "XFG-002");
    }
}
