//! 事件分发与触发。
//!
//! 三条共同纪律（`TRG-007`），每条都有它防的那件事：
//!
//! ```text
//! 非阻塞  触发的产出是"一次执行进了队列"，不是"任务跑完了"
//! 幂等    同一个外部事件最多产生一次执行
//! 留痕    被拒绝的、被跳过的触发同样留痕
//!         —— 一个静默被跳过的任务，会让人以为它在跑
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use xops_audit::{AuditEnvelope, AuditLog};
use xops_core::{Actor, Clock, Error, Result, Timestamp};
use xops_exec::worksheet::RunId;
use xops_exec::{ExecContract, Status};
use xops_identity::UserId;
use xops_skill::Skills;
use xops_store::Store;
use xops_task::{Overlap, Task, TaskId, Tasks};

use crate::event::{Event, EventKind, Trigger};
use crate::worksheet::assemble;

/// 触发留痕的键空间。**被拒绝与被跳过的也在这儿**。
const TRIGGER_SPACE: &str = "trigger-log";
/// 幂等键的键空间。
const IDEMPOTENCY_SPACE: &str = "trigger-idempotency";

/// 事件类型。
pub mod kinds {
    pub const TRIGGER_ACCEPTED: &str = "trigger.accepted";
    pub const TRIGGER_REJECTED: &str = "trigger.rejected";
    pub const TRIGGER_SKIPPED: &str = "trigger.skipped";
}

/// 一次触发的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum Outcome {
    /// 进了队列。**这就是触发的全部产出**——不是"跑完了"。
    Accepted { run: String },
    /// 被拒绝：任务不存在 / 已停用 / 不允许这种触发方式（`TRG-008`）。
    Rejected { why: String },
    /// 被重叠策略跳过（`TSK-008`）。
    Skipped { why: String },
    /// 同一个外部事件已经产生过一次执行（`TRG-013`）。
    Duplicate { run: String },
}

/// 一条触发留痕。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerRecord {
    pub task: TaskId,
    pub kind: EventKind,
    pub at: Timestamp,
    pub outcome: Outcome,
}

/// 事件分发层。
///
/// **RP-13 往这里接两类事件源，RP-14 往这里塞「节点被激活」**——
/// 它们加的是事件的来源，不是新的事件类型。
pub struct Dispatcher {
    tasks: Arc<Tasks>,
    skills: Arc<Skills>,
    exec: Arc<dyn ExecContract>,
    audit: Arc<AuditLog>,
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
    /// 需要代码仓时，工作区从哪来。**没接就等于"不提供"**（`I-I`）。
    workspaces: Option<Arc<dyn WorkspaceSource>>,
}

/// 「按修订备一份只读工作区」的注入位。RP-08 填它。
///
/// 分开是因为 `TRG` 那条验收：**不依赖 RP-08 也能跑通**——
/// 声明"不需要代码仓"的技能，全链路正常。
pub trait WorkspaceSource: Send + Sync + 'static {
    /// # Errors
    /// 备不出来。
    fn prepare(&self, project: xops_identity::ProjectId, revision: Option<&str>)
    -> Result<PathBuf>;
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher")
            .field("workspaces", &self.workspaces.is_some())
            .finish_non_exhaustive()
    }
}

