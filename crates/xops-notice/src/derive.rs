//! 从事件派生通知（`NTF-002`、`NTF-003`、`NTF-004`、`NTF-006`）。
//!
//! 四条一起管住这个文件：
//!
//! ```text
//! NTF-002  只从事件派生 —— **本 crate 里造 Notice 的地方只有 from_event 一处**
//! NTF-003  内容由确定性代码生成，**不经模型**（G8）
//! NTF-004  自由文本**原样引用或截断**，不改写、不摘要、不翻译（G7）
//! NTF-006  **不含凭据、令牌或产物原文，只含指针**
//! ```
//!
//! `NTF-006` 的落法是**结构上的**：正文由固定模板 + 指针 + 一段引用的自由文本拼成，
//! **产物原文根本没有一个字段能进来**——`SourceEvent` 里没有装它的地方。

use xops_core::Timestamp;
use xops_identity::{ProjectId, UserId};

use crate::notice::{Kind, Notice, NoticeId, Recipients};

/// 自由文本引用多长。超了就**截断并标注**——不摘要。
pub const QUOTE_MAX_CHARS: usize = 200;
/// 截断标注。
pub const TRUNCATION_MARK: &str = "……〔已截断〕";

/// 派生通知的那几个事件。
///
/// ⚠️ **每一个变体都对应一个已经发生、已经留痕的事实。**
/// 想加一类通知，先要有一个已存在的事件——反过来不行（`NTF-002`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceEvent {
    /// 节点被激活，这几个人可以处理它。
    NodeActivated {
        project: ProjectId,
        instance: String,
        node: String,
        /// 允许写入者。**由 RP-15 算好传进来**——本包不判权限。
        awaiting: Vec<UserId>,
    },
    /// 实例进了终态。
    InstanceDecided {
        project: ProjectId,
        instance: String,
        /// `approved` / `rejected` / `cancelled` / `expired`。
        state: String,
        /// 发起人与参与过的人。
        interested: Vec<UserId>,
    },
    /// 一行写进了结算表，**但没有被采纳**。
    RowNotSettled {
        project: ProjectId,
        instance: String,
        table: String,
        row: String,
        writer: UserId,
        /// 为什么没算数。**这是自由文本，原样引用或截断。**
        reason: String,
    },
    /// 一次执行结束了。
    RunFinished {
        project: ProjectId,
        run: String,
        task: String,
        /// `succeeded` / `failed` / `cancelled`。
        status: String,
        /// 任务所有者。
        owner: UserId,
        /// 后处理（onComplete）失败时的痕迹（`TSK-012`）。
        ///
        /// **`_runs` 那一行已经写好了**——后处理失败只留自己的痕迹并通知任务所有者，
        /// 它不改执行本身的结论。这是自由文本，**原样引用或截断**。
        after_failure: Option<String>,
    },
    /// 表里的某一行指派给了谁。
    RowAssigned {
        project: ProjectId,
        table: String,
        row: String,
        assignee: UserId,
    },
}

impl SourceEvent {
    /// 这个事件属于哪个项目。**可见权限按它判**（`NTF-005`）。
    #[must_use]
    pub const fn project(&self) -> ProjectId {
        match self {
            Self::NodeActivated { project, .. }
            | Self::InstanceDecided { project, .. }
            | Self::RowNotSettled { project, .. }
            | Self::RunFinished { project, .. }
            | Self::RowAssigned { project, .. } => *project,
        }
    }
}

/// 一条待发的通知：给谁、什么类、指向什么、正文是什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub recipients: Recipients,
    pub kind: Kind,
    pub subject: String,
    pub text: String,
}

/// 从一个事件派生出该发的那些通知。
///
/// **这是本 crate 里唯一造得出 [`Notice`] 的入口**（`NTF-002`）。
#[must_use]
pub fn from_event(event: &SourceEvent) -> Vec<Derived> {
    match event {
        SourceEvent::NodeActivated {
            instance,
            node,
            awaiting,
            ..
        } => vec![Derived {
            recipients: Recipients(awaiting.clone()),
            kind: Kind::NodeAwaitingMe,
            subject: format!("{instance}/{node}"),
            text: format!("流程实例 {instance} 的节点 {node} 在等你处理。"),
        }],
        SourceEvent::InstanceDecided {
            instance,
            state,
            interested,
            ..
        } => vec![Derived {
            recipients: Recipients(interested.clone()),
            kind: Kind::InstanceDecided,
            subject: instance.clone(),
            text: format!("流程实例 {instance} 已决定：{state}。"),
        }],
        SourceEvent::RowNotSettled {
            instance,
            table,
            row,
            writer,
            reason,
            ..
        } => vec![Derived {
            recipients: Recipients(vec![*writer]),
            kind: Kind::RowNotSettled,
            subject: format!("{table}/{row}"),
            // 这一条是自动化失灵时**唯一的信号**，所以理由必须原样带上。
            text: format!(
                "你写进 {table} 的行 {row}（实例 {instance}）没有被采纳。理由原文：{}",
                quote(reason)
            ),
        }],
        SourceEvent::RunFinished {
            run,
            task,
            status,
            owner,
            after_failure,
            ..
        } => vec![Derived {
            recipients: Recipients(vec![*owner]),
            kind: Kind::RunFinished,
            subject: run.clone(),
            // ⚠️ **这里只有指针，没有 output、没有 trace**（NTF-006）——
            // `SourceEvent::RunFinished` 里压根没有装它们的字段。
            text: after_failure.as_ref().map_or_else(
                || format!("任务 {task} 的执行 {run} 结束了：{status}。产出见 _runs 的这一行。"),
                |failure| {
                    format!(
                        "任务 {task} 的执行 {run} 结束了：{status}，\
                         但后处理没跑成。**执行本身的记录不受影响**。痕迹原文：{}",
                        quote(failure)
                    )
                },
            ),
        }],
        SourceEvent::RowAssigned {
            table,
            row,
            assignee,
            ..
        } => vec![Derived {
            recipients: Recipients(vec![*assignee]),
            kind: Kind::RowAssignedToMe,
            subject: format!("{table}/{row}"),
            text: format!("{table} 里的行 {row} 指派给了你。"),
        }],
    }
}

