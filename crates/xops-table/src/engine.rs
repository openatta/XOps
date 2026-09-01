//! 表目录与行的读写。
//!
//! **写串行与四步区间归 RP-01，本包只是用它。** 本包往区间里接两样：
//! ①' 补齐（自动补的列位、自增序号、派生文本——它们必须在区间内算，否则两个并发写会撞号）
//! 与 ① 校验（schema、软删表、不可变列）。

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use serde_json::{Map, Value, json};
use xops_audit::{AuditEnvelope, AuditLog};
use xops_core::{Clock, Error, Id, Result, Role, RowId, TableName, Timestamp, WriteOp};
use xops_identity::{Action, Directory, ProjectId, UserId};
use xops_store::{PreWrite, Row, SchemaCheck, Store, WriteEngine, WriteRequest, keys, space};

use crate::column::{Column, ColumnType, render_template};
use crate::query::{Filter, Page, Query, matches_all};
use crate::system;
use crate::table::{Kind, Protection, TableId, TableSchema, physical_name};
use crate::writtenby::WrittenBy;

/// 表目录落在这张平台表上。**它不是那五张系统表之一**，用户看不到它。
pub const CATALOG_TABLE: &str = "_tables";

/// 一次向存储要多少行。**这是分页粒度，不是结果上限**——
/// 上限由调用方的 `limit` 或 `ceiling` 说了算。
const SCAN_PAGE: usize = 256;
/// 自增序号的计数器。
const SEQUENCE_SPACE: &str = "table-seq";

/// 事件类型。
pub mod kinds {
    pub const TABLE_CREATED: &str = "table.created";
    pub const TABLE_COLUMN_ADDED: &str = "table.column-added";
    pub const TABLE_DROPPED: &str = "table.dropped";
}

/// 一张表能不能被删。
///
/// **被任何流程引用为结算表或主体表的表不能删**（`TBL-026`）——而"哪些表被引用了"
/// 是 RP-14 才知道的事。这个位现在挂一个永远放行的实现，RP-14 接进来时换掉它。
pub trait DropGuard: Send + Sync + 'static {
    /// # Errors
    /// 这张表还被引用着。
    fn allow_drop(&self, project: ProjectId, table: &TableId) -> Result<()>;
}

/// M1 的实现：还没有流程，所以没有谁引用得了。
#[derive(Debug, Default)]
pub struct NoFlows;

impl DropGuard for NoFlows {
    fn allow_drop(&self, _project: ProjectId, _table: &TableId) -> Result<()> {
        Ok(())
    }
}

/// 表目录。**同时是写入区间的 ①' 与 ①。**
pub struct Catalog {
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
    schemas: RwLock<BTreeMap<TableName, TableSchema>>,
}

impl std::fmt::Debug for Catalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Catalog").finish_non_exhaustive()
    }
}

impl Catalog {
    /// 打开目录，把已有的表读进缓存。
    ///
    /// # Errors
    /// 底层不可用或目录损坏。
    pub fn open(store: Arc<dyn Store>, clock: Arc<dyn Clock>) -> Result<Self> {
        let catalog = Self {
            store,
            clock,
            schemas: RwLock::new(BTreeMap::new()),
        };
        catalog.reload()?;
        Ok(catalog)
    }

