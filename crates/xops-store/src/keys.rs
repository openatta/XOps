//! 键的编码。
//!
//! 全部键都是 `表名 \0 <剩下的>`。表名不允许含 `\0`（`TableName::new` 挡住了），
//! 所以 `表名 \0` 是一个安全的前缀——不会有别的表的键落进来。
//!
//! 事件序号用**大端**编码，因为存储只承诺按字节序扫描，而我们要的是按序号扫描。

use xops_core::{RowId, TableName};

const SEPARATOR: u8 = 0;

fn with_table(table: &TableName) -> Vec<u8> {
    let mut key = Vec::with_capacity(table.as_str().len() + 1 + 16);
    key.extend_from_slice(table.as_str().as_bytes());
    key.push(SEPARATOR);
    key
}

/// 一张表的全部键的公共前缀。
#[must_use]
pub fn table_prefix(table: &TableName) -> Vec<u8> {
    with_table(table)
}

/// 事件键：`表名 \0 序号`。
#[must_use]
pub fn event(table: &TableName, seq: u64) -> Vec<u8> {
    let mut key = with_table(table);
    key.extend_from_slice(&seq.to_be_bytes());
    key
}

/// 投影键：`表名 \0 行 ID`。
#[must_use]
pub fn row(table: &TableName, row: RowId) -> Vec<u8> {
    let mut key = with_table(table);
    key.extend_from_slice(row.as_id().as_bytes());
    key
}

/// 水位键：`表名 \0 名字`。
#[must_use]
pub fn meta(table: &TableName, name: &str) -> Vec<u8> {
    let mut key = with_table(table);
    key.extend_from_slice(name.as_bytes());
    key
}

/// 从事件键里把序号取回来。键不是这张表的事件键时返回 `None`。
#[must_use]
pub fn event_seq(table: &TableName, key: &[u8]) -> Option<u64> {
    let prefix = table_prefix(table);
    let rest = key.strip_prefix(prefix.as_slice())?;
    let bytes: [u8; 8] = rest.try_into().ok()?;
    Some(u64::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xops_core::Id;

    fn table(name: &str) -> TableName {
        TableName::new(name).unwrap()
    }

    #[test]
    fn 事件键按序号升序() {
        let bugs = table("bugs");
        let mut keys = [event(&bugs, 300), event(&bugs, 2), event(&bugs, 1_000)];
        keys.sort();
        let seqs: Vec<u64> = keys
            .iter()
            .filter_map(|key| event_seq(&bugs, key))
            .collect();
        assert_eq!(seqs, vec![2, 300, 1_000], "大端编码才有这个性质");
    }

    #[test]
    fn 表之间不串() {
        let bugs = table("bugs");
        // "bugs2" 的键不能落进 "bugs" 的前缀 —— 分隔符就是为了这个。
        let other = table("bugs2");
        assert!(!event(&other, 1).starts_with(&table_prefix(&bugs)));
    }

    #[test]
    fn 行键定长() {
        let key = row(&table("bugs"), RowId::from_id(Id::from_parts(1, 2)));
        assert_eq!(key.len(), "bugs".len() + 1 + 16);
    }

    #[test]
    fn 别的键不会被当成事件键() {
        let bugs = table("bugs");
        assert_eq!(event_seq(&bugs, &meta(&bugs, "seq")), None);
        assert_eq!(event_seq(&bugs, &event(&table("other"), 1)), None);
    }
}
