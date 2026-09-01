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

### Element: rust:xops-audit#AuditEnvelope
- module: xops-audit
- consumers: [xops-identity, 之后每个要留痕的包]
- 一次写的 payload 信封：事件类型 · 所属项目 · 目标 · 主体 · 成败 · 结构化载荷（`AUD-002`）。
- ⚠️ **凡是要留痕的写，payload 一律是一个信封，对象本身装在 `data` 里。**
  这不是约定俗成，是 `AUD-005` 的实现方式：审计事件与业务写是**同一条事件**，
  所以"业务成功但没留痕"在结构上不可能——跨表写没有原子性（`CON-007`），
  任何"先写业务再写审计"的实现都做不到这一条。

### Element: rust:xops-audit#EventKind
- module: xops-audit
- consumers: [全部]
- `<域>.<动作>`，校验形状。`AUD-009` 的"统一目录与扩展方式"= `kinds::ALL` 常量清单 + 这个校验。
- **后面每个包往 `kinds` 里加自己的常量，不要在调用处写裸字符串**——那是唯一一处能回答
  "系统里到底有哪些事件类型"的地方。

### Element: rust:xops-audit#AuditLog
- module: xops-audit
- consumers: [xops-identity, RP-03 起各包]
- 追加没有业务行的留痕（`append`）· 维护索引（`index`）· 查（`query` / `history`）·
  重建索引（`rebuild_index`）· 到期清理（`prune`）。
- ⚠️ **`prune` 只吃 `_audit` 表上的记录。** 有业务行的那些事件**是业务状态本身**
  （项目、成员、令牌都由事件流重建），删了它们 `AUD-004` 当场落空。
  这是 `AUD-010` 与 `RET-001` 分得开的地方。
- 只建一个索引（项目 + 时间）。理由不是省事：`AUD-003` 是**可见性**要求，
  它必须在扫描之前成立，而不是查完再筛。

### Element: rust:xops-audit#Query
- module: xops-audit
- consumers: [RP-03 起各包]
- `AUD-008` 的五个维度。`viewer` 不是可选过滤器，**是可见性判定的入口**：
  平台级事件只有主体本人读得到。

### Element: rust:xops-audit#AuditRecord
- module: xops-audit
- consumers: [RP-03 起各包]

### Element: rust:xops-identity#UserId
- module: xops-identity
- consumers: [全部]
- 稳定、创建后不可变（`IDN-005`）。显示名与邮箱可变，**不得作为关联键使用**。

### Element: rust:xops-identity#User
- module: xops-identity
- consumers: [全部]
- `IDN-004`：顶层主体，彼此地位平等，没有上下级、没有分组。
- `IDN-006`：带来源提供方与外部账号，让"XOps 里的这个人"与"代码仓里提交的那个人"对得上。

### Element: rust:xops-identity#IdentityProvider
- module: xops-identity
- consumers: [RP-05 的 OAuth 回调]
- `IDN-001`：**全部登录经这一个接口**——新增提供方只实现它，不改用户模型、令牌模型与权限判定。
- `BuiltinProvider`（预置账号）与 `OAuthProvider`（GitHub 先做）是它的两个实现。
- ⚠️ **这里没有 HTTP。** 授权码换资料是两次出网调用，属于持有回调端点的那个包（RP-05）；
  本 crate 只定义注入位 `ProfileExchange`。`IDN-007`：回调只能做一件事。

### Element: rust:xops-identity#TokenSecret
- module: xops-identity
- consumers: [RP-03]
- 令牌原文。**只在签发那一刻存在一次**（`TOK-002`、`I-A`）。
- 没有 `Clone`、`Debug` 不打印内容、不实现 `Serialize`——想把它存下来的每条路
  都得先绕过这个类型，而那种绕法在评审里看得见。

### Element: rust:xops-identity#Token
- module: xops-identity
- consumers: [RP-03]
- 存下来的那一份：**只有 SHA-256 摘要，没有原文**。撤销与过期**行为完全一致**（`TOK-004`）。
- `last_used_at` 按分钟节流（`LAST_USED_RESOLUTION_MILLIS`）：认证在每次调用的路径上，
  每次都写会让 `_tokens` 这一张表把全系统的调用串行掉（`CON-001` 是表级锁）。
  **精度是刻意的，不是偷懒。**

### Element: rust:xops-identity#Action
- module: xops-identity
- consumers: [RP-03 起各包]
- 平台里所有要判权的动作，穷举一个 enum。`Action::floor()` 是**"哪个角色能做什么"唯一的一份表**
  （`PRJ-004`）——别处再写一个 `match`，三个角色变四个的那天就会漏掉一处。

### Element: rust:xops-identity#can
- module: xops-identity
- consumers: [RP-03 起各包]
- `PRJ-010`：**确定性纯函数**，`(角色, 动作)` 之外不看任何东西，不读库、不问模型、不看时间（G8）。
- `can_in` 多带一个"项目归没归档"：归档后**任何写动作一律不行，哪怕来的是所有者**（`PRJ-009`）。

### Element: rust:xops-identity#ProjectId
- module: xops-identity
- consumers: [全部]

### Element: rust:xops-identity#Slug
- module: xops-identity
- consumers: [RP-18 模板]
- `PRJ-003`：全平台唯一、创建后不可变、字符集窄到能被人抄进 issue 标题里（`TPL-005` 要用它组合标识）。

### Element: rust:xops-identity#Project
- module: xops-identity
- consumers: [全部]

### Element: rust:xops-identity#Member
- module: xops-identity
- consumers: [全部]
- `PRJ-007`：成员关系是 `(项目, 用户)` 的一条记录，**不是用户身上的一个属性**——
  没有跨项目全局角色，没有组织级继承（G4）。

### Element: rust:xops-identity#owners_after
- module: xops-identity
- consumers: [xops-identity]
- `PRJ-006` 的纯函数形态："这次改动之后还剩几个所有者"。单独拎出来，是因为它要在
  写入区间里被调用，而区间里不该有别的逻辑。

### Element: rust:xops-identity#Directory
- module: xops-identity
- consumers: [RP-03 起各包]
- 身份、令牌、项目、成员的读写面，以及它们与审计的接缝。
- **`resolve(secret)` 是全系统唯一的身份来源**（`TOK-007`、G5）。四种解析失败给的是
  逐字一致的一句话（`TOK-005`）。
- **`authorize` 是所有写操作前的那一道**：项目不存在 / 不是成员 / 角色不够 / 项目已归档，
  **四种情形返回同一个错误**（`PRJ-008` + `MCP-008`）——否则错误码本身就是探测他人项目的工具。
- `rebuild_at(t)` 仅凭事件流重建那一刻的状态，**不读任何当前视图**（`AUD-004`）。

### Element: rust:xops-identity#Identity
- module: xops-identity
- consumers: [RP-03]
- 解析出来的调用者。`actor()` 是写入时署的名——`I-B`：**它来自这里，不来自请求体。**

### Element: rust:xops-identity#PLATFORM_TABLES
- module: xops-identity
- consumers: [RP-04, RP-05]
- 四张平台表：`_users` · `_tokens` · `_projects` · `_members`。
- ⚠️ **它们不是「五张系统表」**（`_runs` 那五张是业务上看得见的）。平台表是平台自己的账，
  **不参与建表、看板与表专属 tool 的派发**——RP-04 要把它们排除在外。

### Element: rust:xops-mcp#Schema
- module: xops-mcp
- consumers: [RP-04 起各包]
- 窄接口的输入形状。`FieldType` 是穷举 enum，**没有 `Object` / `Any` / `Json`**（`MCP-004`）。
- 本次新增 `Record`：**形状被声明死了的**嵌套记录。它不是口子，理由见
  `api:mcp.registry.record-field`。
- `to_json_schema()` 渲染 JSON Schema 2020-12，**嵌套对象一样带 `additionalProperties: false`**。

### Element: rust:xops-mcp#ToolSpec
- module: xops-mcp
- consumers: [全部 tool]
- **新增 `text_only`**：回话只给一个 `text` 类型的 content item，
  **不带 `structuredContent`**（`XFG-009`）。
- ⚠️ **别的 tool 一律不要打开它**——`structuredContent` 是调用方少写一次
  `JSON.parse` 的地方。默认关着，有测试盯着默认值。

