//! SQLite 实现。
//!
//! ⚠️ **这是全仓唯一允许出现 `rusqlite` 的文件。** 越过这条线，D46 就落空了，
//! 而发现它的时机通常是真要换库的那天。`tests/no_sqlite_outside_store.rs` 会枚举全仓证明它。
//!
//! 这里只用一张两列主键的表和四条最普通的语句。**刻意不用**：触发器、存储过程、
//! 行锁、事务隔离级别、MVCC、外键、级联、JSON 列（`CON-012`）。连 `BEGIN` 都不写——
//! 一次写就是一条语句，它自己是原子的，再多的原子性本契约不承诺（`CON-007`）。

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use xops_core::{Error, Result};

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

    fn locked(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
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
