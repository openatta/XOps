//! 仓绑定的读写面。

use std::path::PathBuf;
use std::sync::Arc;

use xops_audit::{AuditEnvelope, AuditLog};
use xops_core::{Actor, Clock, Error, Result, RowId, TableName, Timestamp, WriteOp};
use xops_identity::{Action, Directory, ProjectId, UserId};
use xops_store::{Store, WriteEngine, WriteRequest};

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
    pub const REPO_WEBHOOK_SET: &str = "repo.webhook-secret-set";
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
        secret: Option<Secret>,
    ) -> Result<Binding> {
        self.directory
            .authorize(actor, project, Action::BindRepository)?;
        if self.binding(project)?.is_some() {
            return Err(Error::conflict(
                "这个项目已经绑了一个仓（RPO-001：当前绑一个）",
            ));
        }
        crate::binding::check_remote(remote)?;
        let local = crate::local::path_of(remote).is_some();
        if local && secret.is_some() {
            return Err(Error::invalid(
                "本地仓不要给凭据 —— 它的取用不经过认证，给了也不会被用到",
            ));
        }
        // 远端仓**必须**给:`RPO-002` 要试的就是这把凭据写不写得进去。
        if !local && secret.is_none() {
            return Err(Error::invalid("远端仓要给一把只读凭据"));
        }
        // 本地仓那条路不看它（`local::probe` 问的是文件系统，不是这把凭据）。
        let unused = Secret::new("");
        let probe_secret = secret.as_ref().unwrap_or(&unused);

        match self.platform.probe_write_access(remote, probe_secret)? {
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
            // 本地仓不是任何一个平台的仓。记成 github 会让 `repo.status` 说谎。
            if local { "local" } else { self.platform.id() },
            secret.map(|secret| self.sealer.seal(&secret)).transpose()?,
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
        if crate::local::path_of(&binding.remote).is_some() {
            return Err(Error::invalid(
                "本地仓没有凭据可换 —— 它的取用不经过认证（要改只读与否，改目录权限）",
            ));
        }
        if self.platform.probe_write_access(&binding.remote, &secret)? == WriteProbe::Writable {
            return Err(Error::invalid("新凭据写得进去，不能用（RPO-013）"));
        }
        binding.credential = Some(self.sealer.seal(&secret)?);
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

    /// 设这个项目的 Git webhook 验签密钥（`TRG-012`）。设过再设就是换一把。
    ///
    /// # Errors
    /// 没权限 · 这个项目还没绑仓（**明确失败，绝不静默创建**）。
    pub fn set_webhook_secret(
        &self,
        actor: UserId,
        project: ProjectId,
        secret: &Secret,
    ) -> Result<Binding> {
        self.directory
            .authorize(actor, project, Action::BindRepository)?;
        let mut binding = self
            .binding(project)?
            .ok_or_else(|| Error::not_found("这个项目还没绑仓"))?;
        binding.webhook_secret = Some(self.sealer.seal(secret)?);
        self.persist(&binding, kinds::REPO_WEBHOOK_SET, WriteOp::Update, actor)?;
        Ok(binding)
    }

    /// 这次投递是**哪个项目**的:逐个绑定试验签，签得过的那一个就是。
    ///
    /// ⚠️ **先验签再认项目，而不是先按仓名找项目再验签**（`TRG-012`）。
    /// 按仓名找会开一条探测信道:同一个仓名，绑过的与没绑过的走的分支不一样。
    /// 这里两种情形都是"从头试到尾、一个都没过"。
    ///
    /// ⚠️ 而且**一次投递最多命中一个项目**。原先那版拿一把平台级密钥验完签，
    /// 就把事件发给**所有**绑过仓的项目——A 仓的一次 push 会触发 B 项目的任务。
    ///
    /// # Errors
    /// 底层不可用。**没验过不是错误**，是 `Ok(None)`——调用方对两者的回应必须一样。
    pub fn webhook_source(&self, body: &[u8], signature: &str) -> Result<Option<Binding>> {
        let mut matched = None;
        for binding in self.all()? {
            let Some(sealed) = &binding.webhook_secret else {
                continue;
            };
            let Ok(secret) = self.sealer.open(sealed) else {
                // 解不开的密文不该让整轮投递失败:换过密钥的部署里，
                // 别的项目的密钥还是好的。
                continue;
            };
            // ⚠️ **命中之后不 break。** 提前退出会让"验了几次"取决于命中的是第几个，
            // 而那是可以从耗时上读出来的。整轮做完，命中的取第一个。
            if self
                .platform
                .verify_webhook(secret.expose(), body, signature)
                && matched.is_none()
            {
                matched = Some(binding);
            }
        }
        Ok(matched)
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

    /// 这个仓此刻的确切修订（默认分支的头）。
    ///
    /// ⚠️ **它解出来的是一个 sha,不是 "HEAD"。** `RPO-010` 要回答的是
    /// "这份报告针对哪版代码",而 `HEAD` 明天指向别处——一次执行的追溯链
    /// 不能挂在一个会动的名字上。触发方没指定修订时走这条,**解完就钉住**。
    ///
    /// # Errors
    /// 没绑过 · 连不上 · 解不出。
    pub fn head_revision(&self, project: ProjectId) -> Result<String> {
        let binding = self.require(project)?;
        let auth = match &binding.credential {
            Some(sealed) => {
                let secret = self.sealer.open(sealed)?;
                let auth = AuthConfig::write(self.platform.auth_header(&secret))?;
                drop(secret);
                auth
            }
            None => AuthConfig::anonymous()?,
        };
        crate::workspace::head_of(&binding.remote, &auth)
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
        // 本地仓没有凭据,用一份空配置——**不是降级,是那条路上真的没有认证**。
        let auth = match &binding.credential {
            Some(sealed) => {
                let secret = self.sealer.open(sealed)?;
                let auth = AuthConfig::write(self.platform.auth_header(&secret))?;
                drop(secret);
                auth
            }
            None => AuthConfig::anonymous()?,
        };

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
        xops_audit::projection::all(self.store.as_ref(), BINDINGS_TABLE)
    }
}

/// 让 `Timestamp` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _TimestampLink = Timestamp;
