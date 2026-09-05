//! SQLite 实现。
//!
//! ⚠️ **这是全仓唯一允许出现 `rusqlite` 的文件。** 越过这条线，D46 就落空了，
//! 而发现它的时机通常是真要换库的那天。`tests/no_sqlite_outside_store.rs` 会枚举全仓证明它。
//!
//! 这里只用一张两列主键的表和四条最普通的语句。**刻意不用**：触发器、存储过程、
//! 行锁、事务隔离级别、MVCC、外键、级联、JSON 列（`CON-012`）。连 `BEGIN` 都不写——
//! 一次写就是一条语句，它自己是原子的，再多的原子性本契约不承诺（`CON-007`）。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use xops_core::{Error, Id, Result, RowId};

use crate::relation::{Direction, Relation, Relations, Select, ValueKind};
use crate::store::{Store, prefix_end};

/// 键值表建表语句。
///
/// `WITHOUT ROWID` 让主键就是聚簇索引——前缀扫描因此是顺序读，不是"先查索引再回表"。
/// 这是**性能选择，不是能力依赖**：去掉它一切照常工作。
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS kv (
    space TEXT NOT NULL,
    key   BLOB NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (space, key)
) WITHOUT ROWID";

/// 默认开几条读连接。
///
/// 读连接的作用是**让读不排在写后面**。多到超过并发读的数量就没有意义了——
/// 每一条都是一个文件句柄和一份页缓存。
pub const READERS: usize = 4;

/// 等一把被别人占着的库锁最多多久。
///
/// ⚠️ **它不是「重试策略」。** SQLite 的写本来就是全库串行的，多开几条写连接
/// 换不来写并发，只会把等待从进程内的 mutex 挪到 `SQLITE_BUSY`。这个超时是给
/// **读连接**用的：WAL 下读一般不会被挡，但检查点那一瞬间会。
const BUSY_TIMEOUT_MILLIS: u64 = 5_000;

/// SQLite 上的存储契约实现。
///
/// # 一条写连接，几条读连接
///
/// ```text
/// put / delete   → 写连接（一条）
/// get / scan     → 读连接（轮着用）
/// ```
///
/// ⚠️ **写连接只有一条不是偷懒，是 SQLite 就这样**：它是单写者模型，
/// 全库同一时刻只有一个写事务。开 N 条写连接不会变快，只会让第二个写拿到
/// `SQLITE_BUSY` 然后在超时里空等——**把排队从一个公平的 mutex 换成一场竞争**。
///
/// **分开读连接换来的是"读不排在写后面"**。早先只有一条连接，
/// 一次看板查询和一次执行落账抢的是同一把锁——那才是"一张热表锁住所有人"的真正位置。
/// （表级写锁从来不是：`TableLocks` 是按表的，`_runs` 的写不挡别的表。）
///
/// ⚠️ **要真正的写并发得换库。** 到 MySQL 那天，这里的"一条写连接"要变成一个写连接池，
/// 而调用方一行不用改——这也是现在就把读写分开的理由之一。
///
/// # 内存库只有一条连接
///
/// `:memory:` 上**每条连接都是一个各自独立的库**，所以内存库不开读连接，
/// 读写都走那一条。测试因此与生产走的是同一份代码，只是并发度不同。
pub struct SqliteStore {
    writer: Mutex<Connection>,
    /// 空表示"读也走写连接"——内存库就是这种。
    readers: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore")
            .field("readers", &self.readers.len())
            .finish_non_exhaustive()
    }
}

