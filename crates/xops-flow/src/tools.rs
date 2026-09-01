//! 流程域的 tool。
//!
//! ⚠️ **「为某实例的某节点写入一行」不在这里**——那是 RP-15 的，因为它是
//! "人做决定"的那条路，要判允许写入者与职责分离。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Error, Id, Result, Role};
use xops_identity::{Action, ProjectId};
use xops_mcp::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};

use crate::definition::{
    Criteria, Definition, Evaluation, Filter, FlowId, Node, RowQuery, Start, State, Step, Writers,
};
use crate::instance::{InstanceId, Subject};
use crate::service::{Flows, kinds};
use xops_task::TaskId;

fn project_field() -> Field {
    Field::required("project", FieldType::Id, "项目标识")
}

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| Error::internal("项目级 tool 却没有项目"))
}

// ————————————————————— 定义那一面的参数形状 —————————————————————
//
// ⚠️ **不接受"一整份 JSON 定义"**（`MCP-004`）。
// 那样写省事，代价是打错一个键名会被静默丢掉——而流程定义里打错的那个键
// 可能正是 `separationOfDuties`，**少了它没有任何症状，只是审批不再需要第二个人**。
// 所以逐字段声明，未声明的键照样被拒。
//
// 带标签的 union（`Filter`、`Step`）在这里拍平成"判别字段 + 可选字段"，
// **与 `board.define` 的筛选是同一种形状**——那边先这么做的，这里不另发明一种。

/// 一条筛选。与 `board.define` 的 `filter` 逐字段一致。
fn filter_record() -> FieldType {
    FieldType::Record {
        fields: vec![
            Field::required(
                "op",
                FieldType::Enum {
                    values: vec!["equals".into(), "present".into()],
                },
                "筛选方式。**只有等值与非空两种**——再多就开始像查询语言了",
            ),
            Field::required("column", FieldType::Text { max_len: 48 }, "哪一列"),
            Field::optional(
                "value",
                FieldType::Text { max_len: 256 },
                "等值筛选的值。**按文本比**——枚举列与文本列是这么存的",
            ),
        ],
    }
}

fn filters_field(name: &'static str, what: &'static str) -> Field {
    Field::optional(
        name,
        FieldType::List {
            of: Box::new(filter_record()),
            max_len: 16,
        },
        what,
    )
}

/// 求值要预取的一批行（`FLW-003`）。
fn row_query_record() -> FieldType {
    FieldType::Record {
        fields: vec![
            Field::required(
                "table",
                FieldType::Text {
                    max_len: xops_table::TableId::MAX_LEN,
                },
                "从哪张表取",
            ),
            filters_field("filters", "取满足哪组筛选的行"),
            Field::required("limit", FieldType::Integer, "最多取几行。**必须有上限**"),
        ],
    }
}

/// 一个节点。`Writers` 的三者并集在这里摊成三个字段。
fn node_record() -> FieldType {
    FieldType::Record {
        fields: vec![
            Field::required("name", FieldType::Text { max_len: 64 }, "节点名"),
            filters_field("pass", "通过条件：结算表上出现满足这组筛选的行"),
            Field::optional("quorum", FieldType::Integer, "会签票数。不给按 1 算"),
            filters_field("reject", "拒绝条件。**满足它整个实例立即进终态**"),
            Field::optional(
                "writerRoles",
                FieldType::List {
                    of: Box::new(FieldType::Enum {
                        values: vec!["owner".into(), "maintainer".into(), "member".into()],
                    }),
                    max_len: 3,
                },
                "① 哪些项目角色能写",
            ),
            Field::optional(
                "writerRoster",
                FieldType::Text {
                    max_len: xops_table::TableId::MAX_LEN,
                },
                "② 名单表。⚠️ **它必须是受保护表**——谁能改名单，谁就能给自己发审批权",
            ),
            Field::optional(
                "writerTask",
                FieldType::Id,
                "③ 指定的**私有**任务。公共任务没有「所有者这个人」，不能算作通过",
            ),
            Field::optional(
                "separationOfDuties",
                FieldType::Bool,
                "要求写入者 ≠ 实例发起人。**不给是关的**——挡闭环自批要显式打开",
            ),
            Field::optional(
                "plugin",
                FieldType::Text { max_len: 64 },
                "改用流转插件求值。给了就必须一并给 pluginInputs",
            ),
            Field::optional(
                "pluginInputs",
                FieldType::List {
                    of: Box::new(row_query_record()),
                    max_len: 8,
                },
                "插件求值要用到哪些行（`FLW-003`）。**插件读不到表**，输入由平台预取",
            ),
        ],
    }
}

