//! 表级写锁。
//!
//! `CON-001`：一张表同一时刻只允许一个写在进行，第二个排队等待，不同的表互不影响。
//! `CON-004`：一组表的锁**一次性按表名全序获取**——全序即无循环等待，构造不出死锁。
//!
//! 用的是排号锁（ticket lock）而不是裸 `Mutex`，理由是"排队"这件事要可测：
//! 先到的写必须先拿到锁，否则"两个并发写的事件顺序确定"这条验收只能靠 sleep 去碰。

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Condvar, Mutex};

use xops_core::{Error, Result, TableName};

#[derive(Debug, Default)]
struct Ticket {
    next: u64,
    serving: u64,
}

#[derive(Debug, Default)]
struct TableLock {
    ticket: Mutex<Ticket>,
    ready: Condvar,
}

impl TableLock {
    fn acquire(&self) {
        let mut ticket = self
            .ticket
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mine = ticket.next;
        ticket.next += 1;
        while ticket.serving != mine {
            ticket = self
                .ready
                .wait(ticket)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn release(&self) {
        let mut ticket = self
            .ticket
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ticket.serving += 1;
        drop(ticket);
        self.ready.notify_all();
    }
}

/// 全部表锁的登记处。一个进程一份。
///
/// ⚠️ **它是进程内的。** 应用层的表级串行只在单实例部署下成立（`CON-012` 的连带前提）；
/// 多实例要一把进程外的锁，那是 M6 的事，现在不做。
#[derive(Debug, Default)]
pub struct TableLocks {
    locks: Mutex<HashMap<TableName, Arc<TableLock>>>,
}

impl TableLocks {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 一次性拿下一组表的锁，**按表名升序**。
    ///
    /// # Errors
    /// 登记处的锁中毒。
    pub fn acquire(&self, tables: &BTreeSet<TableName>) -> Result<Held> {
        let mut guards = Vec::with_capacity(tables.len());
        // BTreeSet 的迭代顺序就是表名的字典序 —— 全序在这一行上。
        for table in tables {
            let lock = {
                let mut locks = self
                    .locks
                    .lock()
                    .map_err(|_| Error::internal("表锁登记处的锁中毒了"))?;
                Arc::clone(locks.entry(table.clone()).or_default())
            };
            lock.acquire();
            guards.push(Guard { lock });
        }
        Ok(Held {
            tables: tables.clone(),
            guards,
        })
    }
}

#[derive(Debug)]
struct Guard {
    lock: Arc<TableLock>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.lock.release();
    }
}

/// 一个区间当前握着的那些锁。**析构即释放**，顺序与获取顺序相反。
#[derive(Debug)]
pub struct Held {
    tables: BTreeSet<TableName>,
    #[allow(dead_code, reason = "只靠 Drop 起作用：它在场就代表锁在手里")]
    guards: Vec<Guard>,
}

impl Held {
    /// 这张表的锁是不是已经在手里。
    ///
    /// **可重入靠它**（`CON-004`）：区间内写回结算表本身时不再取一次锁，
    /// 因为那把锁本来就在手上。
    #[must_use]
    pub fn holds(&self, table: &TableName) -> bool {
        self.tables.contains(table)
    }

    #[must_use]
    pub fn tables(&self) -> &BTreeSet<TableName> {
        &self.tables
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn table(name: &str) -> TableName {
        TableName::new(name).unwrap()
    }

    fn set(names: &[&str]) -> BTreeSet<TableName> {
        names.iter().map(|name| table(name)).collect()
    }

    #[test]
    fn 同一张表排队() {
        let locks = Arc::new(TableLocks::new());
        let held = locks.acquire(&set(&["bugs"])).unwrap();

        let (sender, receiver) = mpsc::channel();
        let background = {
            let locks = Arc::clone(&locks);
            thread::spawn(move || {
                let _second = locks.acquire(&set(&["bugs"])).unwrap();
                sender.send(()).ok();
            })
        };

        assert!(
            receiver.recv_timeout(Duration::from_millis(150)).is_err(),
            "第一个还握着锁，第二个不该进得来"
        );
        drop(held);
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("放开之后第二个应当立刻进来");
        background.join().unwrap();
    }

    #[test]
    fn 不同表互不阻塞() {
        let locks = TableLocks::new();
        let _bugs = locks.acquire(&set(&["bugs"])).unwrap();
        // 拿得到就证明这是表级锁而不是全局锁。
        let _issues = locks.acquire(&set(&["issues"])).unwrap();
    }

    #[test]
    fn 一组表按名字升序拿() {
        let locks = TableLocks::new();
        let held = locks.acquire(&set(&["issues", "_runs", "bugs"])).unwrap();
        let order: Vec<&str> = held.tables().iter().map(TableName::as_str).collect();
        assert_eq!(order, vec!["_runs", "bugs", "issues"]);
    }

    #[test]
    fn 反向声明的两组也不死锁() {
        // 一个流程的结算表是另一个流程的主体表，反之亦然 —— 这是最容易构造出死锁的形状。
        let locks = Arc::new(TableLocks::new());
        let mut handles = Vec::new();
        for round in 0..40 {
            for names in [set(&["a", "b"]), set(&["b", "a"])] {
                let locks = Arc::clone(&locks);
                handles.push(thread::spawn(move || {
                    let held = locks.acquire(&names).unwrap();
                    // 在区间里待一会儿，逼出交错。
                    thread::yield_now();
                    drop(held);
                    round
                }));
            }
        }
        for handle in handles {
            handle.join().expect("全序取锁不该死锁");
        }
    }

    #[test]
    fn 先到先得() {
        let locks = Arc::new(TableLocks::new());
        let held = locks.acquire(&set(&["bugs"])).unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));

        let mut handles = Vec::new();
        for index in 0..5 {
            let locks = Arc::clone(&locks);
            let order = Arc::clone(&order);
            handles.push(thread::spawn(move || {
                let _guard = locks.acquire(&set(&["bugs"])).unwrap();
                order.lock().unwrap().push(index);
            }));
            // 让每个线程都有机会先排上号，再放下一个。
            thread::sleep(Duration::from_millis(20));
        }
        drop(held);
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            *order.lock().unwrap(),
            vec![0, 1, 2, 3, 4],
            "排号锁必须先到先得"
        );
    }
}
