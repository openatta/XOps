//! 两个 tool 的处理与登记的读写。
//!
//! ⚠️ **这个文件里一处降级都没有。**
//!
//! > **不可用时可重试，不是放行**（`XFG-020`）：任何"连不上就跳过"的降级逻辑
//! > 都会让变更被静默放行。
//!
//! 所以查不到项目、查不到登记、查不到流程，一律**明确失败**——
//! 让调用方看到一个能重试的错误，而不是一个看起来成功的空结果。

use std::sync::Arc;

use serde_json::Value;
use xops_core::{Error, Id, Result, RowId};
use xops_flow::instance::{InstanceState, Subject};
use xops_flow::{Flows, Instance};
use xops_identity::{Directory, ProjectId, UserId};
use xops_repo::Repos;
use xops_table::{Tables, WrittenBy};

use crate::approver::resolve;
use crate::registration::{PolicyBinding, Registration, role_name};
use crate::spec::{ApproverOut, PollReply, SubmitArgs, SubmitReply};

/// 主体的种类：**主体 = `governingDigest`**（`XFG-011`）。
pub const SUBJECT_KIND: &str = "xforge";

/// 适配层。
pub struct XForge {
    repos: Arc<Repos>,
    flows: Arc<Flows>,
    tables: Arc<Tables>,
    directory: Arc<Directory>,
}

impl std::fmt::Debug for XForge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XForge").finish_non_exhaustive()
    }
}

impl XForge {
    #[must_use]
    pub fn new(
        repos: Arc<Repos>,
        flows: Arc<Flows>,
        tables: Arc<Tables>,
        directory: Arc<Directory>,
    ) -> Self {
        Self {
            repos,
            flows,
            tables,
            directory,
        }
    }

    /// 写下登记（`XFG-002`、`XFG-003`）。**挂在仓绑定上。**
    ///
    /// # Errors
    /// 没权限 · 还没绑仓 · 登记不合法（**含角色自校验**，`XFG-015`）。
    pub fn register(
        &self,
        actor: UserId,
        project: ProjectId,
        registration: &Registration,
    ) -> Result<()> {
        // 自校验先做 —— **让角色配错在这一刻失败**，不是等到人以为自己批完了。
        registration.check()?;
        for policy in &registration.policies {
            // 流程得真的在，而且是这个项目的。
            let definition = self
                .flows
                .definition(policy.flow, policy.flow_version)
                .map_err(|_| {
                    Error::not_found(format!(
                        "policyId「{}」指向的流程版本不存在。**明确失败，绝不静默创建**（XFG-002）",
                        policy.policy_id
                    ))
                })?;
            if definition.project != project {
                return Err(Error::invalid("登记的流程不在这个项目里"));
            }
        }
        let value = serde_json::to_value(registration)
            .map_err(|error| Error::internal(format!("登记装不下：{error}")))?;
        self.repos.set_xforge(actor, project, value)?;
        Ok(())
    }

    /// 读回登记。
    ///
    /// # Errors
    /// 没绑仓 · 没登记过——**两样都明确失败**（`XFG-002`）。
    pub fn registration(&self, viewer: UserId, project: ProjectId) -> Result<Registration> {
        let binding = self.repos.status(viewer, project)?.ok_or_else(|| {
            Error::not_found("这个项目还没绑仓。② 找不到 → **明确失败，绝不静默创建**（XFG-002）")
        })?;
        let raw = binding.xforge.ok_or_else(|| {
            Error::not_found("这个仓绑定上还没有 XForge 登记。③ 找不到 → 明确失败（XFG-002）")
        })?;
        serde_json::from_value(raw)
            .map_err(|error| Error::internal(format!("登记读不回来：{error}")))
    }

    /// `submit_approval_request` 的处理（`XFG-011`）。
    ///
    /// ```text
    /// 由仓绑定定位 XOps 项目（找不到 → 明确失败）
    ///   → 由 policyId + roles 映射到本项目的流程（找不到 → 明确失败）
    ///   → 按 governingDigest **幂等**发起实例（主体 = governingDigest）
    ///   → 第一个节点激活，**立即返回**
    /// ```
    ///
    /// # Errors
    /// 上面每一处"找不到"。
    pub fn submit(
        &self,
        actor: UserId,
        project: ProjectId,
        args: &SubmitArgs,
    ) -> Result<SubmitReply> {
        let registration = self.registration(actor, project)?;
        let policy = registration.policy(&args.policy_id)?;
        if !policy.accepts(&args.roles) {
            return Err(Error::invalid(format!(
                "policyId「{}」认的角色是 {:?}，这次请求要的是 {:?}——对不上（XFG-015）",
                policy.policy_id, policy.roles, args.roles
            )));
        }

        // **幂等**：governingDigest 到实例是一一映射，**不得重复开单**。
        if let Some(existing) =
            self.flows
                .find_by_subject(project, SUBJECT_KIND, &args.governing_digest)?
        {
            return Ok(SubmitReply {
                governing_digest: args.governing_digest.clone(),
                created: false,
                instance: existing.id.to_string(),
            });
        }

        let instance = self.flows.start(
            actor,
            policy.flow,
            policy.flow_version,
            Subject {
                kind: SUBJECT_KIND.to_owned(),
                id: args.governing_digest.clone(),
                // `gitHead` 同时作为**主体修订**（`XFG-012`）。
                revision: (!args.revision.git_head.is_empty())
                    .then(|| args.revision.git_head.clone()),
            },
            None,
        )?;
        Ok(SubmitReply {
            governing_digest: args.governing_digest.clone(),
            created: true,
            instance: instance.id.to_string(),
        })
    }

