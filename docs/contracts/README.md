# 契约基线

> 状态：**骨架已立，正文为空**。三份基线现在都是 `(none)`，这是正确状态——
> 元素由各需求包在自己的变更里逐条添加，见 §6。
>
> 上游：[概念与架构](../concepts-and-architecture.md)（`D57`）· [需求包总览](../requirements/README.md)

**这里记的是接口在上一次被批准时长什么样。** 实现要引用它，不要复述它；
实现与它不一致时，改实现——改契约是一个需要具名人拍板的决定（§4）。

---

## 1. 三份契约面

| 面 | 基线记录 | 方言文件（实现侧） | 元素前缀 |
|---|---|---|---|
| **对外接口** | [`api.md`](api.md) | `api/mcp/*.json`（JSON Schema 2020-12）<br>`api/read-model.openapi.yaml`（OpenAPI 3.1） | `api:mcp.*` `api:http.*` |
| **crate 接缝** | [`rust.md`](rust.md) | `rust/<crate>.api.txt`（`cargo-public-api` 快照） | `rust:*` |
| **数据库** | [`data.md`](data.md) | `data/schema.sql` + `data/meta-schema.md` | `sql:*` |

**MCP 与只读 HTTP 合成一份，是刻意的。** 两者跑在同一个 `xopsd` 进程、同一层 HTTP 上，
但更实在的理由是 `I-L` 与 `G2`——"会话凭据与 MCP 令牌互不通用"、"后端不存在写路由"
这两条不是任何单一面的性质，**是两面并排放才看得出来的性质**。切成两份，
`RP-05` 那条"枚举后端路由，证明不存在写路由"的验收就没有任何一份文档能承载。

**但方言仍然是两种，不要试图统一。** MCP 的线上格式不是 XOps 的财产（`MCP-001`：
标准 MCP server，任何符合 MCP 的客户端都能接入），用 OpenAPI 描述 JSON-RPC 信封
等于把别人的协议重抄一遍；而 OpenAPI 的寻址单位是 path + method，MCP 的是 tool 名，
全部 tool 走同一个 `POST /mcp`，硬塞进去就是一个 operation 加一个跨全部 tool 的巨型 `oneOf`。
**合的是文档与地址，不是格式。**

## 2. 元素 id（CEID）

```
CEID := <kind> ":" <selector>

<kind>      小写 kebab，须匹配 ^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$
<selector>  不透明字符串，**不含任何空白**，长度 ≤ 512
```

⚠️ **selector 不含空白是硬约束**，不是风格偏好：id 要能同时当 markdown 标题、
台账引用和报告里的一行 token 用，任何一处需要加引号，整套东西就不成立。
所以路由不能写成 `api:http.GET /boards`，要写成路径在前、方法在后：

```
api:mcp.tool.table.create                        建表 tool
api:mcp.tool.table.add-column                    加列 tool
api:mcp.dispatch.table-tools.insert              表专属 insert tool 的**派发规则与固定信封**
api:mcp.error.not-found-or-forbidden             统一错误契约的一条（MCP-007 / MCP-008）
api:http.paths./projects/{projectId}/boards.get  只读 HTTP 的一条路由
api:http.paths./webhooks/git.post                非 MCP 入口（TRG-011）

rust:xops-store#Store::put                       存储契约的一个方法
rust:xops-exec#ExecContract::submit              执行契约的一个方法
rust:xops-flow#Instance::settle                  状态机接口的一个方法

sql:table._runs                                  一张系统表
sql:table._runs.column.tokensUsed                系统表的一列
sql:meta.column-type.derived-text                用户表可用的一种列类型
sql:meta.auto-column.writtenBy                   平台自动补的一个列位（TBL-014）
```

### 治理生成器，不治理实例

XOps 有两处接口是运行时生成的，**基线只记生成规则，不记它生成出来的东西**：