### Element: rust:xops-mcp#Tool
- module: xops-mcp
- consumers: [RP-04 起各包]
- 干活的那一半。**认证、鉴权、schema 校验、幂等、留痕都已经在外面做完了。**

### Element: rust:xops-mcp#CallContext
- module: xops-mcp
- consumers: [RP-04 起各包]
- 已认证的调用上下文（`MCP-012` 第二件）：调用者 · 目标项目 · 角色 · 幂等键 · 参数 · 目录。
- `actor()` 是写入署的名——**来自令牌，不来自请求体**（`I-B`）。
- `envelope()` / `record()` 是统一的留痕构造（`MCP-012` 第四件）。

### Element: rust:xops-mcp#Registry
- module: xops-mcp
- consumers: [RP-04 起各包]
- tool 目录。`visible_to` 与调用鉴权**共用同一个判定**（`allows`）——
  `MCP-009` 的"裁剪不是只藏起来"因此是结构性的。
- 本次接上**动态来源**（`add_source` / `ToolSource`）：`visible_to` 与 `get` 同时看
  静态注册的与此刻派发出来的，**静态的优先**——派发出来的 tool 盖不掉注册过的。
- `visible_to` 多收一个 `project`：表专属 tool 只在它自己那个项目里出现。

### Element: rust:xops-mcp#McpServer
- module: xops-mcp
- consumers: [xopsd]
- 协议核心：一份 JSON-RPC 进来，一份响应出去。**这里没有传输**——协议核心不知道
  自己被谁喂进来，也因此整段可以被直接测。

### Element: rust:xops-mcp#ErrorContract
- module: xops-mcp
- consumers: [RP-04 起各包]

### Element: rust:xops-mcp#Idempotency
- module: xops-mcp
- consumers: [xops-mcp]
- 存的是**响应本身**，不是"见过了"的标记——`MCP-006` 的后半句是"返回与首次相同的结果"，
  不是"第二次报错"。

### Element: rust:xops-mcp#NON_MCP_ENTRYPOINTS
- module: xops-mcp
- consumers: [RP-05, RP-13]
- **四个非 MCP 入口的清单**（`MCP-013`），每一个都带着"它只能做的那一件事"与出处。
- 写成可枚举的常量，是为了让"再加一个入口"必须先改这里——而改这里在评审时看得见。
- ⚠️ **RP-05 与 RP-13 落地时要与这份清单对上**：OAuth 回调 · Git webhook · 会话面 · 令牌管理面。

### Element: rust:xops-mcp#PendingNodes
- module: xops-mcp
- consumers: [RP-14]
- 「我待处理的流程节点」的实现位。**RP-14 换掉的是这个 trait 的实现，不是 tool 的注册与 schema。**

### Element: rust:xops-mcp#transport
- module: xops-mcp
- consumers: [xopsd]
- 手写的、阻塞式的、只认 `Content-Length` 的 HTTP/1.1，加一个 stdio。
- **为一条路由引一整套异步栈进来，换回的是"两个服务面共用一个路由层"这个正好不该有的东西。**

### Element: rust:xops-store#PreWrite
- module: xops-store
- consumers: [RP-04]
- **① 之前的补齐位。** 在取完锁、校验之前改写这次请求。
- 它存在的理由是一件 RP-01 落地时没看清的事：**自动补的列位有一部分必须在区间内算**。
  自增序号就是那一部分——两个并发写如果各自在区间外算一次"下一个号"，会算出同一个。
- RP-01 的包文档写着"点位留错了就回头修本包，不要在下游绕开"。这就是那次回头修：
  **新增一个位，不改 `SchemaCheck` 的形状**，因为后者已经有实现方了。
- 补齐**只能改 payload**：表、行与写法在取锁之前就定了，动它们会让手里那把锁不再是该拿的那把——
  当场报错，不静默越界。

### Element: rust:xops-mcp#ToolSource
- module: xops-mcp
- consumers: [RP-04, RP-17]
- 运行时才知道有哪些 tool 的那一类来源。**`MCP-005` 必须落在其上的位。**
- 派发出来的 tool 与静态注册的**走同一条路**——同样交出五样、同样过 schema 校验、
  同样按角色裁剪。这正是 `MCP-005` 不构成对 `MCP-004` 破例的原因。

### Element: rust:xops-table#ColumnType
- module: xops-table
- consumers: [RP-05 起各包]
- **穷举的十一种**（`TBL-017`）：文本 · 长文本 · 整数 · 小数 · 布尔 · 时间 · 枚举 ·
  自增序号 · 行引用 · 二进制 · 派生文本。
- ⚠️ **没有 `json`，没有"任意对象"**（`TBL-021`）。
- 自增序号与派生文本**用户写不了、insert 之后也改不了**（`TBL-020`）。
- 行引用**只看形状**：平台不校验它指向的行存不存在，也不级联（`TBL-019`、`TBL-023`）。

### Element: rust:xops-table#Column
- module: xops-table
- consumers: [RP-05 起各包]
- 列名撞了 `AUTO_COLUMNS` 就构造不出来——`TBL-014` 的"任何列声明都不能覆盖它们"落在这里。

### Element: rust:xops-table#TableId
- module: xops-table
- consumers: [RP-05 起各包]
- 用户表：小写字母开头、只含小写字母数字与单连字符、**不能以 `_` 或 `sys-` 开头**。
- `slug()` 是它在 tool 名里的那一段（`_runs` → `sys-runs`）。

### Element: rust:xops-table#TableSchema
- module: xops-table
- consumers: [RP-05 起各包]
- 带一份**项目短名的副本**：派生文本要用 `{project.slug}`，而那一步发生在写入区间里——
  在锁内去问身份目录要一次短名，等于把一次查询接进这张表的写吞吐。短名创建后不可变，抄一份是安全的。
- `physical()` 是它给 RP-01 写入路径用的名字：`p<项目>.<表名>`。
  **业务上的"表"是 `(项目, 名字)`**，两个项目各建一张 `bugs` 是完全正常的事。

### Element: rust:xops-table#Protection
- module: xops-table
- consumers: [RP-05 起各包]
- `TBL-004`：**建表时声明，之后不可降级**——API 上根本没有改它的路。

### Element: rust:xops-table#WrittenBy
- module: xops-table
- consumers: [RP-05, RP-11, RP-12, RP-15, RP-16]
- **恰好四种取值，且必须自包含**（`TBL-015`）。②③ 内联那几项，**不能只存一个指向 `_runs` 的指针**——
  `_runs` 的行有保留期而结算行没有，一个月后还要能回答"这一票是谁的"，靠的就是内联的任务所有者。
- `trusted()`：**"不可信内容"不是一个额外的标记位，是这个类型的自然结果**（`TBL-016`）。
- 它与事件上的 `Actor` 是两层：事件的 actor 回答"哪一类"，行上的 `writtenBy` 回答"具体是谁、凭什么"。

### Element: rust:xops-table#Catalog
- module: xops-table
- consumers: [xopsd]
- 表目录，**同时是写入区间的 ①' 与 ①**。目录本身落在 `_tables` 平台表上，
  因而每次 schema 变更都自带一条不可变事件，`reload()` 能从事件流把它重建回来。

### Element: rust:xops-table#Tables
- module: xops-table
- consumers: [RP-05, RP-11, RP-12, RP-15, RP-16, RP-17, RP-18]
- 目录与行的读写面。**写串行与四步区间归 RP-01，本包只是用它。**
- 系统表**只有平台能写**：非 `WrittenBy::Platform` 的写当场被拒（`TBL-003`）。
- `history()` **软删过的表照样查得到**（`TBL-026`）。
- ⚠️ `history()` 扫的是整张表的事件流再按行过滤。表大了之后这里要一个按行的索引，
  而那个索引该建在这里，不是让调用方各自记一份。

### Element: rust:xops-table#DropGuard
- module: xops-table
- consumers: [RP-14]
- "这张表还被流程引用着吗"。**被引用为结算表或主体表的表不能删**（`TBL-026`），
  而那是 RP-14 才知道的事。现在挂一个永远放行的实现。

### Element: rust:xops-table#RowVersion
- module: xops-table
- consumers: [RP-05]

### Element: rust:xops-table#TableTools
- module: xops-table
- consumers: [xopsd]
- 表专属 tool 的派发源。**建表即派发、删表即停派**——每次被问的时候现算。

