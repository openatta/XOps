//! 模板的表示形式。
//!
//! ⚠️ **它是可序列化的声明式表示，不是硬编码在 Rust 里的结构体图**。
//! 理由是 **Q15**（用户自定义模板的导出与提交，M6）：首版三个模板随平台发行
//! （`TPL-008`），但表示形式**要为导出留出可能**——硬编码的那种将来导不出来。
//!
//! 有一条 JSON 往返的测试盯着这件事。

use serde::{Deserialize, Serialize};
use xops_core::Result;
use xops_flow::definition::{Criteria, Evaluation, Start, Writers};
use xops_script::capability::{Capabilities, Position};
use xops_script::plugin::Case;
use xops_table::table::Protection;
use xops_table::{Column, ColumnType};

/// 一列。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ColumnType,
    #[serde(default)]
    pub required: bool,
}

impl ColumnSpec {
    /// 变成表引擎认识的那个列。
    ///
    /// # Errors
    /// 列名或类型不合法——**由 RP-04 判，本包不复述它的规则**。
    pub fn to_column(&self) -> Result<Column> {
        Column::new(&self.name, self.ty.clone(), self.required)
    }
}

/// 一张表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableSpec {
    pub name: String,
    #[serde(default = "normal")]
    pub protection: Protection,
    pub columns: Vec<ColumnSpec>,
}

const fn normal() -> Protection {
    Protection::Normal
}

/// 一个节点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeSpec {
    pub name: String,
    pub pass: Criteria,
    #[serde(default = "one")]
    pub quorum: u32,
    #[serde(default)]
    pub reject: Option<Criteria>,
    pub writers: Writers,
    #[serde(default)]
    pub separation_of_duties: bool,
    #[serde(default)]
    pub evaluation: Evaluation,
}

const fn one() -> u32 {
    1
}

/// 一步。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum StepSpec {
    Single { node: NodeSpec },
    Parallel { nodes: Vec<NodeSpec> },
}

/// 一条流程。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowSpec {
    pub name: String,
    pub settlement_table: String,
    /// **可以没有**——approvals 就没有（`TPL-003`）。
    #[serde(default)]
    pub subject_table: Option<String>,
    #[serde(default)]
    pub start: Start,
    /// 主体表上哪些列是状态列（`FLW-036`）。**由流程声明，不是表声明的。**
    #[serde(default)]
    pub status_columns: Vec<String>,
    pub steps: Vec<StepSpec>,
}

/// 一个插件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginSpec {
    pub name: String,
    pub position: Position,
    pub entry: String,
    pub source: String,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub cases: Vec<Case>,
}

/// 一个模板。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub summary: String,
    pub tables: Vec<TableSpec>,
    #[serde(default)]
    pub flow: Option<FlowSpec>,
    #[serde(default)]
    pub plugins: Vec<PluginSpec>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 三个模板都能原样往返一次json() {
        // Q15 的那条路留在这里：**表示形式导得出去**。
        for template in crate::catalog::ALL() {
            let text = serde_json::to_string(&template).unwrap();
            let back: Template = serde_json::from_str(&text).unwrap();
            assert_eq!(back, template, "{} 往返之后不一样了", template.name);
        }
    }
}
