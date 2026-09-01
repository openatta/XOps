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

/// SQLite 上的存储契约实现。
pub struct SqliteStore {
    connection: Mutex<Connection>,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore").finish_non_exhaustive()
    }
}

impl SqliteStore {
    /// 打开一个文件库，不存在就建。
    ///
    /// # Errors
    /// 打不开或建表失败。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path).map_err(sql_error)?;
        Self::prepare(connection)
    }

    /// 打开一个进程内的临时库。测试用。
    ///
    /// # Errors
    /// 建表失败。
    pub fn in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory().map_err(sql_error)?;
        Self::prepare(connection)
    }

    fn prepare(connection: Connection) -> Result<Self> {
        connection.execute(SCHEMA, []).map_err(sql_error)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub(crate) fn locked(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| Error::internal("SQLite 连接的锁中毒了"))
    }
}

fn sql_error(error: rusqlite::Error) -> Error {
    Error::unavailable(format!("SQLite: {error}"))
}

impl Store for SqliteStore {
    fn get(&self, space: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.locked()?
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
        let connection = self.locked()?;
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

        let connection = self.store.locked()?;
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
}
