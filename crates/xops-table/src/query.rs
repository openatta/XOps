//! 读一张表的查询面。
//!
//! > **这一层有意不是查询语言**：两个算子、一个游标，仅此而已（`BRD-001` 的口径）。
//! > 再多就开始像 SQL 了，而平台不提供通用查询（`BRD-002`、`NTF-009`）。
//!
//! # 它为什么在这里，而不是在存储契约上
//!
//! 存储契约只有四个方法（`CON-012`），它**表达不出谓词**。把查询缝开在这一层的好处是
//! 后面两次替换都不动调用方：
//!
//! ```text
//! 今天  扫一段连续键 + 在内存里过滤     —— 行为与之前一样，但**分页是对的**
//! 明天  背一条键值二级索引             —— 调用方一行不改
//! 后天  背真表 + 真索引（MySQL）        —— 调用方仍然一行不改
//! ```
//!
//! 开在存储契约上就不是这样了：`G12` 那次"换实现不改一行"的硬验收要重做，
//! 而且每个后续实现都得自己兑现一套索引语义。
//!
//! # 两种读，分开的理由
//!
//! ```text
//! [`Tables::query`]      按**行 ID 序**翻页。带游标，内存有界。
//!                        行 ID 是时间有序的，所以它就是写入序。
//!
//! [`Tables::query_all`]  把**全部命中**取回来。要排序、要计数就得用它。
//!                        它有一个扫描上限，**超了明确失败，绝不截断**。
//! ```
//!
//! ⚠️ 把这两种混成一个"扫前 N 行再过滤"是一个**会给出错误答案**的写法：
//! 行 ID 升序意味着截断留下的是**最老的 N 条**，于是"最新在前"的看板会稳定显示最老的一批。
//! 这个坑踩过，它是这个模块存在的直接原因。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xops_core::RowId;

/// 一次扫描最多看多少行。**超了就明确失败。**
///
/// 它是"扫过的行数"而不是"命中的行数"：保护的是那次全表扫描的代价。
/// 真撞上了，正确的动作是**给那一列加一条索引**，不是把这个数字调大。
pub const MAX_SCAN: usize = 100_000;

/// 一条筛选。**只有等值与非空两种**——再多就开始像查询语言了。
///
/// ⚠️ 它的 serde 形态是看板定义落库的形态（`_boards` 表里存着），**不能改**。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Filter {
    /// 某一列等于某个值。
    Equals { column: String, value: Value },
    /// 某一列有值。
    Present { column: String },
}

impl Filter {
    #[must_use]
    pub fn column(&self) -> &str {
        match self {
            Self::Equals { column, .. } | Self::Present { column } => column,
        }
    }

    /// 等值。
    #[must_use]
    pub fn equals(column: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::Equals {
            column: column.into(),
            value: value.into(),
        }
    }

    /// 非空。
    #[must_use]
    pub fn present(column: impl Into<String>) -> Self {
        Self::Present {
            column: column.into(),
        }
    }

    #[must_use]
    pub fn matches(&self, row: &Value) -> bool {
        match self {
            Self::Equals { column, value } => row.get(column) == Some(value),
            Self::Present { column } => row.get(column).is_some_and(|found| !found.is_null()),
        }
    }
}

/// 一组筛选是否全中。空的一组**全中**——"不筛"就是"都要"。
#[must_use]
pub fn matches_all(filters: &[Filter], row: &Value) -> bool {
    filters.iter().all(|filter| filter.matches(row))
}

/// 翻一页。
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub filters: Vec<Filter>,
    /// 这一页最多几行。
    pub limit: usize,
    /// 游标：**从这一行之后**开始。第一页给 `None`。
    pub after: Option<RowId>,
}

impl Query {
    /// 不筛，取前 `limit` 行。
    #[must_use]
    pub const fn first(limit: usize) -> Self {
        Self {
            filters: Vec::new(),
            limit,
            after: None,
        }
    }

    #[must_use]
    pub fn filtered(filters: Vec<Filter>, limit: usize) -> Self {
        Self {
            filters,
            limit,
            after: None,
        }
    }

    /// 接着上一页。
    #[must_use]
    pub fn after(mut self, cursor: Option<RowId>) -> Self {
        self.after = cursor;
        self
    }
}

/// 一页结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub rows: Vec<(RowId, Value)>,
    /// 还有更多时，下一页从**它之后**开始；`None` 表示到底了。
    ///
    /// ⚠️ **它是"可能还有"，不是"一定还有"**：最后一页正好填满时也会给出游标，
    /// 下一页才发现是空的。这比"少给一页"安全。
    pub next: Option<RowId>,
}

impl Page {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 两个算子各挡各的() {
        let row = json!({"status": "open", "note": null});
        assert!(Filter::equals("status", "open").matches(&row));
        assert!(!Filter::equals("status", "closed").matches(&row));
        assert!(Filter::present("status").matches(&row));
        assert!(!Filter::present("note").matches(&row), "null 不算有值");
        assert!(!Filter::present("missing").matches(&row));
    }

    #[test]
    fn 不筛就是都要() {
        assert!(matches_all(&[], &json!({})), "空的一组全中");
    }

    #[test]
    fn 筛选的序列化形态不能变() {
        // `_boards` 里存着的就是这个形状，改了它历史看板读不回来。
        let text = serde_json::to_string(&Filter::equals("status", "open")).unwrap();
        assert_eq!(text, r#"{"op":"equals","column":"status","value":"open"}"#);
        let text = serde_json::to_string(&Filter::present("status")).unwrap();
        assert_eq!(text, r#"{"op":"present","column":"status"}"#);
    }
}
