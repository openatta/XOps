//! 审计流的写入、查询、重建与保留期。

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;
use xops_core::{Actor, Error, Event, Id, Result, RowId, TableName, Timestamp, WriteOp};
use xops_store::{Receipt, Store, WriteEngine, WriteRequest, keys, space};

use crate::envelope::{AuditEnvelope, EventKind, Outcome, kinds};

/// 没有业务行的那些动作，留痕落在这张表上。
pub const AUDIT_TABLE: &str = "_audit";

/// 索引空间：`scope(16) || at(8) || seq(8) || 表名`。
///
/// 只建这**一个**索引，按"所属项目 + 时间"。理由不是省事：`AUD-003`（非成员查不到该项目的
/// 任何事件）是**可见性**要求，它必须在扫描之前就成立，而不是查完再筛——按项目前缀扫，
/// 越权的行根本不进结果集。其余四个维度（类型、行为人、目标、时间细粒度）在这个前缀之内
/// 过滤，那时候的集合已经是调用者本来就有权看的。
const INDEX: &str = "audit-index";
/// 平台级事件（不属于任何项目）在索引里的 scope。
const PLATFORM_SCOPE: [u8; 16] = [0; 16];

/// 一条审计记录：事件的那几样 + 信封里的那几样。
#[derive(Debug, Clone, PartialEq)]
pub struct AuditRecord {
    /// 稳定事件标识（`AUD-002`）。
    pub id: Id,
    pub at: Timestamp,
    pub actor: Actor,
    /// 它在哪张表的事件流上。`_audit` 表示这条动作没有自己的业务行。
    pub table: TableName,
    pub seq: u64,
    pub row: RowId,
    pub envelope: AuditEnvelope,
}

impl AuditRecord {
    fn from_event(event: Event) -> Option<Self> {
        let envelope = AuditEnvelope::from_payload(&event.payload)?;
        Some(Self {
            id: event.id,
            at: event.at,
            actor: event.actor,
            table: event.table,
            seq: event.seq,
            row: event.row,
            envelope,
        })
    }
}

/// 查询条件（`AUD-008`）。
///
/// `viewer` 不是一个可选的过滤器，是**可见性判定的入口**：它决定这次查询能看见哪个 scope。
#[derive(Debug, Clone)]
pub struct Query {
    /// 查哪个项目的事件流。`None` 查平台级事件。
    pub project: Option<Id>,
    /// 查询者。平台级事件只有主体本人读得到（`AUD-003`）。
    pub viewer: Id,
    pub kind: Option<EventKind>,
    pub actor: Option<Actor>,
    pub target: Option<Id>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    pub limit: usize,
}

impl Query {
    /// 某个项目的事件流。**调用方必须先确认 `viewer` 是这个项目的成员**——
    /// 本层不做成员判定（那是 `xops-identity` 的事），它只负责不越过 scope。
    #[must_use]
    pub fn in_project(project: Id, viewer: Id) -> Self {
        Self {
            project: Some(project),
            viewer,
            kind: None,
            actor: None,
            target: None,
            since: None,
            until: None,
            limit: 100,
        }
    }

    /// 平台级事件（建用户、签令牌这些），只看得到自己是主体的那些。
    #[must_use]
    pub fn platform(viewer: Id) -> Self {
        Self {
            project: None,
            ..Self::in_project(Id::from_parts(0, 0), viewer)
        }
    }

    #[must_use]
    pub fn of_kind(mut self, kind: EventKind) -> Self {
        self.kind = Some(kind);
        self
    }

    #[must_use]
    pub fn by(mut self, actor: Actor) -> Self {
        self.actor = Some(actor);
        self
    }

    #[must_use]
    pub fn about(mut self, target: Id) -> Self {
        self.target = Some(target);
        self
    }

    #[must_use]
    pub fn between(mut self, since: Timestamp, until: Timestamp) -> Self {
        self.since = Some(since);
        self.until = Some(until);
        self
    }

    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    fn matches(&self, record: &AuditRecord) -> bool {
        if self.project != record.envelope.project {
            return false;
        }
        if record.envelope.project.is_none() && record.envelope.subject != Some(self.viewer) {
            // 平台级事件只有主体本人可读（AUD-003）。
            return false;
        }
        if self
            .kind
            .as_ref()
            .is_some_and(|kind| *kind != record.envelope.kind)
        {
            return false;
        }
        if self
            .actor
            .as_ref()
            .is_some_and(|actor| *actor != record.actor)
        {
            return false;
        }
        if self
            .target
            .is_some_and(|target| target != record.envelope.target)
        {
            return false;
        }
        if self.since.is_some_and(|since| record.at < since) {
            return false;
        }
        if self.until.is_some_and(|until| record.at > until) {
            return false;
        }
        true
    }
}

