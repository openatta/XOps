//! 任务与执行策略。
//!
//! > **平台只有这一种任务**（`TSK-001`）。质量监管、审批、CI 触发、代码走读四种常见用法
//! > 在平台看来完全一样。**平台不认识这四个词。**
//!
//! 归属：RP-10。**写入路径与清理归 RP-12**，它与本包串行动同一个 crate。

pub mod policy;
pub mod service;
pub mod task;
pub mod tools;

pub use policy::{DEFAULT_TOKEN_BUDGET, OnComplete, Overlap, TerminationStep, VersionPolicy};
pub use service::{TASKS_TABLE, Tasks};
pub use task::{Kind, Task, TaskId};
