//! 事件白名单。
//!
//! > **恰好五类，仅此五类**（`TRG-001`）：定时 · Git 事件 · 手动触发 ·
//! > 流程节点被激活 · 上游任务完成。
//!
//! ⚠️ **白名单里永远不加"某张表被写入"**（`TRG-004`）。一旦任务能订阅表的变化，
//! 就有了不受深度限制的回路——A 写表触发 B，B 写表触发 A。需要串联就用 onComplete
//! 那一层，或者在一个技能内部做完多步。
//!
//! 这个 enum 没有第六个变体，也不该有。想加一个，先回来读上面那段。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Id, Result, Timestamp};
use xops_identity::{ProjectId, UserId};

/// 五类事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    /// 定时（`TRG-009`）。
    Scheduled,
    /// Git 事件（`TRG-011`：由 webhook 端点产生）。
    Git,
    /// 手动触发（`TRG-016`）。
    Manual,
    /// 流程节点被激活。
    ///
    /// ⚠️ **不是任务能自己声明订阅的**（`TRG-003`）：唯一的订阅途径是
    /// **被某个节点指定为写入者**。
    FlowNodeActivated,
    /// 上游任务完成。
    ///
    /// ⚠️ **同样不能自己订阅**：唯一的订阅途径是**被某个任务挂在 onComplete 上**。
    UpstreamTaskCompleted,
}

impl EventKind {
    /// 五类，全在这儿。
    #[must_use]
    pub const fn all() -> [Self; 5] {
        [
            Self::Scheduled,
            Self::Git,
            Self::Manual,
            Self::FlowNodeActivated,
            Self::UpstreamTaskCompleted,
        ]
    }

    /// 任务能不能自己声明订阅它（`TRG-002` / `TRG-003`）。
    #[must_use]
    pub const fn self_subscribable(self) -> bool {
        matches!(self, Self::Scheduled | Self::Git | Self::Manual)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Git => "git",
            Self::Manual => "manual",
            Self::FlowNodeActivated => "flow-node-activated",
            Self::UpstreamTaskCompleted => "upstream-task-completed",
        }
    }

    /// # Errors
    /// **不在白名单里。** 错误消息会点名"某张表被写入"那一类，因为它是最常被想要的那个。
    pub fn parse(text: &str) -> Result<Self> {
        Self::all()
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "不在事件白名单里：{text}。白名单恰好五类，\
                     且**永远不加「某张表被写入」**——那会造出不受深度限制的回路（TRG-004）"
                ))
            })
    }
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一个事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub kind: EventKind,
    pub project: ProjectId,
    /// 外部事件标识。**按它幂等**（`TRG-013`）。
    pub external_id: Option<String>,
    /// 触发者。定时的记为系统，但**必须能追溯到配置该调度的人**（`TRG-009`）。
    pub triggered_by: Trigger,
    /// 这次事件带来的代码修订。**它覆盖任务定义里写死的那个。**
    pub revision: Option<String>,
    pub at: Timestamp,
    /// 结构化载荷。任务按它过滤（`TRG-002`）。
    pub payload: serde_json::Value,
}

/// 谁触发的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Trigger {
    /// 某个人手动触发。
    Person { user: UserId },
    /// 系统（定时）。**带上配置该调度的人**——`TRG-009` 要求能追溯到他。
    Schedule { configured_by: UserId },
    /// 外部（Git webhook）。
    External { source: String },
    /// 平台（节点激活、上游完成）。
    Platform { origin: Id },
}

impl Trigger {
    /// 追得到的那个人。
    #[must_use]
    pub const fn responsible(&self) -> Option<UserId> {
        match self {
            Self::Person { user }
            | Self::Schedule {
                configured_by: user,
            } => Some(*user),
            Self::External { .. } | Self::Platform { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 恰好五类() {
        assert_eq!(EventKind::all().len(), 5, "TRG-001");
    }

    #[test]
    fn 白名单里没有某张表被写入() {
        let error = EventKind::parse("table-written").unwrap_err();
        assert!(
            error.message().contains("不受深度限制的回路"),
            "TRG-004 —— 这是最常被想要的那一类，错误消息要说清为什么不给"
        );
        assert!(EventKind::parse("row-inserted").is_err());
    }

    #[test]
    fn 后两类不能自己订阅() {
        assert!(EventKind::Scheduled.self_subscribable());
        assert!(EventKind::Git.self_subscribable());
        assert!(EventKind::Manual.self_subscribable());
        assert!(!EventKind::FlowNodeActivated.self_subscribable(), "TRG-003");
        assert!(
            !EventKind::UpstreamTaskCompleted.self_subscribable(),
            "TRG-003"
        );
    }

    #[test]
    fn 定时触发追得到配置它的人() {
        let who = UserId::generate();
        assert_eq!(
            Trigger::Schedule { configured_by: who }.responsible(),
            Some(who),
            "TRG-009"
        );
        assert_eq!(
            Trigger::External {
                source: "github".into()
            }
            .responsible(),
            None
        );
    }

    #[test]
    fn 名字可往返() {
        for kind in EventKind::all() {
            assert_eq!(EventKind::parse(kind.as_str()).unwrap(), kind);
        }
    }
}

/// 订阅声明的校验（`TRG-002` / `TRG-003`）。接在 `xops_task::Tasks` 上。
#[derive(Debug, Clone, Copy, Default)]
pub struct Whitelist;

impl xops_task::SubscriptionCheck for Whitelist {
    fn check(&self, subscription: &str) -> Result<()> {
        let kind = EventKind::parse(subscription)?;
        if !kind.self_subscribable() {
            return Err(Error::invalid(format!(
                "{kind} 不是任务能自己声明订阅的（TRG-003）。\
                 「流程节点被激活」的唯一订阅途径是被某个节点指定为写入者，\
                 「上游任务完成」的唯一订阅途径是被某个任务挂在 onComplete 上"
            )));
        }
        Ok(())
    }
}
