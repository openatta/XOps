//! 表级写串行与四步区间。
//!
//! ```text
//! ① schema 校验 → ② 追加事件 + 投影 → ③ 流程节点求值 → ④ 插件写回
//! ```
//!
//! **把 ③④ 圈进来是这条规则的全部意义**（`CON-002`）：否则求值读到的表可能已被另一个写
//! 改过，`_flows` 的状态迁移就不是原子的，两条并发的结算行会同时被判为"该节点的最后一票"。
//!
//! 本 crate 不知道什么是 schema、什么是流程、什么是插件。**①③ 是注入进来的**，
//! 没注入时是 no-op；④ 不是注入的，它是平台按 ③ 交回来的行代写这个动作本身（`CON-003`）。

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xops_core::{Actor, Clock, Error, Event, Id, Result, RowId, TableName, Timestamp, WriteOp};

use crate::keys;
use crate::locks::TableLocks;
use crate::store::{Store, space};

/// 事件序号的水位：这张表写到第几条了。
const META_SEQ: &str = "seq";
/// 投影的水位：事件放到第几条了。**它可能落后于 `META_SEQ`，那正是它存在的理由。**
const META_APPLIED: &str = "applied";

/// 一次写的请求。
#[derive(Debug, Clone)]
pub struct WriteRequest {
    pub table: TableName,
    pub op: WriteOp,
    pub row: RowId,
    /// `Insert` / `Update` 是行的新形态；`Delete` 惯例上给 `Value::Null`。
    pub payload: Value,
    /// 谁写的。`I-B`：只能来自令牌、执行标识、插件求值或平台，不来自请求体。
    pub actor: Actor,
}

/// 一行现在是什么。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub table: TableName,
    pub row: RowId,
    /// 让它成为现在这样的那条事件的序号。
    pub seq: u64,
    pub op: WriteOp,
    pub at: Timestamp,
    pub actor: Actor,
    pub payload: Value,
}

impl Row {
    /// 被软删了没有。**删除是软删除**（D42）：行与事件都还在，历史照样查得到。
    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.op == WriteOp::Delete
    }
}

/// ① 之前的补齐位。**RP-04 注入**，本 crate 不知道要补什么。
///
/// 它存在的理由是一件在 RP-01 落地时没看清的事：**自动补的列位有一部分必须在区间内算**。
/// 自增序号就是那一部分——两个并发写如果各自在区间外算一次"下一个号"，会算出同一个。
/// 把它挪进区间，序号就由表锁串行了。
///
/// RP-01 的包文档写着：接进来时若发现必须改写入路径，说明点位当初留错了，**回头修 RP-01**，
/// 不要在下游绕开。这就是那次回头修——**新增一个位，不改 `SchemaCheck` 的形状**，
/// 因为后者已经有实现方了。
pub trait PreWrite: Send + Sync + 'static {
    /// 在取完锁、校验之前改写这次请求。
    ///
    /// # Errors
    /// 补不出来（比如序号读不到）。这时候整次写中止，②③④ 都不发生。
    fn prepare(&self, request: WriteRequest) -> Result<WriteRequest>;
}

/// ① schema 校验。**RP-04 注入**，本 crate 不知道列是什么。
pub trait SchemaCheck: Send + Sync + 'static {
    /// # Errors
    /// 这次写不合 schema。区间会当场中止，②③④ 都不发生。
    fn check(&self, request: &WriteRequest) -> Result<()>;
}

/// ③ 求值时能读到的东西。读到的是**② 已经落定之后**的样子。
pub trait RowView {
    /// # Errors
    /// 底层不可用。
    fn read(&self, table: &TableName, row: RowId) -> Result<Option<Row>>;
}

/// ③ 交回来的、要平台代写的一行（`CON-003`）。
#[derive(Debug, Clone)]
pub struct Writeback {
    pub table: TableName,
    pub op: WriteOp,
    pub row: RowId,
    pub payload: Value,
    pub actor: Actor,
}

/// 这次求值**可能**写回哪些表。**在取锁之前问一次**。
///
/// 存在的理由是 `CON-004`：锁要一次性按表名全序拿下，那就必须在开锁之前知道锁集合。
/// 也正因为它，锁集合在流程定义的时候就是已知且有限的——允许运行时动态取任意多把锁，
/// 死锁立刻可构造。
#[derive(Debug, Clone, Default)]
pub struct EvalScope {
    /// 求值可能写回的表。**平台只肯代写本流程的结算表与主体表**，第三张表不代写（`CON-005`）。
    pub writeback_tables: Vec<TableName>,
    /// 其中只允许 `Update` 的那些（主体表）。
    ///
    /// 主体表**不能 insert**：`FLW` 规定"主体表插入新行"就是随行发起实例的动作，
    /// 允许插件 insert 等于让它开出新实例，自激回路会从求值挪到实例发起。
    pub update_only: Vec<TableName>,
}