    /// 从 `_tables` 的投影重建缓存。
    ///
    /// # Errors
    /// 底层不可用或目录损坏。
    pub fn reload(&self) -> Result<()> {
        let table = TableName::new(CATALOG_TABLE)?;
        let prefix = keys::table_prefix(&table);
        let mut loaded = BTreeMap::new();
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
                    .map_err(|error| Error::internal(format!("目录读不回来：{error}")))?;
                let Some(envelope) = AuditEnvelope::from_payload(&row.payload) else {
                    continue;
                };
                let schema: TableSchema = serde_json::from_value(envelope.data)
                    .map_err(|error| Error::internal(format!("表 schema 读不回来：{error}")))?;
                loaded.insert(schema.physical()?, schema);
            }
        }
        *self.schemas.write().map_err(poisoned)? = loaded;
        Ok(())
    }

    /// 缓存里放一份。建表 / 加列 / 删表之后调。
    ///
    /// # Errors
    /// 缓存锁中毒。
    pub fn put(&self, schema: &TableSchema) -> Result<()> {
        self.schemas
            .write()
            .map_err(poisoned)?
            .insert(schema.physical()?, schema.clone());
        Ok(())
    }

    /// 按物理表名查 schema。
    ///
    /// # Errors
    /// 缓存锁中毒。
    pub fn by_physical(&self, physical: &TableName) -> Result<Option<TableSchema>> {
        Ok(self
            .schemas
            .read()
            .map_err(poisoned)?
            .get(physical)
            .cloned())
    }

    /// 按 `(项目, 表名)` 查。
    ///
    /// # Errors
    /// 名字拼不出来或缓存锁中毒。
    pub fn get(&self, project: Option<ProjectId>, name: &TableId) -> Result<Option<TableSchema>> {
        self.by_physical(&physical_name(project, name)?)
    }

    /// 列出一个项目里**没被软删的**表（`TBL-026`：删了就从列出结果中消失）。
    ///
    /// # Errors
    /// 缓存锁中毒。
    pub fn list(&self, project: ProjectId) -> Result<Vec<TableSchema>> {
        Ok(self
            .schemas
            .read()
            .map_err(poisoned)?
            .values()
            .filter(|schema| schema.project == Some(project) && !schema.is_dropped())
            .cloned()
            .collect())
    }

    /// 全部没被软删的表，含全局的。派发 tool 时用它。
    ///
    /// # Errors
    /// 缓存锁中毒。
    pub fn live(&self) -> Result<Vec<TableSchema>> {
        Ok(self
            .schemas
            .read()
            .map_err(poisoned)?
            .values()
            .filter(|schema| !schema.is_dropped())
            .cloned()
            .collect())
    }

    /// 下一个序号。**只在写入区间内调**——不然两个并发写会算出同一个号。
    fn next_sequence(&self, physical: &TableName, column: &str) -> Result<i64> {
        let key = format!("{physical}\u{0}{column}").into_bytes();
        let current = match self.store.get(SEQUENCE_SPACE, &key)? {
            Some(bytes) => {
                let bytes: [u8; 8] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| Error::internal("序号计数器损坏了"))?;
                i64::from_be_bytes(bytes)
            }
            None => 0,
        };
        let next = current + 1;
        self.store.put(SEQUENCE_SPACE, &key, &next.to_be_bytes())?;
        Ok(next)
    }
}

fn poisoned<T>(_: T) -> Error {
    Error::internal("表目录的锁中毒了")
}

impl PreWrite for Catalog {
    /// ①' 补齐：`at` · 自增序号 · 派生文本。**必须在区间内算**（见 `PreWrite` 的文档）。
    fn prepare(&self, mut request: WriteRequest) -> Result<WriteRequest> {
        let Some(schema) = self.by_physical(&request.table)? else {
            // 不是表引擎管的表（平台表、审计表都走这条），原样放过。
            return Ok(request);
        };
        if request.op == WriteOp::Delete {
            return Ok(request);
        }
        let Some(values) = request.payload.as_object_mut() else {
            return Err(Error::invalid("行必须是一个对象"));
        };

        // TBL-014：平台自动补，**任何列声明都不能覆盖它们**。
        values.insert("at".into(), json!(self.clock.now().as_millis()));
        // `revision`：若这一行来自一次读仓的执行，它跟着 writtenBy 一起进来。
        if let Some(revision) = values
            .get("writtenBy")
            .and_then(|written| written.get("revision"))
            .cloned()
            .filter(|revision| !revision.is_null())
        {
            values.insert("revision".into(), revision);
        }

        if request.op == WriteOp::Insert {
            for column in &schema.columns {
                match &column.ty {
                    ColumnType::Sequence => {
                        let next = self.next_sequence(&request.table, &column.name)?;
                        values.insert(column.name.clone(), json!(next));
                    }
                    ColumnType::Derived { .. } => {}
                    _ => {}
                }
            }
            // 派生文本要在序号补完之后算 —— {project.slug}-{seq} 是它最典型的样子。
            let snapshot = values.clone();
            for column in &schema.columns {
                if let ColumnType::Derived { template } = &column.ty {
                    let rendered = render_template(
                        template,
                        &schema.project_slug,
                        &snapshot,
                        &schema.columns,
                    )?;
                    values.insert(column.name.clone(), json!(rendered));
                }
            }
        }
        Ok(request)
    }
}

