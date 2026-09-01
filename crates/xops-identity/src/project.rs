//! 项目与成员。
//!
//! **项目是唯一的归属边界、权限边界、数据边界。它不是团队，也不是仓库。**

use serde::{Deserialize, Serialize};
use xops_core::{Error, Id, Result, Role, Timestamp};

use crate::user::UserId;

/// 项目标识。稳定、创建后不可变（`PRJ-002`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProjectId(Id);

impl ProjectId {
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

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 项目短名：**全平台唯一、创建后不可变、限定为便于手写的字符集**（`PRJ-003`）。
///
/// 平台自己不认识"缺陷 ID"这种概念，但模板会用短名去组合可被手写引用的标识（`TPL-005`）——
/// 所以字符集要窄到能被人抄进 issue 标题里而不出岔子：小写字母、数字、连字符。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Slug(String);

impl Slug {
    pub const MIN_LEN: usize = 2;
    pub const MAX_LEN: usize = 24;

    /// # Errors
    /// 长度越界、不是小写字母开头、含允许字符之外的东西、或者以连字符收尾。
    pub fn new(slug: impl Into<String>) -> Result<Self> {
        let slug = slug.into();
        let shaped = (Self::MIN_LEN..=Self::MAX_LEN).contains(&slug.len())
            && slug.starts_with(|c: char| c.is_ascii_lowercase())
            && !slug.ends_with('-')
            && !slug.contains("--")
            && slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !shaped {
            return Err(Error::invalid(format!(
                "短名要 {}–{} 个字符，小写字母开头，只含小写字母、数字与单个连字符：{slug}",
                Self::MIN_LEN,
                Self::MAX_LEN
            )));
        }
        Ok(Self(slug))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Slug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 一个项目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    /// `PRJ-003`：**一旦分配就再也改不了。**
    pub slug: Slug,
    /// `PRJ-002`：可变，且**不得作为关联键使用**。
    pub display_name: String,
    pub created_at: Timestamp,
    /// `PRJ-009`：归档后转为只读，历史内容完整保留、可查询。
    pub archived_at: Option<Timestamp>,
}

impl Project {
    #[must_use]
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
}

/// 某个人在某个项目里的角色。
///
/// `PRJ-007`：同一用户在不同项目中的角色**互相独立**——没有跨项目全局角色，
/// 没有组织级继承（G4）。所以成员关系是 `(项目, 用户)` 的一条记录，不是用户身上的一个属性。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub project: ProjectId,
    pub user: UserId,
    pub role: Role,
    pub added_at: Timestamp,
}

/// 一次成员变更的意图。判"改完之后还剩几个所有者"要用到它（`PRJ-006`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberChange {
    Add(Role),
    ChangeRole(Role),
    Remove,
}

/// 改完之后这个项目还有没有所有者。
///
/// `PRJ-006`：**一个项目必须始终至少有一个所有者，移除最后一个所有者被拒绝。**
/// 单独拎成纯函数，是因为它要在写入区间里被调用，而区间里不该有别的逻辑。
#[must_use]
pub fn owners_after(current: &[Member], user: UserId, change: MemberChange) -> usize {
    current
        .iter()
        .filter(|member| {
            let role = if member.user == user {
                match change {
                    MemberChange::Remove => return false,
                    MemberChange::Add(role) | MemberChange::ChangeRole(role) => role,
                }
            } else {
                member.role
            };
            role == Role::Owner
        })
        .count()
        + usize::from(
            matches!(change, MemberChange::Add(Role::Owner))
                && !current.iter().any(|member| member.user == user),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(user: UserId, role: Role) -> Member {
        Member {
            project: ProjectId::from_id(Id::from_parts(1, 1)),
            user,
            role,
            added_at: Timestamp::from_millis(0),
        }
    }

    #[test]
    fn 短名认得出好坏() {
        assert!(Slug::new("acme").is_ok());
        assert!(Slug::new("acme-web-2").is_ok());
        assert!(Slug::new("a").is_err(), "太短");
        assert!(Slug::new("A-team").is_err(), "大写");
        assert!(Slug::new("1st").is_err(), "数字开头");
        assert!(Slug::new("acme-").is_err(), "连字符收尾");
        assert!(Slug::new("acme--web").is_err(), "双连字符");
        assert!(Slug::new("a".repeat(Slug::MAX_LEN + 1)).is_err());
    }

    #[test]
    fn 移除最后一个所有者会剩零个() {
        let owner = UserId::generate();
        let plain = UserId::generate();
        let members = [member(owner, Role::Owner), member(plain, Role::Member)];
        assert_eq!(owners_after(&members, owner, MemberChange::Remove), 0);
        assert_eq!(owners_after(&members, plain, MemberChange::Remove), 1);
    }

    #[test]
    fn 把唯一的所有者降级也会剩零个() {
        let owner = UserId::generate();
        let members = [member(owner, Role::Owner)];
        assert_eq!(
            owners_after(&members, owner, MemberChange::ChangeRole(Role::Maintainer)),
            0
        );
        assert_eq!(
            owners_after(&members, owner, MemberChange::ChangeRole(Role::Owner)),
            1
        );
    }

    #[test]
    fn 两个所有者时移走一个还剩一个() {
        let first = UserId::generate();
        let second = UserId::generate();
        let members = [member(first, Role::Owner), member(second, Role::Owner)];
        assert_eq!(owners_after(&members, first, MemberChange::Remove), 1);
    }

    #[test]
    fn 新加一个所有者算得进去() {
        let owner = UserId::generate();
        let newcomer = UserId::generate();
        let members = [member(owner, Role::Owner)];
        assert_eq!(
            owners_after(&members, newcomer, MemberChange::Add(Role::Owner)),
            2
        );
        assert_eq!(
            owners_after(&members, newcomer, MemberChange::Add(Role::Member)),
            1
        );
    }

    #[test]
    fn 归档看得出来() {
        let mut project = Project {
            id: ProjectId::generate(),
            slug: Slug::new("acme").unwrap(),
            display_name: "Acme".into(),
            created_at: Timestamp::from_millis(0),
            archived_at: None,
        };
        assert!(!project.is_archived());
        project.archived_at = Some(Timestamp::from_millis(1));
        assert!(project.is_archived());
    }
}
