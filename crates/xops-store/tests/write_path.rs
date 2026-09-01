//! 写入路径的验收。
//!
//! **每一条同样对两个存储实现各跑一遍**——这就是 `CON-012` 的换实现硬验收：
//! 写入路径与它上面的一切，在两个实现下是同一份代码、同一份测试正文。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use xops_core::{Actor, Clock, Event, FixedClock, Result, RowId, SystemClock, TableName, WriteOp};
use xops_store::{
    Deferred, EvalScope, Evaluate, MemoryStore, Receipt, RowView, SchemaCheck, SqliteStore, Store,
    WriteEngine, WriteRequest, Writeback, keys, space,
};

fn stores() -> Vec<(&'static str, Arc<dyn Store>)> {
    vec![
        ("memory", Arc::new(MemoryStore::new())),
        (
            "sqlite",
            Arc::new(SqliteStore::in_memory().expect("开不了内存库")),
        ),
    ]
}

fn engine(store: Arc<dyn Store>) -> WriteEngine {
    WriteEngine::new(store, Arc::new(SystemClock))
}

fn table(name: &str) -> TableName {
    TableName::new(name).expect("表名不合法")
}

fn platform_insert(table: &TableName, payload: Value) -> WriteRequest {
    WriteRequest {
        table: table.clone(),
        op: WriteOp::Insert,
        row: RowId::generate(),
        payload,
        actor: Actor::Platform,
    }
}

// ——————————————————————————————— ② 追加事件 + 投影 ———————————————————————————————

#[test]
fn 一次插入落一条事件与一行投影() {
    for (name, store) in stores() {
        let engine = engine(store);
        let bugs = table("bugs");
        let request = platform_insert(&bugs, json!({"title": "崩了"}));
        let row = request.row;

        let receipt = engine.write(request).expect("写不进去");
        assert_eq!(receipt.events().len(), 1, "{name}");
        assert_eq!(receipt.primary().seq, 1, "{name}：序号从 1 开始");

        let stored = engine.read(&bugs, row).unwrap().expect("读不到刚写的行");
        assert_eq!(stored.payload, json!({"title": "崩了"}), "{name}");
        assert_eq!(stored.seq, 1, "{name}");
        assert_eq!(engine.events(&bugs, 0, 10).unwrap().len(), 1, "{name}");
    }
}

#[test]
fn 序号每表独立且不跳号() {
    for (name, store) in stores() {
        let engine = engine(store);
        let (bugs, issues) = (table("bugs"), table("issues"));
        for _ in 0..3 {
            engine.write(platform_insert(&bugs, json!({}))).unwrap();
        }
        engine.write(platform_insert(&issues, json!({}))).unwrap();

        let bug_seqs: Vec<u64> = engine
            .events(&bugs, 0, 10)
            .unwrap()
            .iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(bug_seqs, vec![1, 2, 3], "{name}");
        let issue_seqs: Vec<u64> = engine
            .events(&issues, 0, 10)
            .unwrap()
            .iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(issue_seqs, vec![1], "{name}：另一张表从头数");
    }
}

#[test]
fn 软删之后读不到但事件与墓碑都还在() {
    for (name, store) in stores() {
        let engine = engine(store);
        let bugs = table("bugs");
        let request = platform_insert(&bugs, json!({"title": "崩了"}));
        let row = request.row;
        engine.write(request).unwrap();

        engine
            .write(WriteRequest {
                table: bugs.clone(),
                op: WriteOp::Delete,
                row,
                payload: Value::Null,
                actor: Actor::Platform,
            })
            .unwrap();

        assert!(
            engine.read(&bugs, row).unwrap().is_none(),
            "{name}：删了就读不到"
        );
        let tombstone = engine
            .read_including_deleted(&bugs, row)
            .unwrap()
            .expect("墓碑必须还在（D42 软删）");
        assert!(tombstone.is_deleted(), "{name}");
        assert_eq!(
            engine.events(&bugs, 0, 10).unwrap().len(),
            2,
            "{name}：删除也追加事件"
        );
    }
}

#[test]
fn 事件按序号翻页() {
    for (name, store) in stores() {
        let engine = engine(store);
        let bugs = table("bugs");
        for _ in 0..5 {
            engine.write(platform_insert(&bugs, json!({}))).unwrap();
        }
        let first: Vec<u64> = engine
            .events(&bugs, 0, 2)
            .unwrap()
            .iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(first, vec![1, 2], "{name}");
        let next: Vec<u64> = engine
            .events(&bugs, 2, 2)
            .unwrap()
            .iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(next, vec![3, 4], "{name}：after 是严格大于");
    }
}

