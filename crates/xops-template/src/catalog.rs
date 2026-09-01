//! 平台自带的三个模板（`TPL-003`、`TPL-008`）。
//!
//! ```text
//! bugs       bugs（主体表：状态列 · 自增序号 · 派生的可读 ID）+ bug-events（结算表）
//!            + 一条**随行自动发起**的流程 + bugs 流转插件
//! issues     issues + issue-events + 一条流程 + issues 插件（bugs 的同构简化）
//! approvals  approvals（结算表，**无主体表**）+ 一条审批骨架 + approvals 插件
//! ```
//!
//! **实例化之后它们就是普通的表、流程和插件。模板是起点，不是约束**（`TPL-004`）。

use serde_json::json;
use xops_core::Role;
use xops_flow::definition::{Criteria, Evaluation, Filter, RowQuery, Start, Writers};
use xops_script::capability::{Capabilities, Position};
use xops_script::plugin::Case;
use xops_table::ColumnType;
use xops_table::table::{Protection, TableId};

use crate::template::{ColumnSpec, FlowSpec, NodeSpec, PluginSpec, StepSpec, TableSpec, Template};

/// 三个模板的名字。**首版就这三个，随平台发行。**
pub const NAMES: [&str; 3] = ["bugs", "issues", "approvals"];

/// 全部模板。
#[must_use]
#[allow(non_snake_case, reason = "它读起来是一个常量，实现上是三次构造")]
pub fn ALL() -> Vec<Template> {
    vec![bugs(), issues(), approvals()]
}

/// 按名字找一个。
#[must_use]
pub fn find(name: &str) -> Option<Template> {
    ALL().into_iter().find(|template| template.name == name)
}

fn text(name: &str, max_len: usize, required: bool) -> ColumnSpec {
    ColumnSpec {
        name: name.to_owned(),
        ty: ColumnType::Text { max_len },
        required,
    }
}

fn enumerated(name: &str, values: &[&str], required: bool) -> ColumnSpec {
    ColumnSpec {
        name: name.to_owned(),
        ty: ColumnType::Enum {
            values: values.iter().map(|value| (*value).to_owned()).collect(),
        },
        required,
    }
}

fn plain(name: &str, ty: ColumnType) -> ColumnSpec {
    ColumnSpec {
        name: name.to_owned(),
        ty,
        required: false,
    }
}

fn equals(column: &str, value: &str) -> Criteria {
    Criteria {
        filters: vec![Filter::Equals {
            column: column.to_owned(),
            value: json!(value),
        }],
    }
}

fn members() -> Writers {
    Writers {
        roles: vec![Role::Member, Role::Maintainer, Role::Owner],
        roster: None,
        task: None,
    }
}

/// 求值用到的那些行。**指定了流转插件，就必须声明它要用到哪些行**（`FLW-003`）——
/// 理由是流转插件读不到表，输入由平台在调用前查好喂进去。
fn recent(table: &str, column: &str) -> Vec<RowQuery> {
    vec![RowQuery {
        table: TableId::user(table).unwrap_or_else(|_| TableId::user("rows").unwrap()),
        criteria: Criteria {
            filters: vec![Filter::Present {
                column: column.to_owned(),
            }],
        },
        limit: 50,
    }]
}

