//! 把一张**平台表**的投影整张读回来。
//!
//! 这段循环原本在六个包里各抄了一份,一个字都不差:
//!
//! ```text
//! xops-flow  xops-identity  xops-read  xops-repo  xops-skill  xops-task
//! ```
//!
//! 抄六份的代价不在今天,在换存储的那天:**要改的地方有六处,而且没有谁提醒你漏了一处。**
//! 收在这里之后,平台表的读法只有一个实现。
//!
//! # 它读的是什么
//!
//! 平台表(`_skills` `_tasks` `_repos` …)与用户表不一样:**它们不在表目录里**,
//! 所以 [`xops_table`] 那一层的查询面够不到它们。它们的行是
//! **一个审计信封**,业务对象装在信封的 `data` 里。
//!
//! ⚠️ **它是整张读**。平台表的量级是"一个部署里的技能数、任务数、仓绑定数",
//! 不是行数据——所以整张读是合适的。**真有一天不合适了,该做的是把那张表
//! 独立成一张带索引的真表**(见 `xops_store::relation`),不是在这里加参数。
//!
//! [`xops_table`]: https://docs.rs/xops-table

use serde::de::DeserializeOwned;
use xops_core::{Error, Result, RowId, TableName};
use xops_store::{Row, Store, keys, space};

/// 一次向存储要多少行。
const PAGE: usize = 256;

/// 把一张平台表里能解成 `T` 的行全部读回来。
///
/// **软删的跳过,解不成 `T` 的跳过**——同一张表上可以并存不同形状的行
/// (比如技能与技能版本),解不动的那些不是错误。
///
/// # Errors
/// 表名不合法 · 底层不可用 · 投影解不回来。
pub fn all<T: DeserializeOwned>(store: &dyn Store, table: &str) -> Result<Vec<T>> {
    let table = TableName::new(table)?;
    let prefix = keys::table_prefix(&table);
    let mut out = Vec::new();
    let mut cursor: Option<Vec<u8>> = None;
    loop {
        let page = store.scan(space::ROW, &prefix, cursor.as_deref(), PAGE)?;
        if page.is_empty() {
            return Ok(out);
        }
        cursor = page.last().map(|(key, _)| key.clone());
        for (_, bytes) in page {
            let row: Row = serde_json::from_slice(&bytes)
                .map_err(|error| Error::internal(format!("投影读不回来：{error}")))?;
            if row.is_deleted() {
                continue;
            }
            let Some(envelope) = crate::AuditEnvelope::from_payload(&row.payload) else {
                continue;
            };
            if let Ok(value) = serde_json::from_value::<T>(envelope.data) {
                out.push(value);
            }
        }
    }
}

/// 同上，但**解不成 `T` 就是错误**，而且带上行标识。
///
/// 两个函数的差别只有一处：**解不动的那些行,是跳过还是报错。**
/// 跳过适合"同一张表上并存着不同形状"的表;报错适合"这张表只有一种行"的表——
/// 那种表上出现一行解不动的,是一件该被看见的事。
///
/// # Errors
/// 表名不合法 · 底层不可用 · **任何一行解不回来**。
pub fn all_strict<T: DeserializeOwned>(store: &dyn Store, table: &str) -> Result<Vec<(RowId, T)>> {
    let name = TableName::new(table)?;
    let prefix = keys::table_prefix(&name);
    let mut out = Vec::new();
    let mut cursor: Option<Vec<u8>> = None;
    loop {
        let page = store.scan(space::ROW, &prefix, cursor.as_deref(), PAGE)?;
        if page.is_empty() {
            return Ok(out);
        }
        cursor = page.last().map(|(key, _)| key.clone());
        for (_, bytes) in page {
            let row: Row = serde_json::from_slice(&bytes)
                .map_err(|error| Error::internal(format!("投影读不回来：{error}")))?;
            if row.is_deleted() {
                continue;
            }
            let Some(envelope) = crate::AuditEnvelope::from_payload(&row.payload) else {
                continue;
            };
            let value = serde_json::from_value(envelope.data)
                .map_err(|error| Error::internal(format!("{table} 的行读不回来：{error}")))?;
            out.push((row.row, value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xops_store::MemoryStore;

    #[test]
    fn 空表读回来是空的() {
        let store = MemoryStore::new();
        let rows: Vec<serde_json::Value> = all(&store, "_skills").unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn 表名不合法当场拒() {
        let store = MemoryStore::new();
        assert!(all::<serde_json::Value>(&store, "").is_err());
    }
}
