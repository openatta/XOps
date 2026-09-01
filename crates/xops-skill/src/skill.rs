//! 技能与它的版本。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Id, Result, Timestamp};
use xops_identity::{ProjectId, UserId};

use crate::declaration::Declaration;

/// 技能标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SkillId(Id);

impl SkillId {
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

impl std::fmt::Display for SkillId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 归属（`SKL-008`）。**两种，没有第三种。**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Ownership {
    /// 项目公共：成员都能看、能用、能触发。
    Public,
    /// 个人私有：**同项目的其他成员看不到、也用不了**。
    Private { owner: UserId },
}

impl Ownership {
    #[must_use]
    pub const fn owner(self) -> Option<UserId> {
        match self {
            Self::Public => None,
            Self::Private { owner } => Some(owner),
        }
    }
}

/// 版本状态（`SKL-005`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// 草稿。**不会被任何自动触发路径执行**（`SKL-004`）。
    Draft,
    Published,
    /// 停用后不再被触发，**历史执行记录完整保留**。
    Disabled,
}

/// 一个技能。内容与声明挂在版本上，这里只有身份与归属。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub id: SkillId,
    pub project: ProjectId,
    pub name: String,
    pub ownership: Ownership,
    pub created_by: UserId,
    pub created_at: Timestamp,
}

/// 一个版本。
///
/// `SKL-002`：**已发布的版本不可变**（`I-K`）——改内容产生新版本，旧版本原样可查。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub skill: SkillId,
    pub project: ProjectId,
    /// 从 1 开始。
    pub version: u32,
    /// 技能内容。
    ///
    /// ⚠️ `SKL-006`：**它是不可信输入**（G7）。平台不解析其语义、不因其内容改变控制流，
    /// 只负责交给执行运行时。
    pub content: String,
    pub declaration: Declaration,
    pub state: State,
    pub created_by: UserId,
    pub created_at: Timestamp,
    /// 有过一次成功的测试执行吗（`SKL-003`）。**没有就发布不了。**
    pub tested_run: Option<Id>,
    pub published_at: Option<Timestamp>,
    /// `SKL-011` 的那条例外：这个版本被用于满足过某个流程节点。
    ///
    /// **一旦为真，它的内容对项目成员转为可读**——私有是为了不打扰别人，
    /// 不是为了让自动决策不可审查。**标记由 RP-15 打，本包只按它判可见性。**
    pub used_for_settlement: bool,
}

impl Version {
    /// 这个版本能不能被发布。
    ///
    /// # Errors
    /// 没有成功的测试执行（`SKL-003`），或者它已经不是草稿了。
    pub fn check_publishable(&self) -> Result<()> {
        if self.state != State::Draft {
            return Err(Error::invalid("只有草稿能被发布"));
        }
        if self.tested_run.is_none() {
            return Err(Error::invalid(
                "这个版本还没有过一次成功的测试执行，发布不了（SKL-003）。\
                 测试执行由作者手动发起，在与正式执行相同的隔离环境中进行",
            ));
        }
        Ok(())
    }

    /// 能不能被触发执行。
    #[must_use]
    pub const fn runnable(&self) -> bool {
        matches!(self.state, State::Published)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declaration::OutputShape;

    fn version(state: State, tested: bool) -> Version {
        Version {
            skill: SkillId::generate(),
            project: ProjectId::generate(),
            version: 1,
            content: "看看有没有崩的地方".into(),
            declaration: Declaration {
                inputs: vec![],
                output: OutputShape::Report,
                needs_repository: false,
                network: vec![],
                max_duration_millis: 1_000,
            },
            state,
            created_by: UserId::generate(),
            created_at: Timestamp::from_millis(0),
            tested_run: tested.then(Id::generate),
            published_at: None,
            used_for_settlement: false,
        }
    }

    #[test]
    fn 没测过的草稿发布不了() {
        let error = version(State::Draft, false)
            .check_publishable()
            .unwrap_err();
        assert!(error.message().contains("成功的测试执行"), "SKL-003");
        assert!(version(State::Draft, true).check_publishable().is_ok());
    }

    #[test]
    fn 已发布的不能再发布一次() {
        assert!(version(State::Published, true).check_publishable().is_err());
    }

    #[test]
    fn 只有已发布的能被触发() {
        assert!(
            !version(State::Draft, true).runnable(),
            "SKL-004：上传不执行"
        );
        assert!(version(State::Published, true).runnable());
        assert!(
            !version(State::Disabled, true).runnable(),
            "停用后不再被触发"
        );
    }

    #[test]
    fn 归属只有两种() {
        assert_eq!(Ownership::Public.owner(), None);
        let owner = UserId::generate();
        assert_eq!(Ownership::Private { owner }.owner(), Some(owner));
    }
}
