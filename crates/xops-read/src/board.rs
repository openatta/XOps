//! 看板的定义。
//!
//! > **看板 = 一张表的一个视图**：显示哪张表、按什么筛选、按什么排序、显示哪几列（`BRD-001`）。
//!
//! 就这么多。**平台不内建任何报表**（`BRD-002`）——没有燃尽图、没有趋势图、没有跨项目对比。
//! 判断标准很直白（`BRD-003`）：**如果有一天需要在平台代码里写"什么是缺陷密度"，那就越界了。**
//! 所以这里没有聚合、没有指标、没有 join。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Id, Result, Timestamp};
use xops_identity::ProjectId;
use xops_table::TableId;

/// 看板标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BoardId(Id);

impl BoardId {
    #[must_use]
    pub fn generate() -> Self {
        Self(Id::generate())
    }

    #[must_use]
    pub const fn from_id(id: Id) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_id(self) -> Id {
        self.0
    }
}

impl std::fmt::Display for BoardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 一条筛选。**只有等值与非空两种**——再多就开始像查询语言了。
///
/// ⚠️ **它定义在 `xops-table` 的查询面上，这里只是转出去。**
/// 理由是看板的筛选与"从表里取哪些行"是同一件事：定义两份，
/// 将来把它推到索引或 `WHERE` 上的那天就要推两次，而两份总会漂。
/// serde 形态一个字节没变——`_boards` 里存着的历史看板照常读得回来。
pub use xops_table::Filter;

/// 排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Asc,
    Desc,
}

/// 一个看板。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Board {
    pub id: BoardId,
    pub project: ProjectId,
    pub name: String,
    /// 显示哪张表。
    pub table: TableId,
    /// 按什么筛选。
    pub filters: Vec<Filter>,
    /// 按哪一列排序。
    pub sort: Option<String>,
    pub direction: Direction,
    /// 显示哪几列。空表示全部。
    pub columns: Vec<String>,
    pub created_at: Timestamp,
}

/// 定义一个看板要给的那几样。**它就是 `BRD-001` 那句话的逐字形态。**
#[derive(Debug, Clone, PartialEq)]
pub struct BoardSpec {
    pub name: String,
    /// 显示哪张表。
    pub table: TableId,
    /// 按什么筛选。
    pub filters: Vec<Filter>,
    /// 按哪一列排序。
    pub sort: Option<String>,
    pub direction: Direction,
    /// 显示哪几列。空表示全部。
    pub columns: Vec<String>,
}

impl Board {
    /// # Errors
    /// 名字不合法，或者这张表**不允许建自由看板**。
    pub fn new(project: ProjectId, spec: BoardSpec, created_at: Timestamp) -> Result<Self> {
        if spec.name.is_empty() || spec.name.len() > 64 {
            return Err(Error::invalid("看板名要 1–64 字节"));
        }
        check_boardable(&spec.table)?;
        Ok(Self {
            id: BoardId::generate(),
            project,
            name: spec.name,
            table: spec.table,
            filters: spec.filters,
            sort: spec.sort,
            direction: spec.direction,
            columns: spec.columns,
            created_at,
        })
    }
}

/// 这张表能不能建自由看板。
///
/// # Errors
/// `_notices` 不行（`BRD-004`、`NTF-009`）。
///
/// **个人看板是平台内建的固定视图**，不是用户能配的那种看板——它归 RP-17，
/// 而且本包**不得先做一个"简化版通知页"顶上**。
pub fn check_boardable(table: &TableId) -> Result<()> {
    if table.as_str() == xops_table::system::NOTICES {
        return Err(Error::invalid(
            "_notices 不允许建自由看板：个人看板是平台内建的固定视图（BRD-004 / NTF-009）",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table(name: &str) -> TableId {
        if name.starts_with('_') {
            TableId::system(name).unwrap()
        } else {
            TableId::user(name).unwrap()
        }
    }

    #[test]
    fn 通知表建不了自由看板() {
        assert!(check_boardable(&table("_notices")).is_err());
        assert!(check_boardable(&table("_runs")).is_ok(), "别的系统表可以");
        assert!(check_boardable(&table("bugs")).is_ok());
    }

    #[test]
    fn 筛选只有等值与非空() {
        let equals = Filter::Equals {
            column: "state".into(),
            value: json!("新建"),
        };
        assert!(equals.matches(&json!({"state": "新建"})));
        assert!(!equals.matches(&json!({"state": "已修"})));
        assert!(!equals.matches(&json!({})));

        let present = Filter::Present {
            column: "code".into(),
        };
        assert!(present.matches(&json!({"code": "acme-1"})));
        assert!(!present.matches(&json!({"code": null})));
        assert!(!present.matches(&json!({})));
    }

    fn spec(name: &str) -> BoardSpec {
        BoardSpec {
            name: name.into(),
            table: table("bugs"),
            filters: vec![],
            sort: None,
            direction: Direction::Asc,
            columns: vec![],
        }
    }

    #[test]
    fn 看板名挑得住() {
        let project = ProjectId::generate();
        assert!(Board::new(project, spec(""), Timestamp::from_millis(0)).is_err());
        assert!(Board::new(project, spec("全部缺陷"), Timestamp::from_millis(0)).is_ok());
    }
}