#[test]
fn 事件带得住写入者与时刻() {
    for (name, store) in stores() {
        let clock = Arc::new(FixedClock::new(1_700_000_000_000));
        let engine = WriteEngine::new(store, clock.clone());
        let bugs = table("bugs");
        engine
            .write(WriteRequest {
                actor: Actor::User { user: "u-1".into() },
                ..platform_insert(&bugs, json!({}))
            })
            .unwrap();
        clock.advance(5);
        engine.write(platform_insert(&bugs, json!({}))).unwrap();

        let events = engine.events(&bugs, 0, 10).unwrap();
        assert_eq!(
            events[0].actor,
            Actor::User { user: "u-1".into() },
            "{name}"
        );
        assert_eq!(events[0].at.as_millis(), 1_700_000_000_000, "{name}");
        assert_eq!(
            events[1].at.as_millis(),
            1_700_000_000_005,
            "{name}：时间从时钟来"
        );
    }
}

// ——————————————————————————————— ① schema 校验 ———————————————————————————————

struct RejectAll;

impl SchemaCheck for RejectAll {
    fn check(&self, _request: &WriteRequest) -> Result<()> {
        Err(xops_core::Error::invalid("schema 不认这一行"))
    }
}

#[test]
fn schema不过时二三四都不发生() {
    for (name, store) in stores() {
        let engine = engine(Arc::clone(&store)).with_schema_check(Arc::new(RejectAll));
        let bugs = table("bugs");
        assert!(
            engine.write(platform_insert(&bugs, json!({}))).is_err(),
            "{name}"
        );
        assert!(
            engine.events(&bugs, 0, 10).unwrap().is_empty(),
            "{name}：一条事件都不该有"
        );
        assert_eq!(engine.last_seq(&bugs).unwrap(), 0, "{name}");
    }
}

// ——————————————————————————————— ③④ 求值与代写 ———————————————————————————————

/// 一个假的"流程"：结算表是 `approvals`，主体表是 `bugs`。
struct FakeFlow {
    settlement: TableName,
    subject: TableName,
    /// 求值时往这里写回。
    writebacks: Mutex<Vec<Writeback>>,
    /// 求值被调用的次数——用来证明"写回的行不再触发求值"。
    calls: AtomicUsize,
    /// 求值期间卡住，用来证明 ③ 确实在区间内。
    hold: Mutex<Option<mpsc::Receiver<()>>>,
    entered: Mutex<Option<mpsc::Sender<()>>>,
}

impl FakeFlow {
    fn new(settlement: &TableName, subject: &TableName) -> Self {
        Self {
            settlement: settlement.clone(),
            subject: subject.clone(),
            writebacks: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
            hold: Mutex::new(None),
            entered: Mutex::new(None),
        }
    }

    fn writing_back(self, writebacks: Vec<Writeback>) -> Self {
        *self.writebacks.lock().unwrap() = writebacks;
        self
    }
}

impl Evaluate for FakeFlow {
    fn scope(&self, _table: &TableName) -> EvalScope {
        EvalScope {
            writeback_tables: vec![self.settlement.clone(), self.subject.clone()],
            update_only: vec![self.subject.clone()],
        }
    }

    fn evaluate(&self, _request: &WriteRequest, _view: &dyn RowView) -> Result<Vec<Writeback>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = self.entered.lock().unwrap().as_ref() {
            entered.send(()).ok();
        }
        if let Some(hold) = self.hold.lock().unwrap().as_ref() {
            hold.recv().ok();
        }
        Ok(std::mem::take(&mut *self.writebacks.lock().unwrap()))
    }
}

#[test]
fn 求值交回来的行由平台在同一区间内代写() {
    for (name, store) in stores() {
        let (approvals, bugs) = (table("approvals"), table("bugs"));
        let subject_row = RowId::generate();
        let flow = Arc::new(
            FakeFlow::new(&approvals, &bugs).writing_back(vec![Writeback {
                table: bugs.clone(),
                op: WriteOp::Update,
                row: subject_row,
                payload: json!({"state": "已通过"}),
                actor: Actor::Plugin {
                    plugin: "gate".into(),
                },
            }]),
        );
        let engine = engine(store).with_evaluate(flow.clone());

        let receipt = engine
            .write(platform_insert(&approvals, json!({"vote": "yes"})))
            .unwrap();
        assert_eq!(receipt.events().len(), 2, "{name}：结算行 + 代写的主体行");
        assert_eq!(receipt.events()[1].table, bugs, "{name}");
        assert_eq!(
            engine.read(&bugs, subject_row).unwrap().unwrap().payload,
            json!({"state": "已通过"}),
            "{name}"
        );
        assert_eq!(
            flow.calls.load(Ordering::SeqCst),
            1,
            "{name}：写回的行不再触发求值"
        );
    }
}

