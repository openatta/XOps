//! 允许写入者的判定（`FLW-018`、`FLW-026` ②③）。
//!
//! ⚠️ **判定在写入这一刻，不是事先**（`FLW-029`）：名单表可以随时改——
//! 事件发出时 a 在名单里，任务跑了几分钟，期间 a 被移出，② 正好挡住这一情形。

use std::sync::Arc;

use xops_core::{Result, Role};
use xops_flow::Writers;
use xops_identity::{Directory, ProjectId, UserId};
use xops_table::{Tables, WrittenBy};

/// 写这一行的到底是谁——**一个人**。
///
/// `I-O`：**任务不是责任主体，人才是。** 所以写入者是任务时，归到任务所有者头上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Responsible {
    pub user: UserId,
    /// 它是经由一次执行落下来的吗。
    pub via_execution: bool,
}

/// 从 `writtenBy` 归出责任人。
#[must_use]
pub fn responsible(written_by: &WrittenBy) -> Option<Responsible> {
    match written_by {
        WrittenBy::Person { user } => Some(Responsible {
            user: *user,
            via_execution: false,
        }),
        WrittenBy::Execution { task_owner, .. } => Some(Responsible {
            user: *task_owner,
            via_execution: true,
        }),
        // 插件求值写回的行由平台代写，它不是"谁的表态"。
        WrittenBy::Plugin { .. } | WrittenBy::Platform => None,
    }
}

/// 名单表的判定要读表，所以这一层要拿得到目录与表。
pub struct WriterCheck {
    directory: Arc<Directory>,
    tables: Arc<Tables>,
}

impl std::fmt::Debug for WriterCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriterCheck").finish_non_exhaustive()
    }
}

impl WriterCheck {
    #[must_use]
    pub fn new(directory: Arc<Directory>, tables: Arc<Tables>) -> Self {
        Self { directory, tables }
    }

    /// ② 这个人此刻在不在允许写入者集合里。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn allowed(
        &self,
        project: ProjectId,
        writers: &Writers,
        who: Responsible,
        written_by: &WrittenBy,
    ) -> Result<bool> {
        // ① 角色集合。
        if let Some(role) = self.directory.role_of(project, who.user)?
            && writers
                .roles
                .iter()
                .any(|allowed| role_covers(role, *allowed))
        {
            return Ok(true);
        }
        // ② 名单表里的人。**每次现查**——名单可以随时改。
        if let Some(roster) = &writers.roster
            && self.in_roster(project, roster, who.user)?
        {
            return Ok(true);
        }
        // ③ 指定的私有任务写入的行。
        if let Some(task) = writers.task
            && let WrittenBy::Execution { task: wrote, .. } = written_by
            && task.as_id() == *wrote
        {
            return Ok(true);
        }
        Ok(false)
    }

    /// ③ 职责分离：写入者 ≠ 实例发起人。
    #[must_use]
    pub fn separated(&self, who: Responsible, started_by: UserId) -> bool {
        who.user != started_by
    }

    fn in_roster(
        &self,
        project: ProjectId,
        roster: &xops_table::TableId,
        user: UserId,
    ) -> Result<bool> {
        // **名单表上的一次谓词查**，不是"扫前一万行再看看在不在里面"——
        // 后者在名单表长过上限之后会把一个真在名单里的人判成不在，
        // 而那条判定是审批权的元权限（`FLW-019`）。
        let hit = self.tables.query_all(
            Some(project),
            roster,
            &[xops_table::Filter::equals("user", user.to_string())],
            xops_table::MAX_SCAN,
        )?;
        Ok(!hit.is_empty())
    }
}

/// 角色是逐级包含的：所有者能做维护者能做的一切。
fn role_covers(actual: Role, required: Role) -> bool {
    actual.at_least(required)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xops_core::Id;

    #[test]
    fn 任务写的行归到任务所有者头上() {
        let owner = UserId::generate();
        let written = WrittenBy::Execution {
            run: Id::generate(),
            task: Id::generate(),
            task_owner: owner,
            skill: "s".into(),
            skill_version: "1".into(),
            revision: None,
            status: "succeeded".into(),
        };
        let who = responsible(&written).unwrap();
        assert_eq!(who.user, owner, "I-O：任务不是责任主体，人才是");
        assert!(who.via_execution);
    }

    #[test]
    fn 平台与插件写的行不是谁的表态() {
        assert!(responsible(&WrittenBy::Platform).is_none());
        assert!(
            responsible(&WrittenBy::Plugin {
                plugin: "gate".into(),
                version: "1".into(),
                installed_by: UserId::generate(),
                instance: Id::generate(),
            })
            .is_none()
        );
    }

    #[test]
    fn 职责分离比的是人() {
        let a = UserId::generate();
        let b = UserId::generate();
        let check = |user| Responsible {
            user,
            via_execution: false,
        };
        // 用一个不需要 IO 的实例来验这一条。
        assert!(!separated_for_test(check(a), a), "自己批自己不行");
        assert!(separated_for_test(check(b), a));
    }

    fn separated_for_test(who: Responsible, started_by: UserId) -> bool {
        who.user != started_by
    }

    #[test]
    fn 角色逐级包含() {
        assert!(role_covers(Role::Owner, Role::Member));
        assert!(!role_covers(Role::Member, Role::Owner));
    }
}
