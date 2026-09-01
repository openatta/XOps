//! 身份、项目与成员的读写，以及它们与审计的接缝。
//!
//! **四张平台表**：`_users` · `_tokens` · `_projects` · `_members`。它们经 RP-01 的写入路径落盘，
//! 因而每一次变更都自带一条不可变事件——`AUD-005`（不存在"业务成功但没留痕"）就是这么成立的，
//! 不是靠两次写小心翼翼地对齐。
//!
//! 每一行的 payload 是一个 [`AuditEnvelope`]，对象本身装在它的 `data` 里。这样同一条事件
//! 既是业务状态，又带得动 `AUD-002` 要的项目、事件类型与目标。
//!
//! **本包不直接触碰 SQLite**，也不改 `xops-store` ——读行用的是它已经发布的
//! `Store` + `keys` + `space` + `Row` 四样。

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use xops_audit::{AuditEnvelope, AuditLog, kinds};
use xops_core::{Actor, Clock, Error, Id, Result, Role, RowId, TableName, Timestamp, WriteOp};
use xops_store::{Store, WriteEngine, WriteRequest};

use crate::permission::{Action, can_in};
use crate::project::{Member, MemberChange, Project, ProjectId, Slug, owners_after};
use crate::token::{self, Token, TokenId, TokenSecret};
use crate::user::{ExternalAccount, IdentityProvider, User, UserId};

/// 四张平台表。**它们不是「五张系统表」**（那五张是业务上看得见的），
/// 是平台自己的账，不参与建表、看板与表专属 tool。
pub const USERS: &str = "_users";
pub const TOKENS: &str = "_tokens";
pub const PROJECTS: &str = "_projects";
pub const MEMBERS: &str = "_members";

/// 全部平台表。
pub const PLATFORM_TABLES: &[&str] = &[USERS, TOKENS, PROJECTS, MEMBERS];

/// 「不存在」。
///
/// `PRJ-008` / `MCP-008`：**非成员看不到项目的存在，权限不足与对象不存在返回相同的错误。**
/// 给它一个常量，是为了让"顺手回一句更有帮助的话"这件事必须先改到这里——
/// 那句更有帮助的话，就是探测他人项目的工具。
fn invisible() -> Error {
    Error::not_found("不存在")
}

/// 解析出来的调用者。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub user: User,
    pub token: TokenId,
}

impl Identity {
    /// 写入时署的名。`I-B`：**它来自这里，不来自请求体。**
    #[must_use]
    pub fn actor(&self) -> Actor {
        Actor::User {
            user: self.user.id.to_string(),
        }
    }
}

/// 某一时刻的业务状态快照（`AUD-004`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub users: Vec<User>,
    pub projects: Vec<Project>,
    pub members: Vec<Member>,
}

/// 身份、项目、成员与令牌的读写面。
pub struct Directory {
    engine: Arc<WriteEngine>,
    store: Arc<dyn Store>,
    audit: Arc<AuditLog>,
    clock: Arc<dyn Clock>,
    providers: Vec<Box<dyn IdentityProvider>>,
    self_registration: bool,
}

impl std::fmt::Debug for Directory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Directory")
            .field("providers", &self.providers.len())
            .field("self_registration", &self.self_registration)
            .finish_non_exhaustive()
    }
}

