//! 流程定义与实例的读写面。

use std::sync::Arc;

use xops_audit::{AuditEnvelope, AuditLog};
use xops_core::{Actor, Clock, Error, Result, RowId, TableName, Timestamp, WriteOp};
use xops_identity::{Action, Directory, ProjectId, UserId};
use xops_store::{Row, Store, WriteEngine, WriteRequest, keys, space};
use xops_table::{TableId, Tables};

use crate::definition::{Definition, FlowId, Start, State, Step};
use crate::instance::{Instance, InstanceId, InstanceState, NodeRun, NodeState, Subject};
use crate::validate::require_valid;

/// 流程定义与实例落在这两张平台表上。
pub const FLOWS_TABLE: &str = "_flow_defs";
pub const INSTANCES_TABLE: &str = "_flow_instances";

/// 事件类型。
pub mod kinds {
    pub const FLOW_DEFINED: &str = "flow.defined";
    pub const FLOW_DISABLED: &str = "flow.disabled";
    pub const INSTANCE_STARTED: &str = "flow.instance-started";
    pub const NODE_ACTIVATED: &str = "flow.node-activated";
    pub const INSTANCE_ENDED: &str = "flow.instance-ended";
}

/// 「节点被激活」事件的载荷（`TRG-018`）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NodeActivated {
    pub instance: String,
    pub flow: String,
    pub version: u32,
    pub node: String,
    pub started_by: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub subject_revision: Option<String>,
}

/// 流程。
pub struct Flows {
    engine: Arc<WriteEngine>,
    store: Arc<dyn Store>,
    audit: Arc<AuditLog>,
    directory: Arc<Directory>,
    tables: Arc<Tables>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Flows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flows").finish_non_exhaustive()
    }
}