impl SchemaCheck for Catalog {
    /// ① 校验：软删的表写不了 · 未声明的列拒绝 · 必填列不能少 · 不可变列改不了。
    fn check(&self, request: &WriteRequest) -> Result<()> {
        let Some(schema) = self.by_physical(&request.table)? else {
            return Ok(());
        };
        if schema.is_dropped() {
            return Err(Error::not_found("不存在"));
        }
        if request.op == WriteOp::Delete {
            return Ok(());
        }
        let values = request
            .payload
            .as_object()
            .ok_or_else(|| Error::invalid("行必须是一个对象"))?;

        for name in values.keys() {
            if crate::column::AUTO_COLUMNS.contains(&name.as_str()) {
                continue;
            }
            if schema.column(name).is_none() {
                return Err(Error::invalid(format!("列 {name} 不在这张表的 schema 里")));
            }
        }
        for column in &schema.columns {
            match values.get(&column.name) {
                None | Some(Value::Null) => {
                    if column.required && request.op == WriteOp::Insert {
                        return Err(Error::invalid(format!("缺少必填列 {}", column.name)));
                    }
                }
                Some(value) => column.ty.check(&column.name, value)?,
            }
        }

        if request.op == WriteOp::Update {
            // TBL-020：派生文本 insert 时生成一次、**之后不变**；自增序号同理。
            let previous = self.previous_row(&request.table, request.row)?;
            for column in &schema.columns {
                if column.ty.mutable() {
                    continue;
                }
                let (Some(before), Some(after)) = (
                    previous.as_ref().and_then(|row| row.get(&column.name)),
                    values.get(&column.name),
                ) else {
                    continue;
                };
                if before != after {
                    return Err(Error::invalid(format!(
                        "列 {} 是 insert 时生成一次的，之后改不了（TBL-020）",
                        column.name
                    )));
                }
            }
        }
        Ok(())
    }
}

impl Catalog {
    fn previous_row(&self, physical: &TableName, row: RowId) -> Result<Option<Map<String, Value>>> {
        let Some(bytes) = self.store.get(space::ROW, &keys::row(physical, row))? else {
            return Ok(None);
        };
        let stored: Row = serde_json::from_slice(&bytes)
            .map_err(|error| Error::internal(format!("投影读不回来：{error}")))?;
        Ok(stored.payload.as_object().cloned())
    }
}

/// 一行的一个历史版本（`TBL-012`：任何一行都能查出它的完整历史）。
#[derive(Debug, Clone, PartialEq)]
pub struct RowVersion {
    pub seq: u64,
    pub op: WriteOp,
    pub at: Timestamp,
    pub written_by: Option<WrittenBy>,
    pub values: Value,
}

/// 表引擎。
pub struct Tables {
    engine: Arc<WriteEngine>,
    catalog: Arc<Catalog>,
    audit: Arc<AuditLog>,
    directory: Arc<Directory>,
    clock: Arc<dyn Clock>,
    store: Arc<dyn Store>,
    drop_guard: Arc<dyn DropGuard>,
}

impl std::fmt::Debug for Tables {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tables").finish_non_exhaustive()
    }
}

