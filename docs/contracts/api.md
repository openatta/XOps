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
- 渲染出的 JSON Schema 一律 `additionalProperties: false`：`MCP-003` 在协议层的兑现。
- 长文本超限**拒绝，不截断**（`MCP-014`）——截断会让调用方以为写进去的是完整内容。

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