impl Flows {
    #[must_use]
    pub fn new(
        engine: Arc<WriteEngine>,
        store: Arc<dyn Store>,
        audit: Arc<AuditLog>,
        directory: Arc<Directory>,
        tables: Arc<Tables>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            engine,
            store,
            audit,
            directory,
            tables,
            clock,
        }
    }

    /// 校验一条定义。**不落库**（`FLW-008`）。
    ///
    /// # Errors
    /// 没权限，或者形状本身就不对。
    pub fn check(&self, actor: UserId, definition: &Definition) -> Result<Vec<crate::Finding>> {
        self.directory
            .authorize(actor, definition.project, Action::DefineFlow)?;
        crate::validate::validate(definition)
    }

    /// 定义一条流程（或发布新版本）。
    ///
    /// # Errors
    /// 没权限 · 校验不过 · 结算表 / 主体表不存在。
    pub fn define(&self, actor: UserId, mut definition: Definition) -> Result<Definition> {
        self.directory
            .authorize(actor, definition.project, Action::DefineFlow)?;
        require_valid(&definition)?;
        // 表得真的存在。
        self.tables
            .describe(actor, definition.project, &definition.settlement_table)?;
        if let Some(subject) = &definition.subject_table {
            self.tables.describe(actor, definition.project, subject)?;
        }
        definition.version = self
            .versions(definition.flow)?
            .iter()
            .map(|version| version.version)
            .max()
            .unwrap_or(0)
            + 1;
        definition.created_by = actor;
        definition.created_at = self.clock.now();
        definition.state = State::Published;
        self.put_definition(&definition, kinds::FLOW_DEFINED, WriteOp::Insert, actor)?;
        Ok(definition)
    }

    /// 停用一个版本。**不能再发起新实例，在途实例继续执行完**（`FLW-006`）。
    ///
    /// # Errors
    /// 没权限 · 没有这个版本。
    pub fn disable(&self, actor: UserId, flow: FlowId, version: u32) -> Result<Definition> {
        let mut definition = self.definition(flow, version)?;
        self.directory
            .authorize(actor, definition.project, Action::DefineFlow)?;
        definition.state = State::Disabled;
        self.put_definition(&definition, kinds::FLOW_DISABLED, WriteOp::Update, actor)?;
        Ok(definition)
    }

    /// 发起一个实例。
    ///
    /// **实例创建的同一步，第一个节点随即激活**并产生「节点被激活」事件（`FLW-011`）。
    ///
    /// # Errors
    /// 没权限 · 流程已停用 · 没有这个版本。
    pub fn start(
        &self,
        actor: UserId,
        flow: FlowId,
        version: u32,
        subject: Subject,
        expires_at: Option<Timestamp>,
    ) -> Result<Instance> {
        let definition = self.definition(flow, version)?;
        self.directory
            .authorize(actor, definition.project, Action::ParticipateFlow)?;
        if definition.state == State::Disabled {
            return Err(Error::invalid("这个流程版本已停用，发不了新实例"));
        }
        self.start_with(&definition, actor, subject, expires_at)
    }

    /// 随行自动发起（`FLW-009`）：**主体表插入一条新行时**，主体就是那一行。
    ///
    /// ⚠️ `FLW-010`：**结算行的插入永远不会再开出一个实例**——随行发起只看主体表。
    /// 调用方（RP-15 的写入区间）负责只在主体表的 insert 上叫它。
    ///
    /// # Errors
    /// 没有已发布的版本，或者底层写失败。
    pub fn start_automatically(
        &self,
        definition: &Definition,
        by: UserId,
        subject: Subject,
    ) -> Result<Option<Instance>> {
        if definition.start != Start::Automatic || definition.state == State::Disabled {
            return Ok(None);
        }
        self.start_with(definition, by, subject, None).map(Some)
    }

    fn start_with(
        &self,
        definition: &Definition,
        actor: UserId,
        subject: Subject,
        expires_at: Option<Timestamp>,
    ) -> Result<Instance> {
        let now = self.clock.now();
        let id = InstanceId::generate();
        let mut nodes = Vec::new();
        for (step, entry) in definition.steps.iter().enumerate() {
            for node in entry.activation_set() {
                nodes.push(NodeRun {
                    instance: id,
                    step,
                    node: node.name.clone(),
                    state: NodeState::Inactive,
                    activated_at: None,
                    settled_at: None,
                    settled_by: Vec::new(),
                });
            }
        }
        let mut instance = Instance {
            id,
            project: definition.project,
            flow: definition.flow,
            // FLW-007：**发起时的版本，之后不受版本变更影响。**
            version: definition.version,
            subject,
            started_by: actor,
            state: InstanceState::Running,
            started_at: now,
            ended_at: None,
            expires_at,
            step: 0,
            nodes,
        };
        // FLW-011：同一步激活第一个节点。
        let activated = instance.activate_step(0, now);
        self.put_instance(&instance, kinds::INSTANCE_STARTED, WriteOp::Insert)?;
        self.announce(&instance, definition, &activated)?;
        Ok(instance)
    }

    /// 为「节点被激活」逐个产生事件（`TRG-018`）。
    fn announce(
        &self,
        instance: &Instance,
        definition: &Definition,
        nodes: &[String],
    ) -> Result<()> {
        for node in nodes {
            let payload = NodeActivated {
                instance: instance.id.to_string(),
                flow: definition.flow.to_string(),
                version: definition.version,
                node: node.clone(),
                started_by: instance.started_by.to_string(),
                subject_kind: instance.subject.kind.clone(),
                subject_id: instance.subject.id.clone(),
                subject_revision: instance.subject.revision.clone(),
            };
            let envelope = AuditEnvelope::project_scoped(
                kinds::NODE_ACTIVATED,
                instance.project.as_id(),
                instance.id.as_id(),
                serde_json::to_value(&payload)
                    .map_err(|error| Error::internal(format!("载荷装不下：{error}")))?,
            )?;
            self.audit.append(&Actor::Platform, &envelope)?;
        }
        Ok(())
    }

    /// 存一个实例的当前状态。**RP-15 驱动完迁移之后调它。**
    ///
    /// # Errors
    /// 底层写失败。
    pub fn save(&self, instance: &Instance) -> Result<()> {
        let kind = if instance.state.is_terminal() {
            kinds::INSTANCE_ENDED
        } else {
            kinds::INSTANCE_STARTED
        };
        self.put_instance(instance, kind, WriteOp::Update)
    }

    /// 推进一个实例：这一步全通过就激活下一步，并发事件。
    ///
    /// **这是 RP-15 唯一该用的推进入口**——它不得自己去改 `_flows` / `_flow_nodes`。
    ///
    /// # Errors
    /// 实例已终态或底层写失败。
    pub fn advance(&self, instance: &mut Instance) -> Result<Vec<String>> {
        let definition = self.definition(instance.flow, instance.version)?;
        let activated = instance.advance(definition.steps.len(), self.clock.now())?;
        self.save(instance)?;
        self.announce(instance, &definition, &activated)?;
        Ok(activated)
    }

    /// 取消一个实例。
    ///
    /// # Errors
    /// 没权限 · 实例不存在 · 已经是终态。
    pub fn cancel(&self, actor: UserId, instance: InstanceId) -> Result<Instance> {
        let mut record = self.instance(instance)?;
        self.directory
            .authorize(actor, record.project, Action::ManageBusinessObject)?;
        record.cancel(self.clock.now())?;
        self.save(&record)?;
        Ok(record)
    }

    /// 把过期的实例收掉（`FLW-017`）。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn expire_due(&self, now: Timestamp) -> Result<usize> {
        let mut expired = 0;
        for mut instance in self.all_instances()? {
            if instance.state.is_terminal() {
                continue;
            }
            if instance.expires_at.is_some_and(|at| at <= now) {
                instance.expire(now)?;
                self.save(&instance)?;
                expired += 1;
            }
        }
        Ok(expired)
    }

    /// 查一个实例。
    ///
    /// # Errors
    /// 非成员看到的与不存在一致。
    pub fn status(&self, viewer: UserId, instance: InstanceId) -> Result<Instance> {
        let record = self.instance(instance)?;
        self.directory
            .authorize(viewer, record.project, Action::ReadProject)?;
        Ok(record)
    }

    /// **跨项目聚合**：我待处理的流程节点（`FLW-016`）。
    ///
    /// 判"这个人是不是这个节点的允许写入者"归 RP-15；本包给的是
    /// **他看得见的项目里，此刻激活着的那些节点**。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn pending_for(&self, user: UserId) -> Result<Vec<(Instance, String)>> {
        let mut out = Vec::new();
        let projects: Vec<ProjectId> = self
            .directory
            .my_projects(user)?
            .into_iter()
            .map(|(project, _)| project.id)
            .collect();
        for instance in self.all_instances()? {
            if instance.state.is_terminal() || !projects.contains(&instance.project) {
                continue;
            }
            for node in instance.active() {
                out.push((instance.clone(), node.node.clone()));
            }
        }
        Ok(out)
    }

    /// 一条流程的全部版本。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn versions(&self, flow: FlowId) -> Result<Vec<Definition>> {
        Ok(self
            .all::<Definition>(FLOWS_TABLE)?
            .into_iter()
            .filter(|definition| definition.flow == flow)
            .collect())
    }

    /// 项目里的流程定义。
    ///
    /// # Errors
    /// 非成员看不到。
    pub fn list(&self, viewer: UserId, project: ProjectId) -> Result<Vec<Definition>> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        Ok(self
            .all::<Definition>(FLOWS_TABLE)?
            .into_iter()
            .filter(|definition| definition.project == project)
            .collect())
    }

    /// 引用了这张表（作结算表或主体表）的流程。**RP-04 的删表判定要问它**（`TBL-026`）。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn referencing(&self, project: ProjectId, table: &TableId) -> Result<Vec<Definition>> {
        Ok(self
            .all::<Definition>(FLOWS_TABLE)?
            .into_iter()
            .filter(|definition| definition.project == project)
            .filter(|definition| {
                definition.settlement_table == *table
                    || definition.subject_table.as_ref() == Some(table)
            })
            .collect())
    }

    /// 取一个版本的定义。
    ///
    /// # Errors
    /// 没有这个版本。
    pub fn definition(&self, flow: FlowId, version: u32) -> Result<Definition> {
        self.versions(flow)?
            .into_iter()
            .find(|definition| definition.version == version)
            .ok_or_else(|| Error::not_found("不存在"))
    }

    /// 取一个实例。
    ///
    /// # Errors
    /// 不存在。
    pub fn instance(&self, instance: InstanceId) -> Result<Instance> {
        self.load::<Instance>(INSTANCES_TABLE, RowId::from_id(instance.as_id()))?
            .ok_or_else(|| Error::not_found("不存在"))
    }

    fn all_instances(&self) -> Result<Vec<Instance>> {
        self.all::<Instance>(INSTANCES_TABLE)
    }

    fn put_definition(
        &self,
        definition: &Definition,
        kind: &str,
        op: WriteOp,
        actor: UserId,
    ) -> Result<()> {
        let row = definition_row(definition);
        self.write(
            FLOWS_TABLE,
            row,
            definition.project,
            definition.flow.as_id(),
            kind,
            op,
            definition,
            &Actor::User {
                user: actor.to_string(),
            },
        )
    }

    fn put_instance(&self, instance: &Instance, kind: &str, op: WriteOp) -> Result<()> {
        self.write(
            INSTANCES_TABLE,
            RowId::from_id(instance.id.as_id()),
            instance.project,
            instance.id.as_id(),
            kind,
            op,
            instance,
            &Actor::Platform,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "内部写入辅助，摊平比包一层结构更好读"
    )]
    fn write<T: serde::Serialize>(
        &self,
        table: &str,
        row: RowId,
        project: ProjectId,
        target: xops_core::Id,
        kind: &str,
        op: WriteOp,
        value: &T,
        actor: &Actor,
    ) -> Result<()> {
        let envelope = AuditEnvelope::project_scoped(
            kind,
            project.as_id(),
            target,
            serde_json::to_value(value)
                .map_err(|error| Error::internal(format!("装不下：{error}")))?,
        )?;
        let receipt = self.engine.write(WriteRequest {
            table: TableName::new(table)?,
            op,
            row,
            payload: envelope.to_payload()?,
            actor: actor.clone(),
        })?;
        self.audit.index(&receipt)
    }

    fn load<T: serde::de::DeserializeOwned>(&self, table: &str, row: RowId) -> Result<Option<T>> {
        let table = TableName::new(table)?;
        let Some(record) = self.engine.read(&table, row)? else {
            return Ok(None);
        };
        let Some(envelope) = AuditEnvelope::from_payload(&record.payload) else {
            return Err(Error::internal("行不是一个审计信封"));
        };
        serde_json::from_value(envelope.data)
            .map(Some)
            .map_err(|error| Error::internal(format!("读不回来：{error}")))
    }

    fn all<T: serde::de::DeserializeOwned>(&self, table: &str) -> Result<Vec<T>> {
        let table = TableName::new(table)?;
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
                if let Ok(value) = serde_json::from_value::<T>(envelope.data) {
                    out.push(value);
                }
            }
        }
        Ok(out)
    }
}

/// 定义的行标识由 `(流程, 版本)` 定死——同一个版本的每次状态变更落在同一行上。
fn definition_row(definition: &Definition) -> RowId {
    let seed = format!("{}#{}", definition.flow, definition.version);
    let low = fnv1a(seed.as_bytes());
    let high = fnv1a(&low.to_be_bytes());
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&high.to_be_bytes());
    bytes[8..].copy_from_slice(&low.to_be_bytes());
    RowId::from_id(
        xops_core::Id::parse(&encode(bytes))
            .unwrap_or_else(|_| xops_core::Id::from_parts(0, u128::from(low))),
    )
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

fn encode(bytes: [u8; 16]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let value = u128::from_be_bytes(bytes) >> 3;
    let mut out = [0u8; 26];
    for (index, slot) in out.iter_mut().enumerate() {
        let shift = 5 * (26 - 1 - index);
        *slot = ALPHABET[usize::try_from((value >> shift) & 0x1F).unwrap_or(0)];
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 让 `Step` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _StepLink = Step;
