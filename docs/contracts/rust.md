# crate 接缝

## Purpose

crate 与 crate 之间**已定型的 trait 与读模型**。需求包总览 §3 第 2 条"接缝先于实现"
说的就是这份记录：一个包依赖另一个包时，依赖的是这里的元素，不是它的内部结构。

**crate 内部的类型不进这里。** 判据是：换一个实现进去，上层要不要改。要改的才是接缝。

## 谁往这里加元素

| 前缀 | 归属 | 硬验收 |
|---|---|---|
| `rust:xops-core#*` `rust:xops-store#*` | RP-01 | 换一个内存实现进去，写入路径与它上面的一切不改一行（`CON-012`） |
| `rust:xops-exec#*` | RP-07 WP-A | 换成桩引擎不改契约（`EXE-014`）；契约里不出现引擎的任何类型 |
| `rust:xops-identity#*` `rust:xops-audit#*` | RP-02 | |
| `rust:xops-table#*` | RP-04 | |
| `rust:xops-read#*` | RP-05 | 读模型是前端唯一能看见的东西；它同时以 `api:http.*` 的形态出现在 api.md |
| `rust:xops-repo#*` | RP-08 | |
| `rust:xops-skill#*` `rust:xops-task#*` `rust:xops-dispatch#*` | RP-09 · RP-10 · RP-11 · RP-12 · RP-13 | |
| `rust:xops-flow#*` | RP-14 WP-C | RP-15 只经它驱动迁移，**不得自己改 `_flows` / `_flow_nodes`** |
| `rust:xops-settle#*` `rust:xops-script#*` | RP-15 · RP-16 | |
| `rust:xops-notice#*` `rust:xops-template#*` `rust:xops-xforge#*` | RP-17 · RP-18 · RP-19 | |

## 命名

```
rust:<crate>#<路径>        例：rust:xops-store#Store::put
                                rust:xops-exec#ExecContract::submit
                                rust:xops-flow#Instance::settle
```

方言文件（有实现之后）：`rust/<crate>.api.txt`，由 `cargo-public-api` 生成。
**手写 trait 签名只是第一阶段**——代码一落地就切成快照当权威，因为手写的接口文档必然漂移。

## Elements

### Element: rust:xops-core#Error
- module: xops-core
- consumers: [全部]
- 错误 + `ErrorKind` 七类 + `Result`。`ErrorKind::retriable()` 回答"该重试还是该改参数"（`MCP-007`）。
- ⚠️ `Denied` **对外不得原样透出**：`MCP-008` 要求无权限与不存在返回一致，映射在 RP-03。

### Element: rust:xops-core#Clock
- module: xops-core
- consumers: [xops-store, 之后每个要写时刻的包]
- `Timestamp`（UTC 毫秒）+ `Clock` + `SystemClock` / `FixedClock`。
- 写入路径上的时刻**只从 `Clock` 来**，否则"事件顺序确定"这类验收只能靠 sleep 去碰。

### Element: rust:xops-core#Id
- module: xops-core
- consumers: [全部]
- 128 位、按时间可排序、定长 26 字符 Crockford base32。48 位毫秒 + 80 位熵。
- **同一毫秒内严格递增**：熵的高 48 位是进程内计数器。顺序反过来写这条就没了。

### Element: rust:xops-core#TableName
- module: xops-core
- consumers: [全部]
- 表名。**串行区间按它的字典序取锁**（`CON-004`），所以它必须可全序比较；不许含 `\0`（键编码靠它分段）。

### Element: rust:xops-core#RowId
- module: xops-core
- consumers: [全部]

### Element: rust:xops-core#WriteOp
- module: xops-core
- consumers: [全部]
- `Insert` / `Update` / `Delete`。**只有 `Insert` 参与流程求值**（D45）。

### Element: rust:xops-core#Actor
- module: xops-core
- consumers: [全部]
- `I-B`：写入者只能是令牌解析的用户、执行标识、插件求值或平台自身四者之一，**不来自请求体**。

### Element: rust:xops-core#Event
- module: xops-core
- consumers: [全部]
- 事件形状。`seq` **每表独立、从 1 开始、不跳号**。`payload` 对本层不透明——列与类型归 RP-04。
- `I-D`：写进去就不再变。

### Element: rust:xops-core#Role
- module: xops-core
- consumers: [xops-identity]
- 三个角色与它们的序（`PRJ-004`）。**"谁能做什么"那张表不在这里**，归 RP-02 的权限判定纯函数。

