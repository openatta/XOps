//! 流程定义的校验（`FLW-008`）。**不落库。**
//!
//! 至少抓三件事：
//!
//! ```text
//! ① 结算表 ≠ 主体表
//! ② 无主体表时不得声明随行发起
//! ③ 筛选不得重叠
//! ```
//!
//! ③ 的判定口径（D47）把流程展开成一个**激活集合序列**——串行节点各自是一个集合，
//! 一个并行组整体是一个集合——则**同一集合内两两之间**、以及**相邻两个集合之间**，
//! 筛选都不得重叠。
//!
//! - 同一集合内同时激活：一行落进来会被多个节点同时求值。
//! - 相邻集合之间：会在前一个通过的瞬间被同一行结算。
//!
//! ⚠️ **保守口径：只要不能证明两个筛选互斥，就判为重叠并拒绝。宁可误拒**——
//! 误放的后果是运行时一行同时结算两个节点，而那是事后查不出来的。

use xops_core::{Error, Result};

use crate::definition::{Definition, Node, Start};

/// 一条校验发现的问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: &'static str,
    pub detail: String,
}

/// 校验一条流程定义。**返回全部问题，不是第一个**——
/// 一次改完比来回三次强。
///
/// # Errors
/// 形状本身就不对（名字、节点、票数这类）。
pub fn validate(definition: &Definition) -> Result<Vec<Finding>> {
    definition.check_shape()?;
    let mut findings = Vec::new();

    // ① 结算表 ≠ 主体表。
    if definition.subject_table.as_ref() == Some(&definition.settlement_table) {
        findings.push(Finding {
            rule: "FLW-004",
            detail: format!(
                "结算表与主体表都是 {}。分开之后模型才是统一的：\
                 主体表放「这件事本身」，结算表放「谁对它做了什么表态」",
                definition.settlement_table
            ),
        });
    }

    // ② 无主体表时不得声明随行发起。
    if definition.start == Start::Automatic && definition.subject_table.is_none() {
        findings.push(Finding {
            rule: "FLW-009",
            detail: "声明了随行自动发起，却没有主体表——随行发起只看主体表".into(),
        });
    }

    // ③ 状态列长在主体表上 —— 没有主体表就没有状态列可声明（`FLW-036`）。
    if !definition.status_columns.is_empty() && definition.subject_table.is_none() {
        findings.push(Finding {
            rule: "FLW-036",
            detail: "声明了状态列，却没有主体表——状态列是主体表上的列".into(),
        });
    }

    // ④ 筛选不得重叠。
    let sets = definition.activation_sets();
    for (index, set) in sets.iter().enumerate() {
        // 同一集合内两两。
        for (left, right) in pairs(set) {
            if let Some(finding) = overlap(
                left,
                right,
                "同一个激活集合内同时激活，一行落进来会被多个节点同时求值",
            ) {
                findings.push(finding);
            }
        }
        // 与下一个集合。
        if let Some(next) = sets.get(index + 1) {
            for left in set {
                for right in next {
                    if let Some(finding) = overlap(
                        left,
                        right,
                        "相邻两个集合之间，会在前一个通过的瞬间被同一行结算",
                    ) {
                        findings.push(finding);
                    }
                }
            }
        }
    }

    Ok(findings)
}

fn pairs<'a>(nodes: &[&'a Node]) -> Vec<(&'a Node, &'a Node)> {
    let mut out = Vec::new();
    for (index, left) in nodes.iter().enumerate() {
        for right in nodes.iter().skip(index + 1) {
            out.push((*left, *right));
        }
    }
    out
}

fn overlap(left: &Node, right: &Node, why: &str) -> Option<Finding> {
    if left.pass.provably_disjoint(&right.pass) {
        return None;
    }
    Some(Finding {
        rule: "FLW-008③",
        detail: format!(
            "节点「{}」与「{}」的筛选证不出互斥：{why}。\
             判定取保守口径——**宁可误拒**，误放的后果是运行时一行同时结算两个节点，\
             而那是事后查不出来的",
            left.name, right.name
        ),
    })
}

