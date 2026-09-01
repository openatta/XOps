//! 四个 tool：**两个形状定死的**，加两个登记用的。
//!
//! ⚠️ 前两个的名字带下划线、不合 `<域>.<动作>` 的形状——**那是外部规格定死的**
//! （`XFG-010`），由 `xops_mcp::registry::EXTERNAL_NAMES` 那张写死的白名单放行。
//! 它们的回话**只有一个 `text` content item，不带 `structuredContent`**（`XFG-009`）。

use std::sync::Arc;

use serde_json::Value;
use xops_core::{Error, Result};
use xops_identity::{Action, ProjectId};
use xops_mcp::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};

use crate::registration::Registration;
use crate::service::XForge;
use crate::spec::SubmitArgs;

/// 事件类型。
pub mod kinds {
    pub const APPROVAL_SUBMITTED: &str = "xforge.approval.submitted";
    pub const APPROVAL_POLLED: &str = "xforge.approval.polled";
    pub const REGISTERED: &str = "xforge.registered";
}

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| Error::internal("项目级 tool 却没有项目"))
}

fn project_field() -> Field {
    Field::required("project", FieldType::Id, "项目标识")
}

/// `submit_approval_request`。**名字与参数由 XForge 定死。**
pub struct SubmitApprovalRequest {
    spec: ToolSpec,
    xforge: Arc<XForge>,
}

impl SubmitApprovalRequest {
    /// # Errors
    /// 声明不合形状。
    pub fn new(xforge: Arc<XForge>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("submit_approval_request")
                .summary(
                    "提交一次审批请求。**名字与参数由 XForge 定死，XOps 只有实现义务**\
                     （XFG-007 / XFG-010）",
                )
                .input(
                    Schema::new()
                        .field(project_field())
                        .field(Field::required(
                            "change",
                            FieldType::Text { max_len: 128 },
                            "变更",
                        ))
                        .field(Field::required(
                            "flow",
                            FieldType::Text { max_len: 128 },
                            "XForge 侧的 Flow",
                        ))
                        .field(Field::required(
                            "stage",
                            FieldType::Text { max_len: 128 },
                            "阶段",
                        ))
                        .field(Field::required(
                            "transition",
                            FieldType::Text { max_len: 128 },
                            "迁移",
                        ))
                        .field(Field::required(
                            "policyId",
                            FieldType::Text { max_len: 128 },
                            "哪条 policy",
                        ))
                        .field(Field::required(
                            "revision",
                            FieldType::Record {
                                fields: vec![
                                    Field::optional(
                                        "stateRevision",
                                        FieldType::Text { max_len: 128 },
                                        "状态修订",
                                    ),
                                    Field::optional(
                                        "contentRevision",
                                        FieldType::Text { max_len: 128 },
                                        "内容修订",
                                    ),
                                    Field::optional(
                                        "policySnapshotDigest",
                                        FieldType::Text { max_len: 128 },
                                        "policy 快照摘要",
                                    ),
                                    Field::optional(
                                        "gitBase",
                                        FieldType::Text { max_len: 128 },
                                        "基线提交",
                                    ),
                                    Field::optional(
                                        "gitHead",
                                        FieldType::Text { max_len: 128 },
                                        "被审提交。**同时作为主体修订**",
                                    ),
                                ],
                            },
                            "原样保存 —— 人做决定时要看清「我批的是哪一版」（XFG-012）",
                        ))
                        .field(Field::required(
                            "governingDigest",
                            FieldType::Text { max_len: 128 },
                            "**主体，也是幂等键**",
                        ))
                        .field(Field::optional(
                            "roles",
                            FieldType::List {
                                of: Box::new(FieldType::Text { max_len: 32 }),
                                max_len: 8,
                            },
                            "这次要满足的角色",
                        ))
                        .field(Field::optional(
                            "reason",
                            FieldType::Text { max_len: 2_000 },
                            "**不可信自由文本**：原样保存与展示，不解析（XFG-016）",
                        )),
                )
                .requires(Requirement::InProject(Action::ParticipateFlow))
                // 幂等键是 governingDigest 本身，不是 MCP 层那个 —— 见 XForge::submit。
                .idempotency(Idempotency::Keyed)
                .audits(kinds::APPROVAL_SUBMITTED)
                // XFG-009：**不带 structuredContent。**
                .text_only()
                .build()?,
            xforge,
        })
    }
}

impl Tool for SubmitApprovalRequest {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let project = require_project(context)?;
        let args = SubmitArgs::from_value(&context.args)?;
        // **发起者 = 调用所用令牌的持有人**（XFG-005）——职责分离整个压在这上面。
        let reply = self
            .xforge
            .submit(context.identity.user.id, project, &args)?;
        Ok(reply.to_json())
    }
}

/// `poll_approval`。**参数只有 `governingDigest`**（外加平台需要的 project）。
pub struct PollApproval {
    spec: ToolSpec,
    xforge: Arc<XForge>,
}

