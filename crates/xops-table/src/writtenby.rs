//! `writtenBy`：**恰好四种取值，且必须自包含**（`TBL-015`）。
//!
//! ②③ 必须内联那几项，**不能只存一个指向 `_runs` 的指针**——`_runs` 的行有保留期
//! 而结算行没有，一个月后还要能回答"这一票是谁的"，靠的就是这里内联的任务所有者
//! （`I-B`、G10）。
//!
//! `TBL-016`：**"不可信内容"不再是一个特殊标记，而是这个类型的自然结果**。
//! 责任归属、看板上的来源标识、流程节点的写入者判定，全都读它。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xops_core::{Actor, Error, Id, Result};
use xops_identity::UserId;

/// 这一行是谁写的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum WrittenBy {
    /// ① 某个人。
    Person { user: UserId },
    /// ② 某次执行。**六项全内联。**
    Execution {
        run: Id,
        task: Id,
        /// 任务所有者。**一个月后 `_runs` 那行没了，还要靠它回答"这一票是谁的"。**
        task_owner: UserId,
        skill: String,
        skill_version: String,
        /// 读的哪个代码修订（若来自读仓的执行）。
        revision: Option<String>,
        /// 该执行的最终状态。
        status: String,
    },
    /// ③ 某次插件求值。**四项全内联。**
    Plugin {
        plugin: String,
        version: String,
        /// 安装它的人。
        installed_by: UserId,
        /// 哪个流程实例。
        instance: Id,
    },
    /// ④ 平台自身。
    Platform,
}

impl WrittenBy {
    /// 看板上的来源标识（`TBL-016`）。
    #[must_use]
    pub fn origin(&self) -> &'static str {
        match self {
            Self::Person { .. } => "person",
            Self::Execution { .. } => "execution",
            Self::Plugin { .. } => "plugin",
            Self::Platform => "platform",
        }
    }

    /// 这一行的责任人。**平台自己写的那一类没有责任人**——它本来就不该有。
    #[must_use]
    pub fn responsible(&self) -> Option<UserId> {
        match self {
            Self::Person { user } => Some(*user),
            Self::Execution { task_owner, .. } => Some(*task_owner),
            Self::Plugin { installed_by, .. } => Some(*installed_by),
            Self::Platform => None,
        }
    }

    /// 内容可不可信。
    ///
    /// **这不是一个额外的标记位，是 `writtenBy` 的自然结果**（`TBL-016`）：
    /// 由模型产出的内容（执行）不可信，人写的与平台写的可信，插件求值是确定性代码因而可信。
    #[must_use]
    pub fn trusted(&self) -> bool {
        !matches!(self, Self::Execution { .. })
    }

    /// 落到事件上的粗粒度署名（RP-01 的 [`Actor`]）。
    ///
    /// 两者是两层：事件的 `actor` 回答"哪一类"，行上的 `writtenBy` 回答"具体是谁、
    /// 凭什么"——后者要自包含，前者不需要。
    #[must_use]
    pub fn actor(&self) -> Actor {
        match self {
            Self::Person { user } => Actor::User {
                user: user.to_string(),
            },
            Self::Execution { run, .. } => Actor::Execution { run: *run },
            Self::Plugin { plugin, .. } => Actor::Plugin {
                plugin: plugin.clone(),
            },
            Self::Platform => Actor::Platform,
        }
    }

    /// 装进行里。
    ///
    /// # Errors
    /// 序列化失败。
    pub fn to_value(&self) -> Result<Value> {
        serde_json::to_value(self)
            .map_err(|error| Error::internal(format!("writtenBy 装不进行里：{error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execution() -> WrittenBy {
        WrittenBy::Execution {
            run: Id::generate(),
            task: Id::generate(),
            task_owner: UserId::generate(),
            skill: "查缺陷".into(),
            skill_version: "v3".into(),
            revision: Some("abc123".into()),
            status: "succeeded".into(),
        }
    }

    #[test]
    fn 执行那一类把该内联的都内联了() {
        let value = execution().to_value().unwrap();
        for field in [
            "run",
            "task",
            "task_owner",
            "skill_version",
            "revision",
            "status",
        ] {
            assert!(
                value.get(field).is_some(),
                "少了 {field} —— TBL-015 要它自包含"
            );
        }
    }

    #[test]
    fn 一个月后还答得出这一票是谁的() {
        // _runs 的行有保留期，结算行没有。所以责任人必须在行上，不能靠指针回查。
        let written = execution();
        assert!(written.responsible().is_some());
        if let WrittenBy::Execution { task_owner, .. } = &written {
            assert_eq!(written.responsible(), Some(*task_owner));
        }
    }

    #[test]
    fn 可不可信是writtenby的自然结果() {
        assert!(!execution().trusted(), "模型产出的内容不可信");
        assert!(
            WrittenBy::Person {
                user: UserId::generate()
            }
            .trusted()
        );
        assert!(WrittenBy::Platform.trusted());
        assert!(
            WrittenBy::Plugin {
                plugin: "gate".into(),
                version: "1".into(),
                installed_by: UserId::generate(),
                instance: Id::generate(),
            }
            .trusted(),
            "插件求值是确定性代码"
        );
    }

    #[test]
    fn 四种取值各有一个来源标识() {
        let all = [
            WrittenBy::Person {
                user: UserId::generate(),
            },
            execution(),
            WrittenBy::Plugin {
                plugin: "g".into(),
                version: "1".into(),
                installed_by: UserId::generate(),
                instance: Id::generate(),
            },
            WrittenBy::Platform,
        ];
        let origins: std::collections::BTreeSet<&str> = all.iter().map(WrittenBy::origin).collect();
        assert_eq!(origins.len(), 4, "恰好四种");
    }

    #[test]
    fn 事件署名与行上的writtenby是两层() {
        let written = WrittenBy::Person {
            user: UserId::generate(),
        };
        assert!(matches!(written.actor(), Actor::User { .. }));
    }
}
