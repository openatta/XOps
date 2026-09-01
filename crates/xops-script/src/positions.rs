//! 两个调用位置（`PLG-001`）。**不存在第三个。**
//!
//! ```text
//! 流转插件  流程节点求值时      能力为零 · 输入由平台预取 · 输出交回平台代写
//! 输出插件  任务 onComplete 时  三样按声明 · **写不了任何表**（I-R）
//! ```
//!
//! ⚠️ **本包负责把插件的输出交出来，不负责写。** 代写发生在 RP-01 的区间 ④，
//! 写回目标由 RP-15 声明。这里只做一件事：**把不该代写的交回请求挡在交出去之前。**

use std::sync::Arc;

use serde_json::Value;
use xops_core::{Error, Result};
use xops_table::{TableId, WrittenBy};

use crate::capability::Position;
use crate::carrier::{Grant, Host, Outcome, invoke};
use crate::plugin::Plugin;

/// 流转插件求值的输入（`PLG-002`）。**三样都由平台在调用前查好。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionInput {
    /// 实例状态。
    pub instance: Value,
    /// 刚写入的那一行。
    pub row: Value,
    /// 节点声明的相关行（`FLW-003`）。
    pub related: Value,
}

impl TransitionInput {
    fn to_json(&self) -> Value {
        serde_json::json!({
            "instance": self.instance,
            "row": self.row,
            "related": self.related,
        })
    }
}

/// 这个节点过 / 不过 / 拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Reject,
}

/// 交回来要平台代写的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Writeback {
    pub table: TableId,
    /// 给了行标识就是 update，没给就是 insert。
    pub row: Option<String>,
    pub values: Value,
}

impl Writeback {
    const fn is_insert(&self) -> bool {
        self.row.is_none()
    }
}

/// 一次流转求值的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settled {
    pub verdict: Verdict,
    pub writes: Vec<Writeback>,
    /// 出了什么事。**过不了的时候这里说得出为什么。**
    pub note: String,
}

impl Settled {
    /// 未通过。**超时与异常都走这里——绝不视为通过**（`PLG-013`）。
    fn not_passed(note: String) -> Self {
        Self {
            verdict: Verdict::Fail,
            writes: Vec::new(),
            note,
        }
    }
}

/// 求值一个流转插件。
///
/// `settlement` 是这个流程的结算表，`subject` 是主体表（可能没有）。
/// **平台只肯代写这两张**（`CON-003`），且**对主体表只能 update**——
/// 否则等于让插件开出新实例（`I-R`）。
///
/// # Errors
/// 载体建不起来 · 插件不是流转插件 · 它交回了平台不肯代写的东西。
///
/// ⚠️ **超时与异常不是 `Err`**：它们是"这个节点没过"，是一个正常结论。
pub fn evaluate_transition(
    plugin: &Plugin,
    input: &TransitionInput,
    settlement: &TableId,
    subject: Option<&TableId>,
) -> Result<Settled> {
    if plugin.position != Position::Transition {
        return Err(Error::invalid("这不是一个流转插件"));
    }
    // 能力为零，没有可声明项 —— 所以这里给的永远是空的那一份。
    let outcome = invoke(
        &plugin.source,
        &plugin.entry,
        &input.to_json(),
        Position::Transition,
        &Grant::none(),
    )?;
    let value = match &outcome {
        Outcome::Returned(value) => value,
        Outcome::TimedOut | Outcome::Threw(_) => return Ok(Settled::not_passed(outcome.note())),
    };

    let verdict = match value.get("verdict").and_then(Value::as_str) {
        Some("pass") => Verdict::Pass,
        Some("reject") => Verdict::Reject,
        Some("fail") => Verdict::Fail,
        other => {
            return Ok(Settled::not_passed(format!(
                "插件没给出过 / 不过 / 拒绝，给的是 {other:?}"
            )));
        }
    };
    let writes = parse_writes(value)?;
    for write in &writes {
        check_writeback(write, settlement, subject)?;
    }
    Ok(Settled {
        verdict,
        writes,
        note: String::new(),
    })
}

