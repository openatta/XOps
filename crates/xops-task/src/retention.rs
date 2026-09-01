//! 保留期与到期清理。
//!
//! `RET-010`：**除到期清理外，系统内不存在任何其它删行路径。**
//! 这是全系统唯一一处硬删除，理由只有一条：存储不是无限的。
//! 它与 `I-D` 不冲突——**不可变说的是"不会被改写"，不是"永久保留"**，且删除这件事本身留痕。

use serde::{Deserialize, Serialize};
use xops_core::Timestamp;

/// 输出保留多久（`RET-001`）。默认 **1 个月**。
pub const DEFAULT_OUTPUT_RETENTION_MILLIS: i64 = 30 * 24 * 60 * 60 * 1_000;
/// 过程记录保留多久。默认 **7 天**。
///
/// 为什么比输出短：**过程用于排查，结论用于回看，价值衰减速度不同。**
pub const DEFAULT_TRACE_RETENTION_MILLIS: i64 = 7 * 24 * 60 * 60 * 1_000;
/// `_notices` 自己的保留期（`RET-008`，平台级配置）。默认 **3 个月**。
pub const DEFAULT_NOTICE_RETENTION_MILLIS: i64 = 90 * 24 * 60 * 60 * 1_000;

/// 一个任务的保留期声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retention {
    pub output_millis: i64,
    pub trace_millis: i64,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            output_millis: DEFAULT_OUTPUT_RETENTION_MILLIS,
            trace_millis: DEFAULT_TRACE_RETENTION_MILLIS,
        }
    }
}

impl Retention {
    /// 这一行什么时候到期。
    ///
    /// ⚠️ `RET-002`：**值取自该任务当时声明的保留期，不靠回查任务再取配置。**
    /// 任务的保留期可以随时改，而已经写下的行不应该因为任务改了配置就提前消失或延后清理。
    #[must_use]
    pub const fn retain_until(&self, now: Timestamp) -> Timestamp {
        Timestamp::from_millis(now.as_millis() + self.output_millis)
    }

    /// 过程记录什么时候到期。**它先于输出过期**（`RET-004`）。
    #[must_use]
    pub const fn trace_retain_until(&self, now: Timestamp) -> Timestamp {
        Timestamp::from_millis(now.as_millis() + self.trace_millis)
    }
}

/// 一行为什么不该被清理（`RET-006`）。
///
/// **豁免优先于任务保留期**——`RET-007` 说明了为什么必须有优先级：
/// 任务完全可以往主体表写行，那批行两条规则都命中。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exemption {
    /// ① 带 `_instance`，或被 `_flows.subject` 引用**过**。
    ///
    /// **一个还在进行中的流程实例，它的主体行或结算行被清理了就等于实例被腰斩**（`I-X`）。
    PartOfFlow,
    /// ② `_flows` / `_flow_nodes` / `_plugins`。
    FlowOrPluginSystemTable,
    /// ③ 项目 / 成员 / 技能 / 任务 / 流程 / 表 的定义。
    Definition,
    /// ④ `_notices`——它另有自己的保留期。
    Notices,
}

impl Exemption {
    #[must_use]
    pub const fn why(self) -> &'static str {
        match self {
            Self::PartOfFlow => "带 _instance 或被 subject 引用过：清了等于把实例腰斩（I-X）",
            Self::FlowOrPluginSystemTable => "_flows / _flow_nodes / _plugins 不参与任务保留期",
            Self::Definition => "定义类对象没有保留期",
            Self::Notices => "_notices 另有自己的保留期（RET-008）",
        }
    }
}

/// 这张表 / 这一行豁不豁免。
///
/// **判定顺序就是 `RET-006` 的编号顺序**，而第一条排在最前是 `RET-007` 要求的：
/// 两条规则都命中时，豁免赢。
#[must_use]
pub fn exemption(table: &str, row: &serde_json::Value) -> Option<Exemption> {
    if row.get("_instance").is_some_and(|value| !value.is_null()) {
        return Some(Exemption::PartOfFlow);
    }
    match table {
        "_flows" | "_flow_nodes" | "_plugins" => Some(Exemption::FlowOrPluginSystemTable),
        "_notices" => Some(Exemption::Notices),
        "_projects" | "_members" | "_users" | "_tokens" | "_skills" | "_skill_versions"
        | "_tasks" | "_tables" | "_repos" | "_boards" | "_flow_defs" => Some(Exemption::Definition),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 过程记录先于输出过期() {
        let retention = Retention::default();
        let now = Timestamp::from_millis(0);
        assert!(
            retention.trace_retain_until(now) < retention.retain_until(now),
            "RET-001：过程用于排查，结论用于回看，价值衰减速度不同"
        );
    }

    #[test]
    fn 带instance的行豁免且豁免优先() {
        // RET-007：任务写进主体表的行两条规则都命中，豁免赢。
        let row = json!({"title": "崩了", "_instance": "某个实例"});
        assert_eq!(exemption("bugs", &row), Some(Exemption::PartOfFlow));
        assert_eq!(
            exemption("bugs", &json!({"title": "崩了"})),
            None,
            "普通行不豁免"
        );
    }

    #[test]
    fn 豁免清单四项都在() {
        assert_eq!(
            exemption("_flows", &json!({})),
            Some(Exemption::FlowOrPluginSystemTable)
        );
        assert_eq!(
            exemption("_plugins", &json!({})),
            Some(Exemption::FlowOrPluginSystemTable)
        );
        assert_eq!(exemption("_notices", &json!({})), Some(Exemption::Notices));
        assert_eq!(exemption("_tasks", &json!({})), Some(Exemption::Definition));
        assert_eq!(
            exemption("_skills", &json!({})),
            Some(Exemption::Definition)
        );
    }

    #[test]
    fn 每一项豁免都说得出理由() {
        for exemption in [
            Exemption::PartOfFlow,
            Exemption::FlowOrPluginSystemTable,
            Exemption::Definition,
            Exemption::Notices,
        ] {
            assert!(!exemption.why().is_empty());
        }
    }

    #[test]
    fn runs不在豁免清单里() {
        // _runs 就是保留期作用的那张表（RET-003）。
        assert_eq!(exemption("_runs", &json!({})), None);
    }
}
