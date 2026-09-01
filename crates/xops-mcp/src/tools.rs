//! 身份域的三个 tool。**本包只有这三个**——别的域各归其包。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::Result;
use xops_identity::Directory;

use crate::registry::{CallContext, Idempotency, Requirement, Tool, ToolSpec};
use crate::schema::{Field, FieldType, Schema};

/// 「我待处理的流程节点」的实现位。
///
/// **注册位在 RP-03，实现在 RP-14。** 现在挂一个空实现：tool 存在、形状定死、
/// 调得通、返回空列表。这样 RP-14 接进来时改的是这个 trait 的实现，
/// 不是 MCP 这一层的注册与 schema——而后者一旦要改，全部客户端跟着改。
pub trait PendingNodes: Send + Sync + 'static {
    /// 跨项目聚合。
    ///
    /// # Errors
    /// 底层不可用。
    fn pending_for(&self, user: xops_identity::UserId) -> Result<Vec<Value>>;
}

/// M1 的实现：还没有流程，所以永远是空的。
#[derive(Debug, Default)]
pub struct NoPendingNodes;

impl PendingNodes for NoPendingNodes {
    fn pending_for(&self, _user: xops_identity::UserId) -> Result<Vec<Value>> {
        Ok(Vec::new())
    }
}

/// 查询当前身份。
pub struct WhoAmI {
    spec: ToolSpec,
}

impl WhoAmI {
    /// # Errors
    /// 声明不合形状——只可能是这个文件被改坏了。
    pub fn new() -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("identity.whoami")
                .summary("查询当前令牌对应的身份")
                .input(Schema::new())
                .requires(Requirement::Platform)
                .idempotency(Idempotency::ReadOnly)
                .audits(xops_audit::kinds::CALL_REJECTED)
                .build()?,
        })
    }
}

impl Tool for WhoAmI {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let user = &context.identity.user;
        Ok(json!({
            "user": user.id.to_string(),
            "displayName": user.display_name,
            "provider": user.account.provider.as_str(),
            "account": user.account.account,
            "token": context.identity.token.to_string(),
        }))
    }
}

/// 能力发现：**这个人在这个项目里能调哪些 tool**（`MCP-009`）。
///
/// 与 `tools/list` 的区别是它精确到项目——协议里的 `tools/list` 没有项目这个概念。
pub struct Capabilities {
    spec: ToolSpec,
    directory: Arc<Directory>,
}

impl Capabilities {
    /// # Errors
    /// 声明不合形状。
    pub fn new(directory: Arc<Directory>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("identity.capabilities")
                .summary("列出我在某个项目里能调用的 tool")
                .input(Schema::new().field(Field::optional(
                    "project",
                    FieldType::Id,
                    "项目标识；不给就按我参与的项目里拿到过的最高角色给一个概览",
                )))
                .requires(Requirement::Platform)
                .idempotency(Idempotency::ReadOnly)
                .audits(xops_audit::kinds::CALL_REJECTED)
                .build()?,
            directory,
        })
    }
}

impl Tool for Capabilities {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let (role, archived) = match context.arg("project") {
            Some(project) => {
                let project = xops_identity::ProjectId::from_id(xops_core::Id::parse(
                    project.as_str().unwrap_or_default(),
                )?);
                // 非成员在这里得到的与"项目不存在"完全一致（PRJ-008）。
                let (record, role) = self.directory.authorize(
                    context.identity.user.id,
                    project,
                    xops_identity::Action::ReadProject,
                )?;
                (Some(role), record.is_archived())
            }
            None => (
                self.directory
                    .my_projects(context.identity.user.id)?
                    .into_iter()
                    .filter(|(project, _)| !project.is_archived())
                    .map(|(_, role)| role)
                    .max(),
                false,
            ),
        };
        let names: Vec<String> = context
            .registry
            .visible_to(role, archived)
            .iter()
            .map(|spec| spec.name().as_str().to_owned())
            .collect();
        Ok(json!({"role": role.map(|role| role.as_str()), "tools": names}))
    }
}

/// 查询我待处理的流程节点（**跨项目聚合**）。
pub struct MyPendingNodes {
    spec: ToolSpec,
    source: Arc<dyn PendingNodes>,
}

impl MyPendingNodes {
    /// # Errors
    /// 声明不合形状。
    pub fn new(source: Arc<dyn PendingNodes>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("identity.pending-nodes")
                .summary("查询我待处理的流程节点，跨项目聚合")
                .input(Schema::new())
                .requires(Requirement::Platform)
                .idempotency(Idempotency::ReadOnly)
                .audits(xops_audit::kinds::CALL_REJECTED)
                .build()?,
            source,
        })
    }
}

impl Tool for MyPendingNodes {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        Ok(json!({"nodes": self.source.pending_for(context.identity.user.id)?}))
    }
}
