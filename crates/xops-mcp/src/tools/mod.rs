//! 身份、项目、令牌与审计域的 tool。
//!
//! **它们的语义在 RP-02，注册在 RP-03**——`tool 由 RP-03 注册，语义在本包`
//! （RP-02 的接口面原话）。为什么代码落在这个 crate 而不是 `xops-identity`：
//! 依赖方向是 `xops-mcp → xops-identity`，反过来会成环。所以这里只有一层薄壳，
//! **每个 tool 的正文都是一次 `Directory` 调用**，没有第二处业务判断。
//!
//! 表域的 tool 不在这里——那一族由 RP-04 自己注册（它的 crate 在 `xops-mcp` 之上）。

pub mod audit;
pub mod identity;
pub mod project;
pub mod token;

pub use identity::{Capabilities, MyPendingNodes, NoPendingNodes, PendingNodes, WhoAmI};

use std::sync::Arc;

use xops_audit::AuditLog;
use xops_core::Result;
use xops_identity::Directory;

use crate::Registry;

/// 把身份、项目、令牌、审计四个域的 tool 一次注册齐。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(
    registry: &mut Registry,
    directory: &Arc<Directory>,
    audit: &Arc<AuditLog>,
    pending: Arc<dyn PendingNodes>,
) -> Result<()> {
    registry.register(Arc::new(WhoAmI::new()?))?;
    registry.register(Arc::new(Capabilities::new(Arc::clone(directory))?))?;
    registry.register(Arc::new(MyPendingNodes::new(pending)?))?;
    project::register(registry, directory)?;
    token::register(registry, directory)?;
    audit::register(registry, directory, audit)?;
    Ok(())
}
