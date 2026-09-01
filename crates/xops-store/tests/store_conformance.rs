//! 存储契约的一致性测试。
//!
//! **每一条都对两个实现各跑一遍。** 这不是为了覆盖率——`CON-012` 的硬验收是
//! "换一个内存实现进去，写入路径与它上面的一切不改一行"，而唯一能证明这句话的办法，
//! 就是让同一份测试正文在两个实现上都成立，实现只由一个构造函数区分。

use std::sync::Arc;

use xops_store::{MemoryStore, SqliteStore, Store, space};

fn implementations() -> Vec<(&'static str, Arc<dyn Store>)> {
    vec![
        ("memory", Arc::new(MemoryStore::new())),
        (
            "sqlite",
            Arc::new(SqliteStore::in_memory().expect("开不了内存库")),
        ),
    ]
}

#[test]
fn 写了能读回来() {
    for (name, store) in implementations() {
        assert_eq!(store.get(space::ROW, b"missing").unwrap(), None, "{name}");
        store.put(space::ROW, b"k", b"v").unwrap();
        assert_eq!(
            store.get(space::ROW, b"k").unwrap(),
            Some(b"v".to_vec()),
            "{name}"
        );
    }
}

#[test]
fn 同一个键写第二次是覆盖() {
    for (name, store) in implementations() {
        store.put(space::ROW, b"k", b"first").unwrap();
        store.put(space::ROW, b"k", b"second").unwrap();
        assert_eq!(
            store.get(space::ROW, b"k").unwrap(),
            Some(b"second".to_vec()),
            "{name}"
        );
    }
}

#[test]
fn 删不存在的键也算成功() {
    for (name, store) in implementations() {
        store
            .delete(space::ROW, b"never-existed")
            .unwrap_or_else(|_| panic!("{name}"));
        store.put(space::ROW, b"k", b"v").unwrap();
        store.delete(space::ROW, b"k").unwrap();
        assert_eq!(store.get(space::ROW, b"k").unwrap(), None, "{name}");
    }
}

#[test]
fn 空间之间互不可见() {
    for (name, store) in implementations() {
        store.put(space::ROW, b"k", b"row").unwrap();
        store.put(space::EVENT, b"k", b"event").unwrap();
        assert_eq!(
            store.get(space::ROW, b"k").unwrap(),
            Some(b"row".to_vec()),
            "{name}"
        );
        assert_eq!(
            store.get(space::EVENT, b"k").unwrap(),
            Some(b"event".to_vec()),
            "{name}"
        );
        assert!(
            store.scan(space::META, b"", None, 10).unwrap().is_empty(),
            "{name}"
        );
    }
}

#[test]
fn 按前缀升序扫() {
    for (name, store) in implementations() {
        for key in [&b"a:3"[..], b"a:1", b"a:2", b"b:1"] {
            store.put(space::ROW, key, b"v").unwrap();
        }
        let keys: Vec<Vec<u8>> = store
            .scan(space::ROW, b"a:", None, 10)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            vec![b"a:1".to_vec(), b"a:2".to_vec(), b"a:3".to_vec()],
            "{name}"
        );
    }
}

#[test]
fn 扫描认上限() {
    for (name, store) in implementations() {
        for key in [&b"a:1"[..], b"a:2", b"a:3"] {
            store.put(space::ROW, key, b"v").unwrap();
        }
        assert_eq!(
            store.scan(space::ROW, b"a:", None, 2).unwrap().len(),
            2,
            "{name}"
        );
        assert_eq!(
            store.scan(space::ROW, b"a:", None, 0).unwrap().len(),
            0,
            "{name}"
        );
    }
}

#[test]
fn 从游标之后继续扫且不含游标本身() {
    for (name, store) in implementations() {
        for key in [&b"a:1"[..], b"a:2", b"a:3"] {
            store.put(space::ROW, key, b"v").unwrap();
        }
        let keys: Vec<Vec<u8>> = store
            .scan(space::ROW, b"a:", Some(b"a:1"), 10)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, vec![b"a:2".to_vec(), b"a:3".to_vec()], "{name}");
    }
}

#[test]
fn 前缀不越界() {
    for (name, store) in implementations() {
        store.put(space::ROW, b"ab", b"in").unwrap();
        store.put(space::ROW, b"ac", b"out").unwrap();
        let keys: Vec<Vec<u8>> = store
            .scan(space::ROW, b"ab", None, 10)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, vec![b"ab".to_vec()], "{name}");
    }
}

#[test]
fn 全是ff的前缀也扫得动() {
    // prefix_end 对全 0xFF 的前缀返回 None，走的是"没有上界"那条分支。
    for (name, store) in implementations() {
        store.put(space::ROW, &[0xFF, 0x01], b"v").unwrap();
        store.put(space::ROW, &[0xFF, 0x02], b"v").unwrap();
        assert_eq!(
            store.scan(space::ROW, &[0xFF], None, 10).unwrap().len(),
            2,
            "{name}"
        );
    }
}

#[test]
fn 值可以是任意字节() {
    for (name, store) in implementations() {
        let value = vec![0u8, 0xFF, 0x7F, b'\n'];
        store.put(space::ROW, b"k", &value).unwrap();
        assert_eq!(store.get(space::ROW, b"k").unwrap(), Some(value), "{name}");
    }
}
