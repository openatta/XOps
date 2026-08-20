# RP-08 XForge 审批 Provider 适配

## 归属与依赖

- **依赖**：RP-07（实例与决策的实际承载）、RP-03（MCP 服务形态）、RP-02（角色映射）。
- **被依赖**：无。这是一个出口。
- **不做这个包会阻塞谁**：不阻塞任何 XOps 内部功能，但**不做它，XOps 就不是"配合 XForge 使用"的平台**，只是一个独立的审批系统。

## 背景与价值

这个包是 XOps 与 XForge 之间**唯一的"拦住"通道**。

XForge 的 `xforge approve --provider <id>` 本身就是一个 MCP client——它会连上 XOps，调两个**名字固定、不协商**的 tool，然后把结果写成仓内的审批回执并追加进审计链。这条链路的规格是 XForge 已经定死的，**XOps 没有任何设计自由度，只有实现义务**。

为什么值得单独成包，而不是塞进 RP-07：

1. **它的形状不由我们决定。** 两个 tool 的名字、参数、返回值、返回格式（`text` content item 里装 JSON 字符串，不用 `structuredContent`）全部由 XForge 规定。这与 RP-03 的窄接口纪律方向一致但来源不同——RP-03 是我们自己的纪律，这里是外部契约。
2. **它有一个必须守住的性质：`poll_approval` 绝不阻塞。** 提交+轮询两段式的存在理由就是"人可能几小时到几天才决定"，任何一个想把它做成"等到有结果再返回"的实现都会直接破坏这个设计。
3. **角色语义要跨系统映射**，这是唯一需要判断的地方（见 Q4）。

还有一条必须记住的边界（G6）：

> **XForge 的 Gate 拿不到凭据。** Gate 子进程的环境过滤会丢弃一切凭据形状的变量名，且不可被重新放行。因此**绝不能**把归档前的校验设计成"Gate 去查 XOps"——它没法认证。正确形态是结论先经审批通道落进仓库成为回执，Gate 只读本地文件。这也是 XOps 挂了不影响开发的原因。

## 范围

### 包含

- `submit_approval_request` tool：按 XForge 规格实现，按 `governingDigest` 幂等
- `poll_approval` tool：按 XForge 规格实现，`pending` 立即返回
- XForge 的 Change/Flow/Stage/policy/revision 上下文 → XOps 审批实例的映射
- XForge 角色 ↔ XOps 项目角色的映射
- 决策结果 → XForge 期望的返回形状

### 不包含

- **审批的实际运转**——归 RP-07。本包是一层薄适配，不含任何审批逻辑。
- **写入任何仓库**——回执由 XForge CLI 自己写进仓里并追加审计链，XOps 不碰。
- **为 Gate 提供查询接口**——G6 明确排除。

## 需求条目

### 两个固定 tool

| ID | 需求 |
|---|---|
| R-08-01 | 实现名为 `submit_approval_request` 的 tool，参数与 XForge 规格完全一致：`change`、`flow`、`stage`、`transition`、`policyId`、`revision`（含 `stateRevision`/`contentRevision`/`policySnapshotDigest`/`gitBase`/`gitHead`）、`governingDigest`、`roles`、`reason`。 |
| R-08-02 | 实现名为 `poll_approval` 的 tool，参数只有 `governingDigest`。 |
| R-08-03 | 两个 tool 的返回值是**一个 `text` 类型的 content item，其 `text` 是一段 JSON 字符串**——不使用 `structuredContent`。 |
| R-08-04 | `submit_approval_request` 按 `governingDigest` **幂等**：该 digest 对应的请求已在途时，本次调用是空操作，返回同样的确认。**不得重复开单。** |
| R-08-05 | `poll_approval` **必须立即返回**，绝不阻塞等待决策。未决时返回 `{"status":"pending"}`。 |
| R-08-06 | 已决策时返回 `status: "decided"`、`decision`（`approve` 或 `reject`）、`approver`（含 `id` 与 `role`）、`reason`，以及可选的 `expiresAt`。 |
| R-08-07 | `poll_approval` 是纯读操作，无副作用，可安全重复调用。 |
| R-08-08 | 对一个从未提交过的 `governingDigest` 调 `poll_approval`，返回明确的未知状态而不是报错——XForge 会整轮重试（连接+提交+轮询），本包必须对重试安全。 |

### 映射

| ID | 需求 |
|---|---|
| R-08-09 | 从 XForge 上下文定位 XOps 项目：需要一条明确的绑定关系（哪个 Git 仓库/哪个 XForge 项目对应哪个 XOps 项目）。绑定不存在时，`submit_approval_request` 明确失败，不静默创建。 |
| R-08-10 | 从 XForge 的 `policyId` 与 `roles` 选择 XOps 的流程定义。选不到时明确失败。 |
| R-08-11 | `approver.role` 返回的必须是 XForge 侧认得的角色名，且该角色同时在 XForge 的 provider 条目与被满足的 Flow policy 角色列表内。**XOps 侧应当自己先做这个校验**，让角色配错在"告诉人类他的批准生效了"之前就失败，而不是等 XForge 报 `XFORGE_APPROVAL_ROLE_FORBIDDEN`。 |
| R-08-12 | XForge 的 `revision` 各字段原样保存在 XOps 审批实例上，供人在做决策时看清"我批的是哪一版"。 |
| R-08-13 | `reason` 字段是不可信自由文本（G7），原样保存与展示，不解析。 |