impl PollApproval {
    /// # Errors
    /// 声明不合形状。
    pub fn new(xforge: Arc<XForge>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("poll_approval")
                .summary(
                    "查一次审批结果。**必须立即返回，绝不阻塞**；纯读、无副作用、\
                     可安全重复调用（XFG-013 / XFG-014）",
                )
                .input(Schema::new().field(project_field()).field(Field::required(
                    "governingDigest",
                    FieldType::Text { max_len: 128 },
                    "查哪一次",
                )))
                .requires(Requirement::InProject(Action::ReadProject))
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::APPROVAL_POLLED)
                .text_only()
                .build()?,
            xforge,
        })
    }
}

impl Tool for PollApproval {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let project = require_project(context)?;
        let reply = self.xforge.poll(
            context.identity.user.id,
            project,
            context.text("governingDigest")?,
        )?;
        Ok(reply.to_json())
    }
}

/// 写下登记。
pub struct RegisterPolicies {
    spec: ToolSpec,
    xforge: Arc<XForge>,
}

impl RegisterPolicies {
    /// # Errors
    /// 声明不合形状。
    pub fn new(xforge: Arc<XForge>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("xforge.register")
                .summary(
                    "登记 policyId → 哪条流程 + 结果列映射。**挂在仓绑定上，不另开一套对象**\
                     （XFG-002 / RPO-014）",
                )
                .input(
                    Schema::new()
                        .field(project_field())
                        .field(Field::required(
                            "providerId",
                            FieldType::Text { max_len: 64 },
                            "这个 provider 在 XForge 侧的 id。④ 的检查要用它",
                        ))
                        .field(Field::required(
                            "policies",
                            FieldType::List {
                                of: Box::new(FieldType::Record {
                                    fields: vec![
                                        Field::required("policyId", FieldType::Text { max_len: 128 }, "XForge 那边的 policyId"),
                                        Field::required("flow", FieldType::Id, "映射到哪条流程"),
                                        Field::required("flowVersion", FieldType::Integer, "哪个版本。**固定版本，不跟随最新**"),
                                        Field::required("decisionColumn", FieldType::Text { max_len: 48 }, "结算表的哪一列是 decision"),
                                        Field::required("reasonColumn", FieldType::Text { max_len: 48 }, "哪一列是 reason"),
                                        Field::required("approveValue", FieldType::Text { max_len: 48 }, "decision 列上哪个取值算 approve"),
                                        Field::required("rejectValue", FieldType::Text { max_len: 48 }, "哪个算 reject"),
                                        Field::required(
                                            "roles",
                                            FieldType::List { of: Box::new(FieldType::Text { max_len: 32 }), max_len: 8 },
                                            "这条 policy 认的角色。**只能是 owner / maintainer / member**（XFG-019）",
                                        ),
                                    ],
                                }),
                                max_len: 32,
                            },
                            "整份覆盖写",
                        )),
                )
                .requires(Requirement::InProject(Action::BindRepository))
                .idempotency(Idempotency::Keyed)
                .audits(kinds::REGISTERED)
                .build()?,
            xforge,
        })
    }
}

impl Tool for RegisterPolicies {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let project = require_project(context)?;
        let registration = Registration {
            provider_id: context.text("providerId")?.to_owned(),
            policies: serde_json::from_value(
                context
                    .arg("policies")
                    .cloned()
                    .unwrap_or(Value::Array(vec![])),
            )
            .map_err(|error| Error::invalid(format!("登记形状不对：{error}")))?,
        };
        self.xforge
            .register(context.identity.user.id, project, &registration)?;
        Ok(serde_json::json!({
            "providerId": registration.provider_id,
            "policies": registration.policies.len(),
        }))
    }
}

/// 读回登记。
pub struct ShowRegistration {
    spec: ToolSpec,
    xforge: Arc<XForge>,
}

impl ShowRegistration {
    /// # Errors
    /// 声明不合形状。
    pub fn new(xforge: Arc<XForge>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("xforge.registration")
                .summary("看这个项目的 XForge 登记")
                .input(Schema::new().field(project_field()))
                .requires(Requirement::InProject(Action::ReadProject))
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::REGISTERED)
                .build()?,
            xforge,
        })
    }
}

impl Tool for ShowRegistration {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let project = require_project(context)?;
        let registration = self
            .xforge
            .registration(context.identity.user.id, project)?;
        serde_json::to_value(&registration)
            .map_err(|error| Error::internal(format!("登记装不下：{error}")))
    }
}

/// 注册 XForge 域。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, xforge: &Arc<XForge>) -> Result<()> {
    registry.register(Arc::new(SubmitApprovalRequest::new(Arc::clone(xforge))?))?;
    registry.register(Arc::new(PollApproval::new(Arc::clone(xforge))?))?;
    registry.register(Arc::new(RegisterPolicies::new(Arc::clone(xforge))?))?;
    registry.register(Arc::new(ShowRegistration::new(Arc::clone(xforge))?))?;
    Ok(())
}