fn parse_writes(value: &Value) -> Result<Vec<Writeback>> {
    let Some(items) = value.get("writes").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    items
        .iter()
        .map(|item| {
            let name = item
                .get("table")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::invalid("交回的行没说是哪张表"))?;
            let table = if name.starts_with('_') {
                TableId::system(name)?
            } else {
                TableId::user(name)?
            };
            Ok(Writeback {
                table,
                row: item.get("row").and_then(Value::as_str).map(str::to_owned),
                values: item.get("values").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

/// 平台肯不肯代写这一行。
fn check_writeback(
    write: &Writeback,
    settlement: &TableId,
    subject: Option<&TableId>,
) -> Result<()> {
    if &write.table == settlement {
        return Ok(());
    }
    if Some(&write.table) == subject {
        if write.is_insert() {
            return Err(Error::invalid(
                "对主体表只能 update，不能 insert——那等于让插件自己开出新实例（I-R）",
            ));
        }
        return Ok(());
    }
    Err(Error::invalid(format!(
        "平台只代写结算表与主体表两张，{} 不在其中（CON-003）",
        write.table
    )))
}

/// 跑一个输出插件（`PLG-003`）。
///
/// 输入是本次执行的 `_runs` 行与它写的那些行；**输出由插件自己决定**——
/// 发邮件、发 IM、推到别的系统，都是插件的事（`PLG-004`）。
///
/// ⚠️ **返回值里没有任何写表的路径**：它拿回来的是 [`Outcome`]，不是一组交回的行。
/// "输出插件写不了任何表"（`I-R`）在这里是**类型上的**，不是检查出来的。
///
/// # Errors
/// 载体建不起来 · 插件不是输出插件。
pub fn run_output(
    plugin: &Plugin,
    run_row: &Value,
    written_rows: &Value,
    host: Option<Arc<dyn Host>>,
) -> Result<Outcome> {
    if plugin.position != Position::Output {
        return Err(Error::invalid("这不是一个输出插件"));
    }
    let input = serde_json::json!({"run": run_row, "rows": written_rows});
    invoke(
        &plugin.source,
        &plugin.entry,
        &input,
        Position::Output,
        &Grant {
            capabilities: plugin.capabilities.clone(),
            host,
        },
    )
}

/// 这一行要不要再触发一次插件求值。
///
/// > **流转插件交回、由平台代写的行不再触发插件求值**（`PLG-013`、`I-R`）——
/// > 自激回路从这里断掉。
#[must_use]
pub const fn triggers_evaluation(written_by: &WrittenBy) -> bool {
    !matches!(written_by, WrittenBy::Plugin { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capabilities;
    use crate::plugin::State;
    use serde_json::json;
    use xops_core::Id;
    use xops_identity::{ProjectId, UserId};

    fn plugin(position: Position, source: &str) -> Plugin {
        Plugin {
            project: ProjectId::generate(),
            name: "gate".into(),
            version: 1,
            position,
            entry: "decide".into(),
            source: source.to_owned(),
            capabilities: Capabilities::none(),
            cases: vec![],
            case_results: vec![],
            state: State::Installed,
            generated_by: None,
            installed_by: None,
            installed_at: None,
        }
    }

    fn input() -> TransitionInput {
        TransitionInput {
            instance: json!({"instance": "x"}),
            row: json!({"vote": "赞成"}),
            related: json!([]),
        }
    }

    fn settle(source: &str) -> Result<Settled> {
        evaluate_transition(
            &plugin(Position::Transition, source),
            &input(),
            &TableId::user("votes").unwrap(),
            Some(&TableId::user("bugs").unwrap()),
        )
    }

    #[test]
    fn 三样输入都喂进去了() {
        let settled = settle(
            "function decide(input) { return { verdict: \
             (input.instance && input.row && input.related) ? 'pass' : 'fail' }; }",
        )
        .unwrap();
        assert_eq!(settled.verdict, Verdict::Pass);
    }

    #[test]
    fn 超时视为未通过绝不视为通过() {
        let settled = settle("function decide() { while (true) {} }").unwrap();
        assert_eq!(settled.verdict, Verdict::Fail, "PLG-013");
        assert!(settled.note.contains("超时"), "而且说得出为什么");
    }

    #[test]
    fn 异常视为未通过并留痕() {
        let settled = settle("function decide() { throw new Error('炸了'); }").unwrap();
        assert_eq!(settled.verdict, Verdict::Fail);
        assert!(settled.note.contains("炸了"));
    }

    #[test]
    fn 乱回一个也不算过() {
        let settled = settle("function decide() { return { verdict: 'yes' }; }").unwrap();
        assert_eq!(settled.verdict, Verdict::Fail);
    }

    #[test]
    fn 结算表可以插也可以改() {
        let settled = settle(
            "function decide() { return { verdict: 'pass', \
             writes: [{ table: 'votes', values: { note: '计过了' } }] }; }",
        )
        .unwrap();
        assert_eq!(settled.writes.len(), 1);
    }

    #[test]
    fn 主体表只能改不能插() {
        let error = settle(
            "function decide() { return { verdict: 'pass', \
             writes: [{ table: 'bugs', values: { state: '已确认' } }] }; }",
        )
        .unwrap_err();
        assert!(error.message().contains("只能 update"), "I-R");
        assert!(
            settle(
                "function decide() { return { verdict: 'pass', \
                 writes: [{ table: 'bugs', row: 'R1', values: { state: '已确认' } }] }; }"
            )
            .is_ok(),
            "改是可以的"
        );
    }

    #[test]
    fn 第三张表平台不代写() {
        let error = settle(
            "function decide() { return { verdict: 'pass', \
             writes: [{ table: 'salaries', row: 'R1', values: { pay: 1 } }] }; }",
        )
        .unwrap_err();
        assert!(error.message().contains("只代写结算表与主体表"), "CON-003");
    }

    #[test]
    fn 插件位置不能串() {
        assert!(
            evaluate_transition(
                &plugin(Position::Output, "function decide() { return {}; }"),
                &input(),
                &TableId::user("votes").unwrap(),
                None,
            )
            .is_err()
        );
        assert!(
            run_output(
                &plugin(Position::Transition, "function decide() { return {}; }"),
                &json!({}),
                &json!([]),
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn 代写的行不再触发求值() {
        let by_plugin = WrittenBy::Plugin {
            plugin: "gate".into(),
            version: "1".into(),
            installed_by: UserId::generate(),
            instance: Id::generate(),
        };
        assert!(!triggers_evaluation(&by_plugin), "自激回路从这里断掉");
        assert!(triggers_evaluation(&WrittenBy::Person {
            user: UserId::generate()
        }));
        assert!(triggers_evaluation(&WrittenBy::Platform));
    }

    #[test]
    fn 输出插件跑得起来而且拿回来的不是一组要写的行() {
        let outcome = run_output(
            &plugin(
                Position::Output,
                "function decide(input) { return { got: !!input.run }; }",
            ),
            &json!({"run": "R"}),
            &json!([]),
            None,
        )
        .unwrap();
        assert_eq!(outcome.value().unwrap()["got"], json!(true));
    }
}
