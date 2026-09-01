//! 引擎这一侧：跑一次要什么、交回什么。
//!
//! **这是 `Runtime` 与具体引擎之间的那道口子**，不是对外的执行契约——
//! 对外的那条在 [`crate::contract`]，它里面不出现引擎的任何类型。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::failure::FailureKind;
use crate::worksheet::Worksheet;

/// 取消信号。超时与主动取消共用它。
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn requested(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 一次跑完之后引擎交回来的东西。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completed {
    pub output: String,
    /// 原始输出流（`EXE-022`）。
    pub trace: String,
    pub tokens_used: u64,
}

/// 一个执行引擎。
pub trait Engine: Send + Sync + 'static {
    /// 引擎此刻在不在。
    ///
    /// `EXE-030` 靠它：**不在就如实归入引擎错误类，绝不在 XOps 进程里就地跑一遍。**
    fn healthy(&self) -> bool;

    /// 跑一次。**同步**——异步由 [`crate::runtime::Runtime`] 负责（`EXE-021`）。
    ///
    /// 实现方必须盯着 `cancel`：`EXE-019` 说超时强制终止时**不得留下孤儿会话
    /// 继续消耗模型额度**，而那件事只有引擎这一侧做得到。
    ///
    /// # Errors
    /// 这次执行失败了，附上它属于哪一类。
    fn run(
        &self,
        worksheet: &Worksheet,
        cancel: &Cancel,
    ) -> std::result::Result<Completed, (FailureKind, String)>;
}
