//! 仓域的 tool。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Error, Result, Timestamp};
use xops_identity::{Action, ProjectId};
use xops_mcp::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};

use crate::credential::Secret;
use crate::service::{Repos, kinds};

fn project_field() -> Field {
    Field::required("project", FieldType::Id, "项目标识")
}

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| Error::internal("项目级 tool 却没有项目"))
}

macro_rules! repo_tool {
    ($name:ident, $tool:expr, $summary:expr, $input:expr, $idem:expr, $audit:expr, $body:expr) => {
        pub struct $name {
            spec: ToolSpec,
            repos: Arc<Repos>,
        }

        impl $name {
            /// # Errors
            /// 声明不合形状。
            pub fn new(repos: Arc<Repos>) -> Result<Self> {
                Ok(Self {
                    spec: ToolSpec::builder($tool)
                        .summary($summary)
                        .input($input)
                        .requires(Requirement::InProject(Action::BindRepository))
                        .idempotency($idem)
                        .audits($audit)
                        .build()?,
                    repos,
                })
            }
        }

        impl Tool for $name {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }

            fn call(&self, context: &CallContext<'_>) -> Result<Value> {
                #[allow(clippy::redundant_closure_call)]
                ($body)(&self.repos, context)
            }
        }
    };
}

repo_tool!(
    BindRepo,
    "repo.bind",
    "绑一个 Git 仓。**绑定前会实际试一次写，写得进去就拒绝**",
    Schema::new()
        .field(project_field())
        .field(Field::required(
            "remote",
            FieldType::Text { max_len: 512 },
            "远端地址。https:// · ssh:// · git@ · file://（本地仓）。**凭据不要写进 URL**"
        ))
        .field(Field::optional(
            "credential",
            FieldType::Text { max_len: 512 },
            "只读凭据。**只呈现这一次**：之后加密存储，任何接口都读不出原文。\
             **本地仓（file://）不要给**——它的取用不经过认证，给了也不会被用到",
        )),
    Idempotency::Keyed,
    kinds::REPO_BOUND,
    |repos: &Arc<Repos>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let binding = repos.bind(
            context.identity.user.id,
            project,
            context.text("remote")?,
            context
                .arg("credential")
                .and_then(Value::as_str)
                .map(Secret::new),
        )?;
        Ok(json!({"remote": binding.remote, "platform": binding.platform}))
    }
);

repo_tool!(
    RotateCredential,
    "repo.rotate",
    "轮换只读凭据。**旧凭据立即失效**",
    Schema::new().field(project_field()).field(Field::required(
        "credential",
        FieldType::Text { max_len: 512 },
        "新的只读凭据",
    )),
    Idempotency::NotIdempotent {
        reason: "轮换两次就该是两把新凭据；返回首次结果等于让旧凭据看起来还活着",
    },
    kinds::REPO_ROTATED,
    |repos: &Arc<Repos>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        repos.rotate(
            context.identity.user.id,
            project,
            Secret::new(context.text("credential")?),
        )?;
        Ok(json!({"rotated": true}))
    }
);

repo_tool!(
    UnbindRepo,
    "repo.unbind",
    "解绑。已备好的工作区按各自生命周期结束",
    Schema::new().field(project_field()),
    Idempotency::Keyed,
    kinds::REPO_UNBOUND,
    |repos: &Arc<Repos>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        repos.unbind(context.identity.user.id, project)?;
        Ok(json!({"unbound": true}))
    }
);

repo_tool!(
    SetWebhookSecret,
    "repo.webhook-secret",
    "设这个项目的 Git webhook 验签密钥。**按项目一把，不是平台一把**",
    Schema::new().field(project_field()).field(Field::required(
        "secret",
        FieldType::Text { max_len: 512 },
        "验签密钥。**只呈现这一次**:之后加密存储，任何接口都读不出原文",
    )),
    Idempotency::NotIdempotent {
        reason: "换两次就该是两把新密钥；返回首次结果等于让上一把看起来还活着",
    },
    kinds::REPO_WEBHOOK_SET,
    |repos: &Arc<Repos>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        repos.set_webhook_secret(
            context.identity.user.id,
            project,
            &Secret::new(context.text("secret")?),
        )?;
        Ok(json!({"configured": true}))
    }
);

/// 查状态。**成员就能查**，所以它不走上面那个宏（那个宏要的是维护者）。
pub struct RepoStatus {
    spec: ToolSpec,
    repos: Arc<Repos>,
}

impl RepoStatus {
    /// # Errors
    /// 声明不合形状。
    pub fn new(repos: Arc<Repos>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("repo.status")
                .summary("查绑定与同步状态：上次拉取时间、当前修订。**不含凭据的任何形态**")
                .input(Schema::new().field(project_field()))
                .requires(Requirement::InProject(Action::ReadProject))
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::REPO_BOUND)
                .build()?,
            repos,
        })
    }
}

impl Tool for RepoStatus {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let project = require_project(context)?;
        let Some(binding) = self.repos.status(context.identity.user.id, project)? else {
            return Ok(json!({"bound": false}));
        };
        Ok(json!({
            "bound": true,
            "remote": binding.remote,
            "platform": binding.platform,
            "boundAt": binding.bound_at.as_millis(),
            "lastFetchAt": binding.last_fetch_at.map(Timestamp::as_millis),
            "lastRevision": binding.last_revision,
            // 设没设 webhook 密钥要看得见 —— **没设就是这个项目收不到 webhook**，
            // 而那件事本身是静默的（端点一律回"不存在"）。这里只说有没有，不说是什么。
            "webhookConfigured": binding.webhook_secret.is_some(),
            // ⚠️ 这里**没有** credential 字段，也不会有：RPO-003 说任何接口都读不出原文，
            // 包括项目所有者自己。
        }))
    }
}

/// 注册仓域。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, repos: &Arc<Repos>) -> Result<()> {
    registry.register(Arc::new(BindRepo::new(Arc::clone(repos))?))?;
    registry.register(Arc::new(RotateCredential::new(Arc::clone(repos))?))?;
    registry.register(Arc::new(UnbindRepo::new(Arc::clone(repos))?))?;
    registry.register(Arc::new(SetWebhookSecret::new(Arc::clone(repos))?))?;
    registry.register(Arc::new(RepoStatus::new(Arc::clone(repos))?))?;
    Ok(())
}
