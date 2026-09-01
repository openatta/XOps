//! 执行运行时与引擎集成。
//!
//! **本包不知道什么是"技能"、什么是"任务"**——它只接收一份派工单并交回一份结果。
//! 这也是它能第一天独立开工的原因（`EXE-013` 之后表也不在数据源里了）。
//!
//! ⚠️ **一条要在读代码之前知道的事**：本实现是**裸跑**，不是 D51 设计的一次性容器。
//! 这是一个明写的决定，它的代价被写成了可枚举的数据——
//! [`provider::IsolationLevel::unsatisfied`] 逐条列出没兑现的需求，并且有测试盯着那张表。
//! 接容器后端进来的那天，那张表要缩短，而缩短这件事是看得见的。
//!
//! 归属：RP-07。

pub mod attacore;
pub mod contract;
pub mod embedded;
pub mod engine;
pub mod failure;
pub mod provider;
pub mod runtime;
pub mod stub;
pub mod worksheet;

pub use contract::{ExecContract, Outcome, Status};
pub use embedded::EmbeddedEngine;
pub use engine::{Cancel, Completed, Engine};
pub use failure::FailureKind;
pub use provider::{BareBackend, IsolationLevel};
pub use runtime::Runtime;
pub use stub::{Behaviour, StubEngine};
pub use worksheet::{Capabilities, Limits, RunId, Worksheet};
