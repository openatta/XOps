//! 流程域的 tool。
//!
//! ⚠️ **「为某实例的某节点写入一行」不在这里**——那是 RP-15 的，因为它是
//! "人做决定"的那条路，要判允许写入者与职责分离。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Error, Id, Result};
use xops_identity::{Action, ProjectId};
use xops_mcp::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};

use crate::definition::FlowId;
use crate::instance::{InstanceId, Subject};
use crate::service::{Flows, kinds};

fn project_field() -> Field {
    Field::required("project", FieldType::Id, "项目标识")
}

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| Error::internal("项目级 tool 却没有项目"))
}

macro_rules! flow_tool {
    ($name:ident, $tool:expr, $summary:expr, $input:expr, $action:expr, $idem:expr, $audit:expr, $body:expr) => {
        pub struct $name {
            spec: ToolSpec,
            flows: Arc<Flows>,
        }

        impl $name {
            /// # Errors
            /// 声明不合形状。
            pub fn new(flows: Arc<Flows>) -> Result<Self> {
                Ok(Self {
                    spec: ToolSpec::builder($tool)
                        .summary($summary)
                        .input($input)
                        .requires(Requirement::InProject($action))
                        .idempotency($idem)
                        .audits($audit)
                        .build()?,
                    flows,
                })
            }
        }

        impl Tool for $name {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }

            fn call(&self, context: &CallContext<'_>) -> Result<Value> {
                #[allow(clippy::redundant_closure_call)]
                ($body)(&self.flows, context)
            }
        }
    };
}

flow_tool!(
    StartInstance,
    "flow.start",
    "发起一个实例。**创建的同一步第一个节点随即激活**",
    Schema::new()
        .field(project_field())
        .field(Field::required("flow", FieldType::Id, "流程标识"))
        .field(Field::required("version", FieldType::Integer, "版本号"))
        .field(Field::required(
            "subjectKind",
            FieldType::Text { max_len: 32 },
            "主体类型。**平台不解释它**"
        ))
        .field(Field::required(
            "subjectId",
            FieldType::Text { max_len: 128 },
            "主体标识"
        ))
        .field(Field::optional(
            "subjectRevision",
            FieldType::Text { max_len: 64 },
            "主体修订"
        ))
        .field(Field::optional(
            "expiresAt",
            FieldType::Timestamp,
            "过期时刻"
        )),
    Action::ParticipateFlow,
    Idempotency::Keyed,
    kinds::INSTANCE_STARTED,
    |flows: &Arc<Flows>, context: &CallContext<'_>| {
        let version = u32::try_from(
            context
                .arg("version")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        )
        .map_err(|_| Error::invalid("版本号不合法"))?;
        let instance = flows.start(
            context.identity.user.id,
            FlowId::from_id(context.id("flow")?),
            version,
            Subject {
                kind: context.text("subjectKind")?.to_owned(),
                id: context.text("subjectId")?.to_owned(),
                revision: context
                    .arg("subjectRevision")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            context
                .arg("expiresAt")
                .and_then(Value::as_i64)
                .map(xops_core::Timestamp::from_millis),
        )?;
        Ok(json!({
            "instance": instance.id.to_string(),
            "active": instance.active().iter().map(|node| node.node.clone()).collect::<Vec<_>>(),
        }))
    }
);

flow_tool!(
    InstanceStatus,
    "flow.status",
    "查一个实例的状态与**卡在哪**。并行组时激活的可能是多个",
    Schema::new().field(project_field()).field(Field::required(
        "instance",
        FieldType::Id,
        "实例标识"
    )),
    Action::ReadProject,
    Idempotency::ReadOnly,
    kinds::INSTANCE_STARTED,
    |flows: &Arc<Flows>, context: &CallContext<'_>| {
        let instance = flows.status(
            context.identity.user.id,
            InstanceId::from_id(context.id("instance")?),
        )?;
        Ok(json!({
            "instance": instance.id.to_string(),
            "state": instance.state,
            "version": instance.version,
            // `_flows` 没有 currentNode —— 权威是激活中的那些行。
            "active": instance.active().iter().map(|node| node.node.clone()).collect::<Vec<_>>(),
            "nodes": instance.nodes,
        }))
    }
);

flow_tool!(
    CancelInstance,
    "flow.cancel",
    "取消一个实例。其余节点转为已作废",
    Schema::new().field(project_field()).field(Field::required(
        "instance",
        FieldType::Id,
        "实例标识"
    )),
    Action::ManageBusinessObject,
    Idempotency::Keyed,
    kinds::INSTANCE_ENDED,
    |flows: &Arc<Flows>, context: &CallContext<'_>| {
        let instance = flows.cancel(
            context.identity.user.id,
            InstanceId::from_id(context.id("instance")?),
        )?;
        Ok(json!({"instance": instance.id.to_string(), "state": instance.state}))
    }
);

flow_tool!(
    ListFlows,
    "flow.list",
    "列出项目里的流程定义与各版本",
    Schema::new().field(project_field()),
    Action::ReadProject,
    Idempotency::ReadOnly,
    kinds::FLOW_DEFINED,
    |flows: &Arc<Flows>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let listed = flows.list(context.identity.user.id, project)?;
        Ok(json!({
            "flows": listed
                .iter()
                .map(|definition| json!({
                    "flow": definition.flow.to_string(),
                    "version": definition.version,
                    "name": definition.name,
                    "state": definition.state,
                    "settlementTable": definition.settlement_table.as_str(),
                    "subjectTable": definition.subject_table.as_ref().map(xops_table::TableId::as_str),
                }))
                .collect::<Vec<_>>(),
        }))
    }
);

/// 「查询我待处理的流程节点」的实现（`FLW-016`）。**注册位在 RP-03。**
pub struct PendingNodes {
    flows: Arc<Flows>,
}

impl PendingNodes {
    #[must_use]
    pub fn new(flows: Arc<Flows>) -> Self {
        Self { flows }
    }
}

impl xops_mcp::PendingNodes for PendingNodes {
    fn pending_for(&self, user: xops_identity::UserId) -> Result<Vec<Value>> {
        Ok(self
            .flows
            .pending_for(user)?
            .into_iter()
            .map(|(instance, node)| {
                json!({
                    "instance": instance.id.to_string(),
                    "project": instance.project.to_string(),
                    "flow": instance.flow.to_string(),
                    "node": node,
                    "subject": instance.subject,
                })
            })
            .collect())
    }
}

/// 注册流程域。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, flows: &Arc<Flows>) -> Result<()> {
    registry.register(Arc::new(StartInstance::new(Arc::clone(flows))?))?;
    registry.register(Arc::new(InstanceStatus::new(Arc::clone(flows))?))?;
    registry.register(Arc::new(CancelInstance::new(Arc::clone(flows))?))?;
    registry.register(Arc::new(ListFlows::new(Arc::clone(flows))?))?;
    Ok(())
}

/// 让 `Id` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _IdLink = Id;