### 边界

| ID | 需求 |
|---|---|
| R-08-14 | 本包不提供任何供 XForge Gate 调用的查询接口（G6）。 |
| R-08-15 | XOps 不可用时，XForge 侧的表现必须是"这一轮 approve 失败、可重试"，而不是"变更被永久卡死"或"变更被放行"。 |

## 数据与不变量

- **I-08-1**：`governingDigest` 到审批实例是一一映射。
- **I-08-2**：本包不含审批逻辑，全部委托 RP-07——一次成立条件判定都不在本包内实现。
- **I-08-3**：本包不持有任何仓库写权限（继承 I-05-3）。

## 接口面

**MCP tools**（形状由 XForge 定死，不可改）

- `submit_approval_request`
- `poll_approval`

**XForge 侧需要的配套配置**（在项目仓里，不属于本包实现，但属于本包的交付说明）

- 一份 `McpServer` 资源（`transport: http`、`url` 指向 XOps、`authTokenEnv`、`timeoutSeconds`），并登记进 `manifest.yaml` 的 `scaffold.mcpServers`
- 一条 `approvals.providers[]` 条目（`type: mcp`、`mcpServer` 引用上面那份、`roles` 与 XOps 实际返回的角色对齐）
- 从某条 Flow 的 `approvalPolicies[].providers` 列表引用该 provider id

## 验收标准

**必须用真实的 `xforge approve --provider <id>` 跑通，不能只做孤立单测。** 这是 XForge 文档明确要求的验收方式。

- 配好 `McpServer` 与 provider 条目后，`xforge doctor` 无相关告警。
- 跑一次 `xforge approve --change <id> --for <transition> --provider xops`，决策未做出时命令成功返回，且 `nextActions` 给出稍后重跑的命令，**仓内没有写入任何回执**。
- 在 XOps 侧做出**通过**决策后重跑同一命令，回执被写入、审计链事件被追加、transition 解锁。
- 在 XOps 侧做出**拒绝**决策后重跑，按 XForge 的拒绝语义正确处理。
- 用一个不在允许角色列表内的人做出决策，XOps 侧先行拒绝，不会走到 XForge 报 `XFORGE_APPROVAL_ROLE_FORBIDDEN`。
- 重复跑 `xforge approve` 十次，XOps 侧只有一个审批实例。
- 关停 XOps 后跑 `xforge approve`，命令报连接失败可重试；重新拉起后重跑成功，且此前状态未损坏。
- 确认没有任何 Gate 去查询 XOps。

## MVP 子集

**不纳入 MVP**（依赖 RP-06/RP-07）。

但这是 **MVP 之后第一优先**。理由：它是 XOps"配合 XForge"这个定位的唯一硬证据，而且它的规格已经写死、无需设计探索，实现风险在整个项目里是最低的一档——只要 RP-07 就位，这个包是纯粹的机械工作。

## 风险与待定

- **Q4：角色映射以谁为准。** XForge 的 `roles` 是仓库层面的概念（`owner`、`maintainer`，跨仓设计里还建议新增 `verifier`），XOps 是项目角色（所有者/维护者/成员）。三个候选：
  1. **同名直映**——要求 XOps 项目角色名与 XForge 角色名一致。最简单，但把 XOps 的角色模型绑死在 XForge 的词汇上。
  2. **绑定时声明映射表**——在 R-08-09 的绑定关系上附一张 XOps 角色 → XForge 角色的映射。灵活，但多一处会长期不一致的配置。
  3. **XOps 项目支持自定义角色名**——把 RP-02 的固定三角色改成可配置。最干净，但会把 RP-02 的复杂度顶上去。

  **倾向 2**，因为映射表很小（三五行）、位置明确（就在绑定上）、且不污染 RP-02。**这条需要你定。**

- **R-08-09 的绑定关系怎么建立**没有定义。候选是在 XOps 项目上登记 XForge 项目标识或仓库 URL。这与 RP-09 的代码仓绑定高度重合，**建议复用 RP-09 的绑定对象**，不要开第二套。

- **职责分离在 XForge 场景下可能落不了地。** R-06-14 要求判定"决策人 ≠ 被审批对象的作者"，但作者集合在仓里，XOps 拿不到（也不该去拿，那需要仓库读权限与在线依赖）。三种出路：跨仓协作设计里那条"服务是唯一能校验轮换不变量的地方"意味着这个能力最终要有；但在 MVP 之后的第一版里，**建议先只支持"决策人 ≠ 审批实例的发起人"**——这是 XOps 自己知道的事实，不需要读仓。完整的作者集合比对留到有真实需求时再说。

- **`expiresAt` 的时钟**：XOps 与开发者机器的时间可能不一致。建议返回绝对时间戳并统一用 UTC，且不要设置过短的有效期。
