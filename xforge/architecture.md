# 架构 — XOps

XOps 是配合 XForge 的云端服务：确定性平面记缺陷账本与审批，分析平面在一次性容器里驱动 dsh 跑用户写的技能，Web 只观察不写入。
非目标：不做工单系统、看板、流水线或报表引擎；不持有任何代码仓写权限；不进入开发的关键路径——XForge 的 Gate 永不查询它。

## 结构

| 模块 | 责任 | 路径 |
|---|---|---|
| api | MCP server、只读 JSON API、OAuth 回调、webhook 接收 | `apps/api` |
| worker | 执行调度、容器编排、seam 服务端 | `apps/worker` |
| web | 只读 SPA | `apps/web` |
| core | 身份与令牌、项目与权限判定、追加式审计 | `packages/core` |
| 领域 | 缺陷、仓与工作区、技能与任务、审批与 XForge provider、触发、通知 | `packages/{defects,repos,tasks,approvals,triggers,notify}` |
| runtime | 执行契约、容器沙箱、seam 协议 | `packages/runtime` |
| store | 事件流、读模型、对象存储访问 | `packages/store` |
| bridge | dsh 侧插件：把 `ctx.fs` 与 `ctx.subprocess` 指向本次执行的容器 | `dsh-bridge/` |

不变量：
- 领域模块之间不得互相 import；跨领域协作只经 `packages/core` 与事件流。
- 业务写入只能经 `apps/api` 的 MCP 基座产生；`apps/web` 不得调用任何写接口。
- XOps 侧任何代码不得 import dsh 的包，只认 `packages/runtime` 的执行契约。
- 凭据不得出现在容器内、产物、过程记录或日志中。
- 常驻 dsh 中，两次执行之间不得有可观测的状态泄漏。
- 任何业务对象的当前状态可仅凭事件流重建；读模型是缓存。

## 决策

### ARC-001 MCP 是唯一写入面，Web 只读
所有写操作经 MCP；Web 与 webhook、OAuth 回调都不构成通用写入面。
**为什么**：产品定位是给 AI 编程工具用。多开一条写入路径，就要多维护一套权限、幂等与审计，而这三样正是账本可信度的全部来源。
**落点**：`apps/api`、`apps/web`

### ARC-002 XOps 与 dsh 分立进程，工具执行经 seam 路由进一次性容器
dsh 常驻容器外只做编排与模型调用；`dsh-bridge` 实现 `ctx.fs` 与 `ctx.subprocess`，把全部文件与进程操作路由进本次执行的一次性容器，用完销毁。
**为什么**：dsh 处于 0.1.0-rc 且预告破坏性变更，分立进程把影响面限死在 bridge 一处；常驻省掉每次执行的冷启动；工具落进容器，使不可信技能内容能驱动的动作全部被隔离，而模型凭据留在容器外、从不进容器。
**否决**：把 XOps 写成 dsh 的 Cordis 插件——生命周期与构建体系会被 rc 阶段的框架绑死。dsh 跑在容器内——隔离更直白，但每次执行都付冷启动。工具也在容器外——最省事，但不可信内容驱动的动作就没有边界了。
**落点**：`apps/worker`、`packages/runtime`、`dsh-bridge/`

### ARC-003 api 与 worker 分立部署
MCP 与 Web 请求走 `apps/api`，执行走 `apps/worker`，之间靠队列。
**为什么**：执行是长时间的重活（拉仓、起容器、跑模型），与要求低延迟的 MCP 请求同进程时，一次大仓分析就能拖慢全部调用；两者的扩容维度也不同。
**否决**：单进程内分模块——MVP 更省事，但把扩容维度与故障域绑在了一起。
**落点**：`apps/api`、`apps/worker`

### ARC-004 关系库承载事件流与读模型，产物走对象存储
业务副作用与审计事件在同一关系库内原子写入；执行产物单独存。
**为什么**：产物体量大、保留期短、且是模型生成的不可信内容。与账本同库会让备份、清理与权限三件事互相牵制，而账本要求的恰恰是不可删除。
**否决**：全部进关系库——部署最简，但库体积增长与账本的不可删除要求冲突。事件流单独分库——职责更清，但业务副作用与事件的原子写入要跨库事务。
**落点**：`packages/store`
