//! 任务的读写面。
//!
//! 创建时要校验的东西比看起来多，而**每一条都在创建时挡住，不留到运行时**——
//! 运行时才发现"引用的是草稿技能"或者"onComplete 套了两层"，那时候已经跑起来了。

use std::sync::Arc;

use xops_audit::{AuditEnvelope, AuditLog};
use xops_core::{Actor, Clock, Error, Result, RowId, TableName, WriteOp};
use xops_identity::{Action, Directory, ProjectId, UserId};
use xops_skill::{Ownership, Skills, State};
use xops_store::{Row, Store, WriteEngine, WriteRequest, keys, space};

use crate::policy::{OnComplete, VersionPolicy};
use crate::task::{Task, TaskId};

/// 任务落在这张平台表上。
pub const TASKS_TABLE: &str = "_tasks";

/// 事件类型。
pub mod kinds {
    pub const TASK_CREATED: &str = "task.created";
    pub const TASK_UPDATED: &str = "task.updated";
    pub const TASK_ENABLED: &str = "task.enabled";
    pub const TASK_DISABLED: &str = "task.disabled";
}

/// 任务定义与执行策略。
pub struct Tasks {
    engine: Arc<WriteEngine>,
    store: Arc<dyn Store>,
    audit: Arc<AuditLog>,
    directory: Arc<Directory>,
    skills: Arc<Skills>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Tasks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tasks").finish_non_exhaustive()
    }
}