impl EvalScope {
    /// 什么都不写回。
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    fn validate(&self) -> Result<()> {
        for table in &self.update_only {
            if !self.writeback_tables.contains(table) {
                return Err(Error::internal(format!(
                    "求值声明 {table} 只能 update，却没有把它列进可写回的表——锁集合会不完整"
                )));
            }
        }
        Ok(())
    }

    fn authorize(&self, writeback: &Writeback, settlement: &TableName) -> Result<()> {
        let allowed =
            &writeback.table == settlement || self.writeback_tables.contains(&writeback.table);
        if !allowed {
            return Err(Error::invalid(format!(
                "平台不代写第三张表：{}。真要写，用一个任务（CON-005）",
                writeback.table
            )));
        }
        if self.update_only.contains(&writeback.table) && writeback.op != WriteOp::Update {
            return Err(Error::invalid(format!(
                "对主体表 {} 只能 update，不能 {:?}（CON-003）",
                writeback.table, writeback.op
            )));
        }
        Ok(())
    }
}

/// ③ 流程节点求值。**RP-15 注入**，本 crate 不知道什么是节点。
pub trait Evaluate: Send + Sync + 'static {
    /// 取锁之前问：这次写的求值可能写回哪些表。
    fn scope(&self, table: &TableName) -> EvalScope;

    /// 区间内调用。返回要平台代写的行。
    ///
    /// # Errors
    /// ⚠️ **返回 `Err` 是"这次写失败了"，不是"这个节点没通过"。**
    /// 求值超时、插件抛异常、死循环被中断——按 §7.4 它们一律是**未通过**，
    /// 行照常留在表里。把它们表达成 `Err` 会让一个坏插件把整张表的写打挂。
    /// 这条纪律在 RP-15 那一侧，本 crate 只能把它写在这里。
    ///
    /// 还有一件事这里保证不了、只能由注入方守住：**② 已经落盘了**。
    /// 没有事务（`CON-007`），`Err` 不会把刚写进去的行撤回来。
    fn evaluate(&self, request: &WriteRequest, view: &dyn RowView) -> Result<Vec<Writeback>>;
}

/// 一次写的回执：这个区间里产生的全部事件，含 ④ 代写的那些。
#[derive(Debug, Clone)]
pub struct Receipt {
    events: Vec<Event>,
}

impl Receipt {
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// 触发这次写的那条事件（区间里的第一条）。
    ///
    /// # Panics
    /// 回执必然至少有一条事件，空回执是内部错误。
    #[must_use]
    pub fn primary(&self) -> &Event {
        self.events.first().expect("回执至少有一条事件")
    }
}

/// 锁**外**要做的事（`CON-006`）。
///
/// 三件事明确在这里而不在区间里：「节点被激活」事件的派发与任务入队（**模型调用绝不在锁内**）·
/// 通知行的写入 · 到期清理。
///
/// 它们失败不回滚业务写——业务写在调用它之前就已经落定了。
pub trait Deferred: Send + Sync + 'static {
    fn after_commit(&self, receipt: &Receipt);
}

/// 写入路径。**全系统唯一的业务写入口。**
///
/// `I-N`：不存在只改投影而不写事件的代码路径——投影是私有的，只有这里能碰。
pub struct WriteEngine {
    store: Arc<dyn Store>,
    clock: Arc<dyn Clock>,
    locks: TableLocks,
    pre_write: Option<Arc<dyn PreWrite>>,
    schema: Option<Arc<dyn SchemaCheck>>,
    evaluate: Option<Arc<dyn Evaluate>>,
    deferred: Option<Arc<dyn Deferred>>,
}

impl std::fmt::Debug for WriteEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteEngine")
            .field("pre_write", &self.pre_write.is_some())
            .field("schema", &self.schema.is_some())
            .field("evaluate", &self.evaluate.is_some())
            .field("deferred", &self.deferred.is_some())
            .finish()
    }
}

impl WriteEngine {
    /// ①③ 与锁外都没接的写入路径。**M0 的形态就是这个**。
    #[must_use]
    pub fn new(store: Arc<dyn Store>, clock: Arc<dyn Clock>) -> Self {
        Self {
            store,
            clock,
            locks: TableLocks::new(),
            pre_write: None,
            schema: None,
            evaluate: None,
            deferred: None,
        }
    }

    /// 接上 ① 之前的补齐位。RP-04 用。
    #[must_use]
    pub fn with_pre_write(mut self, pre_write: Arc<dyn PreWrite>) -> Self {
        self.pre_write = Some(pre_write);
        self
    }

