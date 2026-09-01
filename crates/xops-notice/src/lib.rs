//! 通知。
//!
//! > **通知不是一条投递管道，是 `_notices` 表上属于我的那些行。**
//!
//! 这个设计省掉了三样东西：**投递渠道 · 重试策略 · 聚合窗口**（`NTF-013`）。
//! 要把消息送出 XOps（邮件、IM），用**输出插件**（RP-16）——
//! **平台侧不存在任何一条能承载任意内容、发给任意人的通路**（`I-W`）。
//!
//! 两条结构性保证，**不是概率话术**：
//!
//! ```text
//! 通知的失败绝不影响业务操作   通知行在业务写的串行区间**之外**追加（CON-006）
//!                              写失败只留痕，**绝不回滚业务写**
//! 不会出现「发了通知但账本里    通知**只从事件派生**——本 crate 里造 Notice 的路径
//! 没有对应事实」                只有 [`derive::from_event`] 一条，有测试盯着
//! ```
//!
//! 归属：RP-17。

pub mod derive;
pub mod notice;
pub mod retention;
pub mod service;
pub mod tools;

pub use derive::{SourceEvent, from_event};
pub use notice::{Kind, Notice, NoticeId, Recipients};
pub use retention::Retention;
pub use service::{Failure, NOTICES_TABLE, Notices};
