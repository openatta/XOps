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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
    /// 这一次不提交：重叠策略（`TSK-008`）或并发已满（`EXE-027`）。
    /// **任务本身没问题**——这是它与 `Rejected` 的分界。
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
    pub(crate) tasks: Arc<Tasks>,
    skills: Arc<Skills>,
    exec: Arc<dyn ExecContract>,
    audit: Arc<AuditLog>,
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
    /// 需要代码仓时，工作区从哪来。**没接就等于"不提供"**（`I-I`）。
    workspaces: Option<Arc<dyn WorkspaceSource>>,
    /// 并发名额（`EXE-027`）。**没接就等于不限**——所以装配层必须接。
    slots: Option<Arc<Slots>>,
}

/// 名额的持有处。
///
/// `Permit` 是析构即归还的，可这里没有一个"跟着这次执行活着"的对象可以放它:
/// 提交是非阻塞的，跑完由 [`Reaper`] 在另一轮里发现。
/// 所以名额按 run 存着，**落账的那一刻还回去**——
/// 归还点和"这次执行结束了"是同一件事，不会各算各的。
pub struct Slots {
    concurrency: Arc<xops_task::Concurrency>,
    held: Mutex<HashMap<String, xops_task::Permit>>,
}

impl std::fmt::Debug for Slots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Slots")
            .field(
                "held",
                &self.held.lock().map(|held| held.len()).unwrap_or(0),
            )
            .finish_non_exhaustive()
    }
}

impl Slots {
    #[must_use]
    pub fn new(concurrency: Arc<xops_task::Concurrency>) -> Self {
        Self {
            concurrency,
            held: Mutex::new(HashMap::new()),
        }
    }

    fn take(&self, project: xops_identity::ProjectId) -> Option<xops_task::Permit> {
        self.concurrency.acquire(project)
    }

    /// 名额跟着这个 run 存起来。
    fn keep(&self, run: &str, permit: xops_task::Permit) {
        self.locked().insert(run.to_owned(), permit);
    }

    /// 还回去。**没有这一条就等于上限只减不增**——第一批跑完之后平台就再也接不了活。
    fn give_back(&self, run: &str) {
        self.locked().remove(run);
    }

    /// 此刻占着几个名额。
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.concurrency.in_flight()
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, HashMap<String, xops_task::Permit>> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
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
            slots: None,
        }
    }

    /// 接上并发名额（`EXE-027`）。
    #[must_use]
    pub fn with_concurrency(mut self, slots: Arc<Slots>) -> Self {
        self.slots = Some(slots);
        self
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

        // EXE-027：名额先要到手再提交。
        // **要不到就不提交**——排队策略（`TSK-008` 的 Queue）说的"由执行层的并发上限
        // 兜着"兜的就是这里；没有队列可排，所以这一次落为跳过，下一次触发再来。
        let permit = match self.slots.as_ref() {
            Some(slots) => match slots.take(task.project) {
                Some(permit) => Some(permit),
                None => {
                    return Ok(Outcome::Skipped {
                        why: "并发已达上限（EXE-027），这次不提交".into(),
                    });
                }
            },
            None => None,
        };

        let worksheet = assemble(task, &version, event, workspace)?;
        let run = self.exec.submit(worksheet)?;
        if let (Some(slots), Some(permit)) = (self.slots.as_ref(), permit) {
            slots.keep(&run.to_string(), permit);
        }
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

    /// 全部"被接受过"的触发对应的执行。**收割器按它找该落账的那些。**
    ///
    /// # Errors
    /// 底层不可用。
    pub fn accepted_runs(&self) -> Result<Vec<(TaskId, String)>> {
        let mut out = Vec::new();
        let mut cursor: Option<Vec<u8>> = None;
        loop {
            let page = self
                .store
                .scan(TRIGGER_SPACE, &[], cursor.as_deref(), 256)?;
            if page.is_empty() {
                return Ok(out);
            }
            cursor = page.last().map(|(key, _)| key.clone());
            for (_, bytes) in page {
                let Ok(record) = serde_json::from_slice::<TriggerRecord>(&bytes) else {
                    continue;
                };
                if let Outcome::Accepted { run } = record.outcome {
                    out.push((record.task, run));
                }
            }
        }
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

/// 「发起一次测试执行」这条链（`SKL-003`）。
///
/// ⚠️ **它走的是与正式执行完全相同的那条路**:同一份派工单装配、同一个执行契约、
/// 同一个引擎。`SKL-003` 要的就是这个——"在与正式执行相同的隔离环境中进行"。
/// 另开一条更简单的路会让"测过了"这个事实变得不作数。
///
/// # 为什么它等着跑完
///
/// `EXE-021` 的"提交即返回"管的是**触发**（`run.trigger`）:那条路上没有人在等。
/// 测试执行是作者手动发起、要当场看结果的——**提交完就返回等于没有回答他的问题**。
/// 技能试跑。**也占名额**（`EXE-027`）——它跑的是真的执行，
/// 计不计数不该取决于是谁点的。它自己等到跑完，所以名额是个局部变量。
pub struct TestRuns {
    exec: Arc<dyn ExecContract>,
    workspaces: Option<Arc<dyn WorkspaceSource>>,
    clock: Arc<dyn Clock>,
    concurrency: Option<Arc<xops_task::Concurrency>>,
}

impl std::fmt::Debug for TestRuns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestRuns").finish_non_exhaustive()
    }
}

