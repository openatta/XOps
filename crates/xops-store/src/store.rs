//! 存储契约。
//!
//! **只有四个方法，这不是简化，是 `CON-012` 本身**：换一个数据库不需要改写入路径，
//! 唯一能保证这件事的办法就是让上层根本用不到数据库特有的能力。所以这里没有事务、
//! 没有条件更新、没有批量原子写、没有索引、没有查询语言——**增、删、改、查，仅此**。
//!
//! 代价认下来：一次写要落"事件 + 投影 + 两个水位"四个键，它们之间**没有原子性**。
//! 补法不是把原子性加回契约里，是让事件成为真相、投影可重放（见 `serial::repair`）。

use xops_core::Result;

/// 键空间。相当于一张扁平的键值表，键在空间内按字节序有序。
pub mod space {
    /// 事件。键 = `表名 \0 序号(8 字节大端)`。写进去就不再变（`I-D`）。
    pub const EVENT: &str = "event";
    /// 投影，也就是"这一行现在是什么"。键 = `表名 \0 行 ID(16 字节)`。
    pub const ROW: &str = "row";
    /// 水位与其它小记录。键 = `表名 \0 名字`。
    pub const META: &str = "meta";
}

/// 一层只有基本增删改查的存储。
///
/// 实现方必须保证：**同一个键的单次 `put` / `delete` 是原子的**（要么生效要么没生效，
/// 不会写出半个值）。除此之外不承诺任何跨键的东西。
pub trait Store: Send + Sync + 'static {
    /// 读一个键。不存在返回 `None`。
    ///
    /// # Errors
    /// 底层不可用。
    fn get(&self, space: &str, key: &[u8]) -> Result<Option<Vec<u8>>>;

    /// 写一个键。已存在就覆盖。
    ///
    /// # Errors
    /// 底层不可用。
    fn put(&self, space: &str, key: &[u8], value: &[u8]) -> Result<()>;

    /// 删一个键。不存在也算成功。
    ///
    /// # Errors
    /// 底层不可用。
    fn delete(&self, space: &str, key: &[u8]) -> Result<()>;

    /// 按前缀升序扫。`after` 给了就从**严格大于**它的键开始（翻页用）。
    ///
    /// # Errors
    /// 底层不可用。
    fn scan(
        &self,
        space: &str,
        prefix: &[u8],
        after: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>>;
}

/// 前缀扫描的右开边界：把前缀最后一个非 `0xFF` 字节加一。
///
/// 返回 `None` 表示"没有上界"——前缀全是 `0xFF`，或者前缀为空。
#[must_use]
pub fn prefix_end(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.pop() {
        if last != 0xFF {
            end.push(last + 1);
            return Some(end);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 前缀上界() {
        assert_eq!(prefix_end(b"ab"), Some(b"ac".to_vec()));
        assert_eq!(prefix_end(&[0x01, 0xFF]), Some(vec![0x02]));
        assert_eq!(prefix_end(&[0xFF, 0xFF]), None);
        assert_eq!(prefix_end(b""), None);
    }
}
