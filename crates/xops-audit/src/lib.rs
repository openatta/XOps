//! 追加式审计。
//!
//! **一条要先说清楚的事**：这里没有第二套事件流。`AUD` 与「表对内追加」是同一套流的
//! 两个视角，所以 `AUD-005`（不存在"业务成功但没留痕"或"留了痕但业务没生效"的中间态）
//! 是**结构性成立**的——跨表写没有原子性（`CON-007`），任何"先写业务再写审计"的实现
//! 都做不到这一条。
//!
//! 归属：RP-02。

pub mod envelope;
pub mod log;

pub use envelope::{AuditEnvelope, EventKind, Outcome, catalog, kinds};
pub use log::{AUDIT_TABLE, AuditLog, AuditRecord, Query, data};