/// 一套"主体 + 表态"的模板，bugs 与 issues 只差名字与状态取值。
fn tracker(
    name: &str,
    subject: &str,
    settlement: &str,
    states: &[&str],
    decisions: &[&str],
) -> Template {
    let closed = states.last().copied().unwrap_or("已关闭");
    let confirmed = states.get(1).copied().unwrap_or(closed);
    let accept = decisions.first().copied().unwrap_or("确认");
    let refuse = decisions.get(1).copied().unwrap_or("关闭");
    Template {
        name: name.to_owned(),
        summary: format!(
            "{subject} 主体表 + {settlement} 结算表 + 一条随行发起的流程 + 一个流转插件"
        ),
        tables: vec![
            TableSpec {
                name: subject.to_owned(),
                protection: Protection::Normal,
                columns: vec![
                    text("title", 200, true),
                    // **状态列。** 流程会声明它受保护——用户的 update 写不了它（`FLW-036`）。
                    enumerated("status", states, true),
                    plain("detail", ColumnType::LongText { max_len: 65_536 }),
                    // 自增序号：**项目内、每表独立**（`TBL-018`）。
                    plain("seq", ColumnType::Sequence),
                    // TPL-005：`<项目短名>-<序号>`。**平台不认识「缺陷 ID」这个概念**——
                    // 它只提供「自增序号」「派生文本」两个列类型和「项目短名」这个属性。
                    //
                    // 用派生列而不是让写入方 agent 自己拼字符串，是因为这个 ID
                    // 会被手写进提交历史——**它必须简单到不可能出错**。
                    plain(
                        "ref",
                        ColumnType::Derived {
                            template: "{project.slug}-{seq}".to_owned(),
                        },
                    ),
                ],
            },
            TableSpec {
                name: settlement.to_owned(),
                protection: Protection::Normal,
                columns: vec![
                    // TPL-007：**一个行引用列指回主体行**，方便人和 agent
                    // 各自把完整时间线拼起来（`BRD-006`）。
                    plain(subject, ColumnType::RowRef),
                    enumerated("decision", decisions, true),
                    text("reason", 500, false),
                ],
            },
        ],
        flow: Some(FlowSpec {
            name: format!("{name} 流转"),
            settlement_table: settlement.to_owned(),
            subject_table: Some(subject.to_owned()),
            // 主体表插入一条新行时**随行自动发起**。
            start: Start::Automatic,
            // **状态列只有平台与流转插件能写，用户的 update 写不了它**（`FLW-036`、`I-P`）。
            // 不声明它，任何成员都能直接把状态改成完成态绕过整条流程。
            status_columns: vec!["status".to_owned()],
            steps: vec![StepSpec::Single {
                node: NodeSpec {
                    name: "分诊".into(),
                    pass: equals("decision", accept),
                    quorum: 1,
                    reject: Some(equals("decision", refuse)),
                    writers: members(),
                    separation_of_duties: false,
                    evaluation: Evaluation::Plugin {
                        plugin: name.to_owned(),
                        inputs: recent(settlement, "decision"),
                    },
                },
            }],
        }),
        plugins: vec![PluginSpec {
            name: name.to_owned(),
            position: Position::Transition,
            entry: "decide".into(),
            // 能力为零 —— **流转插件没有可声明项**（`PLG-002`）。
            capabilities: Capabilities::none(),
            source: tracker_plugin(subject, accept, refuse, confirmed, closed),
            cases: vec![
                Case {
                    name: "有一票确认就过，并把状态写成已确认".into(),
                    input: json!({
                        "instance": {"subjectRow": "R1"},
                        "row": {"decision": accept},
                        "related": [{"decision": accept}],
                    }),
                    expected: json!({
                        "verdict": "pass",
                        "writes": [{"table": subject, "row": "R1", "values": {"status": confirmed}}],
                    }),
                },
                Case {
                    name: "有一票关闭就拒，并把状态写成已关闭".into(),
                    input: json!({
                        "instance": {"subjectRow": "R1"},
                        "row": {"decision": refuse},
                        "related": [{"decision": refuse}],
                    }),
                    expected: json!({
                        "verdict": "reject",
                        "writes": [{"table": subject, "row": "R1", "values": {"status": closed}}],
                    }),
                },
                Case {
                    name: "没人表态就不结算，而且一行都不写回".into(),
                    input: json!({"instance": {"subjectRow": "R1"}, "row": {}, "related": []}),
                    expected: json!({"verdict": "fail", "writes": []}),
                },
            ],
        }],
    }
}