| 运行时生成的 | 基线里记什么 | 基线里**不**记什么 |
|---|---|---|
| 表专属读写 tool（`MCP-005`） | `api:mcp.dispatch.table-tools.*`：派发规则、固定信封、schema 由列类型生成的映射 | `bugs_insert` 这类逐表实例 |
| 用户表（`TBL-001`） | `sql:meta.column-type.*`、`sql:meta.auto-column.*`、物理映射规则 | 用户建的任何一张表 |

逐条登记实例，基线会被用户建表撑爆，而且**它们本来就不是被批准的东西**——
被批准的是"用户能造出什么形状"。

## 3. 两根模型

```
基线   docs/contracts/*.md       上一次被批准的接口记录。变更进行中无人写它
实现   docs/contracts/{api,rust,data}/**  服务今天真正提供的接口
        由 `node scripts/contracts.mjs dump` 自证

check = diff(基线, 实现) 减去本次 delta 已声明的元素；非空即红
```

**两面已经接上了**（2026-09-05）：`api:mcp.tool.*` 与 `api:http.paths.*`，
来源是 `xopsd --dump-contracts`——**问的是装配好的进程，不是源码**。

⚠️ **为什么必须问进程。** 扫源码只看得见"写下来的"，而这个仓踩过的坑里有一整类是
"**写下来了但没接上**"：`BuiltinProvider` 好好地待在 `xops-identity` 里，
装配层从来没调过 `with_provider`，于是 Web 上一个人都登不进来、日志里一个字都没有。
**源码扫描对那一类是瞎的。**

⚠️ **它第一次跑就撞出 29 处漂移**：基线登记了 71 个 tool 里的 48 个，
漏的那 23 个里有整条技能生命周期与整条任务生命周期。在那之前，
`check` 校验的只有记录格式、delta 结构与台账——**没有任何东西证明代码长得跟基线一样**。

还欠两面：`sql:*`（sqlite schema dump）与 `rust:*`（cargo-public-api 快照）。
**判定规则一个字不用改**，补的只是"实现"那一根的两个来源。

⚠️ **有未合并的 delta 时不比对。** 那时实现比基线多出几条是正常状态，
报出来只会训练人忽略它——**一个经常误报的检查等于没有检查**。

## 4. 改一条接口要做什么

```
① 在 docs/contracts/deltas/<变更名>/<面>.md 写一份 delta，五节齐全，无变化写 (none)
② 破坏性变更额外在 DECISIONS.yaml 记一条，四个字段齐全，decidedBy 写真实身份
③ 改实现
④ node scripts/contracts.mjs dump       问二进制"你实际提供什么"（要先 cargo build -p xopsd）
⑤ node scripts/contracts.mjs check      声明与基线对不对得上，**并比对基线与实现**
⑥ node scripts/contracts.mjs sync       合进基线、删掉已合并的 delta
⑦ 提交 —— 实现与基线的变化在同一次提交里
```

**这一轮之后留在历史里的是什么：**

```text
基线的 git diff        这次动了哪些元素 —— 它就是最终的那份 delta
DECISIONS.yaml         破坏性变更的理由与拍板人（sync 不吃它，它是持久的）
提交信息               Consumer Impact 那一节的内容写这里
deltas/ 目录           空的。delta 是工作中的草稿，合进基线就没有它的事了
```

**delta 的五节与 `(none)`：**

```markdown
## ADDED Contract Elements

### Element: api:mcp.tool.table.create
- module: xops-mcp
- consumers: [agent]

## MODIFIED Contract Elements

(none)

## REMOVED Contract Elements

(none)

## Breaking Changes

(none)

## Consumer Impact

(none)
```

`(none)` 是一条断言而不是省略——"这一节我看过，确实没有"。空着不写，检查会红。

**破坏性变更**在 `## Breaking Changes` 一节标 `**BREAKING**`，并在其后 8 行内给出一行
决策引用，形如 `decision: DECISIONS.yaml#cbc-runs-trace-column`。台账里那条必须有
`question` / `decision` /
`decidedBy` / `decidedAt` 四个字段——**字段名是硬编码的，没有别名**。

