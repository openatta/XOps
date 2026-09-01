//! 审计流的写入、查询、重建与保留期。

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;
use xops_core::{Actor, Error, Event, Id, Result, RowId, TableName, Timestamp, WriteOp};
use xops_store::{
    Column, Receipt, Relation, Relations, Select, Store, WriteEngine, WriteRequest, keys, space,
};

use crate::envelope::{AuditEnvelope, EventKind, Outcome, kinds};

/// 没有业务行的那些动作，留痕落在这张表上。
pub const AUDIT_TABLE: &str = "_audit";

/// 索引空间：`scope(16) || at(8) || seq(8) || 表名`。
///
/// 只建这**一个**索引，按"所属项目 + 时间"。理由不是省事：`AUD-003`（非成员查不到该项目的
/// 任何事件）是**可见性**要求，它必须在扫描之前就成立，而不是查完再筛——按项目前缀扫，
/// 越权的行根本不进结果集。其余四个维度（类型、行为人、目标、时间细粒度）在这个前缀之内
/// 过滤，那时候的集合已经是调用者本来就有权看的。
/// 审计的**关系投影**：一张真表，`project` · `at` · `kind` · `target` 上有真索引（`D60`）。
///
/// # 它取代了什么
///
/// 早先这里是一条**手写的键值二级索引**：键是 `scope|时刻|序号|表名`，值是空的。
/// 那条索引能做的只有"按 scope 前缀扫、按时间排"——`kind` / `actor` / `target`
/// 这几样筛选还是得把每一条记录从事件流里读出来再比。
///
/// **手写索引是在重新实现数据库已经做好的事**：自己维护、自己修、自己保证一致，
/// 而换回来的能力还不如一条真索引。现在筛选全部发生在 SQL 里，
/// **只有命中的那几条才会去事件流取内容**。
///
/// # 存的是指针，不是内容
///
/// 载荷只有 `(表, 序号)`。**事件流仍然是唯一的一份内容**——
/// 这一层是索引，索引不该把被索引的东西再抄一遍。
pub const AUDIT_RELATION: &str = "audit";

/// 关系投影的列。
fn audit_relation() -> Relation {
    Relation {
        name: AUDIT_RELATION.to_owned(),
        columns: vec![
            // 平台级事件这一列**是 NULL**，不是某个占位值——`IS NULL` 才查得干净。
            Column::text("project", true),
            Column::integer("at", true),
            Column::text("kind", true),
            Column::text("target", true),
            // 平台级事件的可见性判定按它（`AUD-003`）。
            Column::text("subject", true),
            Column::text("actor", false),
            // ⚠️ 排序键：`at` 与序号拼成定宽十六进制，**字典序即 (时刻, 序号) 序**。
            // 单独一列是因为同一毫秒内的两条要有确定的先后——
            // 光按 `at` 排，同刻的顺序就交给引擎决定了，而那在两个实现之间会漂。
            Column::text("orderKey", true),
        ],
    }
}

/// 排序键：定宽十六进制拼接，**字典序即 `(at, seq)` 序**。
fn order_key(at: Timestamp, seq: u64) -> String {
    format!("{:016x}{seq:016x}", at.as_millis())
}

/// 一条记录的索引列。
fn index_columns(record: &AuditRecord) -> serde_json::Value {
    serde_json::json!({
        "project": record.envelope.project.map(|id| id.to_string()),
        "at": record.at.as_millis(),
        "kind": record.envelope.kind.as_str(),
        "target": record.envelope.target.to_string(),
        "subject": record.envelope.subject.map(|id| id.to_string()),
        "actor": serde_json::to_string(&record.actor).unwrap_or_default(),
        "orderKey": order_key(record.at, record.seq),
    })
}

/// 索引里存的**指针**：内容仍然只在事件流里有一份。
fn index_pointer(record: &AuditRecord) -> serde_json::Value {
    serde_json::json!({"table": record.table.as_str(), "seq": record.seq})
}

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
    /// 从一条事件解出一条审计记录。**不是信封就返回 `None`**——
    /// 同一条事件流上并存着业务行与留痕，解不出来的那些不是错误。
    #[must_use]
    pub fn from_event(event: Event) -> Option<Self> {
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

    /// 一条记录合不合这次查询。
    ///
    /// ⚠️ **它是参考语义，不是查询路径。** 真正的筛选发生在 SQL 里
    /// （见 [`AuditLog::query`]）——留着它是为了**有一个东西能与那份翻译对答案**：
    /// 把筛选翻成 `WHERE` 是最容易漏一条、错一个边界的地方，
    /// 而一份独立的、显然正确的实现就是最好的对照。验收里有一条逐条比对的测试。
    #[must_use]
    pub fn matches(&self, record: &AuditRecord) -> bool {
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
    relations: Arc<dyn Relations>,
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
    /// `_audit` 这个表名不合法（只可能是常量被改坏了）· 关系投影建不起来。
    pub fn new(
        engine: Arc<WriteEngine>,
        store: Arc<dyn Store>,
        relations: Arc<dyn Relations>,
    ) -> Result<Self> {
        relations.declare(&audit_relation())?;
        let mut watched = BTreeSet::new();
        watched.insert(TableName::new(AUDIT_TABLE)?);
        Ok(Self {
            engine,
            store,
            relations,
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
            self.relations.upsert(
                AUDIT_RELATION,
                RowId::from_id(record.id),
                &index_columns(&record),
                &index_pointer(&record),
            )?;
        }
        Ok(())
    }

    /// 查（`AUD-008`）。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn query(&self, query: &Query) -> Result<Vec<AuditRecord>> {
        // **筛选全部发生在 SQL 里**，只有命中的那几条才去事件流取内容。
        let mut select = Select::new().oldest_first("orderKey").take(query.limit);
        match query.project {
            Some(project) => select = select.equal("project", project.to_string()),
            None => {
                // 平台级事件**只有主体本人读得到**（`AUD-003`）——
                // 这一条现在是 WHERE 的一部分，不是取回来之后再过滤掉。
                select = select
                    .null("project")
                    .equal("subject", query.viewer.to_string());
            }
        }
        if let Some(kind) = &query.kind {
            select = select.equal("kind", kind.as_str());
        }
        if let Some(target) = query.target {
            select = select.equal("target", target.to_string());
        }
        if let Some(actor) = &query.actor {
            select = select.equal(
                "actor",
                serde_json::to_string(actor)
                    .map_err(|error| Error::internal(format!("actor 装不下：{error}")))?,
            );
        }
        if let Some(since) = query.since {
            select = select.no_earlier_than("at", since.as_millis());
        }
        if let Some(until) = query.until {
            select = select.no_later_than("at", until.as_millis());
        }

        let mut out = Vec::new();
        for (_, pointer) in self.relations.select(AUDIT_RELATION, &select)? {
            let Some(table) = pointer.get("table").and_then(Value::as_str) else {
                continue;
            };
            let Some(seq) = pointer.get("seq").and_then(Value::as_u64) else {
                continue;
            };
            if let Some(record) = self.load(&TableName::new(table)?, seq)? {
                out.push(record);
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
        self.relations.clear(AUDIT_RELATION)?;
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
                    self.relations.upsert(
                        AUDIT_RELATION,
                        RowId::from_id(record.id),
                        &index_columns(&record),
                        &index_pointer(&record),
                    )?;
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
                self.relations
                    .remove(AUDIT_RELATION, RowId::from_id(record.id))?;
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