impl Dispatcher {
    #[must_use]
    pub fn new(
        tasks: Arc<Tasks>,
        skills: Arc<Skills>,
        exec: Arc<dyn ExecContract>,
        audit: Arc<AuditLog>,
        store: Arc<dyn Store>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            tasks,
            skills,
            exec,
            audit,
            store,
            clock,
            workspaces: None,
        }
    }

    /// 接上工作区来源。RP-08 用。
    #[must_use]
    pub fn with_workspaces(mut self, source: Arc<dyn WorkspaceSource>) -> Self {
        self.workspaces = Some(source);
        self
    }

    /// 分发一个事件：找出订阅它的任务，逐个触发。
    ///
    /// **非阻塞**：返回的是每个任务这次的结果，不是执行的结果。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn dispatch(&self, event: &Event) -> Result<Vec<TriggerRecord>> {
        let subscribers = self.tasks.subscribers(event.project, event.kind.as_str())?;
        subscribers
            .iter()
            .map(|task| self.trigger(task, event))
            .collect()
    }

    /// 触发一个具体的任务。
    ///
    /// # Errors
    /// 底层不可用。**"被拒绝"不是 `Err`**——它是一条有痕迹的结果。
    pub fn trigger(&self, task: &Task, event: &Event) -> Result<TriggerRecord> {
        let outcome = self.evaluate(task, event)?;
        let record = TriggerRecord {
            task: task.id,
            kind: event.kind,
            at: self.clock.now(),
            outcome: outcome.clone(),
        };
        self.record(task, event, &record)?;
        Ok(record)
    }

    fn evaluate(&self, task: &Task, event: &Event) -> Result<Outcome> {
        // TRG-008：任务存在（调用方已解出来）、已启用、允许这种触发方式。
        if !task.responds_to_triggers() {
            return Ok(Outcome::Rejected {
                why: "任务已停用，不响应任何触发（包括手动）".into(),
            });
        }
        if !self.allows(task, event.kind) {
            return Ok(Outcome::Rejected {
                why: format!("这个任务不接受 {} 这种触发方式", event.kind),
            });
        }

        // TRG-013：按外部事件标识幂等。
        if let Some(external) = event.external_id.as_deref()
            && let Some(previous) = self.previous_run(task.id, external)?
        {
            return Ok(Outcome::Duplicate { run: previous });
        }

        // TSK-008：重叠策略。
        if let Some(running) = self.running_run(task.id)? {
            match task.overlap {
                Overlap::Skip => {
                    return Ok(Outcome::Skipped {
                        why: format!("上一次执行 {running} 还没结束，按重叠策略跳过"),
                    });
                }
                Overlap::Restart => {
                    if let Ok(run) = xops_core::Id::parse(&running) {
                        self.exec.cancel(RunId::from_id(run))?;
                    }
                }
                Overlap::Queue => {
                    // 排队：这一版就是"照常提交"，由执行层的并发上限兜着。
                }
            }
        }

        let version_number = self.tasks.resolve_skill_version(task)?;
        let version = self
            .skills
            .versions(task.skill)?
            .into_iter()
            .find(|candidate| candidate.version == version_number)
            .ok_or_else(|| Error::internal("解出来的版本读不回来"))?;

        let workspace = if version.declaration.needs_repository {
            let source = self
                .workspaces
                .as_ref()
                .ok_or_else(|| Error::invalid("这个技能要读代码仓，但没有接工作区来源（RP-08）"))?;
            Some(source.prepare(task.project, event.revision.as_deref())?)
        } else {
            None
        };

        let worksheet = assemble(task, &version, event, workspace)?;
        let run = self.exec.submit(worksheet)?;
        self.remember(task.id, event.external_id.as_deref(), run)?;
        Ok(Outcome::Accepted {
            run: run.to_string(),
        })
    }

    /// 这个任务接不接受这种触发方式（`TRG-002` / `TRG-003`）。
    fn allows(&self, task: &Task, kind: EventKind) -> bool {
        match kind {
            // 前三类：任务自己声明订阅。手动触发不需要声明——它本来就是人点的。
            EventKind::Manual => true,
            EventKind::Scheduled | EventKind::Git => task
                .subscriptions
                .iter()
                .any(|subscribed| subscribed == kind.as_str()),
            // 后两类：**唯一的订阅途径不是声明**。
            // 「节点被激活」由 RP-15 指定写入者，「上游完成」由 onComplete 挂上。
            // 分发层到这里只认"调用方已经确认过那层关系"，所以放行。
            EventKind::FlowNodeActivated | EventKind::UpstreamTaskCompleted => true,
        }
    }

    fn record(&self, task: &Task, event: &Event, record: &TriggerRecord) -> Result<()> {
        let kind = match record.outcome {
            Outcome::Accepted { .. } | Outcome::Duplicate { .. } => kinds::TRIGGER_ACCEPTED,
            Outcome::Rejected { .. } => kinds::TRIGGER_REJECTED,
            Outcome::Skipped { .. } => kinds::TRIGGER_SKIPPED,
        };
        // 留痕先落自己的键空间 —— 触发历史要按任务查得到（`TSK-016`）。
        let key = format!("{}\u{0}{}", task.id, xops_core::Id::generate()).into_bytes();
        self.store.put(
            TRIGGER_SPACE,
            &key,
            &serde_json::to_vec(record)
                .map_err(|error| Error::internal(format!("留痕装不下：{error}")))?,
        )?;

        let envelope = AuditEnvelope::project_scoped(
            kind,
            task.project.as_id(),
            task.id.as_id(),
            serde_json::to_value(record)
                .map_err(|error| Error::internal(format!("留痕装不下：{error}")))?,
        )?;
        let envelope = if matches!(record.outcome, Outcome::Rejected { .. }) {
            envelope.rejected()
        } else {
            envelope
        };
        let actor = match &event.triggered_by {
            Trigger::Person { user }
            | Trigger::Schedule {
                configured_by: user,
            } => Actor::User {
                user: user.to_string(),
            },
            Trigger::External { .. } | Trigger::Platform { .. } => Actor::Platform,
        };
        self.audit.append(&actor, &envelope).map(|_| ())
    }

    /// 一个任务的触发历史，**含被拒绝与被跳过的**（`TSK-016`）。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn trigger_history(&self, task: TaskId) -> Result<Vec<TriggerRecord>> {
        let prefix = format!("{task}\u{0}").into_bytes();
        let mut out = Vec::new();
        let mut cursor: Option<Vec<u8>> = None;
        loop {
            let page = self
                .store
                .scan(TRIGGER_SPACE, &prefix, cursor.as_deref(), 256)?;
            if page.is_empty() {
                break;
            }
            cursor = page.last().map(|(key, _)| key.clone());
            for (_, bytes) in page {
                if let Ok(record) = serde_json::from_slice::<TriggerRecord>(&bytes) {
                    out.push(record);
                }
            }
        }
        Ok(out)
    }

    /// 这个任务此刻有没有还在跑的执行。
    fn running_run(&self, task: TaskId) -> Result<Option<String>> {
        for record in self.trigger_history(task)? {
            if let Outcome::Accepted { run } = record.outcome
                && let Ok(id) = xops_core::Id::parse(&run)
                && self
                    .exec
                    .status(RunId::from_id(id))
                    .is_ok_and(|status| status == Status::Running)
            {
                return Ok(Some(run));
            }
        }
        Ok(None)
    }

    fn previous_run(&self, task: TaskId, external: &str) -> Result<Option<String>> {
        let key = format!("{task}\u{0}{external}").into_bytes();
        Ok(self
            .store
            .get(IDEMPOTENCY_SPACE, &key)?
            .and_then(|bytes| String::from_utf8(bytes).ok()))
    }

    fn remember(&self, task: TaskId, external: Option<&str>, run: RunId) -> Result<()> {
        let Some(external) = external else {
            return Ok(());
        };
        let key = format!("{task}\u{0}{external}").into_bytes();
        self.store
            .put(IDEMPOTENCY_SPACE, &key, run.to_string().as_bytes())
    }
}

/// 让 `UserId` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _UserLink = UserId;
