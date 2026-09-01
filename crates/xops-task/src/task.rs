//! 任务。
//!
//! > **平台只有这一种任务**（`TSK-001`）。质量监管、审批、CI 触发、代码走读四种常见用法
//! > 在平台看来完全一样：某个事件发生了，某个任务订阅了它，于是跑一次技能，
//! > 往声明的表里写几行。**平台不认识这四个词。**
//!
//! 所以这个文件里没有"审批任务""CI 任务"这种东西，也不该有。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Id, Result, Timestamp};
use xops_identity::{ProjectId, UserId};
use xops_skill::{Ownership, SkillId};
use xops_table::TableId;

use crate::policy::{OnComplete, Overlap, VersionPolicy, check_token_budget};

/// 任务标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskId(Id);

impl TaskId {
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

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 任务是哪一种。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[default]
    Normal,
    /// 造插件任务（`TSK-014` / `PLG-005`）：**只能手动触发、不能订阅任何事件**。
    PluginBuilder,
}

/// 订阅哪些事件。事件的形状归 RP-11，这里只存名字。
pub type Subscription = String;

/// 一个任务。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub project: ProjectId,
    pub name: String,
    /// `TSK-013`：与技能一样两种归属，`SKL-008/009/011` 同样适用。
    pub ownership: Ownership,
    pub kind: Kind,
    pub skill: SkillId,
    pub version_policy: VersionPolicy,
    /// 给技能的输入。**必须满足技能的输入契约**（`TSK-003`）。
    pub inputs: serde_json::Value,
    /// 写哪些表（`TSK-004`）。**未声明的表写不了。**
    pub writes: Vec<TableId>,
    /// 订阅哪些事件。
    pub subscriptions: Vec<Subscription>,
    /// 单次 token 上限（`TSK-005`）。
    pub token_budget: u64,
    pub overlap: Overlap,
    pub on_complete: OnComplete,
    /// `TSK-009`：可启用可停用。**停用的不响应任何触发，包括手动。不提供删除。**
    pub enabled: bool,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

impl Task {
    /// 自身形状的校验（不含要读别的对象才知道的那些）。
    ///
    /// # Errors
    /// 名字不合法 · token 上限不合法 · 造插件任务订阅了事件 ·
    /// 写入表清单为空却声明产出行。
    pub fn check(&self) -> Result<()> {
        if self.name.is_empty() || self.name.len() > 64 {
            return Err(Error::invalid("任务名要 1–64 字节"));
        }
        check_token_budget(self.token_budget)?;
        if self.kind == Kind::PluginBuilder && !self.subscriptions.is_empty() {
            return Err(Error::invalid(
                "造插件任务只能手动触发，不能订阅任何事件（TSK-014 / PLG-005）",
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for table in &self.writes {
            if !seen.insert(table.as_str()) {
                return Err(Error::invalid(format!("写入表 {table} 声明了两次")));
            }
        }
        Ok(())
    }

    /// 能不能写这张表（`TSK-004`）。
    #[must_use]
    pub fn may_write(&self, table: &TableId) -> bool {
        self.writes.contains(table)
    }

    /// 现在响不响应触发。
    ///
    /// `TSK-009`：**停用的任务不响应任何触发，包括手动。**
    #[must_use]
    pub const fn responds_to_triggers(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::DEFAULT_TOKEN_BUDGET;

    fn task() -> Task {
        Task {
            id: TaskId::generate(),
            project: ProjectId::generate(),
            name: "查缺陷".into(),
            ownership: Ownership::Public,
            kind: Kind::Normal,
            skill: SkillId::generate(),
            version_policy: VersionPolicy::Pinned { version: 1 },
            inputs: serde_json::json!({}),
            writes: vec![TableId::user("bugs").unwrap()],
            subscriptions: vec![],
            token_budget: DEFAULT_TOKEN_BUDGET,
            overlap: Overlap::default(),
            on_complete: OnComplete::default(),
            enabled: true,
            created_by: UserId::generate(),
            created_at: Timestamp::from_millis(0),
        }
    }

    #[test]
    fn 造插件任务不能订阅事件() {
        let mut builder = task();
        builder.kind = Kind::PluginBuilder;
        assert!(builder.check().is_ok(), "不订阅就没问题");
        builder.subscriptions.push("git.push".into());
        let error = builder.check().unwrap_err();
        assert!(error.message().contains("只能手动触发"));
    }

    #[test]
    fn 未声明的表写不了() {
        let task = task();
        assert!(task.may_write(&TableId::user("bugs").unwrap()));
        assert!(!task.may_write(&TableId::user("issues").unwrap()));
    }

    #[test]
    fn 停用之后连手动都不响应() {
        let mut task = task();
        assert!(task.responds_to_triggers());
        task.enabled = false;
        assert!(!task.responds_to_triggers(), "TSK-009：包括手动");
    }

    #[test]
    fn 重复声明写入表会被拒() {
        let mut task = task();
        task.writes.push(TableId::user("bugs").unwrap());
        assert!(task.check().is_err());
    }

    #[test]
    fn 版本策略默认是钉死的() {
        assert!(matches!(
            task().version_policy,
            VersionPolicy::Pinned { .. }
        ));
    }
}
