//! 插件这个对象：候选 → 已安装 → 已停用。
//!
//! > **插件不是上传的，是生成的**（`PLG-005`）。造插件任务产出 JS 源码 + 一份能力声明
//! > + 一组测试用例；平台确定性地做三件事（编译 · 跑用例 · 静态检查），三样全过
//! > 才产出一个**候选插件**（还没生效）。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Id, Result, Timestamp};
use xops_identity::{ProjectId, UserId};

use crate::capability::{Capabilities, Position};

/// 插件状态（`TBL-009`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// 三件事全过，但**还没生效**。
    Candidate,
    Installed,
    Disabled,
}

/// 一个测试用例：输入 → 期望输出。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub name: String,
    pub input: serde_json::Value,
    pub expected: serde_json::Value,
}

/// 一次用例跑下来的结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

/// 一个插件的一个版本。
///
/// `PLG-009`：**已安装的版本不可变，能力声明是版本的一部分**。改就是生成一个新候选、
/// 再装一次——**改能力也是**，不存在"给已安装的插件加一项能力"这条路径。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plugin {
    pub project: ProjectId,
    pub name: String,
    pub version: u32,
    pub position: Position,
    /// 入口函数名。
    pub entry: String,
    /// **源码。对全体项目成员必须可读**（`PLG-010`、`I-T`）。
    ///
    /// ⚠️ 理由变了，结论一个字没变：它现在没有完全权限了，**但它的判断仍然能结算流程节点**
    /// ——一个不可审查的、由模型生成的东西替人做决定，没人能对它负责。
    /// **隔离管的是它能碰什么，管不到它说什么。**
    pub source: String,
    pub capabilities: Capabilities,
    pub cases: Vec<Case>,
    pub case_results: Vec<CaseResult>,
    pub state: State,
    /// 哪次执行、哪个技能版本产出的（`PLG-011`）。
    pub generated_by: Option<String>,
    pub installed_by: Option<UserId>,
    pub installed_at: Option<Timestamp>,
}

impl Plugin {
    /// 能不能被引用（流程节点或 onComplete）。
    #[must_use]
    pub const fn usable(&self) -> bool {
        matches!(self.state, State::Installed)
    }

    /// 三件事都过了吗（`PLG-006`）。**过不了就成不了候选，更不能被安装。**
    #[must_use]
    pub fn cases_all_passed(&self) -> bool {
        !self.case_results.is_empty() && self.case_results.iter().all(|result| result.passed)
    }

    /// 安装前的检查。
    ///
    /// # Errors
    /// 不是候选 · 用例没全过 · 能力声明与位置不配。
    pub fn check_installable(&self) -> Result<()> {
        if self.state != State::Candidate {
            return Err(Error::invalid("只有候选插件能被安装"));
        }
        if !self.cases_all_passed() {
            return Err(Error::invalid(
                "用例没有全过，成不了候选——更不能被安装（PLG-006）",
            ));
        }
        self.capabilities.check(self.position)
    }
}

/// 让 `Id` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _IdLink = Id;

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(state: State, passed: bool) -> Plugin {
        Plugin {
            project: ProjectId::generate(),
            name: "gate".into(),
            version: 1,
            position: Position::Transition,
            entry: "decide".into(),
            source: "function decide() { return {pass: true}; }".into(),
            capabilities: Capabilities::none(),
            cases: vec![],
            case_results: vec![CaseResult {
                name: "基本".into(),
                passed,
                detail: String::new(),
            }],
            state,
            generated_by: None,
            installed_by: None,
            installed_at: None,
        }
    }

    #[test]
    fn 用例不过的成不了候选也装不上() {
        let error = plugin(State::Candidate, false)
            .check_installable()
            .unwrap_err();
        assert!(error.message().contains("用例没有全过"));
        assert!(plugin(State::Candidate, true).check_installable().is_ok());
    }

    #[test]
    fn 只有已安装的能被引用() {
        assert!(!plugin(State::Candidate, true).usable(), "候选还没生效");
        assert!(plugin(State::Installed, true).usable());
        assert!(!plugin(State::Disabled, true).usable());
    }

    #[test]
    fn 已安装的再装一次不行() {
        assert!(plugin(State::Installed, true).check_installable().is_err());
    }
}