/// 流转插件的源码。
///
/// ⚠️ **它交回的行只有主体表那一张，而且一律带 `row`**——
/// 平台只肯代写结算表与主体表，且**对主体表只能 update**（`CON-003`、`I-R`）。
fn tracker_plugin(
    subject: &str,
    accept: &str,
    refuse: &str,
    confirmed: &str,
    closed: &str,
) -> String {
    format!(
        r#"function decide(input) {{
  var rows = input.related || [];
  var row = input.instance && input.instance.subjectRow;
  function writeStatus(status) {{
    return row ? [{{ table: '{subject}', row: row, values: {{ status: status }} }}] : [];
  }}
  for (var i = 0; i < rows.length; i++) {{
    if (rows[i].decision === '{refuse}') {{
      return {{ verdict: 'reject', writes: writeStatus('{closed}') }};
    }}
  }}
  var yes = 0;
  for (var j = 0; j < rows.length; j++) {{
    if (rows[j].decision === '{accept}') {{ yes++; }}
  }}
  if (yes >= 1) {{ return {{ verdict: 'pass', writes: writeStatus('{confirmed}') }}; }}
  return {{ verdict: 'fail', writes: [] }};
}}"#
    )
}

/// bugs：三套里最全的一套。
#[must_use]
pub fn bugs() -> Template {
    let mut template = tracker(
        "bugs",
        "bugs",
        // ⚠️ `TPL-003` 写的是 `bug_events`。**表名不许有下划线**（`TBL-001` 的名字规则：
        // 小写字母开头，只含小写字母、数字与单个连字符），所以这里是 `bug-events`。
        // 差的是一个字符，但它是"模板要真的建得出来"与"需求原文"之间的一处对齐。
        "bug-events",
        &["新建", "已确认", "处理中", "已关闭"],
        &["确认", "关闭", "重开"],
    );
    // 状态列由**流程**声明（`FLW-036`），不是表声明的。
    template.summary = "缺陷跟踪：bugs 主体表（含 <项目短名>-<序号> 派生 ID）+ bug-events \
                        结算表 + 随行发起的流程 + 流转插件"
        .to_owned();
    template
}

/// issues：bugs 的同构简化。
#[must_use]
pub fn issues() -> Template {
    tracker(
        "issues",
        "issues",
        "issue-events",
        &["待办", "进行中", "已完成"],
        &["接受", "取消"],
    )
}

