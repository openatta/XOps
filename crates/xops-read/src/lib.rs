//! 读模型：**Web 唯一的查询契约**。
//!
//! 前端不直连库、不拼 SQL、不调 MCP——它只认这里的视图。这份接口的完备性是
//! RP-06 能不能并行开工的全部前提。
//!
//! 归属：RP-05。

pub mod board;
pub mod model;
pub mod tools;

pub use board::{Board, BoardId, BoardSpec, Direction, Filter};
pub use model::{
    BOARDS_TABLE, BoardSummary, BoardView, ColumnSummary, IdentityView, LongTextView, MemberView,
    NoticeView, ProjectView, ReadModel, RowHistoryView, RowView, SettlementView, TableSummary,
    VersionView,
};
