# 对外接口

## Purpose

XOps 对外的两个接口面，合成一份记录：**MCP 写入面**（面向 agent，`MCP-001`～`MCP-015`）
与**只读 HTTP**（面向 Web 前端，`BRD-*`）。合并的理由与"方言不能一起合"的理由见
[README §1](README.md#1-三份契约面)。

**这份记录同时是 `G2`（Web 只读）与 `I-L`（会话与令牌互不通用）的证明位**：
写操作只能出现在 `api:mcp.*` 里，`api:http.*` 里一条都不该有。审这份文件时先数这个。

## 谁往这里加元素

| 前缀 | 归属 |
|---|---|
| `api:mcp.tool.identity.*` `api:mcp.registry.*` `api:mcp.error.*` `api:mcp.protocol.*` `api:mcp.transport.*` `api:mcp.meta.*` | RP-03 MCP 基座 |
| `api:mcp.tool.project.*` `api:mcp.tool.member.*` `api:mcp.tool.token.*` `api:mcp.tool.audit.*` | RP-02 |
| `api:mcp.tool.table.*` `api:mcp.dispatch.table-tools.*` | RP-04 |
| `api:mcp.tool.board.*` `api:http.*` | RP-05（webhook 端点见 RP-13） |
| `api:mcp.tool.repo.*` | RP-08 |
| `api:mcp.tool.skill.*` | RP-09 |
| `api:mcp.tool.task.*` | RP-10 · RP-12 |
| `api:mcp.tool.run.*` `api:mcp.tool.trigger.*` | RP-11 |
| `api:mcp.tool.schedule.*` `api:http.paths./webhooks/git.post` | RP-13 |
| `api:mcp.tool.flow.*` | RP-14 · RP-15 |
| `api:mcp.tool.plugin.*` | RP-16 |
| `api:mcp.tool.notice.*` | RP-17（**只有两个**，`NTF-009`） |
| `api:mcp.tool.template.*` | RP-18 |
| `api:mcp.tool.xforge.*` | RP-19（**形状由 XForge 定死**，`XFG-010`） |
| `api:web.*`（前端自己的纪律与渲染约束） | RP-06 |

## 命名

```
api:mcp.tool.<域>.<动作>                     一个 tool
api:mcp.dispatch.<机制>.<动作>               运行时派发的 tool 的规则与固定信封，不是它的实例
api:mcp.registry.<能力>                      RP-03 对内的注册接口面（MCP-012）
api:mcp.error.<码>                           统一错误契约的一条（MCP-007 / MCP-008）
api:mcp.protocol.<方法>                      JSON-RPC 方法（initialize · tools/list · tools/call）
api:mcp.transport.<形态>                     MCP 的传输面。⚠️ **不写成 api:http.***：
                                             MCP 与只读 HTTP 不是同一个服务面（RP-03 / RP-05），
                                             混进去会让"http 面里一条写路由都没有"这句话失真
api:mcp.meta.<字段>                          params._meta 上的带外字段（幂等键这类）
api:web.<面>.<约束>                          前端自己的约束（渲染子集、只读纪律、没有报表）。
                                             它不是一个网络接口，但**它是一份要被守住的契约**——
                                             而契约基线正是记这种东西的地方
api:http.paths.<路径>.<方法>                 一条只读 HTTP 路由，路径在前、方法在后、无空白
```

方言文件（有实现之后）：`api/mcp/<域>.json`（JSON Schema 2020-12，tool 的输入 schema
原样就是它）与 `api/read-model.openapi.yaml`（OpenAPI 3.1，前端由它生成 TS 客户端；
**后端不从它生成**，一致性由 check 保证）。

## Elements

### Element: api:mcp.protocol.initialize
- module: xops-mcp
- consumers: [任何 MCP 客户端]
- 握手，回协议版本 `2025-06-18` 与 `serverInfo`。
- ⚠️ **握手也要带令牌**（`MCP-002`）：一个连身份都还没有的连接，没有任何理由需要知道
  这个 server 支持什么。

### Element: api:mcp.protocol.tools-list
- module: xops-mcp
- consumers: [任何 MCP 客户端]
- 能力发现（`MCP-009`）。带 `_meta.project` 精确到那个项目里的角色；不带就按这个人
  在自己参与的项目里拿到过的最高角色给一个概览——协议里的 `tools/list` 没有项目这个概念，
  精确的那个答案由 `identity.capabilities` 给。

### Element: api:mcp.protocol.tools-call
- module: xops-mcp
- consumers: [任何 MCP 客户端]
- 一次调用的顺序是固定的，**每一步的先后都有理由**：
  ① 认证 → ② 找 tool → ③ 定项目 → ④ 鉴权 → ⑤ 校验 schema → ⑥ 幂等 → ⑦ 执行 → ⑧ 记幂等 / 失败留痕。
- ⑤ 放在 ④ 之后：**schema 的细节也是信息**，不该让越权者试出来。

### Element: api:mcp.meta.idempotency-key
- module: xops-mcp
- consumers: [任何 MCP 客户端]
- 幂等键在 `params._meta.idempotencyKey`，**不在 `arguments` 里**。
- 放进 `arguments` 就得让每个 tool 的 schema 都声明一个它自己不用的字段，而 `MCP-003`
  又要求未声明字段一律拒绝——两条会互相打架。
- 键**按人分区**：幂等键是调用方自己取的字符串，撞名是正常的，混在一起就是跨用户泄露。

### Element: api:mcp.registry.tool-spec
- module: xops-mcp
- consumers: [RP-04 起各包]
- **注册一个 tool 必须交出五样**：固定形状的输入 schema · 需要的角色 · 是否幂等 ·
  幂等键从哪来 · 留痕形状。交不出的**注册不进来**（`build()` 当场失败）。
- 反过来：**注册一个 tool 即自动获得全套纪律**（`MCP-012`）——认证、鉴权、schema 校验、
  幂等、留痕都在外面做完，各域不写这些，也因此没有各自写错的机会。

### Element: api:mcp.registry.narrow-schema
- module: xops-mcp
- consumers: [RP-04 起各包]
- 字段类型是一个**穷举的 enum，里面没有"任意对象"**（`MCP-004`）。想写一个通用透传 tool，
  得先改那个 enum——这条与 `TBL-021`（不提供 json 列类型）是同一条纪律的两处落点。
- 渲染出的 JSON Schema 一律 `additionalProperties: false`（**嵌套记录也一样**）：
  `MCP-003` 在协议层的兑现。
- 长文本超限**拒绝，不截断**（`MCP-014`）。
- 本次新增 `Record`：见 `api:mcp.registry.record-field`。

### Element: api:mcp.error.contract
- module: xops-mcp
- consumers: [任何 MCP 客户端]
- 稳定错误码 · 可读消息 · 是否该重试（`MCP-007`）。码：`invalid_argument` · `not_found` ·
  `conflict` · `unauthenticated` · `timeout` · `unavailable` · `internal`。
- **客户端按码分支，不按消息分支。**

### Element: api:mcp.error.not-found
- module: xops-mcp
- consumers: [任何 MCP 客户端]
- **XOps 对外没有"无权限"这个错误。** 项目不存在 / 不是成员 / 角色不够 / 项目已归档，
  四种情形返回逐字节一致的响应（`PRJ-008` + `MCP-008`）——否则错误码本身就是探测工具。
- 能走到 `unauthenticated` 的只有一种情况：令牌不对。

### Element: api:mcp.transport.http
- module: xops-mcp
- consumers: [XForge 的 McpServer 资源, 任何 MCP 客户端]
- `POST /mcp`，`Authorization: Bearer <令牌>`。**不支持 chunked、不支持 SSE、不做限流**
  （`MCP-015` 是明写的不做，限流交给部署侧的反向代理）。
- ⚠️ **这条不写成 `api:http.*`**：MCP 与只读 HTTP 不是同一个服务面，也不共用路由层
  （RP-03 / RP-05 的分工）。混进去会让"`api:http.*` 里一条写路由都没有"这句话失真，
  而那句话是 `G2` 在 api.md 上的证明。

### Element: api:mcp.transport.stdio
- module: xops-mcp
- consumers: [本地 MCP 客户端]
- 换行分隔的 JSON-RPC，令牌从 `XOPS_TOKEN` 取。**不从参数里取**（`I-B`）。

### Element: api:mcp.tool.identity.whoami
- module: xops-mcp
- consumers: [agent]
- 查询当前令牌对应的身份。

### Element: api:mcp.tool.identity.capabilities
- module: xops-mcp
- consumers: [agent]
- **我在这个项目里能调哪些 tool**（`MCP-009`）。非成员在这里得到的与"项目不存在"完全一致。

### Element: api:mcp.tool.identity.pending-nodes
- module: xops-mcp
- consumers: [agent]
- 我待处理的流程节点，**跨项目聚合**。
- ⚠️ **注册位在 RP-03，实现在 RP-14。** 现在挂的是空实现：tool 存在、形状定死、调得通、
  返回空列表。RP-14 接进来时改的是那个 trait 的实现，不是这一层的注册与 schema——
  而后者一旦要改，全部客户端跟着改。

### Element: api:mcp.tool.table.create
- module: xops-table
- consumers: [agent]
- 建表：声明列、列类型与保护级别。`columns` 是一个**形状被声明死了的**记录列表。

### Element: api:mcp.tool.table.add-column
- module: xops-table
- consumers: [agent]
- 加一列。**新列对历史行为空。改列类型、删列、改列名不做**——错误信息指向"新建一张表自己搬"。

### Element: api:mcp.tool.table.describe
- module: xops-table
- consumers: [agent]

### Element: api:mcp.tool.table.list
- module: xops-table
- consumers: [agent]
- 列出项目里的表。**软删过的不在其中。**

### Element: api:mcp.tool.table.drop
- module: xops-table
- consumers: [agent]
- 软删（`TBL-026`）：从列出结果中消失、专属 tool 停止派发，**行与事件一律保留、单行历史仍可查**。
- **表名不可复用**；被流程引用为结算表或主体表的表不能删（那道判定的位在 `rust:xops-table#DropGuard`，RP-14 接）。

### Element: api:mcp.tool.table.history
- module: xops-table
- consumers: [agent, RP-05 的读模型]
- 一行的完整历史：谁、何时、改了什么。**删除那一条也带署名**——"谁删的"是这一问的一半。

### Element: api:mcp.dispatch.table-tools
- module: xops-table
- consumers: [agent]
- **`MCP-005` 的落点**：每张表建好之后派发 `row.<表>.{insert,update,delete,select}`，
  各自带**由该表 schema 生成的固定形状输入 schema**。
- **不存在 `{table, values: 任意形状}` 的通用写 tool**：列名与类型在协议层是被声明过的。
- 派发规则，逐条：
  - 系统表**只派发 `select`**——它们只有平台能写（`TBL-003`）。
  - 全局表（`_notices`）**一个都不派发**——它只有两个专属 tool，且在 RP-17（`NTF-009`）。
  - 自增序号与派生文本**不出现在写 tool 的参数里**——它们是平台算的。
  - 写 tool 要的角色由保护级别决定：普通表按 `WriteTable`，受保护表按 `WriteProtectedTable`。
  - tool 名里系统表的 `_` 换成 `sys-`（`_` 不是合法的 tool 名字符）；
    **用户表因此不能以 `sys-` 开头**，否则两者会撞。
- **建表即派发、删表即停派**：它每次被问的时候现算，不需要谁去通知它。

### Element: api:mcp.registry.record-field
- module: xops-mcp
- consumers: [RP-05 起各包]
- 形状被声明死了的嵌套记录字段。
- ⚠️ **它不是 `MCP-004` 的口子**：那一条禁的是"接受**任意**结构"，而这里的子字段逐个声明，
  渲染出的 JSON Schema 里嵌套对象一样带 `additionalProperties: false`。
  没有它，"建表时声明有哪些列"只能拆成几条平行的数组——那不会更窄，只会更容易对错位。

### Element: api:mcp.tool.project.create
- module: xops-mcp
- consumers: [agent]
- 建项目。**任何用户都可以建，无需申请或审批；创建者自动成为所有者**（`PRJ-001`）。
- `Requirement::Platform`——建项目的时候还没有项目，没有项目内角色可判。
- 建完之后 `ProjectHook` 接着把那四张系统表建起来（`TBL-005`）。

### Element: api:mcp.tool.project.mine
- module: xops-mcp
- consumers: [agent]

### Element: api:mcp.tool.project.describe
- module: xops-mcp
- consumers: [agent]
- **非成员得到的与项目不存在完全一致**（`PRJ-008`）。

### Element: api:mcp.tool.project.archive
- module: xops-mcp
- consumers: [agent]
- 归档后转为只读。**归档项目里的写 tool 从能力发现里整批消失**——那是 `can_in` 的自然结果。

### Element: api:mcp.tool.member.list
- module: xops-mcp
- consumers: [agent]

### Element: api:mcp.tool.member.set
- module: xops-mcp
- consumers: [agent]
- 加成员或改角色。**一个项目必须始终至少有一个所有者**（`PRJ-006`）——降级最后一个也算。

### Element: api:mcp.tool.member.remove
- module: xops-mcp
- consumers: [agent]

### Element: api:mcp.tool.token.issue
- module: xops-mcp
- consumers: [agent]
- **原文只在这一次的响应里出现**（`TOK-002`、`I-A`）。
- ⚠️ **刻意不支持幂等键**，理由写在声明里：重复签发就该是两个令牌，而"返回与首次相同的
  结果"会把第一次的原文再吐一遍——那正好破掉 `TOK-002`。这是 `Idempotency::NotIdempotent`
  那条路存在的原因：让"忘了做幂等"与"想清楚了不做"在代码里看起来不一样。

### Element: api:mcp.tool.token.revoke
- module: xops-mcp
- consumers: [agent]
- **立即生效，没有延迟窗口**（`TOK-003`）。

### Element: api:mcp.tool.token.mine
- module: xops-mcp
- consumers: [agent]
- **只有摘要与时间，没有原文。**

### Element: api:mcp.tool.audit.query
- module: xops-mcp
- consumers: [agent]
- 按类型与时间范围查项目事件流（`AUD-008`）。
- `InProject(ReadProject)` 是 `AUD-003` 在协议层的那一半：**不是成员的话，
  连这次调用都到不了查询**——可见性在扫描之前就成立，不是查完再筛。

### Element: api:mcp.tool.audit.history
- module: xops-mcp
- consumers: [agent]
- 某个对象的完整历史。

### Element: api:mcp.tool.board.define
- module: xops-read
- consumers: [agent]
- **看板 = 一张表的一个视图**：显示哪张表、按什么筛选、按什么排序、显示哪几列（`BRD-001`）。
- 筛选**只有等值与非空两种**——再多就开始像查询语言了。
- ⚠️ **`_notices` 建不了**（`BRD-004`）：个人看板是平台内建的固定视图，归 RP-17。

### Element: api:mcp.tool.board.list
- module: xops-read
- consumers: [agent]

### Element: api:mcp.tool.board.show
- module: xops-read
- consumers: [agent]

### Element: api:http.paths./api/me.get
- module: xops-web
- consumers: [web]
- 我是谁（`BRD-011`：**明确展示当前用户身份**）。

### Element: api:http.paths./api/projects.get
- module: xops-web
- consumers: [web]
- 我参与的项目。**可见性完全遵循项目成员边界。**

### Element: api:http.paths./api/projects/{project}/boards.get
- module: xops-web
- consumers: [web]

### Element: api:http.paths./api/projects/{project}/boards/{board}.get
- module: xops-web
- consumers: [web]

### Element: api:http.paths./api/projects/{project}/tables/{table}/rows/{row}/history.get
- module: xops-web
- consumers: [web]
- 单行历史——`BRD-006` 的前一半：**状态怎么变的、谁改的、什么时候**。

### Element: api:http.paths./api/projects/{project}/tables/{table}/instances/{instance}/settlements.get
- module: xops-web
- consumers: [web]
- 同实例的结算行——`BRD-006` 的后一半：**为什么这么变、谁表的态**。
- ⚠️ **与上一条是两个视图、两次查询。平台不做 join**（`TBL-023`）。
  路由表上因此没有任何一条叫 `timeline` 的路由。

### Element: api:http.paths./api/projects/{project}/tables/{table}/rows/{row}/columns/{column}/raw.get
- module: xops-web
- consumers: [web]
- 长文本的**原始形式**（`BRD-010`：供不信任渲染的人自行查看）。
- `Content-Type: text/plain`，**一个字都不动**——不经任何渲染。

### Element: api:http.paths./session.post
- module: xops-web
- consumers: [web]
- 登录。**`MCP-013` 认下的四个例外之一（会话面）**：凭据类，只建立会话，
  **不创建或修改任何业务对象**。
- 下发的 cookie 带 `HttpOnly; SameSite=Strict`。

### Element: api:http.paths./session.delete
- module: xops-web
- consumers: [web]
- 注销。同上。

### Element: api:web.markdown.restricted-subset
- module: web
- consumers: [web]
- **长文本渲染只认一个受限子集**（`BRD-008`）：标题 · 段落 · 无序列表 · 引用 ·
  围栏代码块 · 行内代码 · 粗体 · 斜体 · 链接（**只认 http/https**）。
- **刻意不支持**：图片与任何嵌入（"外部资源不自动加载"因此是结构性的）· 表格 ·
  原始 HTML · 自动链接。不支持的写法一律当**纯文本**。
- ⚠️ **不引渲染库。** `BRD-008` 自己说了为什么：绝大多数 Markdown 渲染库默认开启内联 HTML，
  而这些内容部分来自被分析的代码仓——**能往那个仓提交代码的人，就能影响它**。
- 实现上没有 HTML 字符串这一步：解析成节点树，由 React 渲染成元素。
  **注入不是被过滤掉的，是没有地方可注。**

### Element: api:web.discipline.no-write-calls
- module: web
- consumers: [web]
- `BRD-005` 第 ② 道：**前端不存在调用写接口的代码路径**，由 `npm run check` 里的
  `scripts/frontend-discipline.mjs` 枚举全部前端源码来证明。
- 唯一豁免是 `src/session.ts`（`MCP-013` 的凭据类例外）。**豁免写在检查脚本里，
  不写在注释里**——"再开一个口子"必须先改那个文件。
- ⚠️ **顺序不能反**：第 ① 道（后端不存在写路由）在 RP-05。只有 ② 没有 ①，
  等于把一条安全属性交给前端自觉。

### Element: api:web.discipline.no-reports
- module: web
- consumers: [web]
- `BRD-002` / `BRD-003`：**没有报表。** 同一个检查脚本枚举视图与依赖，
  挡住图表库、`<canvas>` 与 chart 字样。
- 判断标准很直白：**如果有一天需要在平台代码里写"什么是缺陷密度"，那就越界了**——
  而它最先会以一个图表库的 import 出现。
