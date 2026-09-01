//! 失败分类（`EXE-020`）。
//!
//! **八类，一个都不能少。** 分类不是为了好看：调用方要靠它决定"重跑有没有意义"，
//! 而 `_runs.failureKind` 是这个问题在账上唯一的答案。

use serde::{Deserialize, Serialize};

/// 这次执行为什么没成。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    /// 技能本身写错了、跑出来是错的。**重跑没用**。
    Skill,
    /// 凭据不对或过期。
    Credential,
    /// 只读工作区备不出来（仓拉不下来、修订不存在）。
    Workspace,
    /// 模型服务那一侧的问题。**重跑通常有用**。
    ModelService,
    /// 引擎不可用或引擎自己出错。
    ///
    /// ⚠️ `EXE-030`：**引擎不可用时执行失败并如实归入这一类，
    /// 绝不在 XOps 进程里就地跑一遍。**
    Engine,
    /// 超时被强制终止。
    Timeout,
    /// 超出这次执行的 token 预算。
    TokenBudget,
    /// CPU / 内存 / 磁盘超限。
    Resource,
}

impl FailureKind {
    /// 重跑有没有意义。
    #[must_use]
    pub const fn worth_retrying(self) -> bool {
        matches!(self, Self::ModelService | Self::Engine | Self::Resource)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Credential => "credential",
            Self::Workspace => "workspace",
            Self::ModelService => "model-service",
            Self::Engine => "engine",
            Self::Timeout => "timeout",
            Self::TokenBudget => "token-budget",
            Self::Resource => "resource",
        }
    }

    /// 全部八类。枚举验收用它。
    #[must_use]
    pub const fn all() -> [Self; 8] {
        [
            Self::Skill,
            Self::Credential,
            Self::Workspace,
            Self::ModelService,
            Self::Engine,
            Self::Timeout,
            Self::TokenBudget,
            Self::Resource,
        ]
    }
}

impl std::fmt::Display for FailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 八类一个都不少() {
        assert_eq!(FailureKind::all().len(), 8, "EXE-020");
        let names: std::collections::BTreeSet<&str> = FailureKind::all()
            .iter()
            .map(|kind| kind.as_str())
            .collect();
        assert_eq!(names.len(), 8, "名字也不能重");
    }

    #[test]
    fn 该重跑的说得清() {
        assert!(FailureKind::Engine.worth_retrying(), "引擎不可用是暂时的");
        assert!(FailureKind::ModelService.worth_retrying());
        assert!(
            !FailureKind::Skill.worth_retrying(),
            "技能写错了，重跑一百次还是错"
        );
        assert!(
            !FailureKind::TokenBudget.worth_retrying(),
            "预算不会自己变大"
        );
    }

    #[test]
    fn 可往返() {
        for kind in FailureKind::all() {
            let text = serde_json::to_string(&kind).unwrap();
            assert_eq!(serde_json::from_str::<FailureKind>(&text).unwrap(), kind);
        }
    }
}