impl Tables {
    #[must_use]
    pub fn new(
        engine: Arc<WriteEngine>,
        catalog: Arc<Catalog>,
        audit: Arc<AuditLog>,
        directory: Arc<Directory>,
        clock: Arc<dyn Clock>,
        store: Arc<dyn Store>,
    ) -> Self {
        Self {
            engine,
            catalog,
            audit,
            directory,
            clock,
            store,
            drop_guard: Arc::new(NoFlows),
        }
    }

    /// 接上"这张表还被流程引用着吗"。RP-14 用。
    #[must_use]
    pub fn with_drop_guard(mut self, guard: Arc<dyn DropGuard>) -> Self {
        self.drop_guard = guard;
        self
    }

    #[must_use]
    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }

    // ——————————————————————————————— 目录 ———————————————————————————————

    /// 建表（`TBL-001`）。
    ///
    /// # Errors
    /// 没权限 / 项目不存在（同一个错）· 表名已被用过 · 列声明不合法。
    pub fn create(
        &self,
        actor: UserId,
        project: ProjectId,
        name: TableId,
        protection: Protection,
        columns: Vec<Column>,
    ) -> Result<TableSchema> {
        let (record, _) = self
            .directory
            .authorize(actor, project, Action::CreateTable)?;
        // TBL-026：**表名不可复用**——软删过的名字也算用过。
        if self.catalog.get(Some(project), &name)?.is_some() {
            return Err(Error::conflict(format!(
                "表名 {name} 用过了，不可复用（TBL-026）"
            )));
        }
        let schema = TableSchema::new(
            Some(project),
            record.slug.as_str(),
            name,
            Kind::User,
            protection,
            columns,
            self.clock.now(),
        )?;
        self.store_schema(actor, &schema, kinds::TABLE_CREATED)?;
        Ok(schema)
    }

    /// 项目建好之后平台自动建的那四张系统表（`TBL-005`）。
    ///
    /// # Errors
    /// 底层写失败。
    pub fn ensure_system_tables(&self, project: ProjectId, slug: &str) -> Result<()> {
        for name in system::PER_PROJECT {
            let id = TableId::system(name)?;
            if self.catalog.get(Some(project), &id)?.is_some() {
                continue;
            }
            let schema = system::schema(name, Some(project), slug, self.clock.now())?;
            self.store_schema_as_platform(&schema, kinds::TABLE_CREATED)?;
        }
        Ok(())
    }

    /// 全局的 `_notices`。
    ///
    /// # Errors
    /// 底层写失败。
    pub fn ensure_global_tables(&self) -> Result<()> {
        let id = TableId::system(system::NOTICES)?;
        if self.catalog.get(None, &id)?.is_some() {
            return Ok(());
        }
        let schema = system::schema(system::NOTICES, None, "", self.clock.now())?;
        self.store_schema_as_platform(&schema, kinds::TABLE_CREATED)
    }

    /// 加一列（`TBL-022`）。**新列对历史行为空。**
    ///
    /// # Errors
    /// 没权限 · 表不存在 · 列名重复或不合法 · 这是系统表。
    pub fn add_column(
        &self,
        actor: UserId,
        project: ProjectId,
        name: &TableId,
        column: Column,
    ) -> Result<TableSchema> {
        self.directory
            .authorize(actor, project, Action::CreateTable)?;
        let mut schema = self.require(Some(project), name)?;
        if schema.kind == Kind::System {
            return Err(Error::invalid("系统表的 schema 是固定的（TBL-005）"));
        }
        schema.add_column(column)?;
        self.store_schema(actor, &schema, kinds::TABLE_COLUMN_ADDED)?;
        Ok(schema)
    }

    /// 查表结构，**不判权**。
    ///
    /// 给平台自己的写入路径用（RP-12 落账时要按 schema 校验产出行）——
    /// 那条路上没有"调用者"这个概念，写入者是一次执行，不是一个人。
    ///
    /// # Errors
    /// 表不存在或已软删。
    pub fn describe_internal(
        &self,
        project: Option<ProjectId>,
        name: &TableId,
    ) -> Result<TableSchema> {
        self.require(project, name)
    }

    /// 查表结构。
    ///
    /// # Errors
    /// 非成员看到的与表不存在一致。
    pub fn describe(
        &self,
        viewer: UserId,
        project: ProjectId,
        name: &TableId,
    ) -> Result<TableSchema> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        self.require(Some(project), name)
    }

    /// 列出项目的表。
    ///
    /// # Errors
    /// 非成员看不到。
    /// 一个项目里的全部表，**不判权**。给平台自己的后台维护用。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn list_internal(&self, project: ProjectId) -> Result<Vec<TableSchema>> {
        self.catalog.list(project)
    }

    pub fn list(&self, viewer: UserId, project: ProjectId) -> Result<Vec<TableSchema>> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        self.catalog.list(project)
    }

    /// 删表。**软删**（`TBL-026`）：从列出结果中消失、专属 tool 停止派发，
    /// **行与事件一律保留、单行历史仍可查**。
    ///
    /// # Errors
    /// 没权限 · 表不存在 · 这是系统表 · 还被某条流程引用着。
    pub fn drop_table(&self, actor: UserId, project: ProjectId, name: &TableId) -> Result<()> {
        self.directory
            .authorize(actor, project, Action::ManageBusinessObject)?;
        let mut schema = self.require(Some(project), name)?;
        if schema.kind == Kind::System {
            return Err(Error::invalid("系统表删不了（TBL-005）"));
        }
        self.drop_guard.allow_drop(project, name)?;
        schema.dropped_at = Some(self.clock.now());
        self.store_schema(actor, &schema, kinds::TABLE_DROPPED)
    }

    // ——————————————————————————————— 行 ———————————————————————————————

    /// 插一行。
    ///
    /// # Errors
    /// 表不存在 / 已软删 · schema 不过 · 系统表被非平台写。
    pub fn insert(
        &self,
        written_by: &WrittenBy,
        project: Option<ProjectId>,
        name: &TableId,
        values: Value,
    ) -> Result<RowId> {
        let schema = self.require(project, name)?;
        self.guard_system(&schema, written_by)?;
        let row = RowId::generate();
        self.write(&schema, written_by, WriteOp::Insert, row, values)?;
        Ok(row)
    }

    /// 改一行。
    ///
    /// # Errors
    /// 同上，外加：不可变列改不了。
    pub fn update(
        &self,
        written_by: &WrittenBy,
        project: Option<ProjectId>,
        name: &TableId,
        row: RowId,
        values: Value,
    ) -> Result<()> {
        let schema = self.require(project, name)?;
        self.guard_system(&schema, written_by)?;
        // update 的语义是"合并"：没给的列保持原样。
        let mut merged = self
            .read_values(&schema, row)?
            .ok_or_else(|| Error::not_found("不存在"))?;
        if let Some(patch) = values.as_object() {
            for (key, value) in patch {
                merged.insert(key.clone(), value.clone());
            }
        }
        self.write(
            &schema,
            written_by,
            WriteOp::Update,
            row,
            Value::Object(merged),
        )
    }

    /// 删一行。**软删**（`TBL-012`）：行读不到了，事件与历史都还在。
    ///
    /// # Errors
    /// 表不存在 · 系统表被非平台写。
    pub fn delete(
        &self,
        written_by: &WrittenBy,
        project: Option<ProjectId>,
        name: &TableId,
        row: RowId,
    ) -> Result<()> {
        let schema = self.require(project, name)?;
        self.guard_system(&schema, written_by)?;
        self.write(&schema, written_by, WriteOp::Delete, row, Value::Null)
    }

    /// 读一行现在的样子。
    ///
    /// # Errors
    /// 表不存在。
    pub fn get(
        &self,
        project: Option<ProjectId>,
        name: &TableId,
        row: RowId,
    ) -> Result<Option<Value>> {
        let schema = self.require(project, name)?;
        Ok(self.read_values(&schema, row)?.map(Value::Object))
    }

    /// 翻一页（`Query`）。**按行 ID 序，也就是写入序。**
    ///
    /// 内存有界：一次只攒 `limit` 行。要排序或者要全部命中，用 [`Self::query_all`]。
    ///
    /// # Errors
    /// 表不存在或底层不可用。
    pub fn query(&self, project: Option<ProjectId>, name: &TableId, query: &Query) -> Result<Page> {
        let schema = self.require(project, name)?;
        let physical = schema.physical()?;
        let prefix = keys::table_prefix(&physical);
        let mut cursor: Option<Vec<u8>> = query.after.map(|row| keys::row(&physical, row));
        let mut rows = Vec::new();

        while rows.len() < query.limit {
            let page = self
                .store
                .scan(space::ROW, &prefix, cursor.as_deref(), SCAN_PAGE)?;
            if page.is_empty() {
                break;
            }
            cursor = page.last().map(|(key, _)| key.clone());
            for (_, bytes) in page {
                let Some(stored) = Self::live_row(&bytes)? else {
                    continue;
                };
                if !matches_all(&query.filters, &stored.1) {
                    continue;
                }
                rows.push(stored);
                if rows.len() >= query.limit {
                    break;
                }
            }
        }
        // 填满了就给一个游标。**"可能还有"比"少给一页"安全。**
        let next = (rows.len() >= query.limit && query.limit > 0)
            .then(|| rows.last().map(|(row, _)| *row))
            .flatten();
        Ok(Page { rows, next })
    }

    /// 把**全部命中**取回来。
    ///
    /// 要排序、要计数、要"这个实例的所有结算行"——都得用它，
    /// 因为那几件事在拿到全部命中之前答不出来。
    ///
    /// `ceiling` 是**扫描上限**（扫过的行数，不是命中的行数）：
    /// **超了明确失败，绝不截断**。真撞上了，正确的动作是给那一列加一条索引，
    /// 不是把这个数字调大。
    ///
    /// # Errors
    /// 表不存在 · 底层不可用 · **扫描量超过 `ceiling`**。
    pub fn query_all(
        &self,
        project: Option<ProjectId>,
        name: &TableId,
        filters: &[Filter],
        ceiling: usize,
    ) -> Result<Vec<(RowId, Value)>> {
        let schema = self.require(project, name)?;
        let physical = schema.physical()?;
        let prefix = keys::table_prefix(&physical);
        let mut cursor: Option<Vec<u8>> = None;
        let mut scanned = 0usize;
        let mut out = Vec::new();

        loop {
            let page = self
                .store
                .scan(space::ROW, &prefix, cursor.as_deref(), SCAN_PAGE)?;
            if page.is_empty() {
                return Ok(out);
            }
            cursor = page.last().map(|(key, _)| key.clone());
            for (_, bytes) in page {
                scanned += 1;
                if scanned > ceiling {
                    return Err(Error::invalid(format!(
                        "{name} 要扫的行超过 {ceiling} 条，停下了。\
                         **这里不截断**——截断会安静地给出一个错误答案（留下的是最老的那一批）。\
                         要么把筛选收窄，要么给这一列加一条索引"
                    )));
                }
                let Some(stored) = Self::live_row(&bytes)? else {
                    continue;
                };
                if matches_all(filters, &stored.1) {
                    out.push(stored);
                }
            }
        }
    }

    /// 列出一张表的**前 `limit` 行**。
    ///
    /// ⚠️ **它给的是最老的那一批**：行 ID 时间有序，扫描按行 ID 升序。
    /// 想要"最新的 N 条"或者"某个筛选的全部命中"，**用 [`Self::query_all`]**——
    /// 拿这个方法配一个大 limit 去顶，就是那个会安静给出错误答案的写法。
    ///
    /// # Errors
    /// 表不存在或底层不可用。
    pub fn rows(
        &self,
        project: Option<ProjectId>,
        name: &TableId,
        limit: usize,
    ) -> Result<Vec<(RowId, Value)>> {
        Ok(self.query(project, name, &Query::first(limit))?.rows)
    }

    /// 一条投影读回来；软删的返回 `None`。
    fn live_row(bytes: &[u8]) -> Result<Option<(RowId, Value)>> {
        let stored: Row = serde_json::from_slice(bytes)
            .map_err(|error| Error::internal(format!("投影读不回来：{error}")))?;
        if stored.is_deleted() {
            return Ok(None);
        }
        Ok(Some((stored.row, stored.payload)))
    }

    /// 一行的完整历史（`TBL-012`）。
    ///
    /// ⚠️ 它扫的是**整张表的事件流**再按行过滤。表大了之后这里要一个按行的索引，
    /// 而那个索引该建在这里，不是让调用方各自记一份。
    ///
    /// # Errors
    /// 表不存在或底层不可用。
    pub fn history(
        &self,
        project: Option<ProjectId>,
        name: &TableId,
        row: RowId,
    ) -> Result<Vec<RowVersion>> {
        // 软删过的表照样查得到历史（`TBL-026`）——行与事件一律保留。
        let schema = self
            .catalog
            .get(project, name)?
            .ok_or_else(|| Error::not_found("不存在"))?;
        let physical = schema.physical()?;
        let mut out = Vec::new();
        let mut after = 0;
        loop {
            let events = self.engine.events(&physical, after, 256)?;
            if events.is_empty() {
                break;
            }
            after = events.last().map_or(after, |event| event.seq);
            for event in events {
                if event.row != row {
                    continue;
                }
                let written_by = event
                    .payload
                    .get("writtenBy")
                    .and_then(|value| serde_json::from_value(value.clone()).ok());
                out.push(RowVersion {
                    seq: event.seq,
                    op: event.op,
                    at: event.at,
                    written_by,
                    values: event.payload,
                });
            }
        }
        Ok(out)
    }

    /// 写这张表要什么权限（`TBL-025`）。受保护表**只有所有者能写**。
    #[must_use]
    pub fn write_action(schema: &TableSchema) -> Action {
        match schema.protection {
            Protection::Normal => Action::WriteTable,
            Protection::Protected => Action::WriteProtectedTable,
        }
    }

    /// 这个角色能不能写这张表。
    #[must_use]
    pub fn can_write(schema: &TableSchema, role: Role, archived: bool) -> bool {
        schema.kind != Kind::System
            && xops_identity::can_in(role, Self::write_action(schema), archived)
    }

    // ——————————————————————————————— 内部 ———————————————————————————————

    fn require(&self, project: Option<ProjectId>, name: &TableId) -> Result<TableSchema> {
        self.catalog
            .get(project, name)?
            .filter(|schema| !schema.is_dropped())
            .ok_or_else(|| Error::not_found("不存在"))
    }

    fn guard_system(&self, schema: &TableSchema, written_by: &WrittenBy) -> Result<()> {
        if schema.kind == Kind::System && !matches!(written_by, WrittenBy::Platform) {
            return Err(Error::invalid(format!(
                "{} 是系统表，只有平台能写（TBL-003）",
                schema.name
            )));
        }
        Ok(())
    }

    fn read_values(&self, schema: &TableSchema, row: RowId) -> Result<Option<Map<String, Value>>> {
        let physical = schema.physical()?;
        Ok(self
            .engine
            .read(&physical, row)?
            .and_then(|stored| stored.payload.as_object().cloned()))
    }

    fn write(
        &self,
        schema: &TableSchema,
        written_by: &WrittenBy,
        op: WriteOp,
        row: RowId,
        values: Value,
    ) -> Result<()> {
        // 删除也带署名：`TBL-012` 要的"谁、何时、改了什么"里，"谁删的"是其中一问。
        let mut payload = if op == WriteOp::Delete {
            json!({})
        } else {
            values
        };
        let object = payload
            .as_object_mut()
            .ok_or_else(|| Error::invalid("行必须是一个对象"))?;
        // TBL-014 / I-B：**写入者由调用方给的凭据决定，不由请求体决定**。
        // 参数里带的 writtenBy 到这里一律被盖掉。
        object.insert("writtenBy".into(), written_by.to_value()?);
        self.engine.write(WriteRequest {
            table: schema.physical()?,
            op,
            row,
            payload,
            actor: written_by.actor(),
        })?;
        Ok(())
    }

    fn store_schema(&self, actor: UserId, schema: &TableSchema, kind: &str) -> Result<()> {
        let envelope = AuditEnvelope::project_scoped(
            kind,
            schema.project.map_or_else(Id::generate, ProjectId::as_id),
            table_target(schema)?,
            serde_json::to_value(schema)
                .map_err(|error| Error::internal(format!("表 schema 装不下：{error}")))?,
        )?;
        self.persist_schema(
            schema,
            &envelope,
            &xops_core::Actor::User {
                user: actor.to_string(),
            },
        )
    }

    fn store_schema_as_platform(&self, schema: &TableSchema, kind: &str) -> Result<()> {
        let target = table_target(schema)?;
        let envelope = match schema.project {
            Some(project) => AuditEnvelope::project_scoped(
                kind,
                project.as_id(),
                target,
                serde_json::to_value(schema)
                    .map_err(|error| Error::internal(format!("表 schema 装不下：{error}")))?,
            )?,
            None => AuditEnvelope::platform(
                kind,
                target,
                target,
                serde_json::to_value(schema)
                    .map_err(|error| Error::internal(format!("表 schema 装不下：{error}")))?,
            )?,
        };
        self.persist_schema(schema, &envelope, &xops_core::Actor::Platform)
    }

    fn persist_schema(
        &self,
        schema: &TableSchema,
        envelope: &AuditEnvelope,
        actor: &xops_core::Actor,
    ) -> Result<()> {
        let existing = self.catalog.get(schema.project, &schema.name)?;
        let receipt = self.engine.write(WriteRequest {
            table: TableName::new(CATALOG_TABLE)?,
            op: if existing.is_some() {
                WriteOp::Update
            } else {
                WriteOp::Insert
            },
            row: RowId::from_id(table_target(schema)?),
            payload: envelope.to_payload()?,
            actor: actor.clone(),
        })?;
        self.audit.index(&receipt)?;
        self.catalog.put(schema)
    }
}