impl TestRuns {
    /// 接上并发名额（`EXE-027`）。
    #[must_use]
    pub fn with_concurrency(mut self, concurrency: Arc<xops_task::Concurrency>) -> Self {
        self.concurrency = Some(concurrency);
        self
    }

    #[must_use]
    pub fn new(exec: Arc<dyn ExecContract>, clock: Arc<dyn Clock>) -> Self {
        Self {
            exec,
            workspaces: None,
            concurrency: None,
            clock,
        }
    }

    #[must_use]
    pub fn with_workspaces(mut self, source: Arc<dyn WorkspaceSource>) -> Self {
        self.workspaces = Some(source);
        self
    }
}

impl xops_skill::service::TestRunner for TestRuns {
    fn run(
        &self,
        actor: UserId,
        version: &xops_skill::skill::Version,
        inputs: &serde_json::Value,
    ) -> Result<xops_skill::service::TestOutcome> {
        // ⚠️ **一个不落库的任务。** 测试执行不该在账上留下一个任务对象——
        // 它是作者的一次试跑，不是一条自动化。派工单装配要一个任务，所以这里造一个，
        // **但它从不经过 `Tasks` 写入任何地方**。
        let task = Task {
            id: TaskId::generate(),
            project: version.project,
            name: format!("{} 的测试执行", version.skill),
            ownership: xops_skill::Ownership::Private { owner: actor },
            kind: xops_task::task::Kind::Normal,
            skill: version.skill,
            version_policy: xops_task::policy::VersionPolicy::Pinned {
                version: version.version,
            },
            inputs: inputs.clone(),
            writes: Vec::new(),
            subscriptions: Vec::new(),
            token_budget: xops_task::policy::DEFAULT_TOKEN_BUDGET,
            overlap: xops_task::policy::Overlap::Skip,
            on_complete: xops_task::policy::OnComplete::None,
            enabled: true,
            created_by: actor,
            created_at: self.clock.now(),
        };

        let workspace = if version.declaration.needs_repository {
            let source = self.workspaces.as_ref().ok_or_else(|| {
                Error::unavailable("这个技能要读代码仓，而这个部署没有接工作区那条链")
            })?;
            Some(source.prepare(version.project, None)?)
        } else {
            None
        };

        let event = Event {
            kind: EventKind::Manual,
            project: version.project,
            external_id: None,
            triggered_by: Trigger::Person { user: actor },
            revision: None,
            at: self.clock.now(),
            payload: serde_json::json!({}),
        };
        // EXE-027：名额握在手里直到这次试跑收尾（下面等到跑完才返回）。
        let _permit = match self.concurrency.as_ref() {
            Some(concurrency) => Some(
                concurrency
                    .acquire(version.project)
                    .ok_or_else(|| Error::unavailable("并发已达上限（EXE-027），试跑请稍后再来"))?,
            ),
            None => None,
        };

        let worksheet = crate::worksheet::assemble(&task, version, &event, workspace)?;
        let timeout = worksheet.limits.timeout_millis;
        let run = self.exec.submit(worksheet)?;

        // 等它跑完。**上限就是这个技能自己声明的那个**——
        // 超了由执行运行时的看门狗归为超时（`EXE-019`），这里只是别比它先走。
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_millis(timeout + xops_exec::runtime::GRACE_MILLIS + 500);
        let outcome = loop {
            if let Some(done) = self.exec.collect(run)? {
                break done;
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::unavailable(
                    "测试执行超过了这个技能声明的时长上限，还没有收尾",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };

        let succeeded = outcome.status == xops_exec::Status::Succeeded;
        Ok(xops_skill::service::TestOutcome {
            run: run.to_string(),
            succeeded,
            detail: if succeeded {
                String::new()
            } else {
                format!(
                    "{:?}：{}",
                    outcome.failure,
                    outcome.trace.chars().take(500).collect::<String>()
                )
            },
            output: outcome.output,
        })
    }
}

/// 已经落过账的执行。**标记在这里，不在 `_runs` 上**——
/// 问"落过没有"要在写 `_runs` 之前答得出来。
const LANDED_SPACE: &str = "dispatch-landed";

/// 把跑完的执行落成账（`EXE-026`、`TSK-006`）。
///
/// # 它补的是哪个口子
///
/// 触发那条路是**非阻塞**的（`EXE-021`）:`run.trigger` 提交完就返回，没有人在等。
/// 于是"执行跑完之后谁把 `_runs` 那一行写下来"就成了一个**没有主人的问题**——
/// 而它不写，`_runs` 就是空的：执行成功了，账上什么也没有。
///
/// ⚠️ **这个口子是拿真模型跑端到端时撞出来的**:`run.status` 说 `succeeded`，
/// `row.sys-runs.select` 一行都没有。落账的实现（`xops_task::Landing`）一直都在，
/// **只是从来没有谁调用它**。
///
/// # 为什么是轮询
///
/// 执行在自己的线程上跑完，没有一个"完成"的信号能穿回来——
/// `ExecContract` 只有 `collect`（`EXE-014`:引擎的概念不得泄漏进契约，
/// 所以那里没有回调、没有通道）。**扫一遍是这条契约下唯一能做的事。**
pub struct Reaper {
    dispatcher: Arc<Dispatcher>,
    tasks: Arc<Tasks>,
    landing: Arc<xops_task::landing::Landing>,
    store: Arc<dyn Store>,
    exec: Arc<dyn ExecContract>,
    /// 落账即归还名额（`EXE-027`）。**要和 [`Dispatcher`] 拿的是同一个**。
    slots: Option<Arc<Slots>>,
    /// 执行结束要通知任务所有者（`NTF-007`、`EXE-024`）。**没接就等于不通知**——
    /// 而"自动化失灵是静默的"正是通知这条要挡的事。
    notices: Option<Arc<dyn RunNotifier>>,
}

/// 「执行结束了，通知一下」的注入位。RP-17 填它。
///
/// 本 crate 不认识通知，所以留一个位——与 `WorkspaceSource`、`SubscriptionCheck` 同形。
pub trait RunNotifier: Send + Sync + 'static {
    fn finished(&self, task: &Task, run: &str, status: &str);
}

impl std::fmt::Debug for Reaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reaper").finish_non_exhaustive()
    }
}