/// 校验并在有问题时报错。建流程时用它。
///
/// # Errors
/// 有任何一条问题。
pub fn require_valid(definition: &Definition) -> Result<()> {
    let findings = validate(definition)?;
    if findings.is_empty() {
        return Ok(());
    }
    Err(Error::invalid(
        findings
            .iter()
            .map(|finding| format!("[{}] {}", finding.rule, finding.detail))
            .collect::<Vec<_>>()
            .join("；"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{Criteria, Evaluation, Filter, FlowId, Node, State, Step, Writers};
    use serde_json::json;
    use xops_core::{Role, Timestamp};
    use xops_identity::{ProjectId, UserId};
    use xops_table::TableId;

    fn equals(column: &str, value: &str) -> Criteria {
        Criteria {
            filters: vec![Filter::Equals {
                column: column.into(),
                value: json!(value),
            }],
        }
    }

    fn node(name: &str, pass: Criteria) -> Node {
        Node {
            name: name.into(),
            pass,
            quorum: 1,
            reject: None,
            writers: Writers {
                roles: vec![Role::Member],
                ..Writers::default()
            },
            separation_of_duties: false,
            evaluation: Evaluation::default(),
        }
    }

    fn definition(steps: Vec<Step>) -> Definition {
        Definition {
            flow: FlowId::generate(),
            project: ProjectId::generate(),
            version: 1,
            name: "审批".into(),
            settlement_table: TableId::user("approvals").unwrap(),
            subject_table: Some(TableId::user("bugs").unwrap()),
            start: Start::Explicit,
            status_columns: vec![],
            steps,
            state: State::Published,
            created_by: UserId::generate(),
            created_at: Timestamp::from_millis(0),
        }
    }

    #[test]
    fn 结算表等于主体表被拒() {
        let mut broken = definition(vec![Step::Single {
            node: node("批", equals("d", "同意")),
        }]);
        broken.subject_table = Some(broken.settlement_table.clone());
        let findings = validate(&broken).unwrap();
        assert!(findings.iter().any(|finding| finding.rule == "FLW-004"));
    }

    #[test]
    fn 无主体表却随行发起被拒() {
        let mut broken = definition(vec![Step::Single {
            node: node("批", equals("d", "同意")),
        }]);
        broken.subject_table = None;
        broken.start = Start::Automatic;
        let findings = validate(&broken).unwrap();
        assert!(findings.iter().any(|finding| finding.rule == "FLW-009"));
    }

    #[test]
    fn 相邻两步筛选相同被拒() {
        let flow = definition(vec![
            Step::Single {
                node: node("初审", equals("d", "同意")),
            },
            Step::Single {
                node: node("复审", equals("d", "同意")),
            },
        ]);
        let findings = validate(&flow).unwrap();
        assert!(
            findings.iter().any(|finding| finding.rule == "FLW-008③"),
            "会在前一个通过的瞬间被同一行结算"
        );
    }

    #[test]
    fn 相邻两步同列不同值就过() {
        let flow = definition(vec![
            Step::Single {
                node: node("初审", equals("stage", "初")),
            },
            Step::Single {
                node: node("复审", equals("stage", "复")),
            },
        ]);
        assert!(validate(&flow).unwrap().is_empty());
    }

    #[test]
    fn 看起来不重叠但证不出互斥的一律误拒() {
        // 不同列：人看着"一个看 stage 一个看 role"像是不会撞，但证不出来。
        let flow = definition(vec![
            Step::Single {
                node: node("初审", equals("stage", "初")),
            },
            Step::Single {
                node: node("复审", equals("role", "qa")),
            },
        ]);
        let findings = validate(&flow).unwrap();
        assert!(!findings.is_empty(), "宁可误拒");

        // 非空筛选同理。
        let flow = definition(vec![
            Step::Single {
                node: node(
                    "有备注就算",
                    Criteria {
                        filters: vec![Filter::Present {
                            column: "note".into(),
                        }],
                    },
                ),
            },
            Step::Single {
                node: node("复审", equals("stage", "复")),
            },
        ]);
        assert!(!validate(&flow).unwrap().is_empty());
    }

    #[test]
    fn 并行组内两两也判() {
        let flow = definition(vec![Step::Parallel {
            nodes: vec![
                node("甲", equals("d", "同意")),
                node("乙", equals("d", "同意")),
            ],
        }]);
        let findings = validate(&flow).unwrap();
        assert!(findings.iter().any(|finding| finding.rule == "FLW-008③"));

        let ok = definition(vec![Step::Parallel {
            nodes: vec![
                node("甲", equals("who", "甲")),
                node("乙", equals("who", "乙")),
            ],
        }]);
        assert!(validate(&ok).unwrap().is_empty());
    }

    #[test]
    fn 隔了一步的两个集合不判() {
        // 只判同集合内与**相邻**集合之间 —— 隔着一步的两个不会同时活着。
        let flow = definition(vec![
            Step::Single {
                node: node("一", equals("stage", "一")),
            },
            Step::Single {
                node: node("二", equals("stage", "二")),
            },
            Step::Single {
                node: node("三", equals("stage", "一")),
            },
        ]);
        assert!(validate(&flow).unwrap().is_empty(), "一与三不相邻");
    }

    #[test]
    fn 一次报全部问题不是第一个() {
        let mut broken = definition(vec![
            Step::Single {
                node: node("初审", equals("d", "同意")),
            },
            Step::Single {
                node: node("复审", equals("d", "同意")),
            },
        ]);
        broken.subject_table = Some(broken.settlement_table.clone());
        assert!(
            validate(&broken).unwrap().len() >= 2,
            "一次改完比来回三次强"
        );
    }

    #[test]
    fn 形状不对的直接报错() {
        let mut broken = definition(vec![]);
        broken.steps.clear();
        assert!(validate(&broken).is_err());
    }

    #[test]
    fn 没有主体表就没有状态列可声明() {
        let mut definition = definition(vec![Step::Single {
            node: node("批", equals("d", "同意")),
        }]);
        definition.status_columns = vec!["status".into()];
        definition.subject_table = None;
        let findings = validate(&definition).unwrap();
        assert!(
            findings.iter().any(|finding| finding.rule == "FLW-036"),
            "状态列是主体表上的列"
        );
        definition.subject_table = Some(TableId::user("bugs").unwrap());
        assert!(
            !validate(&definition)
                .unwrap()
                .iter()
                .any(|finding| finding.rule == "FLW-036")
        );
    }
}
