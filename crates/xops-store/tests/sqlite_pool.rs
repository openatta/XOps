//! 一条写连接 + 几条读连接。
//!
//! 分开的理由是**读不该排在写后面**——早先只有一条连接，一次看板查询和一次执行落账
//! 抢的是同一把锁。那才是"一张热表锁住所有人"的真正位置；
//! 表级写锁从来不是（`TableLocks` 是按表的，`_runs` 的写不挡别的表）。
//!
//! ⚠️ **多开连接换不来写并发**：SQLite 是单写者模型，全库同一时刻只有一个写事务。
//! 这里测的是**正确性**——分连接之后语义一个字不能变。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use xops_store::{Store, space};

fn temp_db(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("xops-pool-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("db.sqlite")
}

/// 用完就删，连 WAL 的两个附属文件一起。
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.0.clone().into_os_string();
            path.push(suffix);
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir(self.0.parent().unwrap());
    }
}

#[test]
fn 写完立刻在别的连接上读得到() {
    // **读己所写**：写走写连接，读走读连接——中间隔着一次提交，
    // 如果它不成立，这个改动会把整个系统变成一个偶发不一致的东西。
    let path = temp_db("read-your-writes");
    let _scratch = Scratch(path.clone());
    let store = xops_store::SqliteStore::open(&path).unwrap();

    for round in 0..64u32 {
        let key = round.to_be_bytes();
        store.put(space::ROW, &key, b"v1").unwrap();
        assert_eq!(
            store.get(space::ROW, &key).unwrap().as_deref(),
            Some(b"v1".as_slice()),
            "第 {round} 轮：写完马上读，读连接必须看得到"
        );
        store.put(space::ROW, &key, b"v2").unwrap();
        assert_eq!(
            store.get(space::ROW, &key).unwrap().as_deref(),
            Some(b"v2".as_slice()),
            "覆盖之后也一样"
        );
        store.delete(space::ROW, &key).unwrap();
        assert!(store.get(space::ROW, &key).unwrap().is_none(), "删了就没了");
    }
}

#[test]
fn 扫描也看得到刚写进去的() {
    let path = temp_db("scan-sees-writes");
    let _scratch = Scratch(path.clone());
    let store = xops_store::SqliteStore::open(&path).unwrap();
    for index in 0..32u32 {
        store
            .put(
                space::ROW,
                &[b"t\0".as_slice(), &index.to_be_bytes()].concat(),
                b"x",
            )
            .unwrap();
    }
    let seen = store.scan(space::ROW, b"t\0", None, 1_000).unwrap();
    assert_eq!(seen.len(), 32, "扫描走的是读连接，它得看得到全部");
}

#[test]
fn 多个线程一起读写不出错() {
    let path = temp_db("concurrent");
    let _scratch = Scratch(path.clone());
    let store: Arc<dyn Store> = Arc::new(xops_store::SqliteStore::open(&path).unwrap());
    let reads = Arc::new(AtomicUsize::new(0));

    // 一个写线程 + 四个读线程。**写是串行的，读不该被它挡住。**
    let writer = {
        let store = Arc::clone(&store);
        thread::spawn(move || {
            for index in 0..200u32 {
                store
                    .put(
                        space::ROW,
                        &[b"c\0".as_slice(), &index.to_be_bytes()].concat(),
                        b"v",
                    )
                    .unwrap();
            }
        })
    };
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let store = Arc::clone(&store);
            let reads = Arc::clone(&reads);
            thread::spawn(move || {
                for _ in 0..200 {
                    let page = store.scan(space::ROW, b"c\0", None, 16).unwrap();
                    reads.fetch_add(page.len(), Ordering::Relaxed);
                }
            })
        })
        .collect();

    writer.join().unwrap();
    for reader in readers {
        reader.join().unwrap();
    }
    assert_eq!(
        store.scan(space::ROW, b"c\0", None, 1_000).unwrap().len(),
        200,
        "写线程写下的一条不少"
    );
    assert!(reads.load(Ordering::Relaxed) > 0, "读线程确实读到了东西");
}

#[test]
fn 内存库只有一条连接但语义一样() {
    // `:memory:` 上每条连接都是一个各自独立的库，所以内存库不开读连接。
    // **测试因此与生产走同一份代码**，只是并发度不同。
    let store = xops_store::SqliteStore::in_memory().unwrap();
    store.put(space::ROW, b"k", b"v").unwrap();
    assert_eq!(
        store.get(space::ROW, b"k").unwrap().as_deref(),
        Some(b"v".as_slice())
    );
    assert_eq!(store.scan(space::ROW, b"k", None, 10).unwrap().len(), 1);
}

#[test]
fn 关掉读连接也一切照常() {
    // 读连接是**性能选择，不是能力依赖**：`0` 条读连接时读走写连接，语义不变。
    let path = temp_db("no-readers");
    let _scratch = Scratch(path.clone());
    let store = xops_store::SqliteStore::open_with_readers(&path, 0).unwrap();
    store.put(space::ROW, b"k", b"v").unwrap();
    assert_eq!(
        store.get(space::ROW, b"k").unwrap().as_deref(),
        Some(b"v".as_slice())
    );
}
