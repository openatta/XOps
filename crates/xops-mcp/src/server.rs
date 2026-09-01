//! MCP server 的协议核心：一份 JSON-RPC 请求进来，一份响应出去。
//!
//! **这里没有传输。** 传输是 [`crate::transport`] 的事，而它之所以被分开，
//! 是因为「MCP 的服务面」与「Web 的只读 HTTP 面」不是同一个服务面，不共用路由层
//! （RP-03 与 RP-05 的分工）。协议核心不知道自己被谁喂进来，也因此整段可以被直接测。
//!
//! 一次 `tools/call` 的顺序是固定的，**每一步的先后都有理由**：
//!
//! ```text
//! ① 认证        无令牌或令牌无效 → 拒绝，且不执行任何副作用（MCP-002）
//! ② 找 tool     没有这个 tool → not_found
//! ③ 定项目      项目级 tool 从参数里取 project
//! ④ 鉴权        不是成员 / 角色不够 / 项目不存在 / 已归档 → 同一个「不存在」（MCP-008）
//! ⑤ 校验 schema 未声明字段一律拒绝（MCP-003）
//!               —— 放在鉴权之后：schema 的细节也是信息，不该让越权者试出来
//! ⑥ 幂等        同键命中就返回第一次的结果，不再产生副作用（MCP-006）
//! ⑦ 执行
//! ⑧ 记幂等 / 失败留痕（AUD-007）
//! ```

use std::sync::Arc;

use serde_json::{Value, json};
use xops_audit::{AuditLog, kinds};
use xops_core::{Error, Id, Result, Role};
use xops_identity::{Directory, Identity, ProjectId};
use xops_store::Store;

use crate::errors::{ErrorContract, rpc};
use crate::idempotency::Idempotency;
use crate::registry::{CallContext, Registry, Requirement, ToolSpec, allows};

/// 本 server 实现的 MCP 协议版本。
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// MCP server。
pub struct McpServer {
    registry: Registry,
    directory: Arc<Directory>,
    audit: Arc<AuditLog>,
    idempotency: Idempotency,
}

impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("registry", &self.registry)
            .finish_non_exhaustive()
    }
}

impl McpServer {
    #[must_use]
    pub fn new(directory: Arc<Directory>, audit: Arc<AuditLog>, store: Arc<dyn Store>) -> Self {
        Self {
            registry: Registry::new(),
            directory,
            audit,
            idempotency: Idempotency::new(store),
        }
    }