impl Directory {
    #[must_use]
    pub fn new(
        engine: Arc<WriteEngine>,
        store: Arc<dyn Store>,
        audit: Arc<AuditLog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            engine,
            store,
            audit,
            clock,
            providers: Vec::new(),
            // IDN-003：自注册**默认关闭**。
            self_registration: false,
        }
    }

    #[must_use]
    pub fn with_provider(mut self, provider: Box<dyn IdentityProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    /// 打开自注册（`IDN-003`，默认关闭）。
    #[must_use]
    pub fn with_self_registration(mut self, allowed: bool) -> Self {
        self.self_registration = allowed;
        self
    }

    // ——————————————————————————————— 身份 ———————————————————————————————

    /// 登录（`IDN-001`）。
    ///
    /// # Errors
    /// 凭证不对，或者账号没被预置 / 邀请而自注册关着（`IDN-003`）——**这两种情形错误一致，
    /// 且后者不创建任何用户记录**。
    pub fn login(&self, provider: &str, account: &str, secret: &str) -> Result<User> {
        let provider = self
            .providers
            .iter()
            .find(|candidate| candidate.id().as_str() == provider)
            .ok_or_else(|| Error::denied("凭证不对"))?;
        let profile = provider.authenticate(account, secret)?;
        let external = ExternalAccount {
            provider: provider.id(),
            account: profile.account.clone(),
        };

        if let Some(user) = self.user_by_account(&external)? {
            return Ok(user);
        }
        if !self.self_registration {
            // IDN-003：关闭时，未被预置或未被邀请的账号登录后被拒绝，且**不创建任何用户记录**。
            return Err(Error::denied("凭证不对"));
        }
        self.provision(external, &profile.display_name, profile.email.as_deref())
    }

    /// 建一个用户记录（预置或被邀请的那条路）。
    ///
    /// # Errors
    /// 这个外部账号已经有用户了，或者底层写失败。
    pub fn provision(
        &self,
        account: ExternalAccount,
        display_name: &str,
        email: Option<&str>,
    ) -> Result<User> {
        let user = User {
            id: UserId::generate(),
            account,
            display_name: display_name.to_owned(),
            email: email.map(str::to_owned),
        };
        let envelope = AuditEnvelope::platform(
            kinds::USER_CREATED,
            user.id.as_id(),
            user.id.as_id(),
            serde_json::to_value(&user)
                .map_err(|error| Error::internal(format!("用户装不进载荷：{error}")))?,
        )?;
        self.write(
            USERS,
            RowId::from_id(user.id.as_id()),
            WriteOp::Insert,
            &Actor::Platform,
            &envelope,
        )?;
        Ok(user)
    }

    /// 按外部账号找人。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn user_by_account(&self, account: &ExternalAccount) -> Result<Option<User>> {
        Ok(self
            .all::<User>(USERS)?
            .into_iter()
            .find(|(_, user)| user.account.key() == account.key())
            .map(|(_, user)| user))
    }

    /// 按标识找人。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn user(&self, id: UserId) -> Result<Option<User>> {
        self.load::<User>(USERS, RowId::from_id(id.as_id()))
    }

    // ——————————————————————————————— 令牌 ———————————————————————————————

    /// 签一个令牌。**原文只在这里出现一次**（`TOK-002`）。
    ///
    /// # Errors
    /// 用户不存在，或者底层写失败。
    /// 全部项目。**不判权**——给平台自己的后台维护用。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn all_projects(&self) -> Result<Vec<Project>> {
        Ok(self
            .all::<Project>(PROJECTS)?
            .into_iter()
            .map(|(_, project)| project)
            .collect())
    }

    /// 引导：给一个内建账号签一把令牌，账号不在就先建。
    ///
    /// **第一把令牌只能这样来**——签令牌经 MCP 要先有令牌（`MCP-002`），
    /// 于是第一把无处可来。
    ///
    /// ⚠️ **它绕过了自注册开关（`IDN-003`），这是有意的**:那条开关管的是
    /// "陌生人登录能不能自动建号"，而这条路的调用方**已经能读写数据库了**——
    /// 对他来说自注册开不开没有区别。把它做成一个网络接口才是错的:
    /// 那会是一个免认证的、能签出任意权限凭据的入口。
    ///
    /// # Errors
    /// 账号名不合法，或者底层写失败。
    pub fn bootstrap_token(&self, account: &str) -> Result<TokenSecret> {
        let external = ExternalAccount {
            provider: crate::ProviderId::new("builtin")?,
            account: account.to_owned(),
        };
        let user = match self.user_by_account(&external)? {
            Some(user) => user,
            None => self.provision(external, account, None)?,
        };
        Ok(self.issue_token(user.id, "引导", None)?.1)
    }

    pub fn issue_token(
        &self,
        user: UserId,
        label: &str,
        expires_at: Option<Timestamp>,
    ) -> Result<(Token, TokenSecret)> {
        if self.user(user)?.is_none() {
            return Err(invisible());
        }
        let (token, secret) = token::issue(user, label, self.clock.now(), expires_at)?;
        self.store_token(&token, kinds::TOKEN_ISSUED, WriteOp::Insert)?;
        Ok((token, secret))
    }

    /// 撤销。**立即生效，没有延迟窗口**（`TOK-003`）。
    ///
    /// # Errors
    /// 令牌不是这个人的、不存在，或者底层写失败。两种情形错误一致。
    pub fn revoke_token(&self, owner: UserId, id: TokenId) -> Result<()> {
        let mut token = self
            .load::<Token>(TOKENS, RowId::from_id(id.as_id()))?
            .filter(|token| token.user == owner)
            .ok_or_else(invisible)?;
        token.revoked_at = Some(self.clock.now());
        self.store_token(&token, kinds::TOKEN_REVOKED, WriteOp::Update)
    }

    /// 列出某个人的令牌（不含原文，只有摘要与时间）。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn tokens_of(&self, user: UserId) -> Result<Vec<Token>> {
        Ok(self
            .all::<Token>(TOKENS)?
            .into_iter()
            .map(|(_, token)| token)
            .filter(|token| token.user == user)
            .collect())
    }

    /// 令牌 → 身份。**这是全系统唯一的身份来源**（`TOK-007`、G5）。
    ///
    /// # Errors
    /// `TOK-005`：不存在、已撤销、已过期、格式非法——**四种情形错误一模一样**，
    /// 也不泄露这个令牌是否曾经存在。
    pub fn resolve(&self, secret: &str) -> Result<Identity> {
        if !secret.starts_with(token::SECRET_PREFIX) {
            return Err(token::rejection());
        }
        let offered = token::digest(secret);
        let now = self.clock.now();
        let found = self
            .all::<Token>(TOKENS)?
            .into_iter()
            .map(|(_, token)| token)
            .find(|token| token::constant_time_eq(&token.digest, &offered));

        let Some(mut token) = found else {
            return Err(token::rejection());
        };
        if !token.usable_at(now) {
            return Err(token::rejection());
        }
        let Some(user) = self.user(token.user)? else {
            return Err(token::rejection());
        };

        // TOK-006：记最后一次成功使用。按分钟节流 —— 认证在每一次调用的路径上，
        // 每次都写会让 _tokens 这一张表把全系统的调用串行掉（CON-001 是表级锁）。
        if token::should_touch(&token, now) {
            token.last_used_at = Some(now);
            self.store_token(&token, kinds::TOKEN_USED, WriteOp::Update)?;
        }
        Ok(Identity {
            user,
            token: token.id,
        })
    }

    fn store_token(&self, token: &Token, kind: &str, op: WriteOp) -> Result<()> {
        // 载荷就是这条记录本身 —— 它既是业务状态（`resolve` 要靠它重建），又是这次变更的留痕。
        // 里面有 `digest`：那是 256 位随机原文的单向散列，不可逆；而这是一条平台级事件，
        // 只有令牌的主人读得到（`AUD-003`）。**原文从来没有进过这条路**（`TOK-002`）。
        let envelope = AuditEnvelope::platform(
            kind,
            token.user.as_id(),
            token.id.as_id(),
            serde_json::to_value(token)
                .map_err(|error| Error::internal(format!("令牌装不进载荷：{error}")))?,
        )?;
        self.write(
            TOKENS,
            RowId::from_id(token.id.as_id()),
            op,
            &Actor::Platform,
            &envelope,
        )
    }

    // ——————————————————————————————— 项目与成员 ———————————————————————————————

    /// 建项目。**任何用户都可以建，无需申请或审批；创建者自动成为所有者**（`PRJ-001`）。
    ///
    /// # Errors
    /// 短名被占了、用户不存在，或者底层写失败。
    pub fn create_project(
        &self,
        creator: UserId,
        slug: Slug,
        display_name: &str,
    ) -> Result<Project> {
        if self.user(creator)?.is_none() {
            return Err(invisible());
        }
        let project = Project {
            id: ProjectId::generate(),
            slug,
            display_name: display_name.to_owned(),
            created_at: self.clock.now(),
            archived_at: None,
        };
        let actor = Actor::User {
            user: creator.to_string(),
        };
        self.store_project(&project, kinds::PROJECT_CREATED, WriteOp::Insert, &actor)?;
        // PRJ-001：创建者自动成为所有者。
        self.put_member(
            &Member {
                project: project.id,
                user: creator,
                role: Role::Owner,
                added_at: self.clock.now(),
            },
            kinds::MEMBER_ADDED,
            WriteOp::Insert,
            RowId::generate(),
            &actor,
        )?;
        Ok(project)
    }

    /// 归档。归档后**转为只读**：不再接受任何写操作，历史内容完整保留、可查询（`PRJ-009`）。
    ///
    /// # Errors
    /// 不是所有者、项目不存在（两者错误一致），或者底层写失败。
    pub fn archive_project(&self, actor: UserId, project: ProjectId) -> Result<Project> {
        let (mut record, _) = self.authorize(actor, project, Action::ManageProject)?;
        record.archived_at = Some(self.clock.now());
        self.store_project(
            &record,
            kinds::PROJECT_ARCHIVED,
            WriteOp::Update,
            &Actor::User {
                user: actor.to_string(),
            },
        )?;
        Ok(record)
    }

    /// 查项目详情。
    ///
    /// # Errors
    /// **非成员得到的响应与项目真的不存在时完全一致**（`PRJ-008`）。
    pub fn project(&self, viewer: UserId, project: ProjectId) -> Result<Project> {
        self.authorize(viewer, project, Action::ReadProject)
            .map(|(record, _)| record)
    }

    /// 我参与的项目。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn my_projects(&self, user: UserId) -> Result<Vec<(Project, Role)>> {
        let mut out = Vec::new();
        for member in self.members_of_user(user)? {
            if let Some(project) = self.project_raw(member.project)? {
                out.push((project, member.role));
            }
        }
        out.sort_by(|left, right| left.0.slug.cmp(&right.0.slug));
        Ok(out)
    }

    /// 加成员 / 改角色（`PRJ-005`）。
    ///
    /// # Errors
    /// 不是所有者、项目不存在、被加的人不存在，或者这次改动会让项目没有所有者（`PRJ-006`）。
    pub fn set_member(
        &self,
        actor: UserId,
        project: ProjectId,
        user: UserId,
        role: Role,
    ) -> Result<Member> {
        self.authorize(actor, project, Action::ManageMember)?;
        if self.user(user)?.is_none() {
            return Err(invisible());
        }
        let existing = self.member_row(project, user)?;
        let change = existing
            .as_ref()
            .map_or(MemberChange::Add(role), |_| MemberChange::ChangeRole(role));
        self.guard_owners(project, user, change)?;

        let member = Member {
            project,
            user,
            role,
            added_at: self.clock.now(),
        };
        let (row, kind, op) = match existing {
            Some((row, _)) => (row, kinds::MEMBER_ROLE_CHANGED, WriteOp::Update),
            None => (RowId::generate(), kinds::MEMBER_ADDED, WriteOp::Insert),
        };
        self.put_member(
            &member,
            kind,
            op,
            row,
            &Actor::User {
                user: actor.to_string(),
            },
        )?;
        Ok(member)
    }

    /// 移除成员。
    ///
    /// # Errors
    /// 不是所有者、不存在，或者移的是最后一个所有者（`PRJ-006`）。
    pub fn remove_member(&self, actor: UserId, project: ProjectId, user: UserId) -> Result<()> {
        self.authorize(actor, project, Action::ManageMember)?;
        let (row, member) = self.member_row(project, user)?.ok_or_else(invisible)?;
        self.guard_owners(project, user, MemberChange::Remove)?;
        self.put_member(
            &member,
            kinds::MEMBER_REMOVED,
            WriteOp::Delete,
            row,
            &Actor::User {
                user: actor.to_string(),
            },
        )
    }

    /// 某人在某项目里的角色。**不是成员就是 `None`。**
    ///
    /// # Errors
    /// 底层不可用。
    pub fn role_of(&self, project: ProjectId, user: UserId) -> Result<Option<Role>> {
        Ok(self
            .member_row(project, user)?
            .map(|(_, member)| member.role))
    }

    /// 列出项目成员。
    ///
    /// # Errors
    /// 非成员看不到（`PRJ-008`），或者底层不可用。
    pub fn members(&self, viewer: UserId, project: ProjectId) -> Result<Vec<Member>> {
        self.authorize(viewer, project, Action::ReadProject)?;
        self.members_of_project(project)
    }

    /// 判权。**这是所有写操作前的那一道**。
    ///
    /// # Errors
    /// 项目不存在、调用者不是成员、角色不够、或者项目已归档而这是个写动作——
    /// **四种情形返回同一个错误**（`PRJ-008` + `MCP-008`）。
    pub fn authorize(
        &self,
        user: UserId,
        project: ProjectId,
        action: Action,
    ) -> Result<(Project, Role)> {
        let record = self.project_raw(project)?.ok_or_else(invisible)?;
        let role = self.role_of(project, user)?.ok_or_else(invisible)?;
        if !can_in(role, action, record.is_archived()) {
            return Err(invisible());
        }
        Ok((record, role))
    }

    // ——————————————————————————————— 重建（AUD-004） ———————————————————————————————

    /// 仅凭事件流重建 `until` 时刻的业务状态快照。
    ///
    /// **不读任何当前视图**——投影是缓存，事件流才是权威（`I-D`）。
    /// 这是一个受支持的运维操作，可以对任意截止时刻执行。
    ///
    /// # Errors
    /// 底层不可用或事件损坏。
    pub fn rebuild_at(&self, until: Timestamp) -> Result<Snapshot> {
        Ok(Snapshot {
            users: self.replay::<User>(USERS, until)?,
            projects: self.replay::<Project>(PROJECTS, until)?,
            members: self.replay::<Member>(MEMBERS, until)?,
        })
    }

    fn replay<T: DeserializeOwned>(&self, table: &str, until: Timestamp) -> Result<Vec<T>> {
        let table = TableName::new(table)?;
        let mut state: BTreeMap<RowId, Option<serde_json::Value>> = BTreeMap::new();
        let mut after = 0;
        loop {
            let events = self.engine.events(&table, after, 256)?;
            if events.is_empty() {
                break;
            }
            after = events.last().map_or(after, |event| event.seq);
            for event in events {
                if event.at > until {
                    break;
                }
                let value = match event.op {
                    WriteOp::Delete => None,
                    WriteOp::Insert | WriteOp::Update => {
                        AuditEnvelope::from_payload(&event.payload).map(|envelope| envelope.data)
                    }
                };
                state.insert(event.row, value);
            }
        }
        state
            .into_values()
            .flatten()
            .map(|value| {
                serde_json::from_value(value)
                    .map_err(|error| Error::internal(format!("{table} 的行重建不出来：{error}")))
            })
            .collect()
    }

    // ——————————————————————————————— 内部 ———————————————————————————————

    fn guard_owners(&self, project: ProjectId, user: UserId, change: MemberChange) -> Result<()> {
        let members = self.members_of_project(project)?;
        if owners_after(&members, user, change) == 0 {
            return Err(Error::invalid("一个项目必须始终至少有一个所有者"));
        }
        Ok(())
    }

    fn store_project(
        &self,
        project: &Project,
        kind: &str,
        op: WriteOp,
        actor: &Actor,
    ) -> Result<()> {
        if op == WriteOp::Insert && self.project_by_slug(&project.slug)?.is_some() {
            return Err(Error::conflict(format!("短名 {} 已经被占了", project.slug)));
        }
        let envelope = AuditEnvelope::project_scoped(
            kind,
            project.id.as_id(),
            project.id.as_id(),
            serde_json::to_value(project)
                .map_err(|error| Error::internal(format!("项目装不进载荷：{error}")))?,
        )?;
        self.write(
            PROJECTS,
            RowId::from_id(project.id.as_id()),
            op,
            actor,
            &envelope,
        )
    }

    fn put_member(
        &self,
        member: &Member,
        kind: &str,
        op: WriteOp,
        row: RowId,
        actor: &Actor,
    ) -> Result<()> {
        let envelope = AuditEnvelope::project_scoped(
            kind,
            member.project.as_id(),
            member.user.as_id(),
            serde_json::to_value(member)
                .map_err(|error| Error::internal(format!("成员装不进载荷：{error}")))?,
        )?;
        self.write(MEMBERS, row, op, actor, &envelope)
    }

    fn project_raw(&self, project: ProjectId) -> Result<Option<Project>> {
        self.load::<Project>(PROJECTS, RowId::from_id(project.as_id()))
    }

    fn project_by_slug(&self, slug: &Slug) -> Result<Option<Project>> {
        Ok(self
            .all::<Project>(PROJECTS)?
            .into_iter()
            .find(|(_, project)| project.slug == *slug)
            .map(|(_, project)| project))
    }

    fn member_row(&self, project: ProjectId, user: UserId) -> Result<Option<(RowId, Member)>> {
        Ok(self
            .all::<Member>(MEMBERS)?
            .into_iter()
            .find(|(_, member)| member.project == project && member.user == user))
    }

    fn members_of_project(&self, project: ProjectId) -> Result<Vec<Member>> {
        Ok(self
            .all::<Member>(MEMBERS)?
            .into_iter()
            .map(|(_, member)| member)
            .filter(|member| member.project == project)
            .collect())
    }

    fn members_of_user(&self, user: UserId) -> Result<Vec<Member>> {
        Ok(self
            .all::<Member>(MEMBERS)?
            .into_iter()
            .map(|(_, member)| member)
            .filter(|member| member.user == user)
            .collect())
    }

    fn write(
        &self,
        table: &str,
        row: RowId,
        op: WriteOp,
        actor: &Actor,
        envelope: &AuditEnvelope,
    ) -> Result<()> {
        let receipt = self.engine.write(WriteRequest {
            table: TableName::new(table)?,
            op,
            row,
            payload: envelope.to_payload()?,
            actor: actor.clone(),
        })?;
        // 索引是缓存，落在区间外；漏了不丢数据，rebuild_index 补得回来。
        self.audit.index(&receipt)
    }

    fn load<T: DeserializeOwned>(&self, table: &str, row: RowId) -> Result<Option<T>> {
        let table = TableName::new(table)?;
        let Some(record) = self.engine.read(&table, row)? else {
            return Ok(None);
        };
        let Some(envelope) = AuditEnvelope::from_payload(&record.payload) else {
            return Err(Error::internal(format!("{table} 的行不是一个审计信封")));
        };
        serde_json::from_value(envelope.data)
            .map(Some)
            .map_err(|error| Error::internal(format!("{table} 的行读不回来：{error}")))
    }

    /// 扫一张平台表的全部活行。
    ///
    /// ⚠️ **这是一次全表扫描。** 平台表的规模是「一个部署里的人与项目」，M1 够用；
    /// 真到要建二级索引的那天，它该建在这里，而不是让调用方各自记一份。
    fn all<T: DeserializeOwned>(&self, table: &str) -> Result<Vec<(RowId, T)>> {
        xops_audit::projection::all_strict(self.store.as_ref(), table)
    }
}

/// 平台表的 [`TableName`]。
///
/// # Errors
/// 常量被改坏了。
pub fn platform_tables() -> Result<Vec<TableName>> {
    PLATFORM_TABLES
        .iter()
        .map(|name| TableName::new(*name))
        .collect()
}

/// 平台级事件的 scope 占位。
#[must_use]
pub fn platform_scope() -> Id {
    Id::from_parts(0, 0)
}