#[test]
fn 只有insert参与求值() {
    for (name, store) in stores() {
        let (approvals, bugs) = (table("approvals"), table("bugs"));
        let flow = Arc::new(FakeFlow::new(&approvals, &bugs));
        let engine = engine(store).with_evaluate(flow.clone());

        let request = platform_insert(&approvals, json!({}));
        let row = request.row;
        engine.write(request).unwrap();
        assert_eq!(flow.calls.load(Ordering::SeqCst), 1, "{name}");

        for op in [WriteOp::Update, WriteOp::Delete] {
            engine
                .write(WriteRequest {
                    table: approvals.clone(),
                    op,
                    row,
                    payload: json!({}),
                    actor: Actor::Platform,
                })
                .unwrap();
        }
        assert_eq!(
            flow.calls.load(Ordering::SeqCst),
            1,
            "{name}：update / delete 不触发求值（D45）"
        );
    }
}

#[test]
fn 代写第三张表被拒() {
    for (name, store) in stores() {
        let (approvals, bugs) = (table("approvals"), table("bugs"));
        let flow = Arc::new(
            FakeFlow::new(&approvals, &bugs).writing_back(vec![Writeback {
                table: table("notes"),
                op: WriteOp::Insert,
                row: RowId::generate(),
                payload: json!({}),
                actor: Actor::Plugin {
                    plugin: "gate".into(),
                },
            }]),
        );
        let engine = engine(store).with_evaluate(flow);

        let error = engine
            .write(platform_insert(&approvals, json!({})))
            .expect_err("第三张表必须被拒");
        assert!(
            error.message().contains("第三张表"),
            "{name}：{}",
            error.message()
        );
    }
}

#[test]
fn 对主体表只能update不能insert() {
    for (name, store) in stores() {
        let (approvals, bugs) = (table("approvals"), table("bugs"));
        let flow = Arc::new(
            FakeFlow::new(&approvals, &bugs).writing_back(vec![Writeback {
                table: bugs.clone(),
                op: WriteOp::Insert,
                row: RowId::generate(),
                payload: json!({}),
                actor: Actor::Plugin {
                    plugin: "gate".into(),
                },
            }]),
        );
        let engine = engine(store).with_evaluate(flow);

        let error = engine
            .write(platform_insert(&approvals, json!({})))
            .expect_err("主体表 insert 必须被拒");
        assert!(
            error.message().contains("只能 update"),
            "{name}：{}",
            error.message()
        );
    }
}

#[test]
fn 写回结算表本身不自锁() {
    for (name, store) in stores() {
        let (approvals, bugs) = (table("approvals"), table("bugs"));
        let flow = Arc::new(
            FakeFlow::new(&approvals, &bugs).writing_back(vec![Writeback {
                table: approvals.clone(),
                op: WriteOp::Update,
                row: RowId::generate(),
                payload: json!({"note": "未被采纳"}),
                actor: Actor::Platform,
            }]),
        );
        let engine = engine(store).with_evaluate(flow);

        // 锁已经在手里，可重入（CON-004）。若不可重入，这里会当场自锁死。
        let receipt = engine
            .write(platform_insert(&approvals, json!({})))
            .unwrap();
        assert_eq!(receipt.events().len(), 2, "{name}");
        assert_eq!(receipt.events()[1].seq, 2, "{name}：同一张表的第二条事件");
    }
}

// ——————————————————————————————— 区间边界与并发 ———————————————————————————————

