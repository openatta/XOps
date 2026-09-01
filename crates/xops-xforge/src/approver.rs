//! `approver` 的解析（`XFG-004`）。
//!
//! > 由结算行的 `writtenBy` 解析：**是人 → 就是他；是执行 → 取那个私有任务的所有者；
//! > 是插件求值 → 取安装该插件的维护者。** `role` 取该人在本项目的角色。
//!
//! ⚠️ 三种都**不需要回查别的表**：`WrittenBy` 的三个变体把该内联的都内联了
//! （`TBL-016`）。这不是巧合——那些字段存在的理由正是"一个月后 `_runs` 那行没了，
//! 还要回答得出这一票是谁的"。

use xops_identity::UserId;
use xops_table::WrittenBy;

/// 解析出来的那个人。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Approver {
    pub user: UserId,
    /// 他是**怎么**成为这一票的负责人的。
    pub via: Via,
}

/// 三条路。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// 人自己写的。
    Person,
    /// 一次执行写的——**取那个私有任务的所有者**。
    TaskOwner,
    /// 插件求值——**取安装该插件的维护者**。
    PluginInstaller,
}

/// 从 `writtenBy` 解析。
///
/// 平台自己写的行**没有负责人**：它不是谁的表态。
#[must_use]
pub const fn resolve(written_by: &WrittenBy) -> Option<Approver> {
    match written_by {
        WrittenBy::Person { user } => Some(Approver {
            user: *user,
            via: Via::Person,
        }),
        WrittenBy::Execution { task_owner, .. } => Some(Approver {
            user: *task_owner,
            via: Via::TaskOwner,
        }),
        WrittenBy::Plugin { installed_by, .. } => Some(Approver {
            user: *installed_by,
            via: Via::PluginInstaller,
        }),
        WrittenBy::Platform => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xops_core::Id;

    #[test]
    fn 三种都解析得出而且不用回查别的表() {
        let person = UserId::generate();
        let owner = UserId::generate();
        let installer = UserId::generate();
        assert_eq!(
            resolve(&WrittenBy::Person { user: person }).unwrap(),
            Approver {
                user: person,
                via: Via::Person
            }
        );
        assert_eq!(
            resolve(&WrittenBy::Execution {
                run: Id::generate(),
                task: Id::generate(),
                task_owner: owner,
                skill: "s".into(),
                skill_version: "1".into(),
                revision: None,
                status: "succeeded".into(),
            })
            .unwrap(),
            Approver {
                user: owner,
                via: Via::TaskOwner
            }
        );
        assert_eq!(
            resolve(&WrittenBy::Plugin {
                plugin: "approvals".into(),
                version: "1".into(),
                installed_by: installer,
                instance: Id::generate(),
            })
            .unwrap(),
            Approver {
                user: installer,
                via: Via::PluginInstaller
            }
        );
    }

    #[test]
    fn 平台写的行没有负责人() {
        assert!(resolve(&WrittenBy::Platform).is_none(), "它不是谁的表态");
    }
}