    /// 各域往这里注册自己的 tool。
    pub fn registry_mut(&mut self) -> &mut Registry {
        &mut self.registry
    }

    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// 处理一份 JSON-RPC 请求。**通知（没有 `id`）不产生响应**，返回 `None`。
    #[must_use]
    pub fn handle(&self, credential: Option<&str>, request: &Value) -> Option<Value> {
        let id = request.get("id").cloned();
        if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(rpc_error(
                id,
                rpc::INVALID_REQUEST,
                "jsonrpc 必须是 \"2.0\"",
                None,
            ));
        }
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            return Some(rpc_error(id, rpc::INVALID_REQUEST, "缺少 method", None));
        };
        // 通知：没有 id，不回响应。
        id.as_ref()?;

        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        match self.dispatch(credential, method, &params) {
            Ok(result) => Some(json!({"jsonrpc": "2.0", "id": id, "result": result})),
            Err(RpcFailure { code, error }) => {
                let contract = ErrorContract::of(&error);
                Some(rpc_error(
                    id,
                    code,
                    &contract.message,
                    Some(contract.to_json()),
                ))
            }
        }
    }

    fn dispatch(&self, credential: Option<&str>, method: &str, params: &Value) -> RpcResult {
        match method {
            "initialize" => {
                // MCP-002：每次调用都要带令牌，握手也不例外 —— 一个连身份都还没有的连接，
                // 没有任何理由需要知道这个 server 支持什么。
                self.authenticate(credential)?;
                Ok(json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {"listChanged": true}},
                    "serverInfo": {"name": "xops", "version": env!("CARGO_PKG_VERSION")},
                }))
            }
            "ping" => {
                self.authenticate(credential)?;
                Ok(json!({}))
            }
            "tools/list" => {
                let identity = self.authenticate(credential)?;
                self.list_tools(&identity, params)
                    .map_err(RpcFailure::business)
            }
            "tools/call" => {
                let identity = self.authenticate(credential)?;
                self.call_tool(&identity, params)
                    .map_err(RpcFailure::business)
            }
            other => Err(RpcFailure {
                code: rpc::METHOD_NOT_FOUND,
                error: Error::not_found(format!("没有这个方法：{other}")),
            }),
        }
    }

    /// ① 认证。**这是身份的唯一来源**（`TOK-007`、G5）。
    fn authenticate(&self, credential: Option<&str>) -> std::result::Result<Identity, RpcFailure> {
        let credential = credential.ok_or_else(|| RpcFailure {
            code: rpc::UNAUTHENTICATED,
            error: xops_identity::token::rejection(),
        })?;
        self.directory
            .resolve(credential)
            .map_err(|error| RpcFailure {
                code: rpc::UNAUTHENTICATED,
                error,
            })
    }

    /// 能力发现（`MCP-009`）。
    ///
    /// 带 `_meta.project` 就按那个项目里的角色精确裁剪；不带就按**这个人在自己参与的项目里
    /// 拿到过的最高角色**给一个概览——`tools/list` 在协议里没有项目这个概念，
    /// 而"精确到项目"的那个答案由 `identity.capabilities` 给。
    fn list_tools(&self, identity: &Identity, params: &Value) -> Result<Value> {
        let (role, archived) = match meta_project(params)? {
            Some(project) => {
                let (record, role) = self.directory.authorize(
                    identity.user.id,
                    project,
                    xops_identity::Action::ReadProject,
                )?;
                (Some(role), record.is_archived())
            }
            None => (self.best_role(identity)?, false),
        };
        let tools: Vec<Value> = self
            .registry
            .visible_to(role, archived)
            .iter()
            .map(|spec| spec.describe())
            .collect();
        Ok(json!({"tools": tools}))
    }

    fn best_role(&self, identity: &Identity) -> Result<Option<Role>> {
        Ok(self
            .directory
            .my_projects(identity.user.id)?
            .into_iter()
            .filter(|(project, _)| !project.is_archived())
            .map(|(_, role)| role)
            .max())
    }

    fn call_tool(&self, identity: &Identity, params: &Value) -> Result<Value> {
        // ② 找 tool
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::invalid("缺少 name"))?;
        let tool = self
            .registry
            .get(name)
            .ok_or_else(|| Error::not_found(format!("没有这个 tool：{name}")))?
            .clone();
        let spec = tool.spec();
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        // ③④ 定项目 + 鉴权
        let (project, role, archived) = self.resolve_scope(identity, spec, &args)?;
        if !allows(spec, role, archived) {
            // 与「不存在」逐字一致（PRJ-008 + MCP-008）。
            return Err(Error::not_found("不存在"));
        }

        let outcome = self.execute(identity, &tool, project, role, params, args);
        match outcome {
            Ok(result) => Ok(result),
            Err(error) => {
                // AUD-007：失败的调用也留痕，且与成功事件区分开。
                self.record_rejection(identity, project, name, &error);
                Err(error)
            }
        }
    }

    fn execute(
        &self,
        identity: &Identity,
        tool: &Arc<dyn crate::registry::Tool>,
        project: Option<ProjectId>,
        role: Option<Role>,
        params: &Value,
        args: Value,
    ) -> Result<Value> {
        let spec = tool.spec();
        // ⑤ schema 校验
        spec.input().validate(&args)?;

        // ⑥ 幂等
        let key = idempotency_key(params)?;
        let user = identity.user.id.to_string();
        let name = spec.name().as_str();
        if let Some(key) = key.as_deref() {
            if !matches!(spec.idempotency(), crate::registry::Idempotency::Keyed) {
                return Err(Error::invalid(format!("{name} 不接受幂等键")));
            }
            if let Some(stored) = self.idempotency.lookup(&user, name, key)? {
                return Ok(stored);
            }
        }

        // ⑦ 执行
        let context = CallContext::new(
            identity,
            project,
            role,
            key.clone(),
            args,
            &self.registry,
            &self.audit,
        );
        let value = tool.call(&context)?;
        let result = json!({
            "content": [{"type": "text", "text": value.to_string()}],
            "structuredContent": value,
            "isError": false,
        });

        // ⑧ 记幂等
        if let Some(key) = key.as_deref() {
            self.idempotency.remember(&user, name, key, &result)?;
        }
        Ok(result)
    }

    fn resolve_scope(
        &self,
        identity: &Identity,
        spec: &ToolSpec,
        args: &Value,
    ) -> Result<(Option<ProjectId>, Option<Role>, bool)> {
        match spec.requirement() {
            Requirement::Platform => Ok((None, None, false)),
            Requirement::InProject(action) => {
                let project = args
                    .get("project")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::invalid("缺少必填字段 project"))?;
                let project = ProjectId::from_id(Id::parse(project)?);
                let (record, role) = self
                    .directory
                    .authorize(identity.user.id, project, action)?;
                Ok((Some(project), Some(role), record.is_archived()))
            }
        }
    }

    fn record_rejection(
        &self,
        identity: &Identity,
        project: Option<ProjectId>,
        tool: &str,
        error: &Error,
    ) {
        let contract = ErrorContract::of(error);
        let data = json!({"tool": tool, "code": contract.code});
        let envelope = match project {
            Some(project) => xops_audit::AuditEnvelope::project_scoped(
                kinds::CALL_REJECTED,
                project.as_id(),
                identity.user.id.as_id(),
                data,
            ),
            None => xops_audit::AuditEnvelope::platform(
                kinds::CALL_REJECTED,
                identity.user.id.as_id(),
                identity.user.id.as_id(),
                data,
            ),
        };
        // 留痕失败不该把业务错误盖掉 —— 调用方要看到的是它自己那个错。
        if let Ok(envelope) = envelope {
            let _ = self.audit.append(&identity.actor(), &envelope.rejected());
        }
    }
}

