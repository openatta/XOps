//! 流程定义。
//!
//! > **流程吃掉审批**：节点结算的唯一形式是"结算表上出现满足条件的行"。
//! > 人写就是审批，任务或插件判定就是自动化。
//!
//! **不存在流程设计器界面**（`FLW-001`）——定义经 MCP 创建。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Id, Result, Role, Timestamp};
use xops_identity::{ProjectId, UserId};
use xops_table::TableId;
use xops_task::TaskId;

/// 流程标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FlowId(Id);

impl FlowId {
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

impl std::fmt::Display for FlowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 一条筛选。**与看板的筛选是同一种窄形状**——等值或非空，仅此。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Filter {
    Equals {
        column: String,
        value: serde_json::Value,
    },
    Present {
        column: String,
    },
}

impl Filter {
    #[must_use]
    pub fn column(&self) -> &str {
        match self {
            Self::Equals { column, .. } | Self::Present { column } => column,
        }
    }

    #[must_use]
    pub fn matches(&self, row: &serde_json::Value) -> bool {
        match self {
            Self::Equals { column, value } => row.get(column) == Some(value),
            Self::Present { column } => row.get(column).is_some_and(|found| !found.is_null()),
        }
    }
}

/// 一组筛选（全部满足才算命中）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Criteria {
    pub filters: Vec<Filter>,
}

impl Criteria {
    #[must_use]
    pub fn matches(&self, row: &serde_json::Value) -> bool {
        !self.filters.is_empty() && self.filters.iter().all(|filter| filter.matches(row))
    }

    /// 能不能**证明**这两组筛选互斥。
    ///
    /// ⚠️ **保守口径**（D47 / `FLW-008` ③）：只要证不出互斥，就当作重叠。
    /// **宁可误拒**——误放的后果是运行时一行同时结算两个节点，而那是事后查不出来的。
    ///
    /// 目前能证明的只有一种：**同一列被约束成两个不同的字面值**。
    /// 别的一律证不出来。
    #[must_use]
    pub fn provably_disjoint(&self, other: &Self) -> bool {
        for mine in &self.filters {
            for theirs in &other.filters {
                if let (
                    Filter::Equals {
                        column: left,
                        value: left_value,
                    },
                    Filter::Equals {
                        column: right,
                        value: right_value,
                    },
                ) = (mine, theirs)
                    && left == right
                    && left_value != right_value
                {
                    return true;
                }
            }
        }
        false
    }
}

/// 谁能写这个节点的结算行（`FLW-018`）。**三者的并集，不新增概念。**
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Writers {
    /// ① 项目角色集合。
    pub roles: Vec<Role>,
    /// ② 某张名单表里的人。
    ///
    /// ⚠️ **名单表是受保护表，只有项目所有者能写**（`FLW-019`、`I-Q`）——
    /// **名单表的写权限就是审批权的元权限**：谁能改名单，谁就能给自己发审批权。
    pub roster: Option<TableId>,
    /// ③ 某个**指定的私有任务**（该任务写入的行才算数）。
    ///
    /// ⚠️ **只能是私有任务**（`FLW-021`）：项目公共任务没有"所有者这个人"，
    /// 一旦它写的行被算作节点通过，"每一次通过都归属一个具名的人"当场落空。
    pub task: Option<TaskId>,
}

impl Writers {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty() && self.roster.is_none() && self.task.is_none()
    }
}

/// 求值时要预取的一批行（`FLW-003`）。
///
/// **流转插件读不到表**（`PLG-002`），所以它要用的行必须在流程定义里声明出来，
/// 由平台在求值前查好喂进去。**这不是限制，是把一件本来就该做的事挑明了**——
/// 求值发生在写串行区间内，一次自由查询就是一次不确定的写时延。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowQuery {
    pub table: TableId,
    pub criteria: Criteria,
    pub limit: usize,
}

/// 怎么求值。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Evaluation {
    /// 默认：按筛选判定。
    #[default]
    ByCriteria,
    /// 指定一个流转插件。**必须同时声明它要用到哪些行。**
    Plugin {
        plugin: String,
        inputs: Vec<RowQuery>,
    },
}

/// 一个节点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    /// 通过条件：结算表上出现满足这组筛选的行，且数量 ≥ `quorum`。
    pub pass: Criteria,
    /// 会签票数。
    pub quorum: u32,
    /// 拒绝条件（可选）。
    pub reject: Option<Criteria>,
    pub writers: Writers,
    /// 职责分离：要求写入者 ≠ 实例发起人。
    pub separation_of_duties: bool,
    pub evaluation: Evaluation,
}

/// 一步：一个节点，或者一个并行组（同时激活，全部通过才推进）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Step {
    Single { node: Node },
    Parallel { nodes: Vec<Node> },
}

