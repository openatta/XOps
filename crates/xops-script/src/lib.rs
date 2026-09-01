//! 脚本载体与插件。
//!
//! > **能力默认为零，未声明即没有**——不是"调用时被拒绝"，是**那个函数不存在**（`I-Z`）。
//!
//! 载体里一个宿主绑定都不注入：声明出来的那几样由宿主**在调用之前**把数据查好放进输入，
//! 而不是给脚本一个能自己去取的函数。所以"未声明的能力用不了"这件事，
//! 测试里验的是 `typeof fetch === 'undefined'`，不是"调用它报错"。
//!
//! 归属：RP-16。

pub mod capability;
pub mod carrier;
pub mod net;
pub mod pipeline;
pub mod plugin;
pub mod positions;
pub mod service;
pub mod tools;

pub use capability::{Capabilities, Position};
pub use carrier::{Grant, Host, Outcome, compile_check, invoke};
pub use net::{Net, Request, Response};
pub use pipeline::{Generated, generate};
pub use plugin::{Case, CaseResult, Plugin, State};
pub use positions::{
    Settled, TransitionInput, Verdict, Writeback, evaluate_transition, run_output,
};
pub use service::{PLUGINS_TABLE, Plugins};
