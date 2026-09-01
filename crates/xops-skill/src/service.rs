//! 技能的读写面与可见性。
//!
//! 这个文件里最要紧的一条是 `SKL-009`：
//!
//! > **私有技能能读项目数据，是因为它的所有者是项目成员。权限来自人，不来自技能**——
//! > 一个人退出项目，他挂在这个项目里的私有任务立刻失去数据源，不能再执行。
//!
//! 所以可见性与可执行性**每次都现算**，不缓存在技能上。缓存一次，退出项目那条就失效了。

use std::sync::Arc;

use xops_audit::{AuditEnvelope, AuditLog};
use xops_core::{Actor, Clock, Error, Id, Result, RowId, TableName, Timestamp, WriteOp};
use xops_identity::{Action, Directory, ProjectId, UserId};
use xops_store::{Row, Store, WriteEngine, WriteRequest, keys, space};

use crate::declaration::Declaration;
use crate::skill::{Ownership, Skill, SkillId, State, Version};

/// 技能与版本落在这两张平台表上。
pub const SKILLS_TABLE: &str = "_skills";
pub const VERSIONS_TABLE: &str = "_skill_versions";

/// 事件类型。
pub mod kinds {
    pub const SKILL_CREATED: &str = "skill.created";
    pub const SKILL_VERSIONED: &str = "skill.versioned";
    pub const SKILL_TESTED: &str = "skill.tested";
    pub const SKILL_PUBLISHED: &str = "skill.published";
    pub const SKILL_DISABLED: &str = "skill.disabled";
    pub const SKILL_DERIVED: &str = "skill.derived";
}

/// 技能资产。
pub struct Skills {
    engine: Arc<WriteEngine>,
    store: Arc<dyn Store>,
    audit: Arc<AuditLog>,
    directory: Arc<Directory>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Skills {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Skills").finish_non_exhaustive()
    }
}

/// 一个技能的某个版本，连同它的技能。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub skill: Skill,
    pub version: Version,
}

