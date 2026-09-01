//! 仓绑定的读写面。

use std::path::PathBuf;
use std::sync::Arc;

use xops_audit::{AuditEnvelope, AuditLog};
use xops_core::{Actor, Clock, Error, Result, RowId, TableName, Timestamp, WriteOp};
use xops_identity::{Action, Directory, ProjectId, UserId};
use xops_store::{Row, Store, WriteEngine, WriteRequest, keys, space};

use crate::binding::Binding;
use crate::credential::{Sealer, Secret};
use crate::platform::{GitPlatform, WriteProbe};
use crate::workspace::{AuthConfig, Budget, Workspace, prepare};

/// 仓绑定落在这张平台表上。
pub const BINDINGS_TABLE: &str = "_repos";

/// 事件类型。
pub mod kinds {
    pub const REPO_BOUND: &str = "repo.bound";
    pub const REPO_ROTATED: &str = "repo.rotated";
    pub const REPO_UNBOUND: &str = "repo.unbound";
    /// `RPO-006`：**每次使用凭据访问仓库都留一条。**
    pub const REPO_FETCHED: &str = "repo.fetched";
}

/// 绑定与工作区。
pub struct Repos {
    engine: Arc<WriteEngine>,
    store: Arc<dyn Store>,
    audit: Arc<AuditLog>,
    directory: Arc<Directory>,
    clock: Arc<dyn Clock>,
    sealer: Arc<Sealer>,
    platform: Arc<dyn GitPlatform>,
    /// 工作区备在哪个目录下。
    workspaces: PathBuf,
    budget: Budget,
}

impl std::fmt::Debug for Repos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repos")
            .field("platform", &self.platform.id())
            .finish_non_exhaustive()
    }
}

/// 本包要用到的那几样地基。
///
/// 单独拎成一个结构，是因为它们**总是一起出现**——写入路径、存储、审计、身份、时钟。
/// 把它们摊成五个参数，调用处就会开始靠位置记顺序。
pub struct Deps {
    pub engine: Arc<WriteEngine>,
    pub store: Arc<dyn Store>,
    pub audit: Arc<AuditLog>,
    pub directory: Arc<Directory>,
    pub clock: Arc<dyn Clock>,
}

impl Repos {
    #[must_use]
    pub fn new(
        deps: Deps,
        sealer: Arc<Sealer>,
        platform: Arc<dyn GitPlatform>,
        workspaces: PathBuf,
    ) -> Self {
        Self {
            engine: deps.engine,
            store: deps.store,
            audit: deps.audit,
            directory: deps.directory,
            clock: deps.clock,
            sealer,
            platform,
            workspaces,
            budget: Budget::default(),
        }
    }

    #[must_use]
    pub fn with_budget(mut self, budget: Budget) -> Self {
        self.budget = budget;
        self
    }

    /// 绑一个仓。
    ///
    /// ⚠️ **绑定之前先试写**（`RPO-002`）：写得进去就拒绝。这是实际推一次 dry-run，
    /// 不是读凭据的声明——声明会撒谎，也会过期。
    ///
    /// # Errors
    /// 没权限 / 项目不存在（同一个错）· 已经绑过了 · 凭据有写权限 · 试写没能得出结论。
    pub fn bind(
        &self,
        actor: UserId,
        project: ProjectId,
        remote: &str,
        secret: Secret,
    ) -> Result<Binding> {
        self.directory
            .authorize(actor, project, Action::BindRepository)?;
        if self.binding(project)?.is_some() {
            return Err(Error::conflict(
                "这个项目已经绑了一个仓（RPO-001：当前绑一个）",
            ));
        }
        crate::binding::check_remote(remote)?;

        match self.platform.probe_write_access(remote, &secret)? {
            WriteProbe::ReadOnly => {}
            WriteProbe::Writable => {
                return Err(Error::invalid(
                    "这把凭据写得进去，不能绑。XOps 在任何代码路径上都不持有仓库写权限（RPO-013）",
                ));
            }
        }

        let binding = Binding::new(
            project,
            remote,
            self.platform.id(),
            self.sealer.seal(&secret)?,
            actor,
            self.clock.now(),
        )?;
        self.persist(&binding, kinds::REPO_BOUND, WriteOp::Insert, actor)?;
        Ok(binding)
    }

    /// 轮换凭据（`RPO-004`）。**旧凭据立即失效**——它被密文覆盖了，系统里不再有第二份。
    ///
    /// # Errors
    /// 没权限 · 没绑过 · 新凭据有写权限。
    pub fn rotate(&self, actor: UserId, project: ProjectId, secret: Secret) -> Result<Binding> {
        self.directory
            .authorize(actor, project, Action::BindRepository)?;
        let mut binding = self.require(project)?;
        if self.platform.probe_write_access(&binding.remote, &secret)? == WriteProbe::Writable {
            return Err(Error::invalid("新凭据写得进去，不能用（RPO-013）"));
        }
        binding.credential = self.sealer.seal(&secret)?;
        self.persist(&binding, kinds::REPO_ROTATED, WriteOp::Update, actor)?;
        Ok(binding)
    }

    /// 写下 XForge 登记（`RPO-014` / `XFG-002`）。
    ///
    /// **它挂在仓绑定上，不另开一套对象**——内容归 RP-19，本包只负责存取。
    ///
    /// # Errors
    /// 没权限 · 这个项目还没绑仓（**明确失败，绝不静默创建**）。
    pub fn set_xforge(
        &self,
        actor: UserId,
        project: ProjectId,
        registration: serde_json::Value,
    ) -> Result<Binding> {
        self.directory
            .authorize(actor, project, Action::BindRepository)?;
        let mut binding = self
            .binding(project)?
            .ok_or_else(|| Error::not_found("这个项目还没绑仓"))?;
        binding.xforge = Some(registration);
        self.persist(&binding, kinds::REPO_BOUND, WriteOp::Update, actor)?;
        Ok(binding)
    }