/// approvals：**无主体表**，而且"理由必填"由它的插件承接（`TPL-006`）。
#[must_use]
pub fn approvals() -> Template {
    Template {
        name: "approvals".into(),
        summary: "审批：approvals 结算表（无主体表）+ 一条审批骨架 + 承接「理由必填」的插件".into(),
        tables: vec![TableSpec {
            name: "approvals".into(),
            protection: Protection::Normal,
            columns: vec![
                text("subject", 200, true),
                enumerated("decision", &["批准", "驳回"], true),
                // **理由必填不是这一列的 required 管的。**
                // 平台的 required 只挡"没有这个键"，挡不住空串与全空白——
                // 而且**平台本身不认识「理由」这个概念**（`TPL-006`）。
                text("reason", 500, false),
            ],
        }],
        flow: Some(FlowSpec {
            name: "审批".into(),
            settlement_table: "approvals".into(),
            // **无主体表** —— 所以它**不得声明随行发起**（`FLW-009`）。
            subject_table: None,
            start: Start::Explicit,
            // 没有主体表就没有状态列可声明。
            status_columns: vec![],
            steps: vec![StepSpec::Single {
                node: NodeSpec {
                    name: "审批".into(),
                    pass: equals("decision", "批准"),
                    quorum: 1,
                    reject: Some(equals("decision", "驳回")),
                    writers: members(),
                    separation_of_duties: true,
                    evaluation: Evaluation::Plugin {
                        plugin: "approvals".into(),
                        inputs: recent("approvals", "decision"),
                    },
                },
            }],
        }),
        plugins: vec![PluginSpec {
            name: "approvals".into(),
            position: Position::Transition,
            entry: "decide".into(),
            capabilities: Capabilities::none(),
            source: r#"function decide(input) {
  var rows = input.related || [];
  function hasReason(row) {
    return typeof row.reason === 'string' && row.reason.trim() !== '';
  }
  var withReason = [];
  for (var i = 0; i < rows.length; i++) {
    if (hasReason(rows[i])) { withReason.push(rows[i]); }
  }
  for (var j = 0; j < withReason.length; j++) {
    if (withReason[j].decision === '驳回') { return { verdict: 'reject', writes: [] }; }
  }
  for (var k = 0; k < withReason.length; k++) {
    if (withReason[k].decision === '批准') { return { verdict: 'pass', writes: [] }; }
  }
  return { verdict: 'fail', writes: [] };
}"#
            .into(),
            cases: vec![
                Case {
                    name: "有理由的批准算数".into(),
                    input: json!({
                        "instance": {},
                        "row": {},
                        "related": [{"decision": "批准", "reason": "看过了，没问题"}],
                    }),
                    expected: json!({"verdict": "pass", "writes": []}),
                },
                Case {
                    name: "空理由的批准不结算".into(),
                    input: json!({
                        "instance": {},
                        "row": {},
                        "related": [{"decision": "批准", "reason": ""}],
                    }),
                    expected: json!({"verdict": "fail", "writes": []}),
                },
                Case {
                    name: "全是空白的理由也不结算".into(),
                    input: json!({
                        "instance": {},
                        "row": {},
                        "related": [{"decision": "批准", "reason": "   "}],
                    }),
                    expected: json!({"verdict": "fail", "writes": []}),
                },
                Case {
                    name: "有理由的驳回让整个实例进拒绝终态".into(),
                    input: json!({
                        "instance": {},
                        "row": {},
                        "related": [{"decision": "驳回", "reason": "预算不够"}],
                    }),
                    expected: json!({"verdict": "reject", "writes": []}),
                },
            ],
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 首版三个() {
        let all = ALL();
        assert_eq!(all.len(), 3, "TPL-008");
        let names: Vec<&str> = all.iter().map(|template| template.name.as_str()).collect();
        assert_eq!(names, NAMES.to_vec());
        assert!(find("bugs").is_some());
        assert!(find("nope").is_none());
    }

    #[test]
    fn bugs带派生的可读id() {
        let bugs = bugs();
        let subject = &bugs.tables[0];
        let derived = subject
            .columns
            .iter()
            .find(|column| column.name == "ref")
            .unwrap();
        assert_eq!(
            derived.ty,
            ColumnType::Derived {
                template: "{project.slug}-{seq}".into()
            },
            "TPL-005"
        );
        assert!(
            subject
                .columns
                .iter()
                .any(|column| column.ty == ColumnType::Sequence),
            "派生 ID 要有一个序号可依"
        );
    }

    #[test]
    fn 结算表带一个指回主体行的行引用列() {
        for template in [bugs(), issues()] {
            let subject = template.tables[0].name.clone();
            let settlement = &template.tables[1];
            let back = settlement
                .columns
                .iter()
                .find(|column| column.name == subject)
                .unwrap();
            assert_eq!(back.ty, ColumnType::RowRef, "TPL-007 / BRD-006");
        }
    }

    #[test]
    fn 主体表的状态列由流程声明为受保护() {
        for template in [bugs(), issues()] {
            let flow = template.flow.clone().unwrap();
            assert_eq!(
                flow.status_columns,
                vec!["status".to_owned()],
                "FLW-036 / I-P"
            );
            // 而且它真的是主体表上的一列。
            assert!(
                template.tables[0]
                    .columns
                    .iter()
                    .any(|column| column.name == "status")
            );
        }
    }

    #[test]
    fn approvals没有主体表也不随行发起() {
        let approvals = approvals();
        let flow = approvals.flow.unwrap();
        assert!(flow.subject_table.is_none());
        assert_eq!(flow.start, Start::Explicit, "FLW-009");
        assert_eq!(approvals.tables.len(), 1, "只有结算表");
    }

    #[test]
    fn 三个模板的插件都是零能力的流转插件() {
        for template in ALL() {
            for plugin in &template.plugins {
                assert_eq!(plugin.position, Position::Transition);
                assert!(
                    plugin.capabilities.is_empty(),
                    "PLG-002：流转插件没有可声明项"
                );
            }
        }
    }
}