### Element: rust:xops-mcp#ProjectHook
- module: xops-mcp
- consumers: [xopsd, RP-04]
- 「项目建好之后接着做什么」。**平台建那四张系统表就挂在这里**（`TBL-005`）。
- 它存在是因为依赖方向：`xops-mcp → xops-identity`，而"什么是表"在 `xops-table`，
  后者在 `xops-mcp` 之上。让 `xops-identity` 反过来依赖它就成环了，所以留一个位，
  由 `xopsd` 把 RP-04 接进来。

### Element: rust:xops-read#Board
- module: xops-read
- consumers: [xops-web]
- 看板定义。**平台不内建任何报表**（`BRD-002`）——没有聚合、没有指标、没有 join。
  判断标准（`BRD-003`）：**如果有一天需要在平台代码里写"什么是缺陷密度"，那就越界了。**

### Element: rust:xops-read#BoardSpec
- module: xops-read
- consumers: [xops-web]
- 定义一个看板要给的那几样，**就是 `BRD-001` 那句话的逐字形态**。

### Element: rust:xops-read#Filter
- module: xops-read
- consumers: [xops-web]
- **只有等值与非空两种。**

### Element: rust:xops-read#ReadModel
- module: xops-read
- consumers: [xops-web, RP-06 经 HTTP]
- **前端唯一能看见的东西。** 前端不直连库、不拼 SQL、不调 MCP。
- 这份接口的**完备性**是 RP-06 能并行开工的全部前提：它需要的每一样数据都要在这里，
  不能让它去开第二条数据通路。
- 视图：`IdentityView` · `ProjectView` · `BoardSummary` · `BoardView` · `RowHistoryView` ·
  `SettlementView` · `LongTextView`。

### Element: rust:xops-read#BoardView
- module: xops-read
- consumers: [xops-web]
- **`writtenBy` 总是留着**：看板上的来源标识读的就是它（`TBL-016`）。

### Element: rust:xops-read#RowHistoryView
- module: xops-read
- consumers: [xops-web]

### Element: rust:xops-read#SettlementView
- module: xops-read
- consumers: [xops-web]
- **与单行历史是两个视图。** 现在返回空——形状已经定了，值等 RP-15 填 `_instance`。

### Element: rust:xops-web#ROUTES
- module: xops-web
- consumers: [RP-06, RP-13]
- **路由是一张可枚举的常量表，不是散在代码里的一堆 match 分支。**
- 本次加了 `POST /webhooks/git`，`Kind::Webhook`。**非 GET 的路由现在恰好三条**：
  两条凭据面 + 一条 webhook，**全部 `writes_business_objects: false`**——
  它们就是 `MCP-013` 认下的那几个例外，而这张表是它们的清单。
- ⚠️ 再往这张表里加非 GET 路由，先回去读 `MCP-013`：**例外只有四个**。

### Element: rust:xops-web#Sessions
- module: xops-web
- consumers: [xops-web]
- `I-L`：**Web 会话凭据与 MCP 令牌互不通用**。这条不是靠检查，是靠**两套东西根本不认识
  对方**：会话 id 有自己的前缀与键空间，MCP 令牌只认 `xops_` 开头并比对摘要。
  拿一个去换另一个，两边都查不到。

### Element: rust:xops-web#WebServer
- module: xops-web
- consumers: [xopsd]
- 只读 HTTP。`handle(&Request) -> Response` **没有传输，因而整段可以直接测**。
- ⚠️ 它与 MCP 的服务面**不共用路由层**。两边各有一小段 HTTP 解析，**这份重复是故意留的**：
  合起来就等于让两个服务面共用一个路由层，而那正是这条分工要避免的事。

### Element: rust:xops-web#Assets
- module: xops-web
- consumers: [xopsd, RP-06]
- 静态资源托管。三种形态：不带页面 · **编译期嵌入（发行形态）** · 运行时目录（开发用）。
- SPA 的深链回落到 `index.html`；路径穿越被挡在段级检查上。

### Element: rust:xops-web#Assets::embedded
- module: xops-web
- consumers: [xopsd]
- **前端产物在编译期嵌进二进制**（D55：部署方不需要 Node）。不是"运行时去某个目录找"——
  `build.rs` 走一遍 `web/dist`，把每个文件 `include_bytes!` 进来。
- `web/dist` 不在时嵌一张空表，`cargo build` 照样过，但**会打一条 warning**：
  它不该悄悄地过。
- `Assets::Directory` 是开发时的形态：改一行页面不用重编 Rust。

### Element: rust:xops-exec#ExecContract
- module: xops-exec
- consumers: [RP-11]
- **XOps 与执行引擎之间唯一的接缝**（`EXE-014`）。四个方法：`submit` / `status` / `cancel` / `collect`。
- ⚠️ **契约里不出现引擎的任何类型。** 这条有硬验收——换一个毫不相干的引擎进去，
  上面的一切不改一行；还有一个测试直接扫这个模块的源码，确认它没漏进 `attacore` 这类词。
- `EXE-021`：**提交即返回**。引擎不可用**不在 `submit` 上报错**——那是一次失败的执行，
  不是一次失败的提交（`EXE-030`）。

### Element: rust:xops-exec#Worksheet
- module: xops-exec
- consumers: [RP-11]
- 派工单：执行契约的输入。**本包不装配它，只校验它**。
- ⚠️ **表数据不在这里**（`EXE-013` / D44）：技能读不到表，需要表数据的由调用方
  经 MCP 查好、作为 `inputs` 传进来。这也是本包能第一天独立开工的原因。

### Element: rust:xops-exec#Capabilities
- module: xops-exec
- consumers: [RP-11, RP-08]
- 这次执行能碰到什么。**未声明的一律不提供**（`EXE-006`）；网络白名单**空表示不许出网**（`EXE-007`）。

### Element: rust:xops-exec#Limits
- module: xops-exec
- consumers: [RP-10, RP-11]
- 超时 · token 预算（`TSK-005`）· 内存上限。

### Element: rust:xops-exec#FailureKind
- module: xops-exec
- consumers: [RP-11, RP-12]
- **八类，一个都不能少**（`EXE-020`）。落 `_runs.failureKind`。
- `worth_retrying()` 是调用方决定要不要重跑的唯一依据。

### Element: rust:xops-exec#Outcome
- module: xops-exec
- consumers: [RP-11, RP-12]
- 产出 · 过程记录（`EXE-022`）· token 用量 · 起止时刻。**本包产生并移交，不负责持久化。**

### Element: rust:xops-exec#Engine
- module: xops-exec
- consumers: [xops-exec 内部]
- `Runtime` 与具体引擎之间的口子。**不是对外契约**——对外那条里不出现引擎。
- `healthy()` 是 `EXE-030` 的依据；`run()` 必须盯着 `Cancel`，因为
  "超时终止不得留下孤儿会话继续消耗模型额度"只有引擎这一侧做得到。

### Element: rust:xops-exec#Runtime
- module: xops-exec
- consumers: [RP-11]
- 把同步的引擎变成异步的契约，并守四条：提交即返回 · 引擎不可用不就地跑 ·
  超时强制终止 · **引擎崩了或卡死也要在有限时间内收摊**（`EXE-017`）。
- 最后一条落成两样：工作线程外面套 `catch_unwind`，加一个到点必收摊的看门狗
  （`GRACE_MILLIS`）。**没有任何一条路径能让一次执行永远停在 running 上。**

### Element: rust:xops-exec#StubEngine
- module: xops-exec
- consumers: [各包的测试]
- **不是玩具，是 `EXE-014` 的硬验收载体**——与 `CON-012` 的内存存储是同一种验收
  放在两个接缝上（G12）。

### Element: rust:xops-exec#IsolationLevel
- module: xops-exec
- consumers: [RP-11, RP-12, 部署方]
- **这条元素是本次最重要的一条。**
- `unsatisfied()` 逐条列出当前隔离级别**没有兑现**的需求，`still_held()` 列出不靠容器
  也成立的那些。两张表都有测试盯着，且不许有交集。
- 这是 `EXE-029` 那句"**沙箱不静默降级：兑现不了的逐条如实上报，绝不当作已兑现**"
  的落法——把它写成数据而不是散在注释里，是为了让它不会悄悄消失。

### Element: rust:xops-exec#BareBackend
- module: xops-exec
- consumers: [xopsd]
- 裸跑后端：四个执行契约（Process / FileSystem / Network / Sandbox）的裸跑实现。
- ⚠️ `NetworkProvider::enforced()` 返回 `false`：**白名单只被记录，没有被强制**。
  把这件事做成一个能问的方法，是为了让调用方不会以为它生效了。