## 5. 忘了跑 sync 会怎样

**基线不会自己前进**，而它前进不了的后果是延迟出现的：

```
这次正确声明了 3 个元素，实现也改了，但没跑 sync —— 基线还是空的
下次再动同一条元素，ADD 会成功（基线里确实没有它）
于是同一条元素被"首次登记"了两次，而两次的正文可能已经不一样了
```

所以 **check 会在发现有未合并的 delta 时提醒你**，提交前跑一次 sync 就不会漏。

⚠️ **这套东西现在没有服务端强制。** 没有 CI、没有 required review——拦住"改了接口但没说"
的是 `check` 与写它的人，不是一道门。**这是刻意的**：仓里一行实现都没有，
先把地址空间与记账方式定下来，强制层等 XForge 契约治理接进来时由它的 Gate 提供（§8）。
在那之前，唯一要守的纪律只有一条：**改 `docs/contracts/*.md` 只能靠 sync，不要手改基线。**

## 6. 现在不写正文

三份基线现在是空的，这是对的。各面的定型点已经是各需求包**自己的第一件工作包**：

| 定型什么 | 落在哪 |
|---|---|
| 存储契约（`rust:xops-store#*`） | RP-01 全包 |
| 执行契约（`rust:xops-exec#*`） | RP-07 WP-A |
| 读模型（`api:http.*`） | RP-05 WP-A |
| 状态机接口（`rust:xops-flow#*`） | RP-14 WP-C |
| 物理 schema（`sql:table.*`） | RP-01 |
| 元 schema（`sql:meta.*`） | RP-04 |
| MCP 注册骨架与错误契约（`api:mcp.*`） | RP-03 |

**契约不是新增的一道前置工序，它是这些既有 WP-A 产出的落点。** 现在把三份写全，
等于在没有实现反馈的情况下把 19 个包的接缝一次定死。

## 7. 台账里的具名身份

```
DECISIONS.yaml 的 decidedBy    写 xbitsmaster —— Git author，不是别的什么名字
```

将来接 XForge 时它要与 `KnownIdentities` 的比对集合对得上，而那个集合是从提交历史建的。
**一开始就写真实身份**：全新的记录里随便写个名字当场不会被拒，等历史比对集合建起来之后
同样的内容立刻被拒——那时再改，改的是一条已经被引用过的决策。

## 8. 接 XForge 契约治理时会发生什么

`scripts/contracts.mjs` 的解析与合并语义是照着 XForge 的 `core/contract-delta.ts` 与
`core/contract-merger.ts` 写的，**为的是那一天是搬家不是重写**：

| 现在 | 接了之后 |
|---|---|
| `docs/contracts/*.md` | 原样搬进 `xforge/contracts/` |
| `docs/contracts/deltas/<变更>/*.md` | 原样搬进 `xforge/changes/<id>/contracts/` |
| `DECISIONS.yaml` | 转成 `evidence/conditions/contractDecisions.yaml`，字段同名 |
| `scripts/contracts.mjs` → `cargo xtask contracts` | 成为 `contract-compat` / `contract-drift` 的 Gate 命令 |
| 提交前手动跑 `sync` | 换成 archive 时自动合并 |
| **没有强制层** | Rule `interfaces-are-contract-governed` + 四道 Gate 接管拒绝 |

只有后两行会真的被换掉，前四行是搬家。

## 9. 目录

```text
docs/contracts/
├── README.md            本文
├── api.md               对外接口基线（MCP + 只读 HTTP）
├── rust.md              crate 接缝基线
├── data.md              数据库基线
├── DECISIONS.yaml       破坏性变更的具名人拍板台账
├── deltas/              进行中的变更各自的 delta，合并后由 sync 删除
│   └── <变更名>/{api,rust,data}.md
├── api/                 【有实现后】方言文件：JSON Schema 与 OpenAPI
├── rust/                【有实现后】cargo-public-api 快照
└── data/                【有实现后】schema.sql 与元 schema
```
