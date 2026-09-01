//! 任务域的 tool。
//!
//! ⚠️ **手动触发不在这里**——那是 RP-11 的 `run.trigger`。本包只管定义与策略。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Error, Result};
use xops_identity::{Action, ProjectId};
use xops_mcp::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};
use xops_skill::{Ownership, SkillId};
use xops_table::TableId;

use crate::policy::{DEFAULT_TOKEN_BUDGET, OnComplete, Overlap, VersionPolicy};
use crate::service::{Tasks, kinds};
use crate::task::{Kind, Task, TaskId};

fn project_field() -> Field {
    Field::required("project", FieldType::Id, "项目标识")
}

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| Error::internal("项目级 tool 却没有项目"))
}

fn definition_fields() -> Schema {
    Schema::new()
        .field(project_field())
        .field(Field::required(
            "name",
            FieldType::Text { max_len: 64 },
            "任务名",
        ))
        .field(Field::required(
            "skill",
            FieldType::Id,
            "引用哪个技能。**必须是已发布的版本**",
        ))
        .field(Field::optional(
            "skillVersion",
            FieldType::Integer,
            "钉死哪个版本。**不给就是最新已发布版，但那是明确选择**",
        ))
        .field(Field::optional(
            "followLatest",
            FieldType::Bool,
            "跟随最新。⚠️ **默认关**：技能作者一次发布会改变所有引用它的任务的行为",
        ))
        .field(Field::optional(
            "writes",
            FieldType::List {
                of: Box::new(FieldType::Text { max_len: 32 }),
                max_len: 8,
            },
            "写哪些表。**未声明的表写不了**",
        ))
        .field(Field::optional(
            "subscribes",
            FieldType::List {
                of: Box::new(FieldType::Text { max_len: 64 }),
                max_len: 16,
            },
            "订阅哪些事件",
        ))
        .field(Field::optional(
            "tokenBudget",
            FieldType::Integer,
            "单次 token 上限",
        ))
        .field(Field::optional(
            "overlap",
            FieldType::Enum {
                values: ["skip", "queue", "restart"]
                    .iter()
                    .map(|v| (*v).to_owned())
                    .collect(),
            },
            "重叠策略。**默认跳过**——定时任务最常见的故障是堆积成雪崩",
        ))
        .field(Field::optional(
            "onCompletePlugin",
            FieldType::Text { max_len: 64 },
            "完成后调哪个插件",
        ))
        .field(Field::optional(
            "onCompleteTask",
            FieldType::Id,
            "完成后触发哪个任务",
        ))
        .field(Field::optional(
            "private",
            FieldType::Bool,
            "建成个人私有的",
        ))
        .field(Field::optional(
            "pluginBuilder",
            FieldType::Bool,
            "造插件任务。**只能手动触发、不能订阅事件**",
        ))
}

fn build_task(context: &CallContext<'_>, id: TaskId) -> Result<Task> {
    let project = require_project(context)?;
    let follow_latest = context
        .arg("followLatest")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let version_policy = if follow_latest {
        VersionPolicy::Latest
    } else {
        let version = context
            .arg("skillVersion")
            .and_then(Value::as_i64)
            .map(|version| u32::try_from(version).unwrap_or(0));
        match version {
            Some(version) if version > 0 => VersionPolicy::Pinned { version },
            _ => {
                return Err(Error::invalid(
                    "要么钉死一个 skillVersion，要么明确写 followLatest=true——\
                     跟随最新不做默认（TSK-002）",
                ));
            }
        }
    };
    let writes = context
        .arg("writes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(TableId::user)
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let on_complete = match (
        context.arg("onCompletePlugin").and_then(Value::as_str),
        context.arg("onCompleteTask").and_then(Value::as_str),
    ) {
        (Some(_), Some(_)) => {
            return Err(Error::invalid("onComplete 只能挂一样：插件或任务"));
        }
        (Some(plugin), None) => OnComplete::Plugin {
            plugin: plugin.to_owned(),
        },
        (None, Some(task)) => OnComplete::Task {
            task: TaskId::from_id(xops_core::Id::parse(task)?),
        },
        (None, None) => OnComplete::None,
    };
    Ok(Task {
        id,
        project,
        name: context.text("name")?.to_owned(),
        ownership: if context
            .arg("private")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            Ownership::Private {
                owner: context.identity.user.id,
            }
        } else {
            Ownership::Public
        },
        kind: if context
            .arg("pluginBuilder")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            Kind::PluginBuilder
        } else {
            Kind::Normal
        },
        skill: SkillId::from_id(context.id("skill")?),
        version_policy,
        inputs: context.arg("inputs").cloned().unwrap_or_else(|| json!({})),
        writes,
        subscriptions: context
            .arg("subscribes")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        token_budget: context
            .arg("tokenBudget")
            .and_then(Value::as_i64)
            .and_then(|budget| u64::try_from(budget).ok())
            .unwrap_or(DEFAULT_TOKEN_BUDGET),
        overlap: match context.arg("overlap").and_then(Value::as_str) {
            Some("queue") => Overlap::Queue,
            Some("restart") => Overlap::Restart,
            _ => Overlap::Skip,
        },
        on_complete,
        enabled: true,
        created_by: context.identity.user.id,
        created_at: xops_core::Timestamp::from_millis(0),
    })
}