### Element: rust:xops-exec#AttaCoreEngine
- module: xops-exec
- consumers: [xopsd]
- 接 `attacored`：NDJSON over Unix socket，一行一个 JSON-RPC 2.0 对象。
- `EXE-016` 在这里兑现：**一次执行一个会话，用完即弃**，会话 id 进过程记录——
  所以"第二次读不到第一次的痕迹"是**实测得到的**，不是看代码看出来的。
- ⚠️ **socket 路径与令牌绝不进派工单。** AttaCore 自己的文档把话说死了：
  "把 socket 或 token 暴露出去，等同于暴露模型凭据本身"。裸跑下 `EXE-015` 靠的就是这条：
  凭据在 attacored 那一侧，执行方手里没有通往它的路径。
- ⚠️ **不要自己拼 socket 路径去猜**——daemon 换启动参数重启时它会变。

### Element: rust:xops-repo#Sealer
- module: xops-repo
- consumers: [xopsd]
- 只读凭据的**可逆**加密（ChaCha20-Poly1305）。
- ⚠️ **它与访问令牌不是一回事**：令牌存单向摘要（`TOK-002`），因为系统只需要"对不对得上"；
  仓凭据拉取时要用原文，所以必须可逆。**两者共用一套做法是这一处最容易犯的错。**
- 密钥从部署侧来（`XOPS_SECRET_KEY`）。边界说清楚：**加密防的是"库被拷走"，不是"部署被攻陷"**。

### Element: rust:xops-repo#Secret
- module: xops-repo
- consumers: [xops-repo]
- 凭据原文。不实现 `Serialize`、`Debug` 不打印内容——想把它存下来或记进日志的每条路
  都得先绕过这个类型。

### Element: rust:xops-repo#GitPlatform
- module: xops-repo
- consumers: [RP-13]
- `RPO-007`：**平台差异收在一个接口后面**——认证、克隆、权限元数据校验、webhook 签名验证。
- `probe_write_access` 是 `RPO-002` 的落点：**实际推一次 `--dry-run`**，判定是真的、副作用是没有的。
  声明会撒谎，也会过期。
- ⚠️ **分不清是不是只读时一律报错，不猜**：只有"被拒绝"才算只读，"仓不存在""连不上"不算。
- `verify_webhook` 留给 RP-13（GitHub 的 `X-Hub-Signature-256`，HMAC-SHA256 + 等时比较）。

### Element: rust:xops-repo#Binding
- module: xops-repo
- consumers: [RP-11, RP-19]
- `RPO-001`：**当前绑一个**。`RPO-014`：**XForge 的登记挂在它上面，不另开一套对象**——
  `xforge` 那个位已经留好，内容归 RP-19。
- 远端地址里**不许带凭据**：`https://token@host/x.git` 会让凭据跟着 URL 进日志、
  进错误消息、进 `git remote -v`。

### Element: rust:xops-repo#Repos
- module: xops-repo
- consumers: [RP-11]
- 绑定 · 轮换 · 解绑 · 查状态 · **按确切修订备只读工作区**。
- `repo.status` 的响应里**没有 credential 字段，也不会有**（`RPO-003`）。
- `RPO-006`：**每次使用凭据访问仓库都留一条**——哪个项目、哪个仓、拉了什么修订。

### Element: rust:xops-repo#Workspace
- module: xops-repo
- consumers: [RP-07, RP-11]
- 一份备好的只读工作区。**析构即销毁。**
- `RPO-010`：`revision()` 是**确切**修订——"这份报告针对哪版代码"靠它回答。
- ⚠️ **修订不存在时明确失败，不静默用 HEAD 顶替**。这是 `XFG` 那句"gitHead 必须已推送"
  的落点：顶替一次，追溯链就断了，而且断得看不出来。

### Element: rust:xops-repo#AuthConfig
- module: xops-repo
- consumers: [xops-repo]
- **凭据只活在一个 0600 的临时 git 配置文件里，用完即删**（`RPO-005`）。
- 为什么不是环境变量、不是命令行参数：`ps` 看得见 argv，子进程整个继承环境变量。
- 残留风险认下来：**拉取期间它在磁盘上**。它不进 argv、不进环境、不进过程记录，
  但它确实在那儿几秒钟。

### Element: rust:xops-repo#Budget
- module: xops-repo
- consumers: [RP-11]
- 拉取的容量与时间上限（`RPO-011`）。超限失败并明确报告，不允许无限制占用。

### Element: rust:xops-repo#Deps
- module: xops-repo
- consumers: [xopsd]
- 写入路径 · 存储 · 审计 · 身份 · 时钟。**它们总是一起出现**，摊成五个参数
  调用处就会开始靠位置记顺序。

### Element: rust:xops-skill#Declaration
- module: xops-skill
- consumers: [RP-10, RP-11, RP-07]
- **四样，穷举的**（`SKL-007`）：输入契约 · 产出形态 · 是否读仓 + 出网白名单 · 时长上限。
- ⚠️ `I-I`：**未声明的一律不提供。** 这个结构里没有"其它能力"那一栏，
  所以"声明之外还有第五条获取能力的途径"这件事，得先改它才做得到。有测试数这几个字段。
- 输入契约**可机读**，因而 `check_arguments` 能真的校验——多一个没声明的参数就拒，
  与 `MCP-003` 是同一条纪律。

### Element: rust:xops-skill#Skill
- module: xops-skill
- consumers: [RP-10, RP-11]
- 身份与归属。内容与声明挂在版本上。

### Element: rust:xops-skill#Ownership
- module: xops-skill
- consumers: [RP-10, RP-11, RP-15]
- **两种，没有第三种**（`SKL-008`）：项目公共 · 个人私有。

### Element: rust:xops-skill#Version
- module: xops-skill
- consumers: [RP-10, RP-11, RP-15]
- `SKL-002`：**已发布的版本不可变**——改内容产生新版本，旧版本原样可查。
- `content` 是**不可信输入**（`SKL-006`、G7）：平台不解析其语义、不因其内容改变控制流。
- `used_for_settlement` 是 `SKL-011` 那条例外的开关：**标记由 RP-15 打，本包按它判可见性**。

### Element: rust:xops-skill#Skills
- module: xops-skill
- consumers: [RP-10, RP-11, RP-15, RP-16]
- 建 / 改（产生新版本）/ 记测试 / 发布 / 停用 / 派生 / 读 / 列。
- ⚠️ **`runnable_for` 每次现算，不缓存**。`SKL-009`：私有技能能读项目数据，是因为
  **它的所有者是项目成员**——权限来自人，不来自技能。缓存一次，"退出项目即失效"就没了。
- `record_successful_test` 只收下"有过一次成功测试执行"这个事实，**真去跑一次是 RP-11 的事**。
  这也是为什么"未测试不可发布"能在 RP-11 完成之前验收。
- `derive_private` 是**一次拷贝而不是引用**（`SKL-010`）：改私有副本不影响公共的。
- `mark_used_for_settlement` 留给 RP-15。

### Element: rust:xops-task#Task
- module: xops-task
- consumers: [RP-11, RP-12, RP-13, RP-15, RP-16]
- **平台只有这一种任务**（`TSK-001`）。质量监管、审批、CI 触发、代码走读四种常见用法
  在平台看来完全一样——**平台不认识这四个词**，所以这个类型里没有"审批任务"那种东西。
- `may_write` 是 `TSK-004` 的落点：**未声明的表写不了**。
- `responds_to_triggers` 是 `TSK-009` 的落点：**停用的任务不响应任何触发，包括手动**。

### Element: rust:xops-task#VersionPolicy
- module: xops-task
- consumers: [RP-11]
- **默认钉死一个版本**（`TSK-002`）。跟随最新必须是明确选择——
  技能作者一次发布会改变所有引用它的任务的行为。

### Element: rust:xops-task#Overlap
- module: xops-task
- consumers: [RP-11, RP-13]
- 三选一，**默认跳过**（`TSK-008`）。理由写在类型上：**定时任务最常见的故障是
  执行变慢后堆积成雪崩**。

