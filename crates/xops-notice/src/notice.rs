//! 一条通知长什么样。
//!
//! ⚠️ **这个类型没有公开的构造函数。** 唯一造得出它的地方是
//! [`crate::derive::from_event`]——`NTF-002` 说的"不引入独立的产生路径"
//! 在这里是**可见性上的**，不是一句约定。

use serde::{Deserialize, Serialize};
use xops_core::{Id, Timestamp};
use xops_identity::{ProjectId, UserId};

/// 通知标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NoticeId(Id);

impl NoticeId {
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

impl std::fmt::Display for NoticeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 值得通知的**五类**（`NTF-007`）。**没有第六类**——
/// 加一类要先有一个已存在的事件，不是先有一个想通知的场景。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// 有节点在等我处理。
    NodeAwaitingMe,
    /// 流程实例已决定。
    InstanceDecided,
    /// **我写的行未被采纳。** 自动化失灵时**唯一的信号**——
    /// 没有它，一个写了行却没被采纳的人不会知道自己白写了。
    RowNotSettled,
    /// 执行完成或失败（尤其定时任务）。
    RunFinished,
    /// 表里的某行指派给我。
    RowAssignedToMe,
}

impl Kind {
    /// 五类，一个不多一个不少。
    pub const ALL: [Self; 5] = [
        Self::NodeAwaitingMe,
        Self::InstanceDecided,
        Self::RowNotSettled,
        Self::RunFinished,
        Self::RowAssignedToMe,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NodeAwaitingMe => "node-awaiting-me",
            Self::InstanceDecided => "instance-decided",
            Self::RowNotSettled => "row-not-settled",
            Self::RunFinished => "run-finished",
            Self::RowAssignedToMe => "row-assigned-to-me",
        }
    }
}

/// 发给谁。
///
/// **只有"指名道姓的那几个"这一种。** 没有"广播给项目里所有人"那一档——
/// 五类通知每一类都答得出"这条是给谁的"，答不出的那种本来就不该发。
///
/// **只发给对该事件所属项目有可见权限的用户**（`NTF-005`）——
/// 这一层过滤在 [`crate::service::Notices`] 里做，因为只有那里问得到目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipients(pub Vec<UserId>);

/// 一条通知。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub id: NoticeId,
    /// 收信人。**读写被硬限定为 `user = 令牌持有人`**（`NTF-010`）。
    pub user: UserId,
    /// 哪个项目。跨项目聚合靠的是它可以不同（`NTF-014`）。
    pub project: Option<ProjectId>,
    pub kind: Kind,
    /// 指向什么。**是指针，不是内容**（`NTF-006`）。
    pub subject: String,
    /// 正文。**由确定性代码生成，不经模型**（`NTF-003`）；
    /// 里面的自由文本**原样引用或截断**，不改写、不摘要、不翻译（`NTF-004`）。
    pub text: String,
    pub created_at: Timestamp,
    pub read_at: Option<Timestamp>,
}

impl Notice {
    /// **crate 内部专用。** 见本模块开头那句：唯一造得出通知的地方是事件派生。
    pub(crate) const fn new(
        id: NoticeId,
        user: UserId,
        project: Option<ProjectId>,
        kind: Kind,
        subject: String,
        text: String,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            user,
            project,
            kind,
            subject,
            text,
            created_at,
            read_at: None,
        }
    }

    #[must_use]
    pub const fn unread(&self) -> bool {
        self.read_at.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 五类一个不多一个不少() {
        assert_eq!(Kind::ALL.len(), 5, "NTF-007");
        let names: std::collections::BTreeSet<&str> =
            Kind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(names.len(), 5, "名字不能撞");
    }
}