impl SqliteStore {
    /// 打开一个文件库，不存在就建。
    ///
    /// # Errors
    /// 打不开或建表失败。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_readers(path, READERS)
    }

    /// 同上，但自己定读连接数。`0` 表示读也走写连接。
    ///
    /// # Errors
    /// 打不开或建表失败。
    pub fn open_with_readers(path: impl AsRef<Path>, readers: usize) -> Result<Self> {
        let path = path.as_ref();
        let writer = Self::connect(path)?;
        writer.execute(SCHEMA, []).map_err(sql_error)?;
        // WAL：**读不挡写、写不挡读**。它是一个持久设置，建库时定一次。
        //
        // ⚠️ 这不是"依赖数据库特有能力"（`CON-012`）：代码的语义与它无关，
        // 关掉它一切照常工作，只是读会重新排在写后面。同 `WITHOUT ROWID` 一类，
        // **是性能选择，不是能力依赖**；换到 MySQL / PostgreSQL 时它根本不需要。
        let _: String = writer
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(sql_error)?;
        let mut pool = Vec::with_capacity(readers);
        for _ in 0..readers {
            pool.push(Mutex::new(Self::connect(path)?));
        }
        Ok(Self {
            writer: Mutex::new(writer),
            readers: pool,
            next: AtomicUsize::new(0),
        })
    }

    /// 打开一个进程内的临时库。测试用。
    ///
    /// # Errors
    /// 建表失败。
    pub fn in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory().map_err(sql_error)?;
        connection.execute(SCHEMA, []).map_err(sql_error)?;
        Ok(Self {
            writer: Mutex::new(connection),
            // **内存库每条连接都是一个独立的库**，所以这里只有一条。
            readers: Vec::new(),
            next: AtomicUsize::new(0),
        })
    }

    fn connect(path: &Path) -> Result<Connection> {
        let connection = Connection::open(path).map_err(sql_error)?;
        connection
            .busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MILLIS))
            .map_err(sql_error)?;
        Ok(connection)
    }

    /// 写连接。**DDL 也走它**——建表建索引是写。
    pub(crate) fn locked(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.writer
            .lock()
            .map_err(|_| Error::internal("SQLite 写连接的锁中毒了"))
    }

    /// 一条读连接，轮着来。没有读连接时退回写连接。
    pub(crate) fn reading(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        if self.readers.is_empty() {
            return self.locked();
        }
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        self.readers[index]
            .lock()
            .map_err(|_| Error::internal("SQLite 读连接的锁中毒了"))
    }
}

fn sql_error(error: rusqlite::Error) -> Error {
    Error::unavailable(format!("SQLite: {error}"))
}

impl Store for SqliteStore {
    fn get(&self, space: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        // 读走读连接 —— **它不排在写后面**。
        self.reading()?
            .query_row(
                "SELECT value FROM kv WHERE space = ?1 AND key = ?2",
                params![space, key],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(sql_error)
    }

    fn put(&self, space: &str, key: &[u8], value: &[u8]) -> Result<()> {
        self.locked()?
            .execute(
                "INSERT INTO kv (space, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT (space, key) DO UPDATE SET value = excluded.value",
                params![space, key, value],
            )
            .map(|_| ())
            .map_err(sql_error)
    }

    fn delete(&self, space: &str, key: &[u8]) -> Result<()> {
        self.locked()?
            .execute(
                "DELETE FROM kv WHERE space = ?1 AND key = ?2",
                params![space, key],
            )
            .map(|_| ())
            .map_err(sql_error)
    }

    fn scan(
        &self,
        space: &str,
        prefix: &[u8],
        after: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let lower = after.map_or_else(|| prefix.to_vec(), <[u8]>::to_vec);
        let exclusive = after.is_some();
        // 扫描也是读。**看板那一次查询不该跟一次执行落账抢同一把锁。**
        let connection = self.reading()?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);

        let collect = |sql: &str, upper: Option<Vec<u8>>| -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
            let mut statement = connection.prepare(sql).map_err(sql_error)?;
            let rows = match upper {
                Some(upper) => statement
                    .query_map(params![space, lower, upper, limit], |row| {
                        Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
                    })
                    .map_err(sql_error)?
                    .collect::<std::result::Result<Vec<_>, _>>(),
                None => statement
                    .query_map(params![space, lower, limit], |row| {
                        Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
                    })
                    .map_err(sql_error)?
                    .collect::<std::result::Result<Vec<_>, _>>(),
            };
            rows.map_err(sql_error)
        };

        let comparison = if exclusive { ">" } else { ">=" };
        match prefix_end(prefix) {
            Some(upper) => collect(
                &format!(
                    "SELECT key, value FROM kv
                     WHERE space = ?1 AND key {comparison} ?2 AND key < ?3
                     ORDER BY key LIMIT ?4"
                ),
                Some(upper),
            ),
            None => collect(
                &format!(
                    "SELECT key, value FROM kv
                     WHERE space = ?1 AND key {comparison} ?2
                     ORDER BY key LIMIT ?3"
                ),
                None,
            ),
        }
    }
}

// ——————————————————————————————— 关系投影 ———————————————————————————————

/// 关系投影的 SQLite 实现：**一张真表，有真列、真索引**。
///
/// 与 [`SqliteStore`] **共用同一条连接**，所以它们在同一个库文件里。
///
/// ⚠️ 表名是 `rel_<关系名>`，关系名与列名都过
/// [`check_identifier`](crate::relation::check_identifier)——
/// 它们进的是 SQL 标识符的位置，那里不能用参数占位。
/// **这不是"清洗输入"，是只认一个白名单形状。**
pub struct SqliteRelations {
    store: Arc<SqliteStore>,
    declared: Mutex<BTreeMap<String, Relation>>,
}

impl std::fmt::Debug for SqliteRelations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteRelations").finish_non_exhaustive()
    }
}

