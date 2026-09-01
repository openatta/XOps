//! 模板域的**三个** tool（`TPL-002`）。
//!
//! ```text
//! template.list         列出可用模板
//! template.show         查看模板内容
//! template.instantiate  在本项目实例化 —— **建表、建流程、装插件一步完成**
//! ```
//!
//! ⚠️ 实例化整体上要**维护者及以上**：它里面有一步是装插件，而那一步是维护者的
//! （`PLG-008`）。**这里不为模板开一条更松的路**——那等于绕过 `I-K`。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Error, Result};
use xops_identity::{Action, ProjectId};
use xops_mcp::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};

use crate::service::Templates;
use crate::template::Template;

/// 事件类型。
pub mod kinds {
    pub const TEMPLATE_INSTANTIATED: &str = "template.instantiated";
}

fn brief(template: &Template) -> Value {
    json!({
        "template": template.name,
        "summary": template.summary,
        "tables": template.tables.iter().map(|table| &table.name).collect::<Vec<_>>(),
        "flow": template.flow.as_ref().map(|flow| &flow.name),
        "plugins": template.plugins.iter().map(|plugin| &plugin.name).collect::<Vec<_>>(),
    })
}

fn name_field() -> Field {
    Field::required("template", FieldType::Text { max_len: 64 }, "模板名")
}

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| Error::internal("项目级 tool 却没有项目"))
}

/// 列出可用模板。
pub struct ListTemplates {
    spec: ToolSpec,
    templates: Arc<Templates>,
}

impl ListTemplates {
    /// # Errors
    /// 声明不合形状。
    pub fn new(templates: Arc<Templates>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("template.list")
                .summary("列出可用模板")
                .input(Schema::new().field(Field::required("project", FieldType::Id, "项目标识")))
                .requires(Requirement::InProject(Action::ReadProject))
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::TEMPLATE_INSTANTIATED)
                .build()?,
            templates,
        })
    }
}

impl Tool for ListTemplates {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        require_project(context)?;
        Ok(json!({
            "templates": self.templates.list().iter().map(brief).collect::<Vec<_>>()
        }))
    }
}

/// 查看模板内容。
pub struct ShowTemplate {
    spec: ToolSpec,
    templates: Arc<Templates>,
}

impl ShowTemplate {
    /// # Errors
    /// 声明不合形状。
    pub fn new(templates: Arc<Templates>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("template.show")
                .summary("看一个模板要建什么：表 · 流程 · 插件（含插件源码与能力声明）")
                .input(
                    Schema::new()
                        .field(Field::required("project", FieldType::Id, "项目标识"))
                        .field(name_field()),
                )
                .requires(Requirement::InProject(Action::ReadProject))
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::TEMPLATE_INSTANTIATED)
                .build()?,
            templates,
        })
    }
}

impl Tool for ShowTemplate {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        require_project(context)?;
        let template = self.templates.show(context.text("template")?)?;
        serde_json::to_value(&template)
            .map_err(|error| Error::internal(format!("模板装不下：{error}")))
    }
}

/// 在本项目实例化。
pub struct Instantiate {
    spec: ToolSpec,
    templates: Arc<Templates>,
}

impl Instantiate {
    /// # Errors
    /// 声明不合形状。
    pub fn new(templates: Arc<Templates>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("template.instantiate")
                .summary(
                    "在本项目实例化一个模板：**建表、建流程、装插件一步完成**。\
                     实例化之后它们就是普通对象，想怎么改就怎么改",
                )
                .input(
                    Schema::new()
                        .field(Field::required("project", FieldType::Id, "项目标识"))
                        .field(name_field()),
                )
                // 里面有一步是装插件 —— **不为模板开一条更松的路**。
                .requires(Requirement::InProject(Action::InstallPlugin))
                .idempotency(Idempotency::Keyed)
                .audits(kinds::TEMPLATE_INSTANTIATED)
                .build()?,
            templates,
        })
    }
}

impl Tool for Instantiate {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let project = require_project(context)?;
        let done = self.templates.instantiate(
            context.identity.user.id,
            project,
            context.text("template")?,
        )?;
        Ok(json!({
            "template": done.template,
            "tables": done.tables.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "flow": done.flow.map(|flow| flow.to_string()),
            "plugins": done.plugins.iter()
                .map(|(name, version)| json!({"plugin": name, "version": version}))
                .collect::<Vec<_>>(),
        }))
    }
}

/// 注册模板域。**恰好三个**（`TPL-002`）。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, templates: &Arc<Templates>) -> Result<()> {
    registry.register(Arc::new(ListTemplates::new(Arc::clone(templates))?))?;
    registry.register(Arc::new(ShowTemplate::new(Arc::clone(templates))?))?;
    registry.register(Arc::new(Instantiate::new(Arc::clone(templates))?))?;
    Ok(())
}
