//! 只读 Web 后端。
//!
//! **G2「Web 只读」的第 ① 道就在这里，而且是结构性的**：路由是一张可枚举的常量表，
//! 里面一条写路由都没有——前端就算想写也没有地方可发。第 ② 道（前端不存在调用写接口的
//! 代码路径）归 RP-06，**顺序不能反**：只有 ② 没有 ①，等于把一条安全属性交给前端自觉。
//!
//! 归属：RP-05（webhook 端点归 RP-13，虽然它落在同一个 crate 里）。

pub mod assets;
pub mod routes;
pub mod server;
pub mod session;

pub use assets::Assets;
pub use routes::{Kind, ROUTES, Route, match_route};
pub use server::{Request, Response, WebServer, listen};
pub use session::{SESSION_PREFIX, Sessions};