impl Skills {
    #[must_use]
    pub fn new(
        engine: Arc<WriteEngine>,
        store: Arc<dyn Store>,
        audit: Arc<AuditLog>,
        directory: Arc<Directory>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            engine,
            store,
            audit,
            directory,
            clock,
        }
    }

    /// 建一个技能，连同它的第一个版本（草稿）。
    ///
    /// ⚠️ **上传不执行**（`SKL-004`）：这条路径上没有任何提交执行的调用。
    /// 这条防的不主要是别人，是作者自己。
    ///
    /// # Errors
    /// 没权限 / 项目不存在 · 声明不合法 · 名字不合法。
    pub fn create(
        &self,
        actor: UserId,
        project: ProjectId,
        name: &str,
        ownership: Ownership,
        content: &str,
        declaration: Declaration,
    ) -> Result<Resolved> {
        self.directory
            .authorize(actor, project, Action::WriteSkill)?;
        declaration.check()?;
        if name.is_empty() || name.len() > 64 {
            return Err(Error::invalid("技能名要 1–64 字节"));
        }
        if let Ownership::Private { owner } = ownership
            && owner != actor
        {
            return Err(Error::invalid("只能给自己建私有技能"));
        }
        let skill = Skill {
            id: SkillId::generate(),
            project,
            name: name.to_owned(),
            ownership,
            created_by: actor,
            created_at: self.clock.now(),
        };
        let version = Version {
            skill: skill.id,
            project,
            version: 1,
            content: content.to_owned(),
            declaration,
            state: State::Draft,
            created_by: actor,
            created_at: self.clock.now(),
            tested_run: None,
            published_at: None,
            used_for_settlement: false,
        };
        self.put_skill(&skill, kinds::SKILL_CREATED, WriteOp::Insert, actor)?;
        self.put_version(&version, kinds::SKILL_VERSIONED, WriteOp::Insert, actor)?;
        Ok(Resolved { skill, version })
    }

    /// 更新内容或声明。**产生新版本**（`SKL-001`），旧版本原样保留（`SKL-002`）。
    ///
    /// # Errors
    /// 没权限 · 看不到这个技能 · 声明不合法。
    pub fn update(
        &self,
        actor: UserId,
        skill: SkillId,
        content: &str,
        declaration: Declaration,
    ) -> Result<Version> {
        declaration.check()?;
        let resolved = self.require_writable(actor, skill)?;
        let next = self
            .versions(skill)?
            .iter()
            .map(|version| version.version)
            .max()
            .unwrap_or(0)
            + 1;
        let version = Version {
            skill,
            project: resolved.skill.project,
            version: next,
            content: content.to_owned(),
            declaration,
            state: State::Draft,
            created_by: actor,
            created_at: self.clock.now(),
            tested_run: None,
            published_at: None,
            used_for_settlement: false,
        };
        self.put_version(&version, kinds::SKILL_VERSIONED, WriteOp::Insert, actor)?;
        Ok(version)
    }

    /// 记下一次**成功的**测试执行（`SKL-003`）。
    ///
    /// **测试执行由 RP-11 发起并跑完，本包只收下这个事实。** 这也是为什么
    /// "未测试不可发布"这条能在 RP-11 完成之前就验收：伪造一条记录即可。
    ///
    /// # Errors
    /// 没权限 · 看不到 · 没有这个版本。
    pub fn record_successful_test(
        &self,
        actor: UserId,
        skill: SkillId,
        version: u32,
        run: Id,
    ) -> Result<Version> {
        self.require_writable(actor, skill)?;
        let mut record = self.version(skill, version)?;
        record.tested_run = Some(run);
        self.put_version(&record, kinds::SKILL_TESTED, WriteOp::Update, actor)?;
        Ok(record)
    }

    /// 发布。
    ///
    /// # Errors
    /// 没权限 · 没有成功的测试执行 · 已经不是草稿。
    pub fn publish(&self, actor: UserId, skill: SkillId, version: u32) -> Result<Version> {
        self.require_writable(actor, skill)?;
        let mut record = self.version(skill, version)?;
        record.check_publishable()?;
        record.state = State::Published;
        record.published_at = Some(self.clock.now());
        self.put_version(&record, kinds::SKILL_PUBLISHED, WriteOp::Update, actor)?;
        Ok(record)
    }

    /// 停用。**不再被触发，历史执行记录完整保留。**
    ///
    /// # Errors
    /// 没权限 · 看不到。
    pub fn disable(&self, actor: UserId, skill: SkillId, version: u32) -> Result<Version> {
        self.require_writable(actor, skill)?;
        let mut record = self.version(skill, version)?;
        record.state = State::Disabled;
        self.put_version(&record, kinds::SKILL_DISABLED, WriteOp::Update, actor)?;
        Ok(record)
    }

    /// 从一份技能派生一份私有副本（`SKL-010`）。
    ///
    /// **是一次拷贝而不是引用**——改私有副本不影响公共的。
    ///
    /// # Errors
    /// 没权限 · 看不到源技能 · 源技能不在这个项目里。
    pub fn derive_private(&self, actor: UserId, source: SkillId) -> Result<Resolved> {
        let resolved = self.read(actor, source)?;
        self.directory
            .authorize(actor, resolved.skill.project, Action::WriteSkill)?;
        // SKL-012：跨项目复用不做（Q14）。派生一律留在同一个项目里。
        let copy = self.create(
            actor,
            resolved.skill.project,
            &format!("{}（私有副本）", resolved.skill.name),
            Ownership::Private { owner: actor },
            &resolved.version.content,
            resolved.version.declaration.clone(),
        )?;
        let envelope = AuditEnvelope::project_scoped(
            kinds::SKILL_DERIVED,
            resolved.skill.project.as_id(),
            copy.skill.id.as_id(),
            serde_json::json!({"from": source.to_string(), "to": copy.skill.id.to_string()}),
        )?;
        self.audit.append(
            &Actor::User {
                user: actor.to_string(),
            },
            &envelope,
        )?;
        Ok(copy)
    }

    /// 读一个技能的最新版本。**可见性在这里判**。
    ///
    /// # Errors
    /// 看不到——**与"不存在"完全一致**。
    pub fn read(&self, viewer: UserId, skill: SkillId) -> Result<Resolved> {
        let record = self.skill(skill)?;
        let latest = self
            .versions(skill)?
            .into_iter()
            .max_by_key(|version| version.version)
            .ok_or_else(|| Error::not_found("不存在"))?;
        if !self.visible(viewer, &record, &latest)? {
            return Err(Error::not_found("不存在"));
        }
        Ok(Resolved {
            skill: record,
            version: latest,
        })
    }

    /// 列出我看得见的技能。
    ///
    /// # Errors
    /// 非成员看不到这个项目。
    pub fn list(&self, viewer: UserId, project: ProjectId) -> Result<Vec<Resolved>> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        let mut out = Vec::new();
        for record in self.all_skills()? {
            if record.project != project {
                continue;
            }
            let Some(latest) = self
                .versions(record.id)?
                .into_iter()
                .max_by_key(|version| version.version)
            else {
                continue;
            };
            if self.visible(viewer, &record, &latest)? {
                out.push(Resolved {
                    skill: record,
                    version: latest,
                });
            }
        }
        Ok(out)
    }

    /// 这个技能版本现在能不能被用于执行。
    ///
    /// ⚠️ **`SKL-009` 就在这里**：私有技能的所有者若已不是项目成员，**立刻不能再执行**。
    /// 判定每次现算，不缓存——缓存一次，"退出项目即失效"那条就没了。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn runnable_for(&self, skill: SkillId, version: u32) -> Result<bool> {
        let record = self.skill(skill)?;
        let version = self.version(skill, version)?;
        if !version.runnable() {
            return Ok(false);
        }
        match record.ownership {
            Ownership::Public => Ok(true),
            Ownership::Private { owner } => {
                // 权限来自人，不来自技能。
                Ok(self.directory.role_of(record.project, owner)?.is_some())
            }
        }
    }

    /// `SKL-011` 的标记：这个版本被用于满足过某个流程节点。**由 RP-15 打。**
    ///
    /// # Errors
    /// 没有这个版本。
    pub fn mark_used_for_settlement(&self, skill: SkillId, version: u32) -> Result<()> {
        let mut record = self.version(skill, version)?;
        if record.used_for_settlement {
            return Ok(());
        }
        record.used_for_settlement = true;
        self.put_version_as_platform(&record, kinds::SKILL_PUBLISHED, WriteOp::Update)
    }

    // ——————————————————————————————— 内部 ———————————————————————————————

    /// 谁看得见这个版本。
    fn visible(&self, viewer: UserId, skill: &Skill, version: &Version) -> Result<bool> {
        if self.directory.role_of(skill.project, viewer)?.is_none() {
            return Ok(false);
        }
        Ok(match skill.ownership {
            Ownership::Public => true,
            Ownership::Private { owner } => {
                // 本人看得见；别的成员**只有在它被用于满足过流程节点之后**才看得见
                // ——私有是为了不打扰别人，不是为了让自动决策不可审查（`SKL-011`）。
                owner == viewer || version.used_for_settlement
            }
        })
    }

    fn require_writable(&self, actor: UserId, skill: SkillId) -> Result<Resolved> {
        let resolved = self.read(actor, skill)?;
        self.directory
            .authorize(actor, resolved.skill.project, Action::WriteSkill)?;
        if let Ownership::Private { owner } = resolved.skill.ownership
            && owner != actor
        {
            return Err(Error::not_found("不存在"));
        }
        Ok(resolved)
    }

    fn skill(&self, skill: SkillId) -> Result<Skill> {
        self.load::<Skill>(SKILLS_TABLE, RowId::from_id(skill.as_id()))?
            .ok_or_else(|| Error::not_found("不存在"))
    }

    fn version(&self, skill: SkillId, version: u32) -> Result<Version> {
        self.versions(skill)?
            .into_iter()
            .find(|record| record.version == version)
            .ok_or_else(|| Error::not_found("不存在"))
    }

    /// 一个技能的全部版本。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn versions(&self, skill: SkillId) -> Result<Vec<Version>> {
        Ok(self
            .all::<Version>(VERSIONS_TABLE)?
            .into_iter()
            .filter(|version| version.skill == skill)
            .collect())
    }

    fn all_skills(&self) -> Result<Vec<Skill>> {
        self.all::<Skill>(SKILLS_TABLE)
    }

    fn put_skill(&self, skill: &Skill, kind: &str, op: WriteOp, actor: UserId) -> Result<()> {
        self.write(
            SKILLS_TABLE,
            RowId::from_id(skill.id.as_id()),
            skill.project,
            skill.id.as_id(),
            kind,
            op,
            skill,
            &Actor::User {
                user: actor.to_string(),
            },
        )
    }

    fn put_version(&self, version: &Version, kind: &str, op: WriteOp, actor: UserId) -> Result<()> {
        self.write(
            VERSIONS_TABLE,
            version_row(version),
            version.project,
            version.skill.as_id(),
            kind,
            op,
            version,
            &Actor::User {
                user: actor.to_string(),
            },
        )
    }

    fn put_version_as_platform(&self, version: &Version, kind: &str, op: WriteOp) -> Result<()> {
        self.write(
            VERSIONS_TABLE,
            version_row(version),
            version.project,
            version.skill.as_id(),
            kind,
            op,
            version,
            &Actor::Platform,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "内部写入辅助，摊平比包一层结构更好读"
    )]
    fn write<T: serde::Serialize>(
        &self,
        table: &str,
        row: RowId,
        project: ProjectId,
        target: Id,
        kind: &str,
        op: WriteOp,
        value: &T,
        actor: &Actor,
    ) -> Result<()> {
        let envelope = AuditEnvelope::project_scoped(
            kind,
            project.as_id(),
            target,
            serde_json::to_value(value)
                .map_err(|error| Error::internal(format!("装不下：{error}")))?,
        )?;
        let receipt = self.engine.write(WriteRequest {
            table: TableName::new(table)?,
            op,
            row,
            payload: envelope.to_payload()?,
            actor: actor.clone(),
        })?;
        self.audit.index(&receipt)
    }

    fn load<T: serde::de::DeserializeOwned>(&self, table: &str, row: RowId) -> Result<Option<T>> {
        let table = TableName::new(table)?;
        let Some(record) = self.engine.read(&table, row)? else {
            return Ok(None);
        };
        let Some(envelope) = AuditEnvelope::from_payload(&record.payload) else {
            return Err(Error::internal("行不是一个审计信封"));
        };
        serde_json::from_value(envelope.data)
            .map(Some)
            .map_err(|error| Error::internal(format!("读不回来：{error}")))
    }

    fn all<T: serde::de::DeserializeOwned>(&self, table: &str) -> Result<Vec<T>> {
        let table = TableName::new(table)?;
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
                if let Ok(value) = serde_json::from_value::<T>(envelope.data) {
                    out.push(value);
                }
            }
        }
        Ok(out)
    }
}

