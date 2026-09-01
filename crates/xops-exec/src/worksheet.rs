//! 派工单：**执行契约的输入**。
//!
//! **本包不知道什么是"技能"、什么是"任务"**（那是 RP-09 / RP-10），也不装配派工单
//! （那是 RP-11）。它只接收这一份东西并校验它。这也是本包能第一天独立开工的原因。
//!
//! ⚠️ **表数据不在这里**（`EXE-013` / D44）：技能读不到表。需要表数据的，
//! 由调用方经 MCP 查好、作为 `inputs` 传进来。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use xops_core::{Error, Id, Result};

/// 一次执行的标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunId(Id);

impl RunId {
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

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 这次执行能碰到什么（`EXE-006`：**未声明的一律不提供**）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// 只读工作区。由 RP-08 备好，本包只负责把它交给执行方。
    pub workspace: Option<PathBuf>,
    /// 允许出网的主机白名单。**空表示不许出网**（`EXE-007`）。
    pub network: Vec<String>,
}

/// 资源上限（`EXE-008`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// 墙钟上限。到点强制终止（`EXE-019`：**不得留下孤儿会话继续消耗模型额度**）。
    pub timeout_millis: u64,
    /// 这次执行的 token 预算（`TSK-005`）。
    pub token_budget: u64,
    /// 内存上限，字节。
    pub memory_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout_millis: 15 * 60 * 1_000,
            token_budget: 200_000,
            memory_bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

/// 一份派工单。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Worksheet {
    pub run: RunId,
    /// 技能的**内容**与版本。本包不解释它，只把它交给引擎。
    pub instruction: String,
    pub skill: String,
    pub skill_version: String,
    /// 调用方查好、传进来的输入（`EXE-013` 那条出路）。
    pub inputs: String,
    /// 读的哪个代码修订。
    pub revision: Option<String>,
    pub capabilities: Capabilities,
    pub limits: Limits,
}

impl Worksheet {
    /// 派工单 → 喂给引擎的那一段。
    ///
    /// ⚠️ **不含任何凭据、不含 socket 路径、不含到 XOps 的网络路径**
    /// （`EXE-010`、`EXE-004`、`I-F`）。派工单本身就不带这些,这里只是把已有的几样拼起来——
    /// **不要在这里加任何「顺手带上」的东西**。
    ///
    /// 它在派工单上而不在某个引擎里:**两个引擎都要这个映射**,
    /// 各写一份的话它们迟早会不一样,而那种不一样表现成"换个引擎产出就变了"。
    #[must_use]
    pub fn prompt(&self) -> String {
        let mut text = String::new();
        text.push_str(&self.instruction);
        if !self.inputs.trim().is_empty() {
            text.push_str("\n\n## 输入\n\n");
            text.push_str(&self.inputs);
        }
        if let Some(revision) = &self.revision {
            text.push_str(&format!("\n\n（代码修订：{revision}）"));
        }
        text
    }

    /// 校验这份派工单本身。
    ///
    /// # Errors
    /// 指令为空、预算为零、超时为零，或者工作区路径不是绝对路径。
    pub fn check(&self) -> Result<()> {
        if self.instruction.trim().is_empty() {
            return Err(Error::invalid("派工单没有指令"));
        }
        if self.limits.token_budget == 0 {
            return Err(Error::invalid("token 预算不能是 0"));
        }
        if self.limits.timeout_millis == 0 {
            return Err(Error::invalid("超时不能是 0"));
        }
        if let Some(workspace) = &self.capabilities.workspace
            && !workspace.is_absolute()
        {
            return Err(Error::invalid("工作区必须是绝对路径"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worksheet() -> Worksheet {
        Worksheet {
            run: RunId::generate(),
            instruction: "看看有没有崩的地方".into(),
            skill: "查缺陷".into(),
            skill_version: "v1".into(),
            inputs: String::new(),
            revision: None,
            capabilities: Capabilities::default(),
            limits: Limits::default(),
        }
    }

    #[test]
    fn 空指令的派工单不收() {
        let mut sheet = worksheet();
        sheet.instruction = "   ".into();
        assert!(sheet.check().is_err());
    }

    #[test]
    fn 零预算零超时都不收() {
        let mut sheet = worksheet();
        sheet.limits.token_budget = 0;
        assert!(sheet.check().is_err());
        let mut sheet = worksheet();
        sheet.limits.timeout_millis = 0;
        assert!(sheet.check().is_err());
    }

    #[test]
    fn 相对路径的工作区不收() {
        let mut sheet = worksheet();
        sheet.capabilities.workspace = Some(PathBuf::from("relative/path"));
        assert!(sheet.check().is_err());
        sheet.capabilities.workspace = Some(PathBuf::from("/absolute/path"));
        assert!(sheet.check().is_ok());
    }

    #[test]
    fn 默认不许出网() {
        assert!(
            Capabilities::default().network.is_empty(),
            "EXE-007：默认拒绝"
        );
    }
}
