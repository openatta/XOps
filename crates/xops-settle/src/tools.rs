//! 「为某实例的某节点写入一行」——**本包只多这一个 tool**。
//!
//! 它是 `FLW-015` 的最后一项，也是 `FLW-022` 里 `_instance` 三种填法的第一种：
//! **人写行 → 用这个 tool，实例标识作为参数。**
//!
//! 为什么它在本包而不在 RP-14：**因为它是"人做决定"的那条路**，
//! 要判允许写入者与职责分离，而那正是这个包的内容。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Error, Result, RowId};
use xops_flow::Flows;
use xops_flow::instance::InstanceId;
use xops_identity::Action;
use xops_mcp::registry::{CallContext, Idempotency, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Registry, Schema};
use xops_table::{Tables, WrittenBy};

use crate::protection::INSTANCE_COLUMN;

/// 为某实例的某节点写入一行。
pub struct SettleNode {
    spec: ToolSpec,
    flows: Arc<Flows>,
    tables: Arc<Tables>,
}

impl SettleNode {
    /// # Errors
    /// 声明不合形状。
    pub fn new(flows: Arc<Flows>, tables: Arc<Tables>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("flow.settle")
                .summary("为某实例的某节点写入一行。**这是「人做决定」的那条路**")
                .input(
                    Schema::new()
                        .field(Field::required("project", FieldType::Id, "项目标识"))
                        .field(Field::required("instance", FieldType::Id, "实例标识"))
                        .field(Field::required(
                            "values",
                            FieldType::Text { max_len: 8192 },
                            "这一行的内容（JSON 文本）。**不要带 _instance，平台会填**",
                        )),
                )
                .requires(Requirement::InProject(Action::ParticipateFlow))
                .idempotency(Idempotency::Keyed)
                .audits(xops_flow::service::kinds::NODE_ACTIVATED)
                .build()?,
            flows,
            tables,
        })
    }
}

impl Tool for SettleNode {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let instance_id = InstanceId::from_id(context.id("instance")?);
        let instance = self.flows.status(context.identity.user.id, instance_id)?;
        let definition = self.flows.definition(instance.flow, instance.version)?;

        let mut values: Value = serde_json::from_str(context.text("values")?)
            .map_err(|_| Error::invalid("values 不是合法 JSON"))?;
        let Some(object) = values.as_object_mut() else {
            return Err(Error::invalid("values 必须是一个对象"));
        };
        if object.contains_key(INSTANCE_COLUMN) {
            return Err(Error::invalid(
                "不要自己带 _instance —— 它是受保护列，由平台按这个 tool 的参数填（I-P）",
            ));
        }
        // FLW-022 第一种填法：**实例标识作为参数，平台代填。**
        object.insert(INSTANCE_COLUMN.into(), json!(instance_id.to_string()));

        let row = self.tables.insert(
            &WrittenBy::Person {
                user: context.identity.user.id,
            },
            Some(instance.project),
            &definition.settlement_table,
            values,
        )?;
        Ok(json!({
            "row": row.to_string(),
            "instance": instance_id.to_string(),
            // 这一行**照常留在表里**，算不算结算由求值链说了算（FLW-027）。
            "note": "写进去了。算不算结算由七条判定说了算，不算的话会留一条「未被采纳」的痕迹",
        }))
    }
}

/// 注册本包唯一的那个 tool。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, flows: &Arc<Flows>, tables: &Arc<Tables>) -> Result<()> {
    registry.register(Arc::new(SettleNode::new(
        Arc::clone(flows),
        Arc::clone(tables),
    )?))
}

/// 让 `RowId` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _RowLink = RowId;