### Element: rust:xops-task#OnComplete
- module: xops-task
- consumers: [RP-12, RP-16]
- 空 / 一个插件入口 / 另一个任务（`TSK-010`）。
- ⚠️ **深度硬限制 1**（`TSK-011`）：两个方向都挡——我挂的那个任务自己不能再挂，
  我自己被别人挂着的话我也不能挂。**一层是"输出后处理"，两层就是任务编排 DAG**，
  随之而来的是依赖解析、失败传播、循环检测及其可视化。

### Element: rust:xops-task#TerminationStep
- module: xops-task
- consumers: [RP-11, RP-12]
- **终止的时序是定死的**（`TSK-006`），超时与被取消走同一条路：
  ① 中止模型调用与会话 → ② 收敛并移交已产生的行 → ③ **先落 `_runs`，再写产出行** → ④ 销毁。
- 每一步都带着"为什么在这个位置"。③ 的理由最要紧：`FLW-026⑥` 要读 `_runs.status`
  才知道产出行算不算结算。
- ⚠️ **它不是跨表事务**（`CON-011`、D43）：两者之间崩溃是可接受的失败形态——
  `_runs` 行完整、产出行可能缺失；**反过来是不可接受的**，顺序就是为了排除它。
- **RP-12 实现写入路径时要照着这个顺序**，它不是建议。

### Element: rust:xops-task#Tasks
- module: xops-task
- consumers: [RP-11, RP-13]
- 建 / 改 / 启停 / 读 / 列 / 找订阅者 / 解出技能版本。
- **每一条校验都在创建时挡住，不留到运行时。**
- `resolve_skill_version` 转手问 `Skills::runnable_for`，因而 `SKL-009` 在任务这一侧也成立。
- 本次接上 `with_subscription_check`：订阅声明的合法性由 RP-11 判，
  **没接就等于不校验订阅**——那只在没有事件源的部署里成立。

### Element: rust:xops-task#DEFAULT_TOKEN_BUDGET
- module: xops-task
- consumers: [RP-11]
- 未声明时的单次 token 上限（`TSK-005`）。

### Element: rust:xops-dispatch#EventKind
- module: xops-dispatch
- consumers: [RP-13, RP-14, RP-15]
- **恰好五类，仅此五类**（`TRG-001`）：定时 · Git · 手动 · 流程节点被激活 · 上游任务完成。
- ⚠️ **白名单里永远不加「某张表被写入」**（`TRG-004`）：一旦任务能订阅表的变化，
  就有了不受深度限制的回路——A 写表触发 B，B 写表触发 A。
  **这个 enum 没有第六个变体，也不该有。** 解析失败的错误消息会点名这一条，
  因为它是最常被想要的那个。
- `self_subscribable`：**后两类不是任务能自己声明订阅的**（`TRG-003`）——
  「节点被激活」的唯一途径是被某个节点指定为写入者，「上游完成」的唯一途径是被挂在 onComplete 上。

### Element: rust:xops-dispatch#Whitelist
- module: xops-dispatch
- consumers: [xopsd]
- 订阅声明的校验，接在 `xops_task::Tasks` 上。**在创建任务时挡住**——
  留到运行时才发现，任务已经建出来了。

### Element: rust:xops-task#SubscriptionCheck
- module: xops-task
- consumers: [RP-11]
- 「这个订阅声明合不合法」的注入位。事件白名单归 RP-11，而拦截点在 RP-10 的创建路径上，
  所以留一个位——**`xops-task` 不认识事件类型，也不该认识**。

### Element: rust:xops-dispatch#Event
- module: xops-dispatch
- consumers: [RP-13, RP-14, RP-15]
- 一个事件。`revision` **覆盖任务定义里写死的那个**；`external_id` 是幂等的依据（`TRG-013`）。
- `Trigger::Schedule` 带 `configured_by`：`TRG-009` 要求定时触发**能追溯到配置该调度的人**。

### Element: rust:xops-dispatch#Dispatcher
- module: xops-dispatch
- consumers: [RP-13, RP-14, RP-15, xopsd]
- 三条共同纪律（`TRG-007`）各有落点：**非阻塞**（返回的是"进了队列"）·
  **幂等**（同一外部事件最多一次执行）· **留痕**（被拒绝的、被跳过的同样留痕）。
- ⚠️ **"被拒绝"不是 `Err`**，它是一条有痕迹的结果。返回 `Err` 的只有底层不可用。
- `trigger_history` 是 `TSK-016` 的落点：**一个静默被跳过的任务，会让人以为它在跑。**
- **RP-13 往这里接两类事件源，RP-14 往这里塞「节点被激活」**——它们加的是事件的来源，
  不是新的事件类型。

### Element: rust:xops-dispatch#WorkspaceSource
- module: xops-dispatch
- consumers: [RP-08, xopsd]
- 「按修订备一份只读工作区」的注入位。**分开是因为那条验收：不依赖 RP-08 也能跑通**——
  声明"不需要代码仓"的技能，全链路正常，而且**连问都不问一次**。

### Element: rust:xops-dispatch#assemble
- module: xops-dispatch
- consumers: [xopsd]
- 派工单装配。`TSK-015`：**只把技能实际声明的东西放进去，不扩权**。
- `provenance()` 是那张对照表：**派工单上每个字段对应哪条声明**。
  字段多了一个而对照表没加一行，测试就红——这就是"不扩权"的那份证明。
- ⚠️ **自定义出网白名单在 Q10 定下来之前不开放**（`TSK-017`）：技能声明了就**明确拒绝**，
  不是悄悄清空——悄悄清空会让技能作者以为它生效了。

### Element: rust:xops-dispatch#looks_like_credential
- module: xops-dispatch
- consumers: [各包的测试]
- 派工单里有没有凭据形状的值。**验收要求"检查完整派工单内容"**，所以这条判定要能被跑，
  而不是靠读代码。它宁可误报也不漏报，`.sock` 也在名单里——
  socket 等同于模型凭据本身。

### Element: rust:xops-task#Landing
- module: xops-task
- consumers: [xopsd, RP-15]
- 产出落地。**顺序是定死的**（`TSK-006` ③ / `CON-011`）：先落 `_runs`，再写产出行。
  理由不是洁癖——`FLW-026⑥` 要读 `_runs.status` 才知道产出行算不算结算。
- ⚠️ **它不是跨表事务**（D43）：两者之间崩溃是可接受的失败形态；**反过来不是**，
  顺序就是为了排除它。
- ⚠️ `_runs` 那一行署名 **`WrittenBy::Platform`** 而不是这次执行：系统表只有平台能写
  （`TBL-003`），"这次是谁跑的"由 `triggeredBy` / `task` / `skill` 几列回答，
  它们比一个 `writtenBy` 说得更全。**产出行那一侧才用执行的署名。**

### Element: rust:xops-task#Rejection
- module: xops-task
- consumers: [RP-15, RP-17]
- **两层拒绝必须分清**（`EXE-024`）：schema 不过 → **整批行不入表**，执行归为技能错误类；
  schema 过、节点判定不过 → **行入表**，只是不结算。
- `rows_landed()` 是这条区分在代码里的形态。

### Element: rust:xops-task#Notifier
- module: xops-task
- consumers: [RP-17]
- **两种拒绝都要通知**（`EXE-024`）——自动化失灵不能是静默的。RP-17 填这个位。

### Element: rust:xops-task#Retention
- module: xops-task
- consumers: [RP-11, RP-17]
- 输出默认 1 个月，过程记录默认 7 天。**过程用于排查，结论用于回看，价值衰减速度不同**（`RET-001`）。
- ⚠️ `RET-002`：`retainUntil` **取写入当时的配置，不靠回查任务再取**——
  任务的保留期可以随时改，而已经写下的行不应该因为任务改了配置就提前消失或延后清理。

### Element: rust:xops-task#Exemption
- module: xops-task
- consumers: [RP-14, RP-15, RP-17]
- 豁免清单四项（`RET-006`），**豁免优先于任务保留期**（`RET-007`）：
  任务完全可以往主体表写行，那批行两条规则都命中，必须有优先级。
- 第一项最要紧：**一个还在进行中的流程实例，它的主体行或结算行被清理了就等于实例被腰斩**（`I-X`）。

### Element: rust:xops-task#Cleanup
- module: xops-task
- consumers: [xopsd]
- **全系统唯一一处硬删除**（`RET-010`）。它与 `I-D` 不冲突——
  **不可变说的是"不会被改写"，不是"永久保留"**，且删除这件事本身留痕。
