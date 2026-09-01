//! XOps 的 L0：类型、ID、时间、错误、事件形状、角色。
//!
//! **这一层不知道存储、不知道表、不知道项目。** 它只提供上面每一层都要用、
//! 而且换掉任何实现都不该跟着换的那几样东西。
//!
//! 归属：RP-01。**其余包不往这个 crate 里加东西**（需求包总览 §4：
//! 十六个 crate 各只有一个动刀方）。

pub mod error;
pub mod event;
pub mod id;
pub mod role;
pub mod time;

pub use error::{Error, ErrorKind, Result};
pub use event::{Actor, Event, RowId, TableName, WriteOp};
pub use id::Id;
pub use role::Role;
pub use time::{Clock, FixedClock, SystemClock, Timestamp};
