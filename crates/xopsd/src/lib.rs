//! XOps 的装配层。
//!
//! **它没有业务语义。** 19 个包各管各的，这里只把它们接起来——
//! 一处"顺手判一下"都会把语义搬出它该在的包。
//!
//! 两个服务面各监听各的端口、共用同一份状态：
//!
//! ```text
//! MCP 写入面   唯一的写入通道（I-L）
//! 只读 Web 面  结构性地不存在写业务对象的路由（G2）
//! ```

pub mod assemble;
pub mod background;
pub mod banner;
pub mod config;

pub use assemble::{Assembled, assemble};
pub use config::Config;