impl Tasks {
    #[must_use]
    pub fn new(
        engine: Arc<WriteEngine>,
        store: Arc<dyn Store>,
        audit: Arc<AuditLog>,
        directory: Arc<Directory>,
        skills: Arc<Skills>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            engine,
            store,
            audit,
            directory,
            skills,
            clock,
        }
    }

    /// 建一个任务。
    ///
    /// # Errors
    /// 没权限 · 引用了草稿技能（`TSK-002`）· 输入不满足技能的输入契约（`TSK-003`，
    /// **错误会指明缺哪个参数**）· `onComplete` 套了两层（`TSK-011`）· 形状不合法。
    pub fn create(&self, actor: UserId, mut task: Task) -> Result<Task> {
        self.directory
            .authorize(actor, task.project, Action::WriteTask)?;
        task.created_by = actor;
        task.created_at = self.clock.now();
        if let Ownership::Private { owner } = task.ownership
            && owner != actor
        {
            return Err(Error::invalid("只能给自己建私有任务"));
        }
        task.check()?;
        self.check_skill(actor, &task)?;
        self.check_on_complete(&task)?;
        self.put(&task, kinds::TASK_CREATED, WriteOp::Insert, actor)?;
        Ok(task)
    }

    /// 改一个任务。**校验与创建时同一套。**
    ///
    /// # Errors
    /// 同 [`Self::create`]。
    pub fn update(&self, actor: UserId, task: Task) -> Result<Task> {
        let existing = self.require_writable(actor, task.id)?;
        let task = Task {
            created_by: existing.created_by,
            created_at: existing.created_at,
            ..task
        };
        task.check()?;
        self.check_skill(actor, &task)?;
        self.check_on_complete(&task)?;
        self.put(&task, kinds::TASK_UPDATED, WriteOp::Update, actor)?;
        Ok(task)
    }

    /// 启用 / 停用（`TSK-009`）。**不提供删除**——执行记录不能因为任务没了就丢。
    ///
    /// # Errors
    /// 没权限 · 看不到。
    pub fn set_enabled(&self, actor: UserId, task: TaskId, enabled: bool) -> Result<Task> {
        let mut record = self.require_writable(actor, task)?;
        record.enabled = enabled;
        let kind = if enabled {
            kinds::TASK_ENABLED
        } else {
            kinds::TASK_DISABLED
        };
        self.put(&record, kind, WriteOp::Update, actor)?;
        Ok(record)
    }

    /// 读一个任务。
    ///
    /// # Errors
    /// 看不到——**与不存在一致**。
    pub fn read(&self, viewer: UserId, task: TaskId) -> Result<Task> {
        let record = self.load(task)?.ok_or_else(|| Error::not_found("不存在"))?;
        if self.directory.role_of(record.project, viewer)?.is_none() {
            return Err(Error::not_found("不存在"));
        }
        if let Ownership::Private { owner } = record.ownership
            && owner != viewer
        {
            // TSK-013：SKL-011 那条例外同样适用 —— 被用于满足流程节点的，转为可读。
            // 那个标记打在技能版本上，这里跟着它走。
            let readable = self
                .skills
                .versions(record.skill)?
                .iter()
                .any(|version| version.used_for_settlement);
            if !readable {
                return Err(Error::not_found("不存在"));
            }
        }
        Ok(record)
    }

    /// 列出我看得见的任务。
    ///
    /// # Errors
    /// 非成员看不到这个项目。
    pub fn list(&self, viewer: UserId, project: ProjectId) -> Result<Vec<Task>> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        Ok(self
            .all()?
            .into_iter()
            .filter(|task| task.project == project)
            .filter(|task| self.read(viewer, task.id).is_ok())
            .collect())
    }

    /// 订阅了某个事件、且此刻响应触发的那些任务。RP-11 分发时用它。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn subscribers(&self, project: ProjectId, event: &str) -> Result<Vec<Task>> {
        Ok(self
            .all()?
            .into_iter()
            .filter(|task| task.project == project)
            .filter(Task::responds_to_triggers)
            .filter(|task| {
                task.subscriptions
                    .iter()
                    .any(|subscribed| subscribed == event)
            })
            .collect())
    }

    /// 解出这次要跑技能的哪个版本。
    ///
    /// # Errors
    /// 任务不存在 · 技能这会儿跑不了（草稿、已停用、或者私有技能的所有者已退出项目）。
    pub fn resolve_skill_version(&self, task: &Task) -> Result<u32> {
        let versions = self.skills.versions(task.skill)?;
        let version = match task.version_policy {
            VersionPolicy::Pinned { version } => version,
            VersionPolicy::Latest => versions
                .iter()
                .filter(|version| version.state == State::Published)
                .map(|version| version.version)
                .max()
                .ok_or_else(|| Error::invalid("这个技能还没有已发布的版本"))?,
        };
        // SKL-009 那条：**每次现算**，不缓存。
        if !self.skills.runnable_for(task.skill, version)? {
            return Err(Error::invalid(
                "这个技能版本现在跑不了：可能是草稿、已停用，\
                 或者它是私有技能而所有者已经不是项目成员（SKL-009）",
            ));
        }
        Ok(version)
    }

    // ——————————————————————————————— 校验 ———————————————————————————————

    fn check_skill(&self, actor: UserId, task: &Task) -> Result<()> {
        let resolved = self.skills.read(actor, task.skill)?;
        if resolved.skill.project != task.project {
            // SKL-012：跨项目复用不做（Q14）。
            return Err(Error::invalid(
                "技能不在这个项目里（跨项目复用不做，SKL-012）",
            ));
        }
        let versions = self.skills.versions(task.skill)?;
        let version = match task.version_policy {
            VersionPolicy::Pinned { version } => versions
                .iter()
                .find(|candidate| candidate.version == version)
                .ok_or_else(|| Error::invalid(format!("技能没有第 {version} 版")))?,
            VersionPolicy::Latest => versions
                .iter()
                .filter(|version| version.state == State::Published)
                .max_by_key(|version| version.version)
                .ok_or_else(|| Error::invalid("这个技能还没有已发布的版本"))?,
        };
        if version.state != State::Published {
            return Err(Error::invalid(
                "任务只能引用已发布的技能版本，草稿不行（TSK-002）",
            ));
        }
        // TSK-003：不满足输入契约时**指明缺哪个参数**。
        version.declaration.check_arguments(&task.inputs)?;
        Ok(())
    }

    /// `TSK-011`：**深度硬限制 1。**
    ///
    /// > 一层是"输出后处理"，两层就是任务编排 DAG，随之而来的是依赖解析、失败传播、
    /// > 循环检测及其可视化。
    ///
    /// 两个方向都要挡：我挂的那个任务自己不能再挂；我自己被别人挂着的话，我也不能挂。
    fn check_on_complete(&self, task: &Task) -> Result<()> {
        if let Some(target) = task.on_complete.task() {
            if target == task.id {
                return Err(Error::invalid("任务不能把自己挂在自己的 onComplete 上"));
            }
            let downstream = self
                .load(target)?
                .ok_or_else(|| Error::invalid("挂的那个任务不存在"))?;
            if !downstream.on_complete.is_none() {
                return Err(Error::invalid(
                    "被挂在 onComplete 上的任务，它自己的 onComplete 必须为空（TSK-011：深度硬限制 1）",
                ));
            }
        }
        if !task.on_complete.is_none() {
            let hooked_by_someone = self
                .all()?
                .iter()
                .any(|other| other.id != task.id && other.on_complete.task() == Some(task.id));
            if hooked_by_someone {
                return Err(Error::invalid(
                    "这个任务已经被别人挂在 onComplete 上了，它自己不能再挂（TSK-011）",
                ));
            }
        }
        Ok(())
    }

    fn require_writable(&self, actor: UserId, task: TaskId) -> Result<Task> {
        let record = self.read(actor, task)?;
        self.directory
            .authorize(actor, record.project, Action::WriteTask)?;
        if let Ownership::Private { owner } = record.ownership
            && owner != actor
        {
            return Err(Error::not_found("不存在"));
        }
        Ok(record)
    }

    fn load(&self, task: TaskId) -> Result<Option<Task>> {
        let table = TableName::new(TASKS_TABLE)?;
        let Some(row) = self.engine.read(&table, RowId::from_id(task.as_id()))? else {
            return Ok(None);
        };
        let Some(envelope) = AuditEnvelope::from_payload(&row.payload) else {
            return Err(Error::internal("任务不是一个审计信封"));
        };
        serde_json::from_value(envelope.data)
            .map(Some)
            .map_err(|error| Error::internal(format!("任务读不回来：{error}")))
    }

    fn all(&self) -> Result<Vec<Task>> {
        let table = TableName::new(TASKS_TABLE)?;
        let prefix = keys::table_prefix(&table);
        let mut out = Vec::new();
        let mut cursor: Option<Vec<u8>> = None;
        loop {
            let page = self
                .store
                .scan(space::ROW, &prefix, cursor.as_deref(), 256)?;
            if page.is_empty() {
                break;
            }
            cursor = page.last().map(|(key, _)| key.clone());
            for (_, bytes) in page {
                let row: Row = serde_json::from_slice(&bytes)
                    .map_err(|error| Error::internal(format!("投影读不回来：{error}")))?;
                if row.is_deleted() {
                    continue;
                }
                let Some(envelope) = AuditEnvelope::from_payload(&row.payload) else {
                    continue;
                };
                if let Ok(task) = serde_json::from_value::<Task>(envelope.data) {
                    out.push(task);
                }
            }
        }
        Ok(out)
    }

    fn put(&self, task: &Task, kind: &str, op: WriteOp, actor: UserId) -> Result<()> {
        let envelope = AuditEnvelope::project_scoped(
            kind,
            task.project.as_id(),
            task.id.as_id(),
            serde_json::to_value(task)
                .map_err(|error| Error::internal(format!("任务装不下：{error}")))?,
        )?;
        let receipt = self.engine.write(WriteRequest {
            table: TableName::new(TASKS_TABLE)?,
            op,
            row: RowId::from_id(task.id.as_id()),
            payload: envelope.to_payload()?,
            actor: Actor::User {
                user: actor.to_string(),
            },
        })?;
        self.audit.index(&receipt)
    }
}

/// 让 `OnComplete` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _OnCompleteLink = OnComplete;
