//! 表：一切数据的形状。
//!
//! **平台一张业务表都不预置**（`TBL-002`）。用户想记什么就建什么，平台只提供
//! 一组穷举的列类型、一套自动补的来源字段，和"对外 CRUD、对内追加"这条底层规则。
//!
//! 归属：RP-04。**写串行与四步区间归 RP-01，本包只是用它。**

pub mod column;
pub mod engine;
pub mod query;
pub mod system;
pub mod table;
pub mod tools;
pub mod writtenby;

pub use column::{AUTO_COLUMNS, COLUMN_KINDS, Column, ColumnType};
pub use engine::{CATALOG_TABLE, Catalog, DropGuard, NoFlows, RowVersion, Tables};
pub use query::{Filter, MAX_SCAN, Page, Query, matches_all};
pub use table::{Kind, Protection, TableId, TableSchema, physical_name};
pub use writtenby::WrittenBy;