    /// 接上 ①。RP-04 用。
    #[must_use]
    pub fn with_schema_check(mut self, schema: Arc<dyn SchemaCheck>) -> Self {
        self.schema = Some(schema);
        self
    }

    /// 接上 ③（连带 ④ 的代写）。RP-15 用。
    #[must_use]
    pub fn with_evaluate(mut self, evaluate: Arc<dyn Evaluate>) -> Self {
        self.evaluate = Some(evaluate);
        self
    }

    /// 接上锁外三件的出口。RP-11 / RP-12 / RP-17 用。
    #[must_use]
    pub fn with_deferred(mut self, deferred: Arc<dyn Deferred>) -> Self {
        self.deferred = Some(deferred);
        self
    }

    /// 写一行。四步在同一个串行区间内完成。
    ///
    /// # Errors
    /// schema 不过、求值报错、底层不可用。
    pub fn write(&self, request: WriteRequest) -> Result<Receipt> {
        // 只有 insert 参与求值（D45）。update / delete 不触发、不结算、也不撤销已有结算。
        let scope = match (&self.evaluate, request.op) {
            (Some(evaluate), WriteOp::Insert) => evaluate.scope(&request.table),
            _ => EvalScope::none(),
        };
        scope.validate()?;

        let mut tables: BTreeSet<TableName> = scope.writeback_tables.iter().cloned().collect();
        tables.insert(request.table.clone());

        let receipt = {
            // ——— 区间开始：一次性按表名全序取锁（CON-004） ———
            let held = self.locks.acquire(&tables)?;
            for table in held.tables() {
                self.repair(table)?;
            }

            // ①' 补齐（区间内算，序号才不会撞）
            let request = match &self.pre_write {
                Some(pre_write) => {
                    let prepared = pre_write.prepare(request.clone())?;
                    // 锁集合是照着补齐**之前**的请求算的。补齐挪了表、挪了行、换了 op，
                    // 手里这把锁就不再是该拿的那把 —— 那是一个安静的越界，宁可当场炸。
                    if prepared.table != request.table
                        || prepared.row != request.row
                        || prepared.op != request.op
                    {
                        return Err(Error::internal(
                            "补齐只能改 payload：表、行与写法在取锁之前就定了",
                        ));
                    }
                    prepared
                }
                None => request,
            };

            // ① schema 校验
            if let Some(schema) = &self.schema {
                schema.check(&request)?;
            }

            // ② 追加事件 + 投影
            let mut events = vec![self.append(
                &request.table,
                request.op,
                request.row,
                &request.payload,
                &request.actor,
            )?];

            // ③ 流程节点求值 → ④ 平台代写它交回来的行
            if let Some(evaluate) = &self.evaluate
                && request.op == WriteOp::Insert
            {
                let view = EngineView { engine: self };
                for writeback in evaluate.evaluate(&request, &view)? {
                    scope.authorize(&writeback, &request.table)?;
                    debug_assert!(
                        held.holds(&writeback.table),
                        "写回的表必须已经在锁集合里，否则 scope 与 authorize 有一处算错了"
                    );
                    // 写回的行**不再触发求值**（§6.4）——自激回路从这里断掉。
                    events.push(self.append(
                        &writeback.table,
                        writeback.op,
                        writeback.row,
                        &writeback.payload,
                        &writeback.actor,
                    )?);
                }
            }
            Receipt { events }
            // ——— 区间结束：held 在这里析构，锁按相反顺序放开 ———
        };

        // 锁外（CON-006）。失败不回滚业务写。
        if let Some(deferred) = &self.deferred {
            deferred.after_commit(&receipt);
        }
        Ok(receipt)
    }

    /// 读一行现在的样子。**软删的行读出来是 `None`**，但它的事件还在。
    ///
    /// # Errors
    /// 底层不可用或投影损坏。
    pub fn read(&self, table: &TableName, row: RowId) -> Result<Option<Row>> {
        Ok(self.read_row(table, row)?.filter(|row| !row.is_deleted()))
    }

    /// 读一行，**连软删的墓碑一起**。历史查询与清理用它。
    ///
    /// # Errors
    /// 底层不可用或投影损坏。
    pub fn read_including_deleted(&self, table: &TableName, row: RowId) -> Result<Option<Row>> {
        self.read_row(table, row)
    }

    /// 按序号升序读一张表的事件。`after` 是**严格大于**。
    ///
    /// # Errors
    /// 底层不可用或事件损坏。
    pub fn events(&self, table: &TableName, after: u64, limit: usize) -> Result<Vec<Event>> {
        let prefix = keys::table_prefix(table);
        let cursor = keys::event(table, after);
        let after_key = if after == 0 {
            None
        } else {
            Some(cursor.as_slice())
        };
        self.store
            .scan(space::EVENT, &prefix, after_key, limit)?
            .into_iter()
            .map(|(_, bytes)| decode(&bytes, "事件"))
            .collect()
    }