- ⚠️ **没有"删除某一行"的入口**：唯一的公开方法只接受一个时刻（`RET-005`）。
  有测试数这个 crate 里 `pub fn` 的个数。
- 过程记录到期**只清 `trace` 这一列**，行本身按输出保留期走（`RET-004`）。

### Element: rust:xops-task#Concurrency
- module: xops-task
- consumers: [RP-11, xopsd]
- 并发上限，**平台与项目两级**（`EXE-027`）。两级都要有：只有平台级，一个项目就能把
  名额吃光；只有项目级，项目一多平台还是会垮。
- `Permit` **析构即归还**——忘了归还是这类代码最常见的漏。

### Element: rust:xops-table#Tables::describe_internal
- module: xops-table
- consumers: [RP-12]
- 查表结构，**不判权**。给平台自己的写入路径用——那条路上没有"调用者"这个概念，
  写入者是一次执行，不是一个人。

### Element: rust:xops-dispatch#Schedule
- module: xops-dispatch
- consumers: [xopsd]
- 两类表达就够了（`TRG-009`）：**每天某时** 与 **每隔 N 小时**，都带明确的时区——
  「每天 02:00」不说时区等于没说。
- `configured_by`：**触发者记为系统，但必须能追溯到配置该调度的人**。
- `missed_windows` 数得出错过了几个窗口。**它们不补跑**（`TRG-010`）：
  补跑会在恢复瞬间产生一批并发执行，风险大于收益。**但要留痕**——
  静默跳过与"它本来就没到点"在外面看起来一模一样。

### Element: rust:xops-dispatch#Schedules
- module: xops-dispatch
- consumers: [xopsd]
- 调度表。`due()` 顺带把错过的窗口逐个留痕。

### Element: rust:xops-dispatch#GitEvent
- module: xops-dispatch
- consumers: [xops-web, xopsd]
- 从 webhook 载荷里提取出来的**只有四样**：确切提交标识 · 分支 · 事件类型 · 投递标识。
- ⚠️ `TRG-015`：载荷是**不可信输入**（G7）。`extract` **只读那几个已知的键**——
  提交信息、PR 描述里的任何自由文本都不进它的输出，因而也进不了任何控制流。
  有测试往载荷里塞攻击性文本，验证它一个字都没出来。
- **缺了必需字段就报错，不猜、不兜底**：猜错一次，执行读的就是错的那一版代码。
- `revision` **覆盖任务定义里写死的目标修订**（`TRG-017`）。

### Element: rust:xops-dispatch#Filter
- module: xops-dispatch
- consumers: [xopsd]
- 按分支与事件类型过滤（`TRG-015`）。

### Element: rust:xops-web#WebhookSink
- module: xops-web
- consumers: [xopsd]
- Git webhook 的落点。**它被放在 web 这一侧而不是直接依赖 dispatch**，
  是因为 `TRG-014`：端点内不做任何拉取或执行，这个 trait 的实现要在毫秒内返回——
  平台的 webhook 都有超时并会因超时重投，从而放大问题。
- `rejection()` 是验签失败时**唯一**该返回的错误。**端点回的东西必须与"没接落点"
  一模一样**（`TRG-012`）——否则它就成了探测器。有测试逐字节比这两种响应。

### Element: rust:xops-flow#Definition
- module: xops-flow
- consumers: [RP-15, RP-16, RP-18]
- **新增 `status_columns`**：主体表上哪些列是状态列（`FLW-036`）。
- 这是 RP-18 撞出来的一处缺口，按包文档说的办法补的：**缺的回头补那个包，
  不在本包里补**。`FLW-036` 原文写的就是"**流程可以声明**主体表上的哪些列是状态列"，
  而 `Definition` 里一直没有装它的地方——于是 `protection::check` 的 `status_columns`
  参数没有来源，`I-P` 的后半句落不了地。
- **状态列是流程声明的，不是表声明的**：同一张表在不同流程里可以有不同的状态列。
- 校验：没有主体表就没有状态列可声明。

### Element: rust:xops-flow#Criteria
- module: xops-flow
- consumers: [RP-15]
- 一组筛选。`provably_disjoint` 是**保守口径**的落点：
  目前能证明互斥的只有一种——**同一列被约束成两个不同的字面值**，别的一律证不出来。

### Element: rust:xops-flow#Writers
- module: xops-flow
- consumers: [RP-15]
- 允许写入者是三者的并集（`FLW-018`）：项目角色 · 名单表 · **指定的私有任务**。
- ⚠️ **名单表的写权限就是审批权的元权限**（`FLW-019`）：谁能改名单，谁就能给自己发审批权。
- ⚠️ **③ 只能是私有任务**（`FLW-021`）：公共任务没有"所有者这个人"，
  一旦它写的行被算作节点通过，"每一次通过都归属一个具名的人"当场落空。

### Element: rust:xops-flow#RowQuery
- module: xops-flow
- consumers: [RP-15, RP-16]
- 求值时要预取的一批行。**流转插件读不到表**（`PLG-002`），所以它要用的行必须在
  流程定义里声明出来。**这不是限制，是把一件本来就该做的事挑明了**——
  求值发生在写串行区间内，一次自由查询就是一次不确定的写时延。

### Element: rust:xops-flow#validate
- module: xops-flow
- consumers: [RP-15, RP-18]
- **不落库**（`FLW-008`）。一次返回**全部**问题，不是第一个——一次改完比来回三次强。
- ③ 的判定：**同一集合内两两**（同时激活，一行落进来会被多个节点同时求值）
  与**相邻两个集合之间**（会在前一个通过的瞬间被同一行结算）。隔了一步的不判。
- ⚠️ **宁可误拒**：有测试专门构造"看起来不重叠但证不出互斥"的用例，
  验证它误拒而不是误放——误放的后果是运行时一行同时结算两个节点，而那是事后查不出来的。

### Element: rust:xops-flow#Instance
- module: xops-flow
- consumers: [RP-15, RP-17]
- ⚠️ **没有 `currentNode`**：当前激活的节点可能有多个（并行组），
  权威是 `active()` 那些行（`TBL-007`）。**"卡在哪"问的就是它。**
- `reject` / `cancel` / `expire` 都会把剩下的节点转成 **`Void`（已作废）**，
  **不停在"未激活"**——停在那儿会让人以为它还会被激活。

### Element: rust:xops-flow#NodeState
- module: xops-flow
- consumers: [RP-15, RP-17]
- 五态：未激活 · 激活中 · 已通过 · 已拒绝 · **已作废**。

### Element: rust:xops-flow#Flows
- module: xops-flow
- consumers: [RP-15, RP-04, RP-18]
- 定义 / 校验 / 停用 / 发起 / 推进 / 取消 / 过期 / 查状态 / 跨项目待办。
- ⚠️ **`advance` 是 RP-15 唯一该用的推进入口**——它**不得自己去改
  `_flows` / `_flow_nodes`**（这条分工是那一刀能成立的全部前提）。
- `referencing` 给 RP-04 的删表判定用（`TBL-026`：被引用的表不能删）。
- `FLW-007`：实例存的是**发起时的版本**，之后发布新版本不影响它。

### Element: rust:xops-flow#NodeActivated
- module: xops-flow
- consumers: [RP-11, RP-15]
- 「节点被激活」的载荷（`TRG-018`）：实例 · 流程与版本 · 哪个节点 · 发起者 ·
  主体标识与修订。

### Element: rust:xops-settle#Rule
- module: xops-settle
- consumers: [RP-16, RP-17]
- **七条判定，缺一不可**（`FLW-026`）。每一条带着 `why()`——**它挡的是什么**。
  那几句话是这个类型存在的理由：删掉之后这七条就成了七个看不出所以然的 if。
- ② 之所以在**写入这一刻**判：名单表可以随时改（`FLW-029`）。
- ③ 挡的是**闭环自批**；写入者是任务时比**任务所有者**——任务不是责任主体，人才是（`I-O`）。
- ⑥⑦ 挡的是**产出异常**：超时后的残片、读的是 HEAD 而不是这次要它看的那一版。

### Element: rust:xops-settle#Verdict
- module: xops-settle
- consumers: [RP-16]
- 三个结论。**「不结算」不是「拒绝」**：不结算时**行照常留在表里**（它是一条正常数据），
  只是不算数（`FLW-027`）；拒绝则让整个实例立即进入终态。

