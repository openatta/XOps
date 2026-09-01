//! 项目内的三个角色。
//!
//! **只有类型与它的序在这里。** "某个角色能不能做某件事"那张表归 RP-02
//! （`xops-identity` 的权限判定纯函数）——本 crate 不知道有哪些动作。
//!
//! `PRJ-004`：角色集合固定，不做可配置角色系统。

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// 项目内恰好三个角色，权限逐级包含。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// 建表、写表、写技能与任务、参与流程。
    Member,
    /// 成员的一切，加上管理项目内业务对象、安装插件。
    Maintainer,
    /// 维护者的一切，加上管理成员与项目本身、写受保护表、写插件配置。
    Owner,
}

impl Role {
    /// 至少是 `floor` 这一级。
    ///
    /// 这是"逐级包含"唯一该被写下来的地方——别处再写一次 `match`，
    /// 三个角色变四个的那天就会漏掉一处。
    #[must_use]
    pub fn at_least(self, floor: Self) -> bool {
        self >= floor
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Maintainer => "maintainer",
            Self::Owner => "owner",
        }
    }

    /// # Errors
    /// 不是那三个之一。
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "member" => Ok(Self::Member),
            "maintainer" => Ok(Self::Maintainer),
            "owner" => Ok(Self::Owner),
            other => Err(Error::invalid(format!("不认识的角色：{other}"))),
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 逐级包含() {
        assert!(Role::Owner.at_least(Role::Member));
        assert!(Role::Owner.at_least(Role::Owner));
        assert!(Role::Maintainer.at_least(Role::Member));
        assert!(!Role::Maintainer.at_least(Role::Owner));
        assert!(!Role::Member.at_least(Role::Maintainer));
    }

    #[test]
    fn 文本往返() {
        for role in [Role::Member, Role::Maintainer, Role::Owner] {
            assert_eq!(Role::parse(role.as_str()).unwrap(), role);
        }
        assert!(Role::parse("admin").is_err());
    }
}
