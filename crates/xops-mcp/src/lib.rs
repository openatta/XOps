//! MCP：唯一操作面。
//!
//! **本包的全部意义在注册骨架上**（[`registry`]）。注册一个 tool 时必须交出五样：
//! 固定形状的输入 schema · 需要的角色 · 是否幂等 · 幂等键从哪来 · 留痕形状。
//! 交不出的注册不进来。它一旦留了"可以先不声明、以后补"的口子，后面八个包会把这个口子用满。
//!
//! 反过来说：**注册一个 tool 即自动获得全套纪律**（`MCP-012`）——认证、鉴权、schema 校验、
//! 幂等、留痕都在外面做完了，各域不需要自己写这些，也因此没有各自写错的机会。
//!
//! 归属：RP-03。表专属 tool 的**派发机制**归 RP-04，本包只提供它必须落在其上的骨架。

pub mod boundary;
pub mod errors;
pub mod idempotency;
pub mod registry;
pub mod schema;
pub mod server;
pub mod tools;
pub mod transport;

pub use boundary::{Exception, NON_MCP_ENTRYPOINTS};
pub use errors::ErrorContract;
pub use registry::{
    CallContext, Idempotency, Registry, Requirement, Tool, ToolName, ToolSpec, allows,
};
pub use schema::{Field, FieldType, Schema};
pub use server::{McpServer, PROTOCOL_VERSION};
pub use tools::{Capabilities, MyPendingNodes, NoPendingNodes, PendingNodes, WhoAmI};