### Element: rust:xops-settle#WriterCheck
- module: xops-settle
- consumers: [xopsd]
- 允许写入者三者并集的判定。**名单每次现查**——它可以随时改。

### Element: rust:xops-settle#responsible
- module: xops-settle
- consumers: [RP-17]
- 从 `writtenBy` 归出**那个人**。插件与平台写的行不是"谁的表态"，返回 `None`。

### Element: rust:xops-settle#Origin
- module: xops-settle
- consumers: [RP-04, RP-16]
- 谁在写。`may_write_status` / `may_write_instance` 两个判定，各挡一件事：
  - `_instance`：**技能与用户都不能自己写**（`I-P`）。没有它，两个并发实例会在同一张表上
    产生两条同样满足筛选的行，节点判定无从区分——**这是整个流程模型的地基**。
  - 状态列：**只有平台与流转插件能写**（`FLW-036`）。不这么做，任何成员都能直接
    `update status = closed` 绕过整条流程——**七条判定只管"这行算不算结算"，从不阻止写入**。

### Element: rust:xops-settle#Evaluator
- module: xops-settle
- consumers: [xopsd, RP-16]
- 求值链（`FLW-033`/`FLW-034`）。**整段发生在这张表的写入串行区间内**——
  除了最后的"触发任务"那一步，它在锁外入队。
- ⚠️ **`apply` 经 RP-14 的状态机接口驱动迁移**，本包**不碰 `_flows` / `_flow_nodes`**。
  有测试扫源码来守这条——**它是那一刀能成立的全部前提**。
- 指定了流转插件时，① 里的"满足筛选"由插件替代，**②～⑦ 一条不减，且先判完**——
  不满足的行根本不会被交给插件（`FLW-028`）。

### Element: rust:xops-script#Capabilities
- module: xops-script
- consumers: [RP-15, RP-10, RP-18]
- 一份能力声明。**能力默认为零，未声明即没有**（`I-Z`）——**流转插件没有可声明项**，
  输出插件只能声明三样：出网白名单 · 读自己的配置 · 读声明过的表。
- `check(position)` 挡两件：流转插件声明了任何东西 · 声明读 `_notices`（`NTF-012`）。
- `disclose()` 是安装时那份**逐条披露**的原文（`PLG-007`）。它同时是 `install` 的入参，
  **所以"不看披露直接装"在接口上不可表达**，不是靠人自觉。
- `allows_host` 由 `net::fetch` **在重定向循环里面**调用。把它挪到循环外面，
  白名单就只对第一跳成立。

### Element: rust:xops-script#Position
- module: xops-script
- consumers: [RP-15, RP-10]
- 两个调用位置，**不存在第三个**（`PLG-001`）。

### Element: rust:xops-script#Grant
- module: xops-script
- consumers: [RP-15, RP-10]
- 这次调用给了什么：一份能力声明 + 一个可选的宿主。**没有宿主等于三样都给不了。**
- 载体里每一处绑定注入都挂在 `if capabilities.…` 后面，
  有一条枚举源码的测试盯着这件事。

### Element: rust:xops-script#Host
- module: xops-script
- consumers: [RP-10, RP-18]
- 宿主这一侧的三个方法，对应输出插件能声明的三样。**没声明的那一样根本不会被调到**
  ——不是它返回错误，是 JS 那一侧没有对应的函数。

### Element: rust:xops-script#Net
- module: xops-script
- consumers: [xopsd]
- 谁去真的发包。**XOps 不实现它**——这是一个接缝，不是一个 HTTP 客户端
  （`PLG-004`：平台不提供"发消息"这种能力，也不定义"通道"这个概念）。
- 没接后端就传 `Denied`：声明了出网的插件也发不出去，**而这件事在部署层面是看得见的**。

### Element: rust:xops-script#invoke
- module: xops-script
- consumers: [RP-15, RP-10]
- 跑一次插件。**每次新建一个 `Runtime`，调用结束整个扔掉**——"调用之间不共享任何状态"
  是这样兑现的，不是靠清理全局对象。
- **插件自己的失败不是 `Err`**：超时与异常是 `Outcome::TimedOut` / `Outcome::Threw`，
  因为那是一次正常的求值结果。
- 死循环由**字节码级中断**兜住（`PLG-013`）：表现是"这次求值超时"，
  **不是"一个线程转死了"**。

### Element: rust:xops-script#compile_check
- module: xops-script
- consumers: [RP-16]
- `PLG-006` 的"编译"= QuickJS 编译 + **入口导出检查**（D54）。

### Element: rust:xops-script#generate
- module: xops-script
- consumers: [RP-10, RP-18]
- 生成流水线：编译 · **在真载体里按声明的能力跑用例** · 静态检查。
  三样全过才产出一个候选（`I-K`）。
- 静态检查只剩两件（`PLG-017`）：入口在不在 · 声明与位置配不配。
  **够不到的东西不需要禁**——载体不给绑定，脚本就没有那条路。

### Element: rust:xops-script#Plugin
- module: xops-script
- consumers: [RP-15, RP-10, RP-18]
- 一个插件的一个版本。**已安装的版本不可变，能力声明是版本的一部分**（`PLG-009`）：
  改就是生成一个新候选再装一次，**改能力也是**。
- 源码、能力声明、用例、结果、安装人**全部内联**——`RET-009` 的已知悬空要求它自包含。

### Element: rust:xops-script#Plugins
- module: xops-script
- consumers: [RP-15, RP-10, RP-17, RP-18]
- 插件的读写面与安装治理。三档权限照 `PLG-008` 分。
- `install` 要调用方把披露原文**逐条**交回来，对不上就装不上。
- `host_for` 是"配置只注入给该插件、且只在它声明了这项能力时"的那处兑现——
  **没声明就连读都不读一次**。

### Element: rust:xops-script#evaluate_transition
- module: xops-script
- consumers: [RP-15]
- 流转插件求值的入口。三样输入由平台在调用前查好（`PLG-002`）。
- **超时与异常都归到"这个节点没过"，绝不视为通过**（`PLG-013`）。
- 交回的行**只肯代写结算表与主体表两张**，且**对主体表只能 update**——
  insert 等于让插件自己开出新实例（`I-R`、`CON-003`）。
  **本包不写，只把不该代写的挡在交出去之前。**

### Element: rust:xops-script#run_output
- module: xops-script
- consumers: [RP-10]
- 输出插件的入口，接在 onComplete 上。
- 它的返回值里**没有任何写表的路径**——"输出插件写不了任何表"（`I-R`）在这里是
  **类型上的**，不是检查出来的。

### Element: rust:xops-script#triggers_evaluation
- module: xops-script
- consumers: [RP-15]
- **交回、由平台代写的行不再触发插件求值**——自激回路从这里断掉（`PLG-013`、`I-R`）。

### Element: rust:xops-notice#SourceEvent
- module: xops-notice
- consumers: [RP-10, RP-12, RP-15, RP-19]
- 派生通知的那几个事件。**每一个变体都对应一个已经发生、已经留痕的事实**——
  想加一类通知，先要有一个已存在的事件，反过来不行（`NTF-002`）。
- `RunFinished.after_failure` 接住 `TSK-012`：**`_runs` 那一行已经写好了**，
  后处理失败只留自己的痕迹并通知任务所有者，不改执行本身的结论。

### Element: rust:xops-notice#Kind
- module: xops-notice
- consumers: [RP-05, RP-06]
- 值得通知的**五类**（`NTF-007`）。其中"我写的行未被采纳"是
  **自动化失灵时唯一的信号**——没有它，一个写了行却没被采纳的人不会知道自己白写了。

### Element: rust:xops-notice#Notice
- module: xops-notice
- consumers: [RP-05, RP-06]
- 一条通知。**没有公开的构造函数**——唯一造得出它的地方是 `from_event`，
  `NTF-002` 那句"不引入独立的产生路径"在这里是**可见性上的**，不是一句约定。

### Element: rust:xops-notice#from_event
- module: xops-notice
- consumers: [RP-10, RP-12, RP-15, RP-19]
- 从一个事件派生出该发的那些通知。**内容由确定性代码生成，不经模型**（`NTF-003`、`G8`）：
  本 crate 连执行域都不依赖，**没有一条能调到模型的边**。
