//! 审计域的 tool。
//!
//! `AUD-003` 是可见性要求，不是过滤器：查询按项目分区，**越权的行根本不进结果集**。
//! 这里的 `Requirement::InProject(ReadProject)` 就是它在协议层的那一半——
//! 不是成员的话，连这次调用都到不了查询。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_audit::{AuditLog, EventKind, Query, kinds};
use xops_core::{Result, Timestamp};
use xops_identity::{Action, Directory, ProjectId};

use crate::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use crate::schema::{Field, FieldType, Schema};

fn render(record: &xops_audit::AuditRecord) -> Value {
    json!({
        "event": record.id.to_string(),
        "at": record.at.as_millis(),
        "kind": record.envelope.kind.as_str(),
        "outcome": record.envelope.outcome,
        "target": record.envelope.target.to_string(),
        "table": record.table.as_str(),
        "data": record.envelope.data,
    })
}

fn base_schema() -> Schema {
    Schema::new()
        .field(Field::required("project", FieldType::Id, "项目标识"))
        .field(Field::optional(
            "kind",
            FieldType::Text { max_len: 64 },
            "事件类型",
        ))
        .field(Field::optional("since", FieldType::Timestamp, "起"))
        .field(Field::optional("until", FieldType::Timestamp, "止"))
        .field(Field::optional("limit", FieldType::Integer, "最多几条"))
}

fn build_query(context: &CallContext<'_>, project: ProjectId) -> Result<Query> {
    let mut query = Query::in_project(project.as_id(), context.identity.user.id.as_id());
    if let Some(kind) = context.arg("kind").and_then(Value::as_str) {
        query = query.of_kind(EventKind::new(kind)?);
    }
    if let (Some(since), Some(until)) = (
        context.arg("since").and_then(Value::as_i64),
        context.arg("until").and_then(Value::as_i64),
    ) {
        query = query.between(Timestamp::from_millis(since), Timestamp::from_millis(until));
    }
    if let Some(limit) = context.arg("limit").and_then(Value::as_i64) {
        query = query.limit(usize::try_from(limit).unwrap_or(100).min(1_000));
    }
    Ok(query)
}

pub struct QueryEvents {
    spec: ToolSpec,
    directory: Arc<Directory>,
    audit: Arc<AuditLog>,
}

impl QueryEvents {
    /// # Errors
    /// 声明不合形状。
    pub fn new(directory: Arc<Directory>, audit: Arc<AuditLog>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("audit.query")
                .summary("查询项目事件流：按类型、时间范围")
                .input(base_schema())
                .requires(Requirement::InProject(Action::ReadProject))
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::CALL_REJECTED)
                .build()?,
            directory,
            audit,
        })
    }
}

impl Tool for QueryEvents {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let project = require_project(context)?;
        self.directory.project(context.identity.user.id, project)?;
        let records = self.audit.query(&build_query(context, project)?)?;
        Ok(json!({"events": records.iter().map(render).collect::<Vec<_>>()}))
    }
}

pub struct ObjectHistory {
    spec: ToolSpec,
    directory: Arc<Directory>,
    audit: Arc<AuditLog>,
}

impl ObjectHistory {
    /// # Errors
    /// 声明不合形状。
    pub fn new(directory: Arc<Directory>, audit: Arc<AuditLog>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("audit.history")
                .summary("查询某个对象的完整历史")
                .input(base_schema().field(Field::required("target", FieldType::Id, "对象标识")))
                .requires(Requirement::InProject(Action::ReadProject))
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::CALL_REJECTED)
                .build()?,
            directory,
            audit,
        })
    }
}

impl Tool for ObjectHistory {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let project = require_project(context)?;
        self.directory.project(context.identity.user.id, project)?;
        let records = self
            .audit
            .history(context.id("target")?, &build_query(context, project)?)?;
        Ok(json!({"events": records.iter().map(render).collect::<Vec<_>>()}))
    }
}

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| xops_core::Error::internal("项目级 tool 却没有项目"))
}

/// 注册审计域。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(
    registry: &mut Registry,
    directory: &Arc<Directory>,
    audit: &Arc<AuditLog>,
) -> Result<()> {
    registry.register(Arc::new(QueryEvents::new(
        Arc::clone(directory),
        Arc::clone(audit),
    )?))?;
    registry.register(Arc::new(ObjectHistory::new(
        Arc::clone(directory),
        Arc::clone(audit),
    )?))?;
    Ok(())
}
