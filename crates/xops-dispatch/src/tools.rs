//! 触发与执行域的 tool。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Id, Result};
use xops_exec::{ExecContract, worksheet::RunId};
use xops_identity::Action;
use xops_mcp::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};
use xops_task::{TaskId, Tasks};

use crate::dispatch::{Dispatcher, Outcome, kinds};
use crate::event::{Event, EventKind, Trigger};

fn project_field() -> Field {
    Field::required("project", FieldType::Id, "项目标识")
}

/// 手动触发。**非阻塞**：立即返回执行标识（`TRG-016`）。
pub struct TriggerTask {
    spec: ToolSpec,
    dispatcher: Arc<Dispatcher>,
    tasks: Arc<Tasks>,
}

impl TriggerTask {
    /// # Errors
    /// 声明不合形状。
    pub fn new(dispatcher: Arc<Dispatcher>, tasks: Arc<Tasks>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("run.trigger")
                .summary("手动触发一个任务。**非阻塞**——返回的是执行标识，不是执行结果")
                .input(
                    Schema::new()
                        .field(project_field())
                        .field(Field::required("task", FieldType::Id, "任务标识"))
                        .field(Field::optional(
                            "revision",
                            FieldType::Text { max_len: 64 },
                            "读哪个代码修订。**它覆盖任务定义里写死的那个**",
                        )),
                )
                .requires(Requirement::InProject(Action::WriteTask))
                .idempotency(Idempotency::Keyed)
                .audits(kinds::TRIGGER_ACCEPTED)
                .build()?,
            dispatcher,
            tasks,
        })
    }
}

impl Tool for TriggerTask {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        // TRG-020：**触发不允许覆盖输入参数。** schema 里根本没有那个字段，
        // 所以带上它会被 MCP-003 挡在更外面 —— 这一条因此是双重的。
        let task = self.tasks.read(
            context.identity.user.id,
            TaskId::from_id(context.id("task")?),
        )?;
        let event = Event {
            kind: EventKind::Manual,
            project: task.project,
            external_id: context.idempotency_key.clone(),
            triggered_by: Trigger::Person {
                user: context.identity.user.id,
            },
            revision: context
                .arg("revision")
                .and_then(Value::as_str)
                .map(str::to_owned),
            at: xops_core::Timestamp::from_millis(0),
            payload: json!({}),
        };
        let record = self.dispatcher.trigger(&task, &event)?;
        Ok(match &record.outcome {
            Outcome::Accepted { run } | Outcome::Duplicate { run } => {
                json!({"accepted": true, "run": run})
            }
            Outcome::Rejected { why } => json!({"accepted": false, "rejected": why}),
            Outcome::Skipped { why } => json!({"accepted": false, "skipped": why}),
        })
    }
}

macro_rules! run_tool {
    ($name:ident, $tool:expr, $summary:expr, $input:expr, $action:expr, $idem:expr, $body:expr) => {
        pub struct $name {
            spec: ToolSpec,
            exec: Arc<dyn ExecContract>,
        }

        impl $name {
            /// # Errors
            /// 声明不合形状。
            pub fn new(exec: Arc<dyn ExecContract>) -> Result<Self> {
                Ok(Self {
                    spec: ToolSpec::builder($tool)
                        .summary($summary)
                        .input($input)
                        .requires(Requirement::InProject($action))
                        .idempotency($idem)
                        .audits(kinds::TRIGGER_ACCEPTED)
                        .build()?,
                    exec,
                })
            }
        }

        impl Tool for $name {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }

            fn call(&self, context: &CallContext<'_>) -> Result<Value> {
                #[allow(clippy::redundant_closure_call)]
                ($body)(&self.exec, context)
            }
        }
    };
}

run_tool!(
    RunStatus,
    "run.status",
    "查一次执行的状态",
    Schema::new()
        .field(project_field())
        .field(Field::required("run", FieldType::Id, "执行标识")),
    Action::ReadProject,
    Idempotency::ReadOnly,
    |exec: &Arc<dyn ExecContract>, context: &CallContext<'_>| {
        let run = RunId::from_id(context.id("run")?);
        let status = exec.status(run)?;
        let outcome = exec.collect(run)?;
        Ok(json!({
            "run": run.to_string(),
            "status": status,
            "failureKind": outcome.as_ref().and_then(|outcome| outcome.failure),
            "tokensUsed": outcome.as_ref().map(|outcome| outcome.tokens_used),
        }))
    }
);

run_tool!(
    CancelRun,
    "run.cancel",
    "取消一次执行。**已经结束的取消是无操作，不是错误**",
    Schema::new()
        .field(project_field())
        .field(Field::required("run", FieldType::Id, "执行标识")),
    Action::WriteTask,
    Idempotency::Keyed,
    |exec: &Arc<dyn ExecContract>, context: &CallContext<'_>| {
        let run = RunId::from_id(context.id("run")?);
        exec.cancel(run)?;
        Ok(json!({"cancelled": run.to_string()}))
    }
);

/// 查触发历史，**含被拒绝与被跳过的**（`TSK-016`）。
///
/// > 一个静默被跳过的任务，会让人以为它在跑。
pub struct TriggerHistory {
    spec: ToolSpec,
    dispatcher: Arc<Dispatcher>,
}

impl TriggerHistory {
    /// # Errors
    /// 声明不合形状。
    pub fn new(dispatcher: Arc<Dispatcher>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("run.trigger-history")
                .summary("查一个任务的触发历史。**含被拒绝与被跳过的**")
                .input(Schema::new().field(project_field()).field(Field::required(
                    "task",
                    FieldType::Id,
                    "任务标识",
                )))
                .requires(Requirement::InProject(Action::ReadProject))
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::TRIGGER_ACCEPTED)
                .build()?,
            dispatcher,
        })
    }
}

impl Tool for TriggerHistory {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let history = self
            .dispatcher
            .trigger_history(TaskId::from_id(context.id("task")?))?;
        Ok(json!({
            "triggers": history
                .iter()
                .map(|record| json!({
                    "kind": record.kind,
                    "at": record.at.as_millis(),
                    "outcome": record.outcome,
                }))
                .collect::<Vec<_>>(),
        }))
    }
}

/// 注册触发与执行域。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(
    registry: &mut Registry,
    dispatcher: &Arc<Dispatcher>,
    tasks: &Arc<Tasks>,
    exec: &Arc<dyn ExecContract>,
) -> Result<()> {
    registry.register(Arc::new(TriggerTask::new(
        Arc::clone(dispatcher),
        Arc::clone(tasks),
    )?))?;
    registry.register(Arc::new(RunStatus::new(Arc::clone(exec))?))?;
    registry.register(Arc::new(CancelRun::new(Arc::clone(exec))?))?;
    registry.register(Arc::new(TriggerHistory::new(Arc::clone(dispatcher))?))?;
    Ok(())
}

/// 让 `Id` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _IdLink = Id;