/// 审计流。
///
/// 它**不是第二个账本**——写入路径只有一条（[`WriteEngine`]），这里只多做两件事：
/// 给没有业务行的动作提供一张落脚的表，以及维护那一个索引。
pub struct AuditLog {
    engine: Arc<WriteEngine>,
    store: Arc<dyn Store>,
    /// 哪些表的事件流上会出现审计信封。重建索引时要走一遍它们。
    watched: BTreeSet<TableName>,
}

impl std::fmt::Debug for AuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLog")
            .field("watched", &self.watched)
            .finish_non_exhaustive()
    }
}

impl AuditLog {
    /// # Errors
    /// `_audit` 这个表名不合法——只可能是常量被改坏了。
    pub fn new(engine: Arc<WriteEngine>, store: Arc<dyn Store>) -> Result<Self> {
        let mut watched = BTreeSet::new();
        watched.insert(TableName::new(AUDIT_TABLE)?);
        Ok(Self {
            engine,
            store,
            watched,
        })
    }

    /// 登记一张会带审计信封的表。重建索引时会走它的事件流。
    #[must_use]
    pub fn watching(mut self, table: TableName) -> Self {
        self.watched.insert(table);
        self
    }

    /// 追加一条没有业务行的留痕。
    ///
    /// 有业务行的动作**不要走这里**——它自己那次写已经是审计记录了，再写一条就是两份账。
    ///
    /// # Errors
    /// 底层写失败。
    pub fn append(&self, actor: &Actor, envelope: &AuditEnvelope) -> Result<AuditRecord> {
        let table = TableName::new(AUDIT_TABLE)?;
        let receipt = self.engine.write(WriteRequest {
            table,
            op: WriteOp::Insert,
            row: RowId::generate(),
            payload: envelope.to_payload()?,
            actor: actor.clone(),
        })?;
        self.index(&receipt)?;
        AuditRecord::from_event(receipt.primary().clone())
            .ok_or_else(|| Error::internal("刚写进去的信封读不回来"))
    }

    /// 把一次写产生的事件登记进索引。
    ///
    /// **有业务行的写由它的所有者在写完之后调这个**——`WriteEngine` 不认识信封，
    /// 也不该认识。索引是缓存：漏登记不丢数据，[`Self::rebuild_index`] 能补回来。
    ///
    /// # Errors
    /// 底层写失败。
    pub fn index(&self, receipt: &Receipt) -> Result<()> {
        for event in receipt.events() {
            let Some(record) = AuditRecord::from_event(event.clone()) else {
                continue;
            };
            self.store.put(INDEX, &index_key(&record), &[])?;
        }
        Ok(())
    }