impl Reaper {
    #[must_use]
    pub fn new(
        dispatcher: Arc<Dispatcher>,
        tasks: Arc<Tasks>,
        landing: Arc<xops_task::landing::Landing>,
        store: Arc<dyn Store>,
        exec: Arc<dyn ExecContract>,
    ) -> Self {
        Self {
            dispatcher,
            tasks,
            landing,
            store,
            exec,
            slots: None,
            notices: None,
        }
    }

    /// 接上并发名额。**必须与 [`Dispatcher::with_concurrency`] 传同一个** ——
    /// 一个发一个收，分成两份就等于不限。
    #[must_use]
    pub fn with_concurrency(mut self, slots: Arc<Slots>) -> Self {
        self.slots = Some(slots);
        self
    }

    #[must_use]
    pub fn with_notices(mut self, notices: Arc<dyn RunNotifier>) -> Self {
        self.notices = Some(notices);
        self
    }

    /// 扫一遍，把跑完但还没落账的都落了。返回落了几笔。
    ///
    /// # Errors
    /// 底层不可用。**单笔落账失败不中断整轮**——一次写不进去不该让别的执行也落不了账。
    pub fn sweep(&self) -> Result<usize> {
        let mut landed = 0;
        for (task, run) in self.dispatcher.accepted_runs()? {
            let Ok(id) = xops_core::Id::parse(&run) else {
                continue;
            };
            if self.already_landed(&run)? {
                continue;
            }
            let Some(outcome) = self.exec.collect(RunId::from_id(id))? else {
                continue; // 还在跑。
            };
            match self.land_one(task, &outcome) {
                Ok(()) => {
                    self.mark_landed(&run)?;
                    // 跑完了，名额还回去。**在标记之后**——落账失败要重来，
                    // 那次重来还占着这个名额才对。
                    if let Some(slots) = self.slots.as_ref() {
                        slots.give_back(&run);
                    }
                    landed += 1;
                }
                Err(error) => {
                    // ⚠️ **不标记、不中断。** 下一轮再试——
                    // 而"某一笔一直落不下去"要看得见，所以它记一条。
                    xops_core::log::warn(
                        "dispatch.land",
                        &[("run", &run), ("error", &format!("{error}"))],
                    );
                }
            }
        }
        Ok(landed)
    }