macro_rules! task_tool {
    ($name:ident, $tool:expr, $summary:expr, $input:expr, $action:expr, $idem:expr, $audit:expr, $body:expr) => {
        pub struct $name {
            spec: ToolSpec,
            tasks: Arc<Tasks>,
        }

        impl $name {
            /// # Errors
            /// 声明不合形状。
            pub fn new(tasks: Arc<Tasks>) -> Result<Self> {
                Ok(Self {
                    spec: ToolSpec::builder($tool)
                        .summary($summary)
                        .input($input)
                        .requires(Requirement::InProject($action))
                        .idempotency($idem)
                        .audits($audit)
                        .build()?,
                    tasks,
                })
            }
        }

        impl Tool for $name {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }

            fn call(&self, context: &CallContext<'_>) -> Result<Value> {
                #[allow(clippy::redundant_closure_call)]
                ($body)(&self.tasks, context)
            }
        }
    };
}

task_tool!(
    CreateTask,
    "task.create",
    "建一个任务：订阅什么、跑哪个技能、写哪张表、onComplete 挂什么",
    definition_fields().field(Field::optional(
        "inputs",
        FieldType::Text { max_len: 4096 },
        "给技能的输入（JSON 文本）。**必须满足技能的输入契约**",
    )),
    Action::WriteTask,
    Idempotency::Keyed,
    kinds::TASK_CREATED,
    |tasks: &Arc<Tasks>, context: &CallContext<'_>| {
        let mut task = build_task(context, TaskId::generate())?;
        if let Some(text) = context.arg("inputs").and_then(Value::as_str) {
            task.inputs =
                serde_json::from_str(text).map_err(|_| Error::invalid("inputs 不是合法 JSON"))?;
        }
        let created = tasks.create(context.identity.user.id, task)?;
        Ok(json!({"task": created.id.to_string(), "enabled": created.enabled}))
    }
);

task_tool!(
    SetTaskEnabled,
    "task.set-enabled",
    "启用或停用。**停用的任务不响应任何触发，包括手动。不提供删除**",
    Schema::new()
        .field(project_field())
        .field(Field::required("task", FieldType::Id, "任务标识"))
        .field(Field::required("enabled", FieldType::Bool, "开还是关")),
    Action::WriteTask,
    Idempotency::Keyed,
    kinds::TASK_ENABLED,
    |tasks: &Arc<Tasks>, context: &CallContext<'_>| {
        let enabled = context
            .arg("enabled")
            .and_then(Value::as_bool)
            .ok_or_else(|| Error::invalid("缺少 enabled"))?;
        let task = tasks.set_enabled(
            context.identity.user.id,
            TaskId::from_id(context.id("task")?),
            enabled,
        )?;
        Ok(json!({"task": task.id.to_string(), "enabled": task.enabled}))
    }
);

task_tool!(
    ReadTask,
    "task.read",
    "查一个任务的定义",
    Schema::new()
        .field(project_field())
        .field(Field::required("task", FieldType::Id, "任务标识")),
    Action::ReadProject,
    Idempotency::ReadOnly,
    kinds::TASK_CREATED,
    |tasks: &Arc<Tasks>, context: &CallContext<'_>| {
        let task = tasks.read(
            context.identity.user.id,
            TaskId::from_id(context.id("task")?),
        )?;
        serde_json::to_value(task).map_err(|error| Error::internal(format!("任务装不下：{error}")))
    }
);

task_tool!(
    ListTasks,
    "task.list",
    "列出我看得见的任务",
    Schema::new().field(project_field()),
    Action::ReadProject,
    Idempotency::ReadOnly,
    kinds::TASK_CREATED,
    |tasks: &Arc<Tasks>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let listed = tasks.list(context.identity.user.id, project)?;
        Ok(json!({
            "tasks": listed
                .iter()
                .map(|task| json!({
                    "task": task.id.to_string(),
                    "name": task.name,
                    "enabled": task.enabled,
                    "overlap": task.overlap,
                    "onComplete": task.on_complete,
                }))
                .collect::<Vec<_>>(),
        }))
    }
);

/// 注册任务域。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, tasks: &Arc<Tasks>) -> Result<()> {
    registry.register(Arc::new(CreateTask::new(Arc::clone(tasks))?))?;
    registry.register(Arc::new(SetTaskEnabled::new(Arc::clone(tasks))?))?;
    registry.register(Arc::new(ReadTask::new(Arc::clone(tasks))?))?;
    registry.register(Arc::new(ListTasks::new(Arc::clone(tasks))?))?;
    Ok(())
}
