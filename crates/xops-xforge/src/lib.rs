//! XForge 审批 provider 适配。
//!
//! **这个包的形状不由我们定**（`XFG-010`）：
//!
//! > 两个 tool 的名字、参数与返回值形状**由 XForge 定死，不可改**。返回值是
//! > **一个 `text` 类型的 content item，其 `text` 是一段 JSON 字符串，
//! > 不使用 `structuredContent`**。我们只有实现义务。
//!
//! 三条必须记住的边界：
//!
//! ```text
//! Gate 拿不到凭据      Gate 子进程会过滤掉一切凭据形状的环境变量，所以**绝不能**
//!                      把归档前的校验设计成「Gate 去查 XOps」——它没有能力认证。
//!                      **本域不提供任何供 Gate 调用的查询接口**（XFG-018 / G6）
//!
//! 角色以 XOps 为准     XOps 返回自己的三个角色名，XForge 侧去对齐。
//!                      **不要为此把 XOps 改成可配置角色系统**（XFG-019）
//!
//! 不可用时可重试       **任何「连不上就跳过」的降级逻辑都会让变更被静默放行**（XFG-020）
//!                      ——所以本包一处降级都没有：查不到就明确失败
//! ```
//!
//! 归属：RP-19。

pub mod approver;
pub mod registration;
pub mod scaffold;
pub mod service;
pub mod spec;
pub mod tools;

pub use approver::{Approver, resolve};
pub use registration::{PolicyBinding, Registration, XOPS_ROLES};
pub use service::XForge;
pub use spec::{PollReply, Revision, SubmitArgs, SubmitReply};
