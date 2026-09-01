//! 事件、触发与派工单。
//!
//! **事件白名单恰好五类，且永远不加"某张表被写入"**（`TRG-001` / `TRG-004`）——
//! 一旦任务能订阅表的变化，就有了不受深度限制的回路。
//!
//! 另有一条要在实现上分得很开（`TRG-005`）：**写入触发的是插件求值，不是任务。**
//! 插件求值跑在进程内、毫秒级、不产生执行、不烧模型调用。本包的分发层
//! **没有任何一条从"表被写入"到"触发任务"的路径**，而那是靠 [`event::EventKind`]
//! 里没有那个变体做到的，不是靠判断。
//!
//! 归属：RP-11。

pub mod dispatch;
pub mod event;
pub mod schedule;
pub mod schedule_store;
pub mod tools;
pub mod webhook;
pub mod worksheet;

pub use dispatch::{Dispatcher, Outcome, Slots, TriggerRecord, WorkspaceSource};
pub use event::{Event, EventKind, Trigger};
pub use schedule::{Cadence, Schedule};
pub use schedule_store::Schedules;
pub use webhook::{Filter, GitEvent};
pub use worksheet::{assemble, looks_like_credential, provenance};