    /// `poll_approval` 的处理（`XFG-013`、`XFG-014`）。
    ///
    /// **纯读，无副作用，可安全重复调用。必须立即返回，绝不阻塞。**
    ///
    /// # Errors
    /// 底层不可用。**"从未提交过"不是错误**——它是 [`PollReply::Unknown`]。
    pub fn poll(
        &self,
        viewer: UserId,
        project: ProjectId,
        governing_digest: &str,
    ) -> Result<PollReply> {
        let Some(instance) = self
            .flows
            .find_by_subject(project, SUBJECT_KIND, governing_digest)?
        else {
            // **明确的未知状态，不是报错**：XForge 会整轮重试。
            return Ok(PollReply::Unknown);
        };
        let decision = match instance.state {
            InstanceState::Running => return Ok(PollReply::Pending),
            InstanceState::Approved => "approve",
            InstanceState::Rejected => "reject",
            // 取消与过期不是一个决定 —— 对调用方而言它仍然是"没批下来"。
            InstanceState::Cancelled | InstanceState::Expired => return Ok(PollReply::Pending),
        };
        let registration = self.registration(viewer, project)?;
        let definition = self.flows.definition(instance.flow, instance.version)?;
        let policy = registration
            .policies
            .iter()
            .find(|policy| policy.flow == instance.flow)
            .ok_or_else(|| Error::not_found("这条流程没有登记过结果列映射（XFG-003）"))?;

        let (approver, reason) = self.settling_row(project, &instance, policy, &definition)?;
        Ok(PollReply::Decided {
            decision: decision.to_owned(),
            approver,
            reason,
        })
    }

    /// 找出结算这个实例的那一行，解析出 approver 与 reason。
    ///
    /// ⚠️ **这里是按行标识点查，不是扫表。** 实例自己记着是哪几行结算了它
    /// （`_flow_nodes.settledBy`），所以根本不需要去结算表里找。
    ///
    /// 早先这里是"扫前 500 行再比对"，那个写法在结算表超过 500 行之后
    /// **对一个已决实例找不到结算行**——它会报一个 XForge 那边查不出原因的失败。
    /// 行标识就在手里却去扫表，是这一处最没道理的地方。
    fn settling_row(
        &self,
        project: ProjectId,
        instance: &Instance,
        policy: &PolicyBinding,
        definition: &xops_flow::Definition,
    ) -> Result<(ApproverOut, String)> {
        let settled_rows: Vec<String> = instance
            .nodes
            .iter()
            .flat_map(|node| node.settled_by.clone())
            .collect();
        for candidate in &settled_rows {
            let Ok(row) = Id::parse(candidate).map(RowId::from_id) else {
                continue;
            };
            let Some(values) = self
                .tables
                .get(Some(project), &definition.settlement_table, row)?
            else {
                continue;
            };
            let written_by: WrittenBy = values
                .get("writtenBy")
                .cloned()
                .and_then(|raw| serde_json::from_value(raw).ok())
                .unwrap_or(WrittenBy::Platform);
            let Some(approver) = resolve(&written_by) else {
                continue;
            };
            let role = self
                .directory
                .role_of(project, approver.user)?
                .ok_or_else(|| {
                    Error::invalid(
                        "批的人已经不在这个项目里了——role 取不出来，\
                         而 approver.role 是 XForge 那边必须认得的东西（XFG-015）",
                    )
                })?;
            // **原样，不解析**（`XFG-016`）。
            let reason = values
                .get(&policy.reason_column)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            return Ok((
                ApproverOut {
                    id: approver.user.to_string(),
                    role: role_name(role).to_owned(),
                },
                reason,
            ));
        }
        Err(Error::internal(
            "实例已决，却找不到结算它的那一行——**这里不猜一个 approver 出来**",
        ))
    }
}