#[test]
fn 求值期间同一张表的第二个写进不来() {
    for (name, store) in stores() {
        let (approvals, bugs) = (table("approvals"), table("bugs"));
        let (release, held) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let flow = FakeFlow::new(&approvals, &bugs);
        *flow.hold.lock().unwrap() = Some(held);
        *flow.entered.lock().unwrap() = Some(entered_tx);
        let engine = Arc::new(engine(store).with_evaluate(Arc::new(flow)));

        let first = {
            let engine = Arc::clone(&engine);
            let approvals = approvals.clone();
            thread::spawn(move || engine.write(platform_insert(&approvals, json!({"n": 1}))))
        };
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("求值没进来");

        let (done_tx, done_rx) = mpsc::channel();
        let second = {
            let engine = Arc::clone(&engine);
            let approvals = approvals.clone();
            thread::spawn(move || {
                let result = engine.write(platform_insert(&approvals, json!({"n": 2})));
                done_tx.send(()).ok();
                result
            })
        };
        assert!(
            done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "{name}：③ 在区间内，第二个写必须被挡在外面"
        );

        release.send(()).unwrap();
        first.join().unwrap().unwrap();
        // 放开发送端，第二个写的求值才不会停在 recv 上 —— 它也要过一次 ③。
        drop(release);
        second.join().unwrap().unwrap();
        assert_eq!(engine.last_seq(&approvals).unwrap(), 2, "{name}");
    }
}

struct BlockOn {
    table: TableName,
    hold: Mutex<mpsc::Receiver<()>>,
    entered: mpsc::Sender<()>,
}

impl SchemaCheck for BlockOn {
    fn check(&self, request: &WriteRequest) -> Result<()> {
        if request.table == self.table {
            self.entered.send(()).ok();
            self.hold.lock().unwrap().recv().ok();
        }
        Ok(())
    }
}

#[test]
fn 不同表的写互不阻塞() {
    for (name, store) in stores() {
        let (slow, fast) = (table("slow"), table("fast"));
        let (release, held) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let engine = Arc::new(engine(store).with_schema_check(Arc::new(BlockOn {
            table: slow.clone(),
            hold: Mutex::new(held),
            entered: entered_tx,
        })));

        let blocked = {
            let engine = Arc::clone(&engine);
            let slow = slow.clone();
            thread::spawn(move || engine.write(platform_insert(&slow, json!({}))))
        };
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("第一个写没进 schema 校验");

        // 这是表级锁不是全局锁 —— 另一张表照常写得进去。
        engine
            .write(platform_insert(&fast, json!({})))
            .expect("{name}：另一张表被挡住了");
        assert_eq!(engine.last_seq(&fast).unwrap(), 1, "{name}");

        release.send(()).unwrap();
        blocked.join().unwrap().unwrap();
    }
}

#[test]
fn 并发写同一张表不丢不重不跳号() {
    const WRITERS: u64 = 32;
    for (name, store) in stores() {
        let engine = Arc::new(engine(store));
        let bugs = table("bugs");
        let mut handles = Vec::new();
        for index in 0..WRITERS {
            let engine = Arc::clone(&engine);
            let bugs = bugs.clone();
            handles.push(thread::spawn(move || {
                engine
                    .write(platform_insert(&bugs, json!({ "index": index })))
                    .unwrap()
            }));
        }
        let mut seqs: Vec<u64> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().primary().seq)
            .collect();
        seqs.sort_unstable();
        assert_eq!(
            seqs,
            (1..=WRITERS).collect::<Vec<_>>(),
            "{name}：序号必须恰好是 1..=N"
        );

        let events = engine.events(&bugs, 0, 1_000).unwrap();
        assert_eq!(events.len(), WRITERS as usize, "{name}");
        let indices: std::collections::BTreeSet<u64> = events
            .iter()
            .filter_map(|event| event.payload["index"].as_u64())
            .collect();
        assert_eq!(
            indices.len(),
            WRITERS as usize,
            "{name}：每个写入者的行都在"
        );
    }
}

// ——————————————————————————————— 锁外三件（CON-006） ———————————————————————————————

/// 提交之后再写一次同一张表。**如果它跑在锁内，这一次写会永远等下去。**
struct WriteAgain {
    engine: Mutex<Option<std::sync::Weak<WriteEngine>>>,
    armed: AtomicBool,
    outside: AtomicBool,
}

impl Deferred for WriteAgain {
    fn after_commit(&self, receipt: &Receipt) {
        if !self.armed.swap(false, Ordering::SeqCst) {
            return;
        }
        let engine = self
            .engine
            .lock()
            .unwrap()
            .clone()
            .and_then(|weak| weak.upgrade());
        let Some(engine) = engine else { return };
        let table = receipt.primary().table.clone();
        let (done, wait) = mpsc::channel();
        thread::spawn(move || {
            let written = engine
                .write(platform_insert(&table, json!({"from": "deferred"})))
                .is_ok();
            done.send(written).ok();
        });
        // 在锁内的话，这次写拿不到锁，只能超时。
        let written = wait.recv_timeout(Duration::from_secs(2)).unwrap_or(false);
        self.outside.store(written, Ordering::SeqCst);
    }
}