fn parse_filters(value: Option<&Value>) -> Result<Criteria> {
    let mut filters = Vec::new();
    for item in value.and_then(Value::as_array).into_iter().flatten() {
        let column = item["column"].as_str().unwrap_or_default().to_owned();
        filters.push(match item["op"].as_str().unwrap_or_default() {
            "present" => Filter::Present { column },
            "equals" => Filter::Equals {
                column,
                value: item
                    .get("value")
                    .cloned()
                    .ok_or_else(|| Error::invalid("等值筛选要给 value"))?,
            },
            other => return Err(Error::invalid(format!("不认识的筛选方式：{other}"))),
        });
    }
    Ok(Criteria { filters })
}

fn parse_table(value: &Value, what: &str) -> Result<xops_table::TableId> {
    let name = value
        .as_str()
        .ok_or_else(|| Error::invalid(format!("{what} 要给表名")))?;
    xops_table::TableId::user(name)
}

fn parse_node(value: &Value) -> Result<Node> {
    let name = value["name"].as_str().unwrap_or_default().to_owned();
    let mut roles = Vec::new();
    for role in value["writerRoles"].as_array().into_iter().flatten() {
        roles.push(match role.as_str().unwrap_or_default() {
            "owner" => Role::Owner,
            "maintainer" => Role::Maintainer,
            "member" => Role::Member,
            other => return Err(Error::invalid(format!("不认识的角色：{other}"))),
        });
    }
    let reject = parse_filters(value.get("reject"))?;
    // 求值方式：给了插件名就是插件，否则按筛选。
    let evaluation = match value["plugin"].as_str() {
        Some(plugin) => {
            let mut inputs = Vec::new();
            for query in value["pluginInputs"].as_array().into_iter().flatten() {
                inputs.push(RowQuery {
                    table: parse_table(&query["table"], "pluginInputs.table")?,
                    criteria: parse_filters(query.get("filters"))?,
                    limit: usize::try_from(query["limit"].as_u64().unwrap_or(0))
                        .map_err(|_| Error::invalid("pluginInputs.limit 太大"))?,
                });
            }
            Evaluation::Plugin {
                plugin: plugin.to_owned(),
                inputs,
            }
        }
        None => {
            if value.get("pluginInputs").is_some() {
                return Err(Error::invalid(
                    "给了 pluginInputs 却没给 plugin —— 按筛选求值用不到预取的行",
                ));
            }
            Evaluation::ByCriteria
        }
    };
    Ok(Node {
        name,
        pass: parse_filters(value.get("pass"))?,
        quorum: u32::try_from(value["quorum"].as_u64().unwrap_or(1))
            .map_err(|_| Error::invalid("quorum 太大"))?,
        reject: if reject.filters.is_empty() {
            None
        } else {
            Some(reject)
        },
        writers: Writers {
            roles,
            roster: value
                .get("writerRoster")
                .filter(|value| !value.is_null())
                .map(|value| parse_table(value, "writerRoster"))
                .transpose()?,
            task: value["writerTask"]
                .as_str()
                .map(|id| Ok::<_, xops_core::Error>(TaskId::from_id(Id::parse(id)?)))
                .transpose()?,
        },
        separation_of_duties: value["separationOfDuties"].as_bool().unwrap_or(false),
        evaluation,
    })
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

flow_tool!(
    DefineFlow,
    "flow.define",
    "定义一条流程，或给已有流程发布新版本。**不存在流程设计器界面**——定义经这里创建",
    Schema::new()
        .field(project_field())
        .field(Field::optional(
            "flow",
            FieldType::Id,
            "给哪条流程发布新版本。**不给就是新建一条**——版本号由平台排，不由调用方给",
        ))
        .field(Field::required(
            "name",
            FieldType::Text { max_len: 64 },
            "流程名"
        ))
        .field(Field::required(
            "settlementTable",
            FieldType::Text {
                max_len: xops_table::TableId::MAX_LEN,
            },
            "结算表:放「谁对它做了什么表态」",
        ))
        .field(Field::optional(
            "subjectTable",
            FieldType::Text {
                max_len: xops_table::TableId::MAX_LEN,
            },
            "主体表:放「这件事本身」。**不得与结算表同一张**（FLW-004）",
        ))
        .field(Field::optional(
            "start",
            FieldType::Enum {
                values: vec!["automatic".into(), "explicit".into()],
            },
            "怎么发起。automatic 是随行发起（主体表插新行时），不给按 explicit",
        ))
        .field(Field::optional(
            "statusColumns",
            FieldType::List {
                of: Box::new(FieldType::Text { max_len: 48 }),
                max_len: 8,
            },
            "主体表上哪几列是状态列。**只有平台与流转插件能写它们**（FLW-036）——\
             不声明的话，任何成员都能直接 update 绕过整条流程",
        ))
        .field(Field::required(
            "steps",
            FieldType::List {
                of: Box::new(FieldType::List {
                    of: Box::new(node_record()),
                    max_len: 8,
                }),
                max_len: 32,
            },
            "有序的步骤。**每一步是一组节点**:一个是单节点，多个是并行组\
             （同时激活、全部通过才推进）",
        )),
    Action::DefineFlow,
    Idempotency::Keyed,
    kinds::FLOW_DEFINED,
    |flows: &Arc<Flows>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let definition = build_definition(project, context)?;
        let defined = flows.define(context.identity.user.id, definition)?;
        Ok(json!({
            "flow": defined.flow.to_string(),
            "version": defined.version,
            "state": defined.state,
        }))
    }
);

