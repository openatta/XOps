//! 生成流水线：**编译 · 跑用例 · 静态检查**，三样全过才产出一个候选（`PLG-006`）。
//!
//! `PLG-017`（D54）：**静态检查的范围由载体回答**——**够不到的东西不需要禁**。
//! 载体不给绑定，脚本就没有那条路，所以不必再讨论"要不要禁某些 import、禁 `process.env`"。
//! 剩下的静态检查只有两件确定性的事：**① 入口导出在不在；② 声明的能力与披露的是否一致。**

use std::sync::Arc;

use serde_json::Value;
use xops_core::{Error, Result};
use xops_identity::ProjectId;

use crate::capability::{Capabilities, Position};
use crate::carrier::{Grant, Host, Outcome, compile_check, invoke};
use crate::plugin::{Case, CaseResult, Plugin, State};

/// 生成出来的东西。
#[derive(Debug, Clone, PartialEq)]
pub struct Generated {
    pub plugin: Plugin,
}

/// 跑一遍流水线。
///
/// `host` 是跑用例时的宿主：**能力按声明给，不多给**——不给宿主就等于三样都够不到，
/// **流转插件走的永远是 `None`**。
///
/// # Errors
/// 编译不过 · 入口不在 · 能力声明与位置不配 · **任何一个用例不过**。
/// 三样里挂一样就产不出候选。
#[allow(clippy::too_many_arguments, reason = "生成一个插件要的东西就是这么多")]
pub fn generate(
    project: ProjectId,
    name: &str,
    version: u32,
    position: Position,
    entry: &str,
    source: &str,
    capabilities: Capabilities,
    cases: Vec<Case>,
    host: Option<Arc<dyn Host>>,
    generated_by: Option<String>,
) -> Result<Generated> {
    // ① 编译 + 入口导出检查。
    compile_check(source, entry)?;
    // ② 能力声明与位置配不配。
    capabilities.check(position)?;

    // ③ 在**真载体**里跑用例，**能力按声明给，不多给**。
    let grant = Grant {
        capabilities: capabilities.clone(),
        host,
    };
    let mut results = Vec::new();
    for case in &cases {
        let outcome = invoke(source, entry, &case.input, position, &grant)?;
        let (passed, detail) = match &outcome {
            Outcome::Returned(value) => {
                if *value == case.expected {
                    (true, String::new())
                } else {
                    (false, format!("期望 {}，得到 {value}", case.expected))
                }
            }
            Outcome::TimedOut => (false, "超时".to_owned()),
            Outcome::Threw(error) => (false, format!("抛异常：{error}")),
        };
        results.push(CaseResult {
            name: case.name.clone(),
            passed,
            detail,
        });
    }
    if results.iter().any(|result| !result.passed) {
        let failed: Vec<&str> = results
            .iter()
            .filter(|result| !result.passed)
            .map(|result| result.name.as_str())
            .collect();
        return Err(Error::invalid(format!(
            "用例没全过，产不出候选（PLG-006）：{failed:?}"
        )));
    }

    Ok(Generated {
        plugin: Plugin {
            project,
            name: name.to_owned(),
            version,
            position,
            entry: entry.to_owned(),
            source: source.to_owned(),
            capabilities,
            cases,
            case_results: results,
            state: State::Candidate,
            generated_by,
            installed_by: None,
            installed_at: None,
        },
    })
}

/// 让 `Value` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _ValueLink = Value;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cases() -> Vec<Case> {
        vec![Case {
            name: "两票就过".into(),
            input: json!({"votes": 2}),
            expected: json!({"pass": true}),
        }]
    }

    fn generate_with(source: &str, cases: Vec<Case>) -> Result<Generated> {
        generate(
            ProjectId::generate(),
            "gate",
            1,
            Position::Transition,
            "decide",
            source,
            Capabilities::none(),
            cases,
            None,
            None,
        )
    }

    #[test]
    fn 三样全过才产出候选() {
        let generated = generate_with(
            "function decide(input) { return { pass: input.votes >= 2 }; }",
            cases(),
        )
        .unwrap();
        assert_eq!(generated.plugin.state, State::Candidate);
        assert!(generated.plugin.cases_all_passed());
    }

    #[test]
    fn 编译不过就产不出() {
        assert!(generate_with("function ( broken", cases()).is_err());
    }

    #[test]
    fn 入口不在就产不出() {
        assert!(generate_with("function other() { return 1; }", cases()).is_err());
    }

    #[test]
    fn 用例不过就产不出() {
        let error =
            generate_with("function decide() { return { pass: false }; }", cases()).unwrap_err();
        assert!(error.message().contains("用例没全过"));
    }

    #[test]
    fn 死循环的插件在跑用例这一步就被挡下() {
        assert!(generate_with("function decide() { while(true){} }", cases()).is_err());
    }

    #[test]
    fn 流转插件声明了能力就产不出() {
        let result = generate(
            ProjectId::generate(),
            "gate",
            1,
            Position::Transition,
            "decide",
            "function decide() { return {pass: true}; }",
            Capabilities {
                own_config: true,
                ..Capabilities::none()
            },
            vec![],
            None,
            None,
        );
        assert!(result.is_err(), "PLG-002：流转插件没有可声明项");
    }
}