struct RpcFailure {
    code: i64,
    error: Error,
}

impl RpcFailure {
    fn business(error: Error) -> Self {
        let code = match error.kind() {
            xops_core::ErrorKind::Invalid => rpc::INVALID_PARAMS,
            _ => rpc::INVALID_REQUEST,
        };
        Self { code, error }
    }
}

type RpcResult = std::result::Result<Value, RpcFailure>;

fn rpc_error(id: Option<Value>, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({"code": code, "message": message});
    if let Some(data) = data
        && let Some(object) = error.as_object_mut()
    {
        object.insert("data".into(), data);
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

/// 幂等键从 `params._meta.idempotencyKey` 来，**不从 `arguments` 来**。
///
/// 放进 `arguments` 就得让每个 tool 的 schema 都声明一个它自己不用的字段，
/// 而 `MCP-003` 又要求未声明字段一律拒绝——两条会互相打架。`_meta` 是协议留给
/// 这类带外信息的位置。
fn idempotency_key(params: &Value) -> Result<Option<String>> {
    let Some(key) = params.pointer("/_meta/idempotencyKey") else {
        return Ok(None);
    };
    let key = key
        .as_str()
        .ok_or_else(|| Error::invalid("幂等键必须是字符串"))?;
    if key.is_empty() || key.len() > crate::idempotency::MAX_KEY_LEN {
        return Err(Error::invalid("幂等键长度不合法"));
    }
    Ok(Some(key.to_owned()))
}

fn meta_project(params: &Value) -> Result<Option<ProjectId>> {
    let Some(project) = params.pointer("/_meta/project") else {
        return Ok(None);
    };
    let project = project
        .as_str()
        .ok_or_else(|| Error::invalid("project 必须是字符串"))?;
    Ok(Some(ProjectId::from_id(Id::parse(project)?)))
}