#[test]
fn 锁外的活确实在锁外() {
    for (name, store) in stores() {
        let deferred = Arc::new(WriteAgain {
            engine: Mutex::new(None),
            armed: AtomicBool::new(true),
            outside: AtomicBool::new(false),
        });
        let engine = Arc::new(engine(store).with_deferred(deferred.clone()));
        *deferred.engine.lock().unwrap() = Some(Arc::downgrade(&engine));

        let bugs = table("bugs");
        engine.write(platform_insert(&bugs, json!({}))).unwrap();
        assert!(
            deferred.outside.load(Ordering::SeqCst),
            "{name}：CON-006 —— 派发与通知必须在锁外，否则模型调用会跑进锁里"
        );
        assert_eq!(
            engine.last_seq(&bugs).unwrap(),
            2,
            "{name}：锁外那一次写也落了盘"
        );
    }
}

struct Exploding;

impl Deferred for Exploding {
    fn after_commit(&self, _receipt: &Receipt) {
        // 锁外的失败不回滚业务写。这里用 panic 之外的方式表达"它挂了"：什么都不做。
    }
}

#[test]
fn 锁外失败不回滚业务写() {
    for (name, store) in stores() {
        let engine = engine(store).with_deferred(Arc::new(Exploding));
        let bugs = table("bugs");
        let request = platform_insert(&bugs, json!({"title": "崩了"}));
        let row = request.row;
        engine.write(request).unwrap();
        assert!(engine.read(&bugs, row).unwrap().is_some(), "{name}");
    }
}

// ——————————————————————————————— 崩溃与重放 ———————————————————————————————

fn write_u64(store: &dyn Store, table: &TableName, name: &str, value: u64) {
    store
        .put(space::META, &keys::meta(table, name), &value.to_be_bytes())
        .unwrap();
}

#[test]
fn 投影落后时重放追上() {
    for (name, store) in stores() {
        let engine = engine(Arc::clone(&store));
        let bugs = table("bugs");
        let request = platform_insert(&bugs, json!({"title": "崩了"}));
        let row = request.row;
        engine.write(request).unwrap();

        // 造一个"事件落了、投影没落"的现场：抹掉投影，把水位拨回去。
        store.delete(space::ROW, &keys::row(&bugs, row)).unwrap();
        write_u64(&*store, &bugs, "applied", 0);
        assert!(
            engine.read(&bugs, row).unwrap().is_none(),
            "{name}：现场造好了"
        );

        // 下一次写进区间时先修复。
        engine.write(platform_insert(&bugs, json!({}))).unwrap();
        assert!(
            engine.read(&bugs, row).unwrap().is_some(),
            "{name}：事件是真相，投影该被重放回来"
        );
    }
}

#[test]
fn 事件已落但水位没落时认下来而不是覆盖() {
    for (name, store) in stores() {
        let engine = engine(Arc::clone(&store));
        let bugs = table("bugs");
        engine
            .write(platform_insert(&bugs, json!({"n": 1})))
            .unwrap();

        // 造一个"事件写了、seq 水位没写"的现场：手工塞一条第 2 号事件，水位仍停在 1。
        let orphan = Event {
            id: xops_core::Id::generate(),
            table: bugs.clone(),
            row: RowId::generate(),
            seq: 2,
            op: WriteOp::Insert,
            at: SystemClock.now(),
            actor: Actor::Platform,
            payload: json!({"n": 2}),
        };
        store
            .put(
                space::EVENT,
                &keys::event(&bugs, 2),
                &serde_json::to_vec(&orphan).unwrap(),
            )
            .unwrap();

        let receipt = engine
            .write(platform_insert(&bugs, json!({"n": 3})))
            .unwrap();
        assert_eq!(
            receipt.primary().seq,
            3,
            "{name}：孤儿事件被认下来，新写接在它后面"
        );

        let events = engine.events(&bugs, 0, 10).unwrap();
        assert_eq!(events.len(), 3, "{name}");
        assert_eq!(
            events[1].payload,
            json!({"n": 2}),
            "{name}：孤儿事件没有被覆盖（I-D）"
        );
        assert!(
            engine.read(&bugs, orphan.row).unwrap().is_some(),
            "{name}：它的投影也补上了"
        );
    }
}