    /// 这张表写到第几条了。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn last_seq(&self, table: &TableName) -> Result<u64> {
        self.read_u64(table, META_SEQ)
    }

    /// 把投影追到事件的水位上。
    ///
    /// 崩在"事件已落、投影未落"之间是可能的——没有事务（`CON-007`），也不打算有。
    /// **事件是真相，投影是它的缓存**，所以补法是重放，不是回滚。
    ///
    /// 区间开始时对锁集合里的每张表跑一次；正常情况下它只多花一次 `get`。
    fn repair(&self, table: &TableName) -> Result<()> {
        let recorded = self.read_u64(table, META_SEQ)?;
        // 也可能崩在"事件已落、连 seq 水位都还没落"之间，所以要往前探一格。
        let mut seq = recorded;
        while self
            .store
            .get(space::EVENT, &keys::event(table, seq + 1))?
            .is_some()
        {
            seq += 1;
        }
        if seq != recorded {
            self.write_u64(table, META_SEQ, seq)?;
        }

        let applied = self.read_u64(table, META_APPLIED)?;
        if applied >= seq {
            return Ok(());
        }
        for missing in applied + 1..=seq {
            let bytes = self
                .store
                .get(space::EVENT, &keys::event(table, missing))?
                .ok_or_else(|| Error::internal(format!("{table} 的第 {missing} 条事件不见了")))?;
            self.project(&decode::<Event>(&bytes, "事件")?)?;
        }
        self.write_u64(table, META_APPLIED, seq)
    }

    /// ② 的实现：追加一条事件，再把它投影出去。
    fn append(
        &self,
        table: &TableName,
        op: WriteOp,
        row: RowId,
        payload: &Value,
        actor: &Actor,
    ) -> Result<Event> {
        let seq = self.read_u64(table, META_SEQ)? + 1;
        let event = Event {
            id: Id::generate(),
            table: table.clone(),
            row,
            seq,
            op,
            at: self.clock.now(),
            actor: actor.clone(),
            payload: payload.clone(),
        };

        // I-D：事件一经写入即不可变。这条 get 是它在代码里唯一的兑现处。
        let key = keys::event(table, seq);
        if self.store.get(space::EVENT, &key)?.is_some() {
            return Err(Error::conflict(format!(
                "{table} 的第 {seq} 条事件已经存在——事件不可改写（I-D）"
            )));
        }
        self.store
            .put(space::EVENT, &key, &encode(&event, "事件")?)?;
        self.write_u64(table, META_SEQ, seq)?;
        self.project(&event)?;
        self.write_u64(table, META_APPLIED, seq)?;
        Ok(event)
    }

    /// 把一条事件放进投影。**私有**——`I-N` 靠这个可见性成立。
    fn project(&self, event: &Event) -> Result<()> {
        let row = Row {
            table: event.table.clone(),
            row: event.row,
            seq: event.seq,
            op: event.op,
            at: event.at,
            actor: event.actor.clone(),
            payload: event.payload.clone(),
        };
        self.store.put(
            space::ROW,
            &keys::row(&event.table, event.row),
            &encode(&row, "投影")?,
        )
    }

    fn read_row(&self, table: &TableName, row: RowId) -> Result<Option<Row>> {
        self.store
            .get(space::ROW, &keys::row(table, row))?
            .map(|bytes| decode(&bytes, "投影"))
            .transpose()
    }

    fn read_u64(&self, table: &TableName, name: &str) -> Result<u64> {
        let Some(bytes) = self.store.get(space::META, &keys::meta(table, name))? else {
            return Ok(0);
        };
        let bytes: [u8; 8] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| Error::internal(format!("{table} 的水位 {name} 损坏了")))?;
        Ok(u64::from_be_bytes(bytes))
    }

    fn write_u64(&self, table: &TableName, name: &str, value: u64) -> Result<()> {
        self.store
            .put(space::META, &keys::meta(table, name), &value.to_be_bytes())
    }
}

struct EngineView<'a> {
    engine: &'a WriteEngine,
}

impl RowView for EngineView<'_> {
    fn read(&self, table: &TableName, row: RowId) -> Result<Option<Row>> {
        self.engine.read(table, row)
    }
}

fn encode<T: Serialize>(value: &T, what: &str) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| Error::internal(format!("{what}序列化失败：{error}")))
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8], what: &str) -> Result<T> {
    serde_json::from_slice(bytes)
        .map_err(|error| Error::internal(format!("{what}读不回来：{error}")))
}
