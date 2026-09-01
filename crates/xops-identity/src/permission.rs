//! 权限判定。**纯函数，没有 I/O**（`PRJ-010`）。
//!
//! 给定（角色，动作）永远得到相同结果，不读库、不问模型、不看时间（G8）。
//! 判定要用到的那点上下文——这个人在这个项目里是什么角色——由调用方先查好传进来。
//!
//! `G4`：权限一律按项目判定。**没有跨项目全局角色，没有组织级继承。**

use xops_core::Role;

/// 平台里所有需要判权的动作。
///
/// 穷举一个 enum 而不是收一串字符串，是为了让"新加一个动作"这件事必须经过这里——
/// 那样才有一处地方能回答"哪个角色能做什么"（`PRJ-004`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    // —— 成员 ——
    /// 看得见这个项目、读它的内容。
    ReadProject,
    /// 建表、加列。
    CreateTable,
    /// 写普通表的行。
    WriteTable,
    /// 写技能（含发布）。
    WriteSkill,
    /// 写任务、启停任务、手动触发。
    WriteTask,
    /// 参与流程：往结算表写一行、发起实例。
    ParticipateFlow,
    /// 定义流程、发布新版本。
    DefineFlow,

    // —— 维护者 ——
    /// 管理项目内的业务对象（删表、停用技能与任务、取消实例）。
    ManageBusinessObject,
    /// 绑定 Git 仓、轮换只读凭据。
    BindRepository,
    /// 安装候选插件。
    InstallPlugin,

    // —— 所有者 ——
    /// 加成员、改角色、移除成员。
    ManageMember,
    /// 归档项目、改项目显示名。
    ManageProject,
    /// 写受保护表（名单表）。
    WriteProtectedTable,
    /// 读写插件配置。
    WritePluginConfig,
}

impl Action {
    /// 做这件事至少要是哪一级。
    ///
    /// **这是"哪个角色能做什么"唯一的一份表。** 别处再写一个 `match`，
    /// 三个角色变四个的那天就会漏掉一处。
    #[must_use]
    pub const fn floor(self) -> Role {
        match self {
            Self::ReadProject
            | Self::CreateTable
            | Self::WriteTable
            | Self::WriteSkill
            | Self::WriteTask
            | Self::ParticipateFlow
            | Self::DefineFlow => Role::Member,

            Self::ManageBusinessObject | Self::BindRepository | Self::InstallPlugin => {
                Role::Maintainer
            }

            Self::ManageMember
            | Self::ManageProject
            | Self::WriteProtectedTable
            | Self::WritePluginConfig => Role::Owner,
        }
    }

    /// 这个动作会不会写东西。归档项目之后**只有不写的动作还能做**（`PRJ-009`）。
    #[must_use]
    pub const fn writes(self) -> bool {
        !matches!(self, Self::ReadProject)
    }
}

/// 这个角色能不能做这件事。
///
/// **纯函数**：`(角色, 动作)` 之外不看任何东西。项目归没归档是另一个维度，见 [`can_in`]。
#[must_use]
pub fn can(role: Role, action: Action) -> bool {
    role.at_least(action.floor())
}

/// 连项目状态一起判：归档的项目**转为只读**，任何写动作一律不行，
/// 哪怕来的是所有者（`PRJ-009`）。
#[must_use]
pub fn can_in(role: Role, action: Action, archived: bool) -> bool {
    if archived && action.writes() {
        return false;
    }
    can(role, action)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLES: [Role; 3] = [Role::Member, Role::Maintainer, Role::Owner];
    const ACTIONS: [Action; 14] = [
        Action::ReadProject,
        Action::CreateTable,
        Action::WriteTable,
        Action::WriteSkill,
        Action::WriteTask,
        Action::ParticipateFlow,
        Action::DefineFlow,
        Action::ManageBusinessObject,
        Action::BindRepository,
        Action::InstallPlugin,
        Action::ManageMember,
        Action::ManageProject,
        Action::WriteProtectedTable,
        Action::WritePluginConfig,
    ];

    #[test]
    fn 穷举三乘十四() {
        // PRJ-010 说它是确定性纯函数 —— 那它的真值表就该被整个写下来看一眼。
        for role in ROLES {
            for action in ACTIONS {
                assert_eq!(
                    can(role, action),
                    role >= action.floor(),
                    "{role:?} / {action:?}"
                );
            }
        }
    }

    #[test]
    fn 所有者能做全部() {
        assert!(ACTIONS.iter().all(|action| can(Role::Owner, *action)));
    }

    #[test]
    fn 成员碰不到成员管理与受保护表() {
        assert!(!can(Role::Member, Action::ManageMember));
        assert!(!can(Role::Member, Action::WriteProtectedTable));
        assert!(!can(Role::Member, Action::WritePluginConfig));
        assert!(!can(Role::Maintainer, Action::ManageMember));
    }

    #[test]
    fn 维护者管业务对象但不管人() {
        assert!(can(Role::Maintainer, Action::InstallPlugin));
        assert!(can(Role::Maintainer, Action::ManageBusinessObject));
        assert!(!can(Role::Maintainer, Action::ManageProject));
    }

    #[test]
    fn 归档之后只读() {
        for role in ROLES {
            assert!(can_in(role, Action::ReadProject, true), "读还得能读");
            for action in ACTIONS.iter().filter(|action| action.writes()) {
                assert!(
                    !can_in(role, *action, true),
                    "{role:?} 在归档项目里不该能 {action:?}"
                );
            }
        }
    }

    #[test]
    fn 判定不受调用次数影响() {
        // 纯函数最起码的一条：同样的输入连问一千次答案一样。
        for _ in 0..1_000 {
            assert!(can(Role::Member, Action::WriteTable));
            assert!(!can(Role::Member, Action::ManageMember));
        }
    }
}