impl Step {
    /// 这一步同时激活哪些节点。**「激活集合」就是它**（D47 的判定单位）。
    #[must_use]
    pub fn activation_set(&self) -> Vec<&Node> {
        match self {
            Self::Single { node } => vec![node],
            Self::Parallel { nodes } => nodes.iter().collect(),
        }
    }
}

/// 实例怎么发起（`FLW-009`）。**二选一，在定义里声明。**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Start {
    /// 随行自动发起：**主体表**插入一条新行时，主体就是那一行。
    Automatic,
    /// 显式发起：有人经 MCP 调"发起实例"。
    #[default]
    Explicit,
}

/// 流程定义的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Published,
    /// 停用后**不能发起新实例，在途实例继续执行完**（`FLW-006`）。
    Disabled,
}

/// 一条流程定义的一个版本。
///
/// `FLW-007`：**实例始终按发起时的版本走完**，不受后续版本变更影响。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Definition {
    pub flow: FlowId,
    pub project: ProjectId,
    pub version: u32,
    pub name: String,
    /// 结算表：**放"谁对它做了什么表态"**。
    pub settlement_table: TableId,
    /// 主体表：**放"这件事本身"**。可选。
    pub subject_table: Option<TableId>,
    pub start: Start,
    pub steps: Vec<Step>,
    pub state: State,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

impl Definition {
    /// 这条流程的激活集合序列。**筛选重叠的判定单位**（D47）。
    #[must_use]
    pub fn activation_sets(&self) -> Vec<Vec<&Node>> {
        self.steps.iter().map(Step::activation_set).collect()
    }

    /// 第 `index` 步。
    #[must_use]
    pub fn step(&self, index: usize) -> Option<&Step> {
        self.steps.get(index)
    }

    #[must_use]
    pub fn node(&self, step: usize, name: &str) -> Option<&Node> {
        self.step(step)?
            .activation_set()
            .into_iter()
            .find(|node| node.name == name)
    }

    /// # Errors
    /// 名字不合法或一步都没有。
    pub fn check_shape(&self) -> Result<()> {
        if self.name.is_empty() || self.name.len() > 64 {
            return Err(Error::invalid("流程名要 1–64 字节"));
        }
        if self.steps.is_empty() {
            return Err(Error::invalid("一条流程至少要有一个节点"));
        }
        for step in &self.steps {
            for node in step.activation_set() {
                if node.name.is_empty() {
                    return Err(Error::invalid("节点要有名字"));
                }
                if node.quorum == 0 {
                    return Err(Error::invalid(format!("节点 {} 的票数不能是 0", node.name)));
                }
                if node.writers.is_empty() {
                    return Err(Error::invalid(format!(
                        "节点 {} 没有声明允许写入者——那样它永远不会通过",
                        node.name
                    )));
                }
                if node.pass.filters.is_empty() {
                    return Err(Error::invalid(format!("节点 {} 没有通过条件", node.name)));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn equals(column: &str, value: &str) -> Criteria {
        Criteria {
            filters: vec![Filter::Equals {
                column: column.into(),
                value: json!(value),
            }],
        }
    }

    #[test]
    fn 同一列不同字面值才证得出互斥() {
        assert!(equals("decision", "同意").provably_disjoint(&equals("decision", "拒绝")));
        assert!(
            !equals("decision", "同意").provably_disjoint(&equals("decision", "同意")),
            "一样的当然重叠"
        );
        assert!(
            !equals("decision", "同意").provably_disjoint(&equals("stage", "复核")),
            "不同列证不出互斥 —— 保守口径判为重叠"
        );
    }

    #[test]
    fn 非空筛选之间证不出互斥() {
        let present = Criteria {
            filters: vec![Filter::Present {
                column: "note".into(),
            }],
        };
        assert!(
            !present.provably_disjoint(&equals("decision", "同意")),
            "宁可误拒 —— 误放的后果事后查不出来"
        );
    }

    #[test]
    fn 并行组整体是一个激活集合() {
        let node = |name: &str| Node {
            name: name.into(),
            pass: equals("decision", "同意"),
            quorum: 1,
            reject: None,
            writers: Writers {
                roles: vec![Role::Member],
                ..Writers::default()
            },
            separation_of_duties: false,
            evaluation: Evaluation::default(),
        };
        let step = Step::Parallel {
            nodes: vec![node("甲"), node("乙")],
        };
        assert_eq!(step.activation_set().len(), 2);
        assert_eq!(Step::Single { node: node("丙") }.activation_set().len(), 1);
    }

    #[test]
    fn 空筛选不命中任何行() {
        assert!(!Criteria::default().matches(&json!({"decision": "同意"})));
    }
}