/// 引用一段自由文本：**原样，或者截断**（`NTF-004`）。
///
/// 不改写、不摘要、不翻译。**截断按字符**——半个汉字会让整段读不回来。
#[must_use]
pub fn quote(text: &str) -> String {
    if text.chars().count() <= QUOTE_MAX_CHARS {
        return text.to_owned();
    }
    let head: String = text.chars().take(QUOTE_MAX_CHARS).collect();
    format!("{head}{TRUNCATION_MARK}")
}

/// 把派生结果落成一条给某个人的通知。**crate 内部专用。**
pub(crate) fn materialize(
    derived: &Derived,
    user: UserId,
    project: Option<ProjectId>,
    at: Timestamp,
) -> Notice {
    Notice::new(
        NoticeId::generate(),
        user,
        project,
        derived.kind,
        derived.subject.clone(),
        derived.text.clone(),
        at,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> UserId {
        UserId::generate()
    }

    #[test]
    fn 五类都派生得出来() {
        let project = ProjectId::generate();
        let events = [
            SourceEvent::NodeActivated {
                project,
                instance: "I1".into(),
                node: "复核".into(),
                awaiting: vec![user()],
            },
            SourceEvent::InstanceDecided {
                project,
                instance: "I1".into(),
                state: "approved".into(),
                interested: vec![user()],
            },
            SourceEvent::RowNotSettled {
                project,
                instance: "I1".into(),
                table: "approvals".into(),
                row: "R1".into(),
                writer: user(),
                reason: "写入者不在允许名单里".into(),
            },
            SourceEvent::RunFinished {
                project,
                run: "RUN1".into(),
                task: "T1".into(),
                status: "failed".into(),
                owner: user(),
                after_failure: None,
            },
            SourceEvent::RowAssigned {
                project,
                table: "bugs".into(),
                row: "R2".into(),
                assignee: user(),
            },
        ];
        let kinds: Vec<Kind> = events
            .iter()
            .flat_map(from_event)
            .map(|derived| derived.kind)
            .collect();
        assert_eq!(kinds, Kind::ALL.to_vec(), "五类都要真的触发得出来");
    }

    #[test]
    fn 自由文本原样引用() {
        let reason = "写入者不在允许名单里：\"alice\" <script>alert(1)</script> 🙂";
        let derived = from_event(&SourceEvent::RowNotSettled {
            project: ProjectId::generate(),
            instance: "I1".into(),
            table: "approvals".into(),
            row: "R1".into(),
            writer: user(),
            reason: reason.into(),
        });
        assert!(
            derived[0].text.contains(reason),
            "原样引用 —— 不改写、不摘要、不翻译"
        );
    }

    #[test]
    fn 超长的自由文本截断而不摘要() {
        let long = "很".repeat(QUOTE_MAX_CHARS + 50);
        let quoted = quote(&long);
        assert!(quoted.ends_with(TRUNCATION_MARK), "截断要标注出来");
        assert_eq!(
            quoted.chars().count(),
            QUOTE_MAX_CHARS + TRUNCATION_MARK.chars().count()
        );
        assert!(quoted.starts_with("很很很"), "前面那段是原文，不是摘要");
    }

    #[test]
    fn 后处理失败通知任务所有者而且不动执行本身的结论() {
        // TSK-012：`_runs` 那一行已经写好了，后处理失败**只留自己的痕迹**。
        let owner = user();
        let derived = from_event(&SourceEvent::RunFinished {
            project: ProjectId::generate(),
            run: "RUN1".into(),
            task: "T1".into(),
            status: "succeeded".into(),
            owner,
            after_failure: Some("输出插件抛异常：连不上 hooks.example".into()),
        });
        assert_eq!(derived[0].recipients.0, vec![owner], "通知任务所有者");
        assert!(derived[0].text.contains("succeeded"), "执行本身仍然是成功");
        assert!(
            derived[0].text.contains("连不上 hooks.example"),
            "痕迹原样带上"
        );
    }

    #[test]
    fn 执行结束的通知里只有指针() {
        // 构造一次含凭据形状内容的事件 —— **它没有地方能把那些内容带进来**。
        let derived = from_event(&SourceEvent::RunFinished {
            project: ProjectId::generate(),
            run: "RUN1".into(),
            task: "T1".into(),
            status: "succeeded".into(),
            owner: user(),
            after_failure: None,
        });
        let text = &derived[0].text;
        assert!(text.contains("RUN1") && text.contains("_runs"), "只有指针");
        assert!(!text.contains("token") && !text.contains("ghp_"));
    }
}
