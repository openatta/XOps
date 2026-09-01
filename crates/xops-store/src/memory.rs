//! 内存实现。
//!
//! **它不是桩。** `CON-012` 的硬验收是"换一个内存实现进去，写入路径与它上面的一切
//! 不改一行"——那条验收要有一个真的第二实现才跑得起来，这就是它。
//!
//! 第二个实现同时也是契约正确性的证据：只写一个实现的契约会不自觉地长成那个实现的形状。

use std::collections::BTreeMap;
use std::sync::Mutex;

use xops_core::{Error, Result};

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
