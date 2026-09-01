//! 模板。
//!
//! ```text
//! 模板 = 一套表 schema + 可选的流程定义 + 可选的插件
//! ```
//!
//! > **实例化之后它们就是普通的表、流程和插件**，用户想怎么改就怎么改。
//! > **模板是起点，不是约束。**
//!
//! **这个包最重要的性质是它什么都不新增**（`TPL-001`～`TPL-008`）：建表走 RP-04、
//! 建流程走 RP-14、装插件走 RP-16——一处都不绕。有一条枚举源码的测试盯着这件事。
//!
//! 两条落在模板身上、平台自己不认识的概念：
//!
//! ```text
//! 缺陷 ID     `<项目短名>-<序号>` 是 bugs 模板声明的一个**派生文本列**
//!             平台只提供「自增序号」「派生文本」两个列类型和「项目短名」这个属性
//!             **它不认识「缺陷 ID」这个概念**（TPL-005）
//!
//! 理由必填     由 approvals 模板的**插件**承接：reason 为空就不结算
//!             **平台本身不认识「理由」这个概念**（TPL-006）
//! ```
//!
//! 归属：RP-18。

pub mod catalog;
pub mod service;
pub mod template;
pub mod tools;

pub use catalog::{ALL, approvals, bugs, find, issues};
pub use service::{Instantiated, Templates};
pub use template::{ColumnSpec, FlowSpec, NodeSpec, PluginSpec, StepSpec, TableSpec, Template};
