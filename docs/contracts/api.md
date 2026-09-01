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
| `api:mcp.tool.identity.*` `api:mcp.registry.*` `api:mcp.error.*` | RP-03 MCP 基座 |
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
api:http.paths.<路径>.<方法>                 一条只读 HTTP 路由，路径在前、方法在后、无空白
```

方言文件（有实现之后）：`api/mcp/<域>.json`（JSON Schema 2020-12，tool 的输入 schema
原样就是它）与 `api/read-model.openapi.yaml`（OpenAPI 3.1，前端由它生成 TS 客户端；
**后端不从它生成**，一致性由 check 保证）。

## Elements

(none)