impl SqliteRelations {
    #[must_use]
    fn new(store: Arc<SqliteStore>) -> Self {
        Self {
            store,
            declared: Mutex::new(BTreeMap::new()),
        }
    }

    fn relation(&self, name: &str) -> Result<Relation> {
        self.declared
            .lock()
            .map_err(|_| Error::internal("关系投影声明的锁中毒了"))?
            .get(name)
            .cloned()
            .ok_or_else(|| Error::invalid(format!("没有声明过 {name} 这张关系投影")))
    }
}

fn sql_type(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Text => "TEXT",
        ValueKind::Integer | ValueKind::Bool => "INTEGER",
    }
}

/// 把一个 JSON 值变成能进列的东西。**形状不对就当 NULL** —— 关系投影是缓存，
/// 它的职责是"找得到"，不是"校验"；校验在 `xops-table` 那一层已经做过了。
fn cell(kind: ValueKind, value: Option<&serde_json::Value>) -> rusqlite::types::Value {
    use rusqlite::types::Value as Sql;
    match (kind, value) {
        (_, None | Some(serde_json::Value::Null)) => Sql::Null,
        (ValueKind::Text, Some(serde_json::Value::String(text))) => Sql::Text(text.clone()),
        (ValueKind::Text, Some(other)) => Sql::Text(other.to_string()),
        (ValueKind::Integer, Some(found)) => found.as_i64().map_or(Sql::Null, Sql::Integer),
        (ValueKind::Bool, Some(found)) => found
            .as_bool()
            .map_or(Sql::Null, |flag| Sql::Integer(i64::from(flag))),
    }
}

impl Relations for SqliteRelations {
    fn declare(&self, relation: &Relation) -> Result<()> {
        relation.check()?;
        let table = format!("rel_{}", relation.name);
        let columns: Vec<String> = relation
            .columns
            .iter()
            .map(|column| format!("{} {}", column.name, sql_type(column.kind)))
            .collect();
        let create = format!(
            "CREATE TABLE IF NOT EXISTS {table} (
                row TEXT NOT NULL PRIMARY KEY,
                {},
                payload BLOB NOT NULL
            ) WITHOUT ROWID",
            columns.join(",\n                ")
        );
        let connection = self.store.locked()?;
        connection.execute(&create, []).map_err(sql_error)?;
        // **该建的建。** 每一条索引都要在写入那一侧还回去。
        for column in relation.columns.iter().filter(|column| column.indexed) {
            let index = format!(
                "CREATE INDEX IF NOT EXISTS {table}_{0} ON {table} ({0})",
                column.name
            );
            connection.execute(&index, []).map_err(sql_error)?;
        }
        drop(connection);
        self.declared
            .lock()
            .map_err(|_| Error::internal("关系投影声明的锁中毒了"))?
            .insert(relation.name.clone(), relation.clone());
        Ok(())
    }

    fn upsert(
        &self,
        relation: &str,
        row: RowId,
        columns: &serde_json::Value,
        payload: &serde_json::Value,
    ) -> Result<()> {
        let declared = self.relation(relation)?;
        let table = format!("rel_{relation}");
        let names: Vec<&str> = declared
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect();
        let placeholders: Vec<String> = (2..=names.len() + 2).map(|i| format!("?{i}")).collect();
        let updates: Vec<String> = names
            .iter()
            .map(|name| format!("{name} = excluded.{name}"))
            .collect();
        let statement = format!(
            "INSERT INTO {table} (row, {}, payload) VALUES (?1, {}, ?{})
             ON CONFLICT (row) DO UPDATE SET {}, payload = excluded.payload",
            names.join(", "),
            placeholders[..names.len()].join(", "),
            names.len() + 2,
            updates.join(", ")
        );

        let encoded = serde_json::to_vec(payload)
            .map_err(|error| Error::internal(format!("关系投影装不下：{error}")))?;
        // 行标识存成它的 26 字符文本形态：**排序与二进制一致**（都是时间序），
        // 而且拿 sqlite3 直接看这张表时它是可读的。
        let mut bound: Vec<rusqlite::types::Value> =
            vec![rusqlite::types::Value::Text(row.to_string())];
        for column in &declared.columns {
            bound.push(cell(column.kind, columns.get(&column.name)));
        }
        bound.push(rusqlite::types::Value::Blob(encoded));

        self.store
            .locked()?
            .execute(&statement, rusqlite::params_from_iter(bound))
            .map(|_| ())
            .map_err(sql_error)
    }

    fn remove(&self, relation: &str, row: RowId) -> Result<()> {
        self.relation(relation)?;
        self.store
            .locked()?
            .execute(
                &format!("DELETE FROM rel_{relation} WHERE row = ?1"),
                params![row.to_string()],
            )
            .map(|_| ())
            .map_err(sql_error)
    }

    fn select(&self, relation: &str, select: &Select) -> Result<Vec<(RowId, serde_json::Value)>> {
        let declared = self.relation(relation)?;
        select.check(&declared)?;

        let mut clauses: Vec<String> = Vec::new();
        let mut bound: Vec<rusqlite::types::Value> = Vec::new();
        let mut next = 1;
        for (column, value) in &select.equals {
            let kind = declared.column(column).map_or(ValueKind::Text, |c| c.kind);
            clauses.push(format!("{column} = ?{next}"));
            bound.push(cell(kind, Some(value)));
            next += 1;
        }
        for column in &select.is_null {
            clauses.push(format!("{column} IS NULL"));
        }
        for column in &select.is_not_null {
            clauses.push(format!("{column} IS NOT NULL"));
        }
        for (column, bound_value) in &select.at_most {
            clauses.push(format!("{column} <= ?{next}"));
            bound.push(rusqlite::types::Value::Integer(*bound_value));
            next += 1;
        }
        for (column, bound_value) in &select.at_least {
            clauses.push(format!("{column} >= ?{next}"));
            bound.push(rusqlite::types::Value::Integer(*bound_value));
            next += 1;
        }
        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        let order = select
            .order
            .as_ref()
            .map_or_else(String::new, |(column, direction)| {
                let keyword = match direction {
                    Direction::Asc => "ASC",
                    Direction::Desc => "DESC",
                };
                format!(" ORDER BY {column} {keyword}")
            });
        let limit = if select.limit > 0 {
            format!(" LIMIT {}", select.limit)
        } else {
            String::new()
        };
        let statement =
            format!("SELECT row, payload FROM rel_{relation}{where_clause}{order}{limit}");

        let connection = self.store.reading()?;
        let mut prepared = connection.prepare(&statement).map_err(sql_error)?;
        let rows = prepared
            .query_map(rusqlite::params_from_iter(bound), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(sql_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql_error)?;

        rows.into_iter()
            .map(|(key, payload)| {
                let row = Id::parse(&key).map(RowId::from_id)?;
                let values = serde_json::from_slice(&payload)
                    .map_err(|error| Error::internal(format!("关系投影读不回来：{error}")))?;
                Ok((row, values))
            })
            .collect()
    }

    fn clear(&self, relation: &str) -> Result<()> {
        self.relation(relation)?;
        self.store
            .locked()?
            .execute(&format!("DELETE FROM rel_{relation}"), [])
            .map(|_| ())
            .map_err(sql_error)
    }
}