/// 版本的行标识由 `(技能, 版本号)` 定死——**同一个版本的每次状态变更都落在同一行上**，
/// 它的单行历史就是这个版本的状态变更史。
///
/// ⚠️ 早先这里是"把版本号异或进技能 id 的低位"，那样有两个坑，都踩过：
/// 编码时 `>> 3` 会把最低几位丢掉，于是版本 1 与 2 落到同一行；而同一毫秒建的两个技能
/// id 只差一个计数器，异或之后也可能撞上。**散列整个 `(技能, 版本)` 才是对的。**
fn version_row(version: &Version) -> RowId {
    let seed = format!("{}#{}", version.skill, version.version);
    let low = fnv1a(seed.as_bytes());
    let high = fnv1a(&low.to_be_bytes());
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&high.to_be_bytes());
    bytes[8..].copy_from_slice(&low.to_be_bytes());
    RowId::from_id(Id::parse(&encode(bytes)).unwrap_or_else(|_| Id::from_parts(0, u128::from(low))))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

fn encode(bytes: [u8; 16]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let value = u128::from_be_bytes(bytes) >> 3;
    let mut out = [0u8; 26];
    for (index, slot) in out.iter_mut().enumerate() {
        let shift = 5 * (26 - 1 - index);
        *slot = ALPHABET[usize::try_from((value >> shift) & 0x1F).unwrap_or(0)];
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 让 `Timestamp` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _TimestampLink = Timestamp;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declaration::OutputShape;

    fn version(skill: SkillId, number: u32) -> Version {
        Version {
            skill,
            project: ProjectId::generate(),
            version: number,
            content: String::new(),
            declaration: Declaration {
                inputs: vec![],
                output: OutputShape::Report,
                needs_repository: false,
                network: vec![],
                max_duration_millis: 1,
            },
            state: State::Draft,
            created_by: UserId::generate(),
            created_at: Timestamp::from_millis(0),
            tested_run: None,
            published_at: None,
            used_for_settlement: false,
        }
    }

    #[test]
    fn 相邻版本号不会落到同一行() {
        let skill = SkillId::generate();
        let rows: std::collections::BTreeSet<RowId> = (1..=64)
            .map(|number| version_row(&version(skill, number)))
            .collect();
        assert_eq!(rows.len(), 64, "版本 1 与 2 撞在一起是踩过的那个坑");
    }

    #[test]
    fn 同一毫秒建的两个技能也不会撞() {
        let first = SkillId::generate();
        let second = SkillId::generate();
        for number in 1..=8 {
            assert_ne!(
                version_row(&version(first, number)),
                version_row(&version(second, number))
            );
        }
    }

    #[test]
    fn 同一个版本每次都算出同一行() {
        let skill = SkillId::generate();
        assert_eq!(
            version_row(&version(skill, 3)),
            version_row(&version(skill, 3))
        );
    }
}
