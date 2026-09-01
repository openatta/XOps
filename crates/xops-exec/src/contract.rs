//! 执行契约。**XOps 与执行引擎之间唯一的接缝**（`EXE-014`）。
//!
//! ⚠️ **契约里不出现引擎的任何类型。** 这条有硬验收：换一个桩引擎进去，
//! 上面的一切不改一行（[`crate::stub`]）。
//!
//! `EXE-021`：**执行是异步的**——提交后立即返回执行标识，不阻塞等待完成。

use serde::{Deserialize, Serialize};
use xops_core::{Result, Timestamp};

use crate::failure::FailureKind;
use crate::worksheet::{RunId, Worksheet};

/// 一次执行现在处于什么状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl Status {
    #[must_use]
    pub const fn finished(self) -> bool {
        !matches!(self, Self::Running)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// 一次执行跑完之后交回来的东西。
///
/// **本包产生并移交，不负责持久化**——落 `_runs` 与产出行是 RP-12 的事。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub run: RunId,
    pub status: Status,
    /// 没成的话是哪一类（`EXE-020`）。
    pub failure: Option<FailureKind>,
    /// 技能产出的 Markdown 正文。
    pub output: String,
    /// 过程记录，**至少含原始输出流**（`EXE-022`）。落 `_runs.trace`。
    pub trace: String,
    pub tokens_used: u64,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
}

impl Outcome {
    /// 一次没跑起来的执行。
    #[must_use]
    pub fn failed(
        run: RunId,
        failure: FailureKind,
        trace: impl Into<String>,
        started_at: Timestamp,
        finished_at: Timestamp,
    ) -> Self {
        Self {
            run,
            status: Status::Failed,
            failure: Some(failure),
            output: String::new(),
            trace: trace.into(),
            tokens_used: 0,
            started_at,
            finished_at: Some(finished_at),
        }
    }
}

/// 执行契约。**RP-11 只看得见这四个方法。**
pub trait ExecContract: Send + Sync + 'static {
    /// 提交一次执行。**立即返回，不阻塞**（`EXE-021`）。
    ///
    /// # Errors
    /// 派工单不合法。**引擎不可用不在这里报**——那是一次失败的执行，
    /// 不是一次失败的提交（`EXE-030`：如实归入引擎错误类，绝不就地跑）。
    fn submit(&self, worksheet: Worksheet) -> Result<RunId>;

    /// 查状态。
    ///
    /// # Errors
    /// 没有这次执行。
    fn status(&self, run: RunId) -> Result<Status>;

    /// 取消。**已经结束的执行取消是无操作，不是错误。**
    ///
    /// # Errors
    /// 没有这次执行。
    fn cancel(&self, run: RunId) -> Result<()>;

    /// 领取结果。**还没跑完时返回 `None`。**
    ///
    /// # Errors
    /// 没有这次执行。
    fn collect(&self, run: RunId) -> Result<Option<Outcome>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 只有running不算结束() {
        assert!(!Status::Running.finished());
        for status in [Status::Succeeded, Status::Failed, Status::Cancelled] {
            assert!(status.finished(), "{status:?}");
        }
    }

    #[test]
    fn 契约里不出现引擎的任何类型() {
        // 这条靠的是这个模块的 use 列表 —— 它只用 xops_core 与本 crate 自己的类型。
        // 真正的验收在 tests/：换桩引擎进去，上面的一切不改一行。
        // 只看测试模块**之前**的那一段 —— 这几个词在下面这张清单里当然会出现。
        let source = include_str!("contract.rs");
        let production = source.split("#[cfg(test)]").next().unwrap_or_default();
        for engine in [
            "attacore",
            "AttaCore",
            "attacored",
            "session.run_turn",
            "UnixStream",
        ] {
            assert!(
                !production.contains(engine),
                "契约里漏进了引擎的东西：{engine}"
            );
        }
    }
}