    /// 解绑。**已备好的工作区按各自生命周期结束**——它们的析构会收拾自己。
    ///
    /// # Errors
    /// 没权限 · 没绑过。
    pub fn unbind(&self, actor: UserId, project: ProjectId) -> Result<()> {
        self.directory
            .authorize(actor, project, Action::BindRepository)?;
        let binding = self.require(project)?;
        self.persist(&binding, kinds::REPO_UNBOUND, WriteOp::Delete, actor)
    }

    /// 查绑定与同步状态（`RPO-012`）。**不含凭据的任何形态。**
    ///
    /// # Errors
    /// 非成员看到的与项目不存在一致。
    pub fn status(&self, viewer: UserId, project: ProjectId) -> Result<Option<Binding>> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        self.binding(project)
    }

    /// 按确切修订备一份只读工作区，交给执行方。
    ///
    /// `RPO-009`：**一次执行的工作区只包含一个项目的代码**——它是从这个项目的绑定备的，
    /// 别的项目的绑定连读都读不到。
    ///
    /// # Errors
    /// 没绑过 · 修订不存在 · 超时超量 · 凭据解不开。
    pub fn prepare_workspace(&self, project: ProjectId, revision: &str) -> Result<Workspace> {
        let mut binding = self.require(project)?;
        // 唯一一处解开凭据的地方（`RPO-005`）。
        let secret = self.sealer.open(&binding.credential)?;
        let auth = AuthConfig::write(self.platform.auth_header(&secret))?;
        drop(secret);

        std::fs::create_dir_all(&self.workspaces)
            .map_err(|error| Error::internal(format!("建不了工作区目录：{error}")))?;
        let workspace = prepare(
            &binding.remote,
            revision,
            &auth,
            self.budget,
            &self.workspaces,
        )?;

        // RPO-006：每次使用凭据访问仓库都留一条 —— 哪个项目、哪个仓、拉了什么修订。
        let envelope = AuditEnvelope::project_scoped(
            kinds::REPO_FETCHED,
            project.as_id(),
            project.as_id(),
            serde_json::json!({
                "remote": binding.remote,
                "revision": workspace.revision(),
            }),
        )?;
        self.audit.append(&Actor::Platform, &envelope)?;

        binding.last_fetch_at = Some(self.clock.now());
        binding.last_revision = Some(workspace.revision().to_owned());
        self.persist_as_platform(&binding, kinds::REPO_FETCHED, WriteOp::Update)?;
        Ok(workspace)
    }

    fn require(&self, project: ProjectId) -> Result<Binding> {
        self.binding(project)?
            .ok_or_else(|| Error::not_found("不存在"))
    }

    fn binding(&self, project: ProjectId) -> Result<Option<Binding>> {
        let table = TableName::new(BINDINGS_TABLE)?;
        let Some(row) = self.engine.read(&table, RowId::from_id(project.as_id()))? else {
            return Ok(None);
        };
        let Some(envelope) = AuditEnvelope::from_payload(&row.payload) else {
            return Err(Error::internal("仓绑定不是一个审计信封"));
        };
        serde_json::from_value(envelope.data)
            .map(Some)
            .map_err(|error| Error::internal(format!("仓绑定读不回来：{error}")))
    }

    fn persist(&self, binding: &Binding, kind: &str, op: WriteOp, actor: UserId) -> Result<()> {
        self.write(
            binding,
            kind,
            op,
            &Actor::User {
                user: actor.to_string(),
            },
        )
    }

    fn persist_as_platform(&self, binding: &Binding, kind: &str, op: WriteOp) -> Result<()> {
        self.write(binding, kind, op, &Actor::Platform)
    }

    fn write(&self, binding: &Binding, kind: &str, op: WriteOp, actor: &Actor) -> Result<()> {
        let envelope = AuditEnvelope::project_scoped(
            kind,
            binding.project.as_id(),
            binding.project.as_id(),
            serde_json::to_value(binding)
                .map_err(|error| Error::internal(format!("仓绑定装不下：{error}")))?,
        )?;
        let receipt = self.engine.write(WriteRequest {
            table: TableName::new(BINDINGS_TABLE)?,
            op,
            row: RowId::from_id(binding.project.as_id()),
            payload: envelope.to_payload()?,
            actor: actor.clone(),
        })?;
        self.audit.index(&receipt)
    }

    /// 全部绑定。运维用。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn all(&self) -> Result<Vec<Binding>> {
        let table = TableName::new(BINDINGS_TABLE)?;
        let prefix = keys::table_prefix(&table);
        let mut out = Vec::new();
        let mut cursor: Option<Vec<u8>> = None;
        loop {
            let page = self
                .store
                .scan(space::ROW, &prefix, cursor.as_deref(), 256)?;
            if page.is_empty() {
                break;
            }
            cursor = page.last().map(|(key, _)| key.clone());
            for (_, bytes) in page {
                let row: Row = serde_json::from_slice(&bytes)
                    .map_err(|error| Error::internal(format!("投影读不回来：{error}")))?;
                if row.is_deleted() {
                    continue;
                }
                let Some(envelope) = AuditEnvelope::from_payload(&row.payload) else {
                    continue;
                };
                if let Ok(binding) = serde_json::from_value::<Binding>(envelope.data) {
                    out.push(binding);
                }
            }
        }
        Ok(out)
    }
}

/// 让 `Timestamp` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _TimestampLink = Timestamp;
