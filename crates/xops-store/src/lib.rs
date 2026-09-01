//! 存储契约与表级写串行。
//!
//! 两样东西，各自都有一条"事后补等于重写写入路径"的理由（RP-01）：
//!
//! - [`Store`]：一层**只有基本增删改查**的契约。SQLite 是它的第一个实现，
//!   [`MemoryStore`] 是第二个——第二个不是桩，是契约正确性的证据（`CON-012`）。
//! - [`WriteEngine`]：把一次写圈成**四步同一区间**的执行器（`CON-001`～`CON-011`）。
//!
//! **别的 crate 不直接触碰 SQLite。** 这条是 D46 能不能兑现的全部分界线，
//! 由 `tests/no_sqlite_outside_store.rs` 枚举全仓来守。

pub mod keys;
pub mod locks;
pub mod memory;
pub mod relation;
pub mod serial;
pub mod sqlite;
pub mod store;

pub use locks::{Held, TableLocks};
pub use memory::{MemoryRelations, MemoryStore};
pub use relation::{Column, Direction, Relation, Relations, Select, ValueKind};
pub use serial::{
    Deferred, EvalScope, Evaluate, PreWrite, Receipt, Row, RowView, SchemaCheck, WriteEngine,
    WriteRequest, Writeback,
};
pub use sqlite::{SqliteRelations, SqliteStore};
pub use store::{Store, space};