    fn land_one(&self, task: TaskId, outcome: &xops_exec::Outcome) -> Result<()> {
        let task = self.tasks.read_internal(task)?;
        let version = self.dispatcher.tasks.resolve_skill_version(&task)?;
        let completion = xops_task::landing::Completion {
            run: outcome.run.as_id(),
            status: outcome.status.as_str().to_owned(),
            failure_kind: outcome.failure.map(|kind| kind.as_str().to_owned()),
            tokens_used: outcome.tokens_used,
            token_budget: task.token_budget,
            output: outcome.output.clone(),
            trace: outcome.trace.clone(),
            revision: None,
            skill: task.skill.to_string(),
            skill_version: version.to_string(),
            trigger: "manual".to_owned(),
            triggered_by: task.created_by.to_string(),
            started_at: outcome.started_at,
            finished_at: outcome.finished_at,
            rows: Vec::new(),
        };
        // 署名是**那次执行**，六项全内联（`TBL-016`）。
        let written_by = xops_table::WrittenBy::Execution {
            run: outcome.run.as_id(),
            task: task.id.as_id(),
            task_owner: task.created_by,
            skill: task.skill.to_string(),
            skill_version: version.to_string(),
            revision: None,
            status: outcome.status.as_str().to_owned(),
        };
        self.landing.land(
            &task,
            xops_task::retention::Retention::default(),
            &written_by,
            &completion,
        )?;
        // **账先落，再通知。** 反过来会出现"收到通知去看，账上还没有"。
        if let Some(notices) = &self.notices {
            notices.finished(&task, &outcome.run.to_string(), outcome.status.as_str());
        }
        Ok(())
    }

    fn already_landed(&self, run: &str) -> Result<bool> {
        Ok(self.store.get(LANDED_SPACE, run.as_bytes())?.is_some())
    }

    fn mark_landed(&self, run: &str) -> Result<()> {
        self.store.put(LANDED_SPACE, run.as_bytes(), b"1")
    }
}
