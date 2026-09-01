//! 传输。
//!
//! **MCP 的服务面与 Web 的只读 HTTP 面不是同一个服务面，不共用路由层**（RP-03 / RP-05 的分工）。
//! 所以这里有一个自己的、很小的 HTTP 服务，而不是去借 Web 那一侧的。
//!
//! 两种传输，协议核心是同一个（[`crate::McpServer`]）：
//!
//! - [`http`]：`POST /mcp`，`Authorization: Bearer <令牌>`。XForge 侧的 `McpServer`
//!   资源用的就是这个形态（`transport: http · url · authTokenEnv`）。
//! - [`stdio`]：换行分隔的 JSON-RPC，令牌从环境变量取。本地 MCP 客户端用它。

pub mod http;
pub mod stdio;