    /// 查（`AUD-008`）。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn query(&self, query: &Query) -> Result<Vec<AuditRecord>> {
        let scope = scope_bytes(query.project);
        let mut out = Vec::new();
        let mut cursor: Option<Vec<u8>> = None;
        loop {
            let page = self.store.scan(INDEX, &scope, cursor.as_deref(), 256)?;
            if page.is_empty() {
                break;
            }
            cursor = page.last().map(|(key, _)| key.clone());
            for (key, _) in page {
                let Some((table, seq)) = decode_index_key(&key) else {
                    continue;
                };
                let Some(record) = self.load(&table, seq)? else {
                    continue;
                };
                if query.matches(&record) {
                    out.push(record);
                    if out.len() >= query.limit {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }

    /// 某个对象的完整历史（`AUD-008` 后半句）。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn history(&self, target: Id, query: &Query) -> Result<Vec<AuditRecord>> {
        self.query(&query.clone().about(target))
    }

    /// 从被登记的那些表的事件流重建索引（`AUD-004` 的一半）。
    ///
    /// 索引是缓存不是权威——**事件流才是**。这个操作可以随时重跑，结果只与事件流有关。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn rebuild_index(&self) -> Result<usize> {
        for (key, _) in self.store.scan(INDEX, &[], None, usize::MAX)? {
            self.store.delete(INDEX, &key)?;
        }
        let mut rebuilt = 0;
        for table in &self.watched {
            let mut after = 0;
            loop {
                let events = self.engine.events(table, after, 256)?;
                if events.is_empty() {
                    break;
                }
                after = events.last().map_or(after, |event| event.seq);
                for event in events {
                    let Some(record) = AuditRecord::from_event(event) else {
                        continue;
                    };
                    self.store.put(INDEX, &index_key(&record), &[])?;
                    rebuilt += 1;
                }
            }
        }
        Ok(rebuilt)
    }

    /// 到期清理（`AUD-010`）：**整批按时间**删掉 `before` 之前的留痕，不得选择性删个别条。
    ///
    /// ⚠️ **只吃 `_audit` 表上的记录。** 有业务行的那些事件不是"留痕"，它们**是业务状态本身**
    /// （项目、成员、令牌都由事件流重建）——删了它们，`AUD-004` 当场落空。
    /// 保留期管的是"没有业务行的动作留痕"，这是这条与 `RET-001` 分得开的地方。
    ///
    /// 返回删掉的条数。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn prune(&self, before: Timestamp) -> Result<usize> {
        let audit = TableName::new(AUDIT_TABLE)?;
        let mut pruned = 0;
        let mut after = 0;
        loop {
            let events = self.engine.events(&audit, after, 256)?;
            if events.is_empty() {
                break;
            }
            after = events.last().map_or(after, |event| event.seq);
            for event in events {
                if event.at >= before {
                    // 事件按序号升序，而序号与时间同向 —— 到这里就不会再有更旧的了。
                    return Ok(pruned);
                }
                let Some(record) = AuditRecord::from_event(event) else {
                    continue;
                };
                self.store.delete(INDEX, &index_key(&record))?;
                self.store
                    .delete(space::ROW, &keys::row(&record.table, record.row))?;
                self.store
                    .delete(space::EVENT, &keys::event(&record.table, record.seq))?;
                pruned += 1;
            }
        }
        Ok(pruned)
    }

    /// 记一条"清理发生过"——清理本身也要留痕（`AUD-011`）。
    ///
    /// # Errors
    /// 底层写失败。
    pub fn record_prune(&self, actor: &Actor, before: Timestamp, pruned: usize) -> Result<()> {
        let envelope = AuditEnvelope {
            kind: EventKind::new(kinds::AUDIT_PRUNED)?,
            project: None,
            target: Id::from_parts(0, 0),
            subject: None,
            outcome: Outcome::Succeeded,
            data: serde_json::json!({ "before": before.as_millis(), "pruned": pruned }),
        };
        self.append(actor, &envelope).map(|_| ())
    }

    fn load(&self, table: &TableName, seq: u64) -> Result<Option<AuditRecord>> {
        let Some(bytes) = self.store.get(space::EVENT, &keys::event(table, seq))? else {
            return Ok(None);
        };
        let event: Event = serde_json::from_slice(&bytes)
            .map_err(|error| Error::internal(format!("事件读不回来：{error}")))?;
        Ok(AuditRecord::from_event(event))
    }
}

fn scope_bytes(project: Option<Id>) -> Vec<u8> {
    project.map_or_else(|| PLATFORM_SCOPE.to_vec(), |id| id.as_bytes().to_vec())
}

fn index_key(record: &AuditRecord) -> Vec<u8> {
    let mut key = scope_bytes(record.envelope.project);
    // 时间在前、序号在后：同一 scope 内按时间升序，同刻内按序号定序。
    key.extend_from_slice(&record.at.as_millis().to_be_bytes());
    key.extend_from_slice(&record.seq.to_be_bytes());
    key.extend_from_slice(record.table.as_str().as_bytes());
    key
}

fn decode_index_key(key: &[u8]) -> Option<(TableName, u64)> {
    const HEAD: usize = 16 + 8 + 8;
    if key.len() <= HEAD {
        return None;
    }
    let seq = u64::from_be_bytes(key[24..HEAD].try_into().ok()?);
    let table = TableName::new(String::from_utf8(key[HEAD..].to_vec()).ok()?).ok()?;
    Some((table, seq))
}

/// `Value` 的一个小便利：`AuditEnvelope` 之外的载荷。
#[must_use]
pub fn data(pairs: &[(&str, Value)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
    )
}