flow_tool!(
    DisableFlow,
    "flow.disable",
    "停用一个版本。**不能再发起新实例，在途实例继续执行完**（FLW-006）",
    Schema::new()
        .field(project_field())
        .field(Field::required("flow", FieldType::Id, "流程标识"))
        .field(Field::required("version", FieldType::Integer, "版本号")),
    Action::DefineFlow,
    Idempotency::Keyed,
    kinds::FLOW_DISABLED,
    |flows: &Arc<Flows>, context: &CallContext<'_>| {
        let version = u32::try_from(
            context
                .arg("version")
                .and_then(Value::as_i64)
                .ok_or_else(|| Error::invalid("缺少 version"))?,
        )
        .map_err(|_| Error::invalid("版本号不合法"))?;
        let disabled = flows.disable(
            context.identity.user.id,
            FlowId::from_id(context.id("flow")?),
            version,
        )?;
        Ok(
            json!({"flow": disabled.flow.to_string(), "version": disabled.version, "state": disabled.state}),
        )
    }
);

/// 从参数装一份定义出来。
///
/// ⚠️ **版本号、创建人、创建时刻、状态都不从参数来** —— 它们由 `Flows::define` 填。
/// 让调用方给版本号等于让它决定"这一版排在哪"，那是平台的账。
fn build_definition(project: ProjectId, context: &CallContext<'_>) -> Result<Definition> {
    let mut steps = Vec::new();
    for group in context
        .arg("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let nodes = group
            .as_array()
            .ok_or_else(|| Error::invalid("每一步要是一组节点"))?
            .iter()
            .map(parse_node)
            .collect::<Result<Vec<_>>>()?;
        // 一个节点是单步，多个是并行组。**没有第三种，所以不另设一个判别字段**——
        // 判别字段与列表长度对不上是这类参数最常见的错。
        steps.push(match <[Node; 1]>::try_from(nodes) {
            Ok([node]) => Step::Single { node },
            Err(nodes) => Step::Parallel { nodes },
        });
    }
    Ok(Definition {
        flow: match context.arg("flow").and_then(Value::as_str) {
            Some(id) => FlowId::from_id(Id::parse(id)?),
            None => FlowId::generate(),
        },
        project,
        version: 0,
        name: context.text("name")?.to_owned(),
        settlement_table: parse_table(
            context.arg("settlementTable").unwrap_or(&Value::Null),
            "settlementTable",
        )?,
        subject_table: context
            .arg("subjectTable")
            .filter(|value| !value.is_null())
            .map(|value| parse_table(value, "subjectTable"))
            .transpose()?,
        start: match context.arg("start").and_then(Value::as_str) {
            Some("automatic") => Start::Automatic,
            _ => Start::Explicit,
        },
        status_columns: context
            .arg("statusColumns")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
        steps,
        state: State::Published,
        created_by: context.identity.user.id,
        created_at: xops_core::Timestamp::from_millis(0),
    })
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
    registry.register(Arc::new(DefineFlow::new(Arc::clone(flows))?))?;
    registry.register(Arc::new(DisableFlow::new(Arc::clone(flows))?))?;
    Ok(())
}

/// 让 `Id` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _IdLink = Id;