/// 目录里一张表的行标识：由 `(项目, 表名)` 确定，因而同一张表的每次 schema 变更
/// 都落在同一行上——它的历史就是这张表的 schema 变更史。
fn table_target(schema: &TableSchema) -> Result<Id> {
    let mut bytes = [0u8; 16];
    let project = schema.project.map(|project| *project.as_id().as_bytes());
    let seed = format!("{}{}", project.map_or(String::new(), hex), schema.name);
    // 一个稳定的、够用的散列：FNV-1a 64 位取两遍，凑满 128 位。
    let low = fnv1a(seed.as_bytes());
    let high = fnv1a(&low.to_be_bytes());
    bytes[..8].copy_from_slice(&high.to_be_bytes());
    bytes[8..].copy_from_slice(&low.to_be_bytes());
    Id::parse(&encode_id(bytes))
}

fn hex(bytes: [u8; 16]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// 把 16 字节编成 [`Id`] 的文本形态。
fn encode_id(bytes: [u8; 16]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let value = u128::from_be_bytes(bytes) >> 3; // 留出最高 3 位，免得溢出 26 个字符
    let mut out = [0u8; 26];
    for (index, slot) in out.iter_mut().enumerate() {
        let shift = 5 * (26 - 1 - index);
        let digit = usize::try_from((value >> shift) & 0x1F).unwrap_or(0);
        *slot = ALPHABET[digit];
    }
    String::from_utf8_lossy(&out).into_owned()
}