- 自由文本**原样引用或截断**，不改写、不摘要、不翻译（`NTF-004`、`G7`）。
- **不含凭据、令牌或产物原文，只含指针**（`NTF-006`）——落法是结构上的：
  `SourceEvent` 里没有装它们的字段。

### Element: rust:xops-notice#Notices
- module: xops-notice
- consumers: [RP-10, RP-12, RP-15, RP-19, xopsd]
- 通知的写入与读取。
- ⚠️ **`notify` 的返回类型里没有 `Result`**——它交回一组 `Failure`。
  这是 `NTF-008` 的落法：调用方**拿不到一个能用 `?` 把业务写带崩的东西**，
  于是"通知的失败绝不影响产生该事件的业务操作"成为**结构保证**，
  而不是"几乎不会失败"这种概率话术（`I-W`）。
- `unread` 没有"看别人的"那个参数；`mark_read` 写下去的 patch 里**只有 `readAt`**。

### Element: rust:xops-notice#Retention
- module: xops-notice
- consumers: [xopsd]
- `_notices` 自己的保留期：**平台级配置，默认 3 个月**，与任务无关（`RET-008`）。
  清理**整批按时间进行**（`RET-005`）。

### Element: rust:xops-template#Template
- module: xops-template
- consumers: [RP-19]
- 一个模板：**一套表 schema + 可选的流程定义 + 可选的插件**（`TPL-001`）。
- ⚠️ **它是可序列化的声明式表示，不是硬编码在 Rust 里的结构体图。**
  理由是 **Q15**（用户自定义模板的导出与提交，M6）：首版三个模板随平台发行，
  但**表示形式要为导出留出可能**——硬编码的那种将来导不出来。
  有一条 JSON 往返的测试盯着。

### Element: rust:xops-template#Templates
- module: xops-template
- consumers: [xopsd, RP-19]
- 列出 · 查看 · **在本项目实例化（建表、建流程、装插件一步完成）**。
- **"中途失败不留下半套东西"靠预检，不靠事务**：存储契约只有基本增删改查
  （`CON-012`），跨表事务这个东西不存在，所以这里不假装有——
  ① 名字全部先查一遍，撞了就在动手之前失败；② 建表 → 装插件 → 建流程；
  ③ 出错就把这次已经建出来的表软删掉。**③ 是尽力而为的**，撤不掉的会写在错误消息里。
- 实例化整体上要**维护者及以上**：里面有一步是装插件（`PLG-008`）。
  **不为模板开一条更松的路**——那等于绕过 `I-K`。

### Element: rust:xops-template#catalog
- module: xops-template
- consumers: [RP-19]
- 平台自带三个：**bugs · issues · approvals**（`TPL-003`、`TPL-008`）。
- ⚠️ **approvals 的结算表列名直接决定 RP-19 那边的结果列映射**能不能拼出
  `poll_approval` 的返回值——两包要一起看。

### Element: rust:xops-xforge#Registration
- module: xops-xforge
- consumers: [xopsd]
- `policyId → 流程 + 结果列映射`（`XFG-003`）。**没有结果列映射，适配层拼不出
  `poll_approval` 的返回值。**
- **它没有自己的表**：序列化之后放进 `Binding.xforge`——`RPO-014` 早就留好了那个位子。
- `XOPS_ROLES` 是 XOps 会返回的角色名，**恰好三个**（`XFG-019`）。
  ⚠️ **不要为了迁就 XForge 侧的某条 policy 把 XOps 改成可配置角色系统**——
  两条出路是"约定只用这三个名字"或"日后在绑定上加一张三五行的映射表"。

### Element: rust:xops-xforge#XForge
- module: xops-xforge
- consumers: [xopsd]
- 两个 tool 的处理与登记的读写。
- ⚠️ **这里一处降级都没有**（`XFG-020`）：查不到项目、查不到登记、查不到流程，
  一律**明确失败**——让调用方看到一个能重试的错误，
  而不是一个看起来成功的空结果。**"连不上就跳过"会让变更被静默放行。**
- `submit` 按 `governingDigest` 幂等；`poll` 立即返回，三种状态。

### Element: rust:xops-xforge#resolve
- module: xops-xforge
- consumers: [xopsd]
- 从结算行的 `writtenBy` 解析 approver（`XFG-004`）：**人 → 就是他；执行 →
  那个私有任务的所有者；插件求值 → 安装该插件的维护者**；平台写的行没有负责人。
- 三种都**不需要回查别的表**——`WrittenBy` 把该内联的都内联了（`TBL-016`）。
  那些字段存在的理由正是"一个月后 `_runs` 那行没了，还要回答得出这一票是谁的"。

### Element: rust:xops-xforge#scaffold
- module: xops-xforge
- consumers: [xopsd, 运维]
- XForge 侧配套四样的模板，以及**检出 ③④ 缺失**的检查（`XFG-021`、`XFG-022`）。
- ⚠️ **③④ 缺了会静默失效**：`xforge doctor` 对未被引用的扩展资源只警告、从不阻塞，
  于是 provider 装好了、连得上、却没有任何一条 Flow 引用它——
  **这道审批门等于不存在，而一切看起来都正常**。
- ⚠️ 检查的口径是"这几个名字在不在文本里"，**不是一次 YAML 解析**：
  它证明得了**缺失**（`XFG-022` 要的就是这个），证明不了"结构完全正确"。

### Element: rust:xops-flow#Flows::find_by_subject
- module: xops-flow
- consumers: [RP-19]
- 按主体找实例。**`XFG-011` 的幂等靠它**：同一个 `governingDigest` 重复提交，
  **不得开出第二个实例**。

### Element: rust:xops-repo#Repos::set_xforge
- module: xops-repo
- consumers: [RP-19]
- 写下 XForge 登记。**挂在仓绑定上，不另开一套对象**（`RPO-014`）。
  没绑仓时**明确失败，绝不静默创建**（`XFG-002`）。

### Element: rust:xops-mcp#ToolName
- module: xops-mcp
- consumers: [全部 tool]
- tool 名的形状：`<域>.<动作>`，小写字母开头，只含小写字母、数字与连字符。
- **`EXTERNAL_NAMES` 是一张写死的白名单**，放行两个**由外部规格定死**的 tool 名
  （`submit_approval_request` · `poll_approval`）。它们带下划线、不合上面那条形状——
  **`XFG-010` 写得很直白：XOps 没有任何设计自由度，只有实现义务。**
- ⚠️ **它是白名单，不是开关。** 它挡住的正是"以后再往里加一个"：
  加一条要有一份同样级别的外部规格，**而那件事在代码审查里看得见**。
  别的名字规则一个字没改：`submit_anything` 照拒。

### Element: rust:xopsd#Config
- module: xopsd
- consumers: [部署]
- 从环境变量读的一份启动配置。**没有配置文件格式**——单实例部署，环境变量够了。
- ⚠️ **加密密钥没有默认值**：空的时候装配拒绝起来。
  **一个写死的默认密钥看起来是加密的，实际不是**——那比没有密钥更糟。

### Element: rust:xopsd#assemble
- module: xopsd
- consumers: [部署]
- 把 19 个 crate 接成一个进程。**这里没有业务判断，一句都没有**：
  建对象 · 按依赖顺序接起来 · 把两个服务面交出去。
  任何一处"顺手在这里判一下"都是把语义搬出它该在的包。
- 两个服务面**各监听各的端口、共用同一份状态**（`I-L`、`G2`）。分开不是为了好看：
  `xops-web` 里结构性地不存在写业务对象的路由，**而那件事只有在两个面分开时才成立**。
- 装配层挂的 `ProjectHook` 负责在建项目之后把那四张系统表建起来（`TBL-005`）。

### Element: rust:xopsd#Assembled
- module: xopsd
- consumers: [部署]
- 交出去的两个服务面，外加**两处不能悄悄发生的降级**：
  `engine_kind`（引擎是真的还是桩）与 `unsatisfied`（裸跑没兑现的八条，`D58`、`EXE-029`）。
- ⚠️ 它们不是给日志看的，是**启动横幅必须说出来**的东西：
  "以为接了真引擎、其实跑的是桩"是一种查起来很慢的错。

### Element: rust:xops-repo#Sealer::from_hex
- module: xops-repo
- consumers: [xopsd]
- 从十六进制文本取密钥。**密钥从哪来是装配的事，不是这个类型的事**——
  读环境变量那一步挪到进程边界上做，测试才好构造。`from_env` 现在走它。