### Element: rust:xops-store#Store
- module: xops-store
- consumers: [全部]
- **存储契约，只有四个方法**：`get` / `put` / `delete` / `scan`。
- `CON-012` 的落点。多一个方法就是下一个实现要多兑现的一条承诺，也是契约往某个具体库形状上长的第一步。
- 硬验收：换 `MemoryStore` 进去，写入路径与它上面的一切不改一行。

### Element: rust:xops-store#space
- module: xops-store
- consumers: [全部]
- 三个键空间：`event`（事件）· `row`（投影）· `meta`（水位）。

### Element: rust:xops-store#MemoryStore
- module: xops-store
- consumers: [全部的测试]
- 契约的第二个实现。**它不是桩，是契约正确性的证据**——只写一个实现的契约会长成那个实现的形状。

### Element: rust:xops-store#SqliteStore
- module: xops-store
- consumers: [xopsd]
- 契约的第一个实现。**全仓唯一允许出现 `rusqlite` 的地方**，由 `tests/no_sqlite_outside_store.rs` 枚举全仓来守。

### Element: rust:xops-store#TableLocks
- module: xops-store
- consumers: [xops-store 内部]
- 表级写锁，排号锁（先到先得）。`acquire` **一次性按表名升序**拿下一组表（`CON-004`）。
- `Held::holds` 是可重入的依据：锁已在手里就不再取。
- ⚠️ 进程内的锁。应用层的表级串行**只在单实例部署下成立**；多实例要一把进程外的锁（M6）。

### Element: rust:xops-store#WriteEngine
- module: xops-store
- consumers: [全部]
- **全系统唯一的业务写入口。** 四步同一区间：① schema 校验 → ② 追加事件 + 投影 → ③ 求值 → ④ 代写。
- `I-N`：投影是私有的，不存在只改投影而不写事件的路径。
- `read` 读不到软删的行，`read_including_deleted` 读得到墓碑（D42）。

### Element: rust:xops-store#WriteRequest
- module: xops-store
- consumers: [全部]

### Element: rust:xops-store#Row
- module: xops-store
- consumers: [全部]
- 一行现在的样子 + 让它成为这样的那条事件的序号。

### Element: rust:xops-store#Receipt
- module: xops-store
- consumers: [RP-11, RP-12, RP-17]
- 一个区间里产生的**全部**事件，含 ④ 代写的那些。

### Element: rust:xops-store#SchemaCheck
- module: xops-store
- consumers: [RP-04]
- **① 的注入位。** 未注入时是 no-op。不过就当场中止，②③④ 都不发生。

### Element: rust:xops-store#Evaluate
- module: xops-store
- consumers: [RP-15]
- **③ 的注入位**，连带 ④ 的代写。`scope()` 在**取锁之前**被问一次——锁集合必须先于区间已知（`CON-004`）。
- ⚠️ `evaluate` 返回 `Err` 是"这次写失败了"，**不是"节点没通过"**。求值超时、插件异常、死循环被中断，
  按 §7.4 一律是未通过、行照常留在表里。把它们表达成 `Err` 会让一个坏插件把整张表的写打挂——
  这条纪律在 RP-15 那一侧，本 crate 只能声明它。
- ⚠️ 没有事务：`Err` 不会把 ② 已经落盘的行撤回来（`CON-007`）。

### Element: rust:xops-store#EvalScope
- module: xops-store
- consumers: [RP-15]
- 求值可能写回哪些表，其中哪些**只允许 update**（主体表，`CON-003`）。
- 第三张表平台不代写（`CON-005`）——由 `WriteEngine` 强制，不靠调用方自觉。

### Element: rust:xops-store#Writeback
- module: xops-store
- consumers: [RP-15, RP-16]
- ③ 交回来、由平台代写的一行。**写回的行不再触发求值**（§6.4，自激回路从这里断掉）。

### Element: rust:xops-store#RowView
- module: xops-store
- consumers: [RP-15]
- 求值时能读到的东西，读到的是 ② 已落定之后的样子。

### Element: rust:xops-store#Deferred
- module: xops-store
- consumers: [RP-11, RP-12, RP-17]
- **锁外三件的出口**（`CON-006`）：事件派发与任务入队（模型调用绝不在锁内）· 通知行写入 · 到期清理。
- 它们失败不回滚业务写——业务写在调用它之前就已经落定。

### Element: rust:xops-store#keys
- module: xops-store
- consumers: [xops-store, 需要造崩溃现场的测试]
- 键编码：全部键都是 `表名 \0 <剩下的>`；事件序号**大端**，因为存储只承诺按字节序扫描。