impl SqliteStore {
    /// 这个库上的关系投影面。**同一条连接、同一个文件。**
    #[must_use]
    pub fn relations(self: &Arc<Self>) -> Arc<dyn Relations> {
        Arc::new(SqliteRelations::new(Arc::clone(self)))
    }

    /// 这个库里**实际存在**的表与它们的列。**契约自证用**。
    ///
    /// ⚠️ **它必须住在这个 crate 里。** `CON-012`：`rusqlite` 只允许出现在
    /// `xops-store`，有一条枚举全仓的测试证明这件事
    /// （`tests/no_sqlite_outside_store.rs`）。所以"读一遍 sqlite_master"这件事
    /// 不能由调用方自己去做——它要在这里露出一个口子。
    ///
    /// ⚠️ **回的是「这个库此刻长什么样」，不是「代码里写着什么」。**
    /// 关系投影表是**运行时声明的**（`Relations::declare`），
    /// 谁声明了才有——一个没被装配起来的服务，它的投影表就不在这张表里。
    /// 这正是自证要的那件事：**问跑起来的东西，不问源码。**
    ///
    /// 内部索引（`sqlite_*`）不在里面。
    ///
    /// # Errors
    /// 库读不了。
    pub fn schema(&self) -> Result<Vec<(String, Vec<String>)>> {
        let connection = self.locked()?;
        let mut tables: Vec<String> = {
            let mut statement = connection
                .prepare(
                    "SELECT name FROM sqlite_master \
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
                )
                .map_err(sql_error)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(sql_error)?;
            rows.collect::<std::result::Result<_, _>>()
                .map_err(sql_error)?
        };
        tables.sort_unstable();
        let mut out = Vec::with_capacity(tables.len());
        for table in tables {
            // `PRAGMA` 不吃参数绑定，而表名来自 `sqlite_master` 本身——
            // 不是外来输入。上面那条查询已经把它限在这个库里的真表上了。
            let mut statement = connection
                .prepare(&format!("PRAGMA table_info({table})"))
                .map_err(sql_error)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(sql_error)?;
            let columns: Vec<String> = rows
                .collect::<std::result::Result<_, _>>()
                .map_err(sql_error)?;
            out.push((table, columns));
        }
        Ok(out)
    }
}
