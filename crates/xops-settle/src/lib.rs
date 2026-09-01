//! 结算判定与保护。
//!
//! **七条判定，缺一不可**（`FLW-026`）。每一条挡的是一件具体的事，
//! 那几句话写在 [`verdict::Rule::why`] 上——删掉之后这七条就成了七个看不出所以然的 if。
//!
//! ⚠️ **本包不得自己去改 `_flows` / `_flow_nodes`**：它经 RP-14 的状态机接口驱动迁移。
//! **这条分工是那一刀能成立的全部前提。**
//!
//! 归属：RP-15。

pub mod chain;
pub mod evaluate;
pub mod protection;
pub mod tools;
pub mod verdict;
pub mod writers;

pub use chain::{Chain, NotSettledNotifier, PluginEvaluator, PluginVerdict, TransitionCall};
pub use evaluate::{Evaluator, Written};
pub use protection::{INSTANCE_COLUMN, Origin};
pub use verdict::{Rule, Verdict};
pub use writers::{Responsible, WriterCheck, responsible};
