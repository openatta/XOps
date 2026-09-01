//! 执行策略：版本、重叠、onComplete、终止时序。
//!
//! 这个文件里的每一条都是**默认值本身在防一类故障**，所以默认值要写清理由。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Result};

use crate::task::TaskId;

/// 引用技能的哪个版本（`TSK-002`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum VersionPolicy {
    /// **默认**：钉死一个版本。
    Pinned { version: u32 },
    /// 跟随最新。
    ///
    /// ⚠️ **这必须是明确选择而不是默认**：技能作者一次发布会改变所有引用它的任务的行为。
    Latest,
}

/// 上次执行还没结束又被触发了，怎么办（`TSK-008`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Overlap {
    /// **默认**。理由写在需求里：定时任务最常见的故障是执行变慢后堆积成雪崩。
    #[default]
    Skip,
    /// 排队等待。
    Queue,
    /// 终止上次并重跑。
    Restart,
}

/// 正常完成、行已写入之后，再做一步（`TSK-010`）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum OnComplete {
    #[default]
    None,
    /// 调一个插件入口，把本次执行的 `_runs` 行交给它。
    Plugin { plugin: String },
    /// 触发另一个任务，事件载荷带上本次执行的标识与产出行。
    Task { task: TaskId },
}

impl OnComplete {
    #[must_use]
    pub const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// 它挂的是哪个任务。
    #[must_use]
    pub const fn task(&self) -> Option<TaskId> {
        match self {
            Self::Task { task } => Some(*task),
            _ => None,
        }
    }
}

/// 终止的时序（`TSK-006`）。**超时与被取消走同一条路。**
///
/// 顺序是定死的，每一步都有它非在这个位置不可的理由：
///
/// ```text
/// ① 中止模型调用与会话   先停掉烧钱的那一头
/// ② 收敛并移交已产生的行  它们已经产生了，丢掉等于凭空少一段事实
/// ③ 先落 _runs，再写产出行 因为 FLW-026⑥ 要读 _runs.status 才知道产出行算不算结算
/// ④ 销毁容器
/// ```
///
/// ⚠️ ③ 的顺序**不是跨表事务**（`CON-011`、D43）：两者之间崩溃是可接受的失败形态
/// ——`_runs` 行完整、产出行可能缺失；**反过来（产出行在、执行状态未定）是不可接受的**，
/// 顺序就是为了排除它。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TerminationStep {
    AbortModelAndSession,
    CollectProducedRows,
    WriteRunsThenRows,
    DestroySandbox,
}

impl TerminationStep {
    /// 四步，**按顺序**。
    #[must_use]
    pub const fn order() -> [Self; 4] {
        [
            Self::AbortModelAndSession,
            Self::CollectProducedRows,
            Self::WriteRunsThenRows,
            Self::DestroySandbox,
        ]
    }

    #[must_use]
    pub const fn why(self) -> &'static str {
        match self {
            Self::AbortModelAndSession => "先停掉烧钱的那一头",
            Self::CollectProducedRows => "已经产生的行丢掉，等于凭空少一段事实",
            Self::WriteRunsThenRows => "FLW-026⑥ 要读 _runs.status 才知道产出行算不算结算",
            Self::DestroySandbox => "收摊",
        }
    }
}

/// 平台默认的单次 token 上限（`TSK-005`：未声明就用它）。
pub const DEFAULT_TOKEN_BUDGET: u64 = 200_000;

/// 校验一个 token 上限。
///
/// # Errors
/// 是 0 或者大得离谱。
pub fn check_token_budget(budget: u64) -> Result<()> {
    if budget == 0 {
        return Err(Error::invalid("token 上限不能是 0"));
    }
    if budget > 10_000_000 {
        return Err(Error::invalid(
            "token 上限大得不像话——它是防失控的，不是走过场",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 重叠策略默认跳过() {
        assert_eq!(
            Overlap::default(),
            Overlap::Skip,
            "定时任务最常见的故障是堆积成雪崩"
        );
    }

    #[test]
    fn oncomplete默认为空() {
        assert!(OnComplete::default().is_none());
    }

    #[test]
    fn 终止四步的顺序是定死的() {
        let order = TerminationStep::order();
        assert_eq!(
            order[0],
            TerminationStep::AbortModelAndSession,
            "先停烧钱的那一头"
        );
        assert_eq!(order[2], TerminationStep::WriteRunsThenRows);
        assert_eq!(order[3], TerminationStep::DestroySandbox);
        // 每一步都说得出为什么在这个位置。
        assert!(order.iter().all(|step| !step.why().is_empty()));
        // 顺序即枚举序 —— 排序之后还是它自己。
        let mut sorted = order;
        sorted.sort_unstable();
        assert_eq!(sorted, order);
    }

    #[test]
    fn token上限挑得住() {
        assert!(check_token_budget(0).is_err());
        assert!(check_token_budget(DEFAULT_TOKEN_BUDGET).is_ok());
        assert!(check_token_budget(99_000_000).is_err());
    }
}
