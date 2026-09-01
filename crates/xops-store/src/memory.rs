//! 内存实现。
//!
//! **它不是桩。** `CON-012` 的硬验收是"换一个内存实现进去，写入路径与它上面的一切
//! 不改一行"——那条验收要有一个真的第二实现才跑得起来，这就是它。
//!
//! 第二个实现同时也是契约正确性的证据：只写一个实现的契约会不自觉地长成那个实现的形状。

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::Value;
use xops_core::{Error, Result, RowId};

use crate::relation::{Direction, Relation, Relations, Select};
use crate::store::{Store, prefix_end};

/// 键是 `(空间, 键)`，排序即扫描顺序。
type Entries = BTreeMap<(String, Vec<u8>), Vec<u8>>;

/// 一张按 `(空间, 键)` 排序的表，锁在最外层。
#[derive(Debug, Default)]
pub struct MemoryStore {
    data: Mutex<Entries>,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前有多少个键。测试用。
    ///
    /// # Panics
    /// 锁中毒。
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.lock().expect("内存存储的锁中毒了").len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn locked(&self) -> Result<std::sync::MutexGuard<'_, Entries>> {
        self.data
            .lock()
            .map_err(|_| Error::internal("内存存储的锁中毒了"))
    }
}

impl Store for MemoryStore {
    fn get(&self, space: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        Ok(self
            .locked()?
            .get(&(space.to_owned(), key.to_vec()))
            .cloned())
    }

    fn put(&self, space: &str, key: &[u8], value: &[u8]) -> Result<()> {
        self.locked()?
            .insert((space.to_owned(), key.to_vec()), value.to_vec());
        Ok(())
    }

    fn delete(&self, space: &str, key: &[u8]) -> Result<()> {
        self.locked()?.remove(&(space.to_owned(), key.to_vec()));
        Ok(())
    }

    fn scan(
        &self,
        space: &str,
        prefix: &[u8],
        after: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let data = self.locked()?;
        let start = after.map_or_else(|| prefix.to_vec(), <[u8]>::to_vec);
        let end = prefix_end(prefix);
        let mut out = Vec::new();
        for ((entry_space, key), value) in data.range((space.to_owned(), start.clone())..) {
            if entry_space != space || !key.starts_with(prefix) {
                break;
            }
            if let Some(end) = end.as_ref()
                && key.as_slice() >= end.as_slice()
            {
                break;
            }
            if after.is_some_and(|after| key.as_slice() <= after) {
                continue;
            }
            out.push((key.clone(), value.clone()));
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }
}

// ——————————————————————————————— 关系投影 ———————————————————————————————

/// 关系投影的内存实现。
///
/// **它不是桩**，理由与 [`MemoryStore`] 那句一样：只写一个实现的契约会不自觉地
/// 长成那个实现的形状。两条缝各有一个第二实现，`G12` 的验收才对两条都跑得起来。
#[derive(Debug, Default)]
pub struct MemoryRelations {
    declared: Mutex<BTreeMap<String, Relation>>,
    /// `(用来找的那几样, 原样带回来的东西)`。
    rows: Mutex<BTreeMap<(String, RowId), (Value, Value)>>,
}

impl MemoryRelations {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

impl Relations for MemoryRelations {
    fn declare(&self, relation: &Relation) -> Result<()> {
        relation.check()?;
        self.declared
            .lock()
            .map_err(|_| Error::internal("关系投影声明的锁中毒了"))?
            .entry(relation.name.clone())
            .or_insert_with(|| relation.clone());
        Ok(())
    }

    fn upsert(&self, relation: &str, row: RowId, columns: &Value, payload: &Value) -> Result<()> {
        self.relation(relation)?;
        self.rows
            .lock()
            .map_err(|_| Error::internal("关系投影的锁中毒了"))?
            .insert(
                (relation.to_owned(), row),
                (columns.clone(), payload.clone()),
            );
        Ok(())
    }

    fn remove(&self, relation: &str, row: RowId) -> Result<()> {
        self.rows
            .lock()
            .map_err(|_| Error::internal("关系投影的锁中毒了"))?
            .remove(&(relation.to_owned(), row));
        Ok(())
    }

    fn select(&self, relation: &str, select: &Select) -> Result<Vec<(RowId, Value)>> {
        let declared = self.relation(relation)?;
        select.check(&declared)?;
        let rows = self
            .rows
            .lock()
            .map_err(|_| Error::internal("关系投影的锁中毒了"))?;
        let mut hit: Vec<(RowId, Value, Value)> = rows
            .iter()
            .filter(|((name, _), _)| name == relation)
            .filter(|(_, (columns, _))| matches(select, columns))
            .map(|((_, row), (columns, payload))| (*row, columns.clone(), payload.clone()))
            .collect();
        if let Some((column, direction)) = &select.order {
            hit.sort_by(|left, right| {
                let ordering = compare(left.1.get(column), right.1.get(column));
                match direction {
                    Direction::Asc => ordering,
                    Direction::Desc => ordering.reverse(),
                }
            });
        }
        if select.limit > 0 {
            hit.truncate(select.limit);
        }
        Ok(hit
            .into_iter()
            .map(|(row, _, payload)| (row, payload))
            .collect())
    }

    fn clear(&self, relation: &str) -> Result<()> {
        self.rows
            .lock()
            .map_err(|_| Error::internal("关系投影的锁中毒了"))?
            .retain(|(name, _), _| name != relation);
        Ok(())
    }
}

fn matches(select: &Select, values: &Value) -> bool {
    let absent = |column: &str| values.get(column).is_none_or(Value::is_null);
    select
        .equals
        .iter()
        .all(|(column, value)| values.get(column) == Some(value))
        && select.is_null.iter().all(|column| absent(column))
        && select.is_not_null.iter().all(|column| !absent(column))
        && select.at_most.iter().all(|(column, bound)| {
            values
                .get(column)
                .and_then(Value::as_i64)
                .is_some_and(|found| found <= *bound)
        })
        && select.at_least.iter().all(|(column, bound)| {
            values
                .get(column)
                .and_then(Value::as_i64)
                .is_some_and(|found| found >= *bound)
        })
}

/// 与 SQLite 的排序对齐：null 最小，然后数字，然后文本。
fn compare(left: Option<&Value>, right: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (left, right) {
        (None | Some(Value::Null), None | Some(Value::Null)) => Ordering::Equal,
        (None | Some(Value::Null), _) => Ordering::Less,
        (_, None | Some(Value::Null)) => Ordering::Greater,
        (Some(Value::Number(a)), Some(Value::Number(b))) => a
            .as_f64()
            .partial_cmp(&b.as_f64())
            .unwrap_or(Ordering::Equal),
        (Some(a), Some(b)) => a.to_string().cmp(&b.to_string()),
    }
}
