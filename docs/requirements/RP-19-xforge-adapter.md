# RP-19 XForge 适配

> 两个形状定死的 tool · 一次绑仓 + 一条登记 · 不可用时可重试不是放行

> ⚠️ **两条验收在本仓跑不了，写出来而不是当作通过**：`XFG-024`（真实的
> `xforge approve --provider xops` 端到端）与 `XFG-020` 的断网测试，都要**一个跑起来的
> XOps 服务**加**一份装好的 XForge**——本仓还没有可执行入口，XForge 的契约治理版本
> 也还没发布。本包能做到的是**让降级逻辑不存在**（有一条枚举源码的测试盯着），
> 而"断网时到底发生什么"由 XForge 侧的传输层决定。
>
> ⚠️ **两个 tool 的名字带下划线，不合 `<域>.<动作>`**。`xops-mcp` 为此加了一张
> **写死的白名单**（`EXTERNAL_NAMES`，恰好两条）与一位 `text_only`（回话不带
> `structuredContent`）。**都是只加不改**：别的 tool 一个字节没变。
>
> ⚠️ **首版不返回 `expiresAt`**（`Q12` 未定）。规格里它是可选的——
> **要与 XForge 侧确认它接受缺席。**

## 归属与依赖

- **依赖**：RP-14 流程定义与实例 · RP-15 结算判定与保护（被适配物）、RP-08 Git 仓绑定（登记挂在仓绑定上）、RP-18 模板（approvals 那一套）。
- **被依赖**：无。
- **里程碑**：M5。
- **独立性**：做完即可验收——`xforge approve --provider xops` 真实跑通，且关停 XOps 后报可重试。

## Crate 归属

| crate | 本包是否新建 | 对外只经什么被用 |
|---|---|---|
| `xops-xforge` | **新建** | 两个形状定死的 tool · 登记的读写 · 结果列映射 |

**跨仓交付物**：XForge 侧的配套四样（见下）。它们不在本仓，**但版本对齐责任在 XOps 侧**（D40）。

## 背景与价值

这个包把 XOps 接成 XForge 的一个审批 provider。它的形状**不由我们定**：

> 两个 tool 的名字、参数与返回值形状**由 XForge 定死，不可改**。返回值是**一个 `text` 类型的 content item，其 `text` 是一段 JSON 字符串，不使用 `structuredContent`**。我们只有实现义务。

**三条必须记住的边界：**

| 边界 | 说明 |
|---|---|
| **Gate 拿不到凭据** | XForge 的 Gate 子进程会过滤掉一切凭据形状的环境变量。所以**绝不能**把归档前的校验设计成"Gate 去查 XOps"——它没有能力认证。正确形态是决策先经审批通道落进仓库成为回执，Gate 只读本地文件。**这也是 XOps 挂了不影响开发的原因**（G6） |
| **角色以 XOps 为准（D15）** | XOps 返回自己的项目角色名，XForge 侧去对齐。**代价**：XOps 的角色固定为所有者/维护者/成员，若 XForge 侧某条 policy 要求 `verifier`，校验将永远失败且无法绕过。**不要为此把 XOps 改成可配置角色系统** |
| **不可用时可重试，不是放行** | 关停 XOps 之后跑 `xforge approve`，必须报连接失败可重试。**任何"连不上就跳过"的降级逻辑都会让变更被静默放行**——这条必须**实际断网测一次** |

### XForge 侧的配套四样，缺一样门就不存在

```text
① 一份 `McpServer` 资源（transport: http · url · authTokenEnv · timeoutSeconds）
② 它在 `manifest.yaml` 的 `scaffold.mcpServers` 里的登记
③ 一条 `approvals.providers[]` 条目（roles 与 XOps 实际返回的角色对齐）
④ 某条 Flow 的 `approvalPolicies[].providers` 里**引用这个 provider id**
```

> ⚠️ **①② 缺了会加载失败，③④ 缺了会静默失效**——后者更危险：`xforge doctor` 对未被引用的扩展资源**只警告、从不阻塞**，于是 provider 装好了、连得上、却没有任何一条 Flow 引用它，**这道审批门等于不存在，而一切看起来都正常**。
>
> **这四样是 XOps 的持续交付物**（D40），不是一次性配置说明。**版本对齐责任在 XOps 侧。**

### 一条安全前提，不是运维建议

> **每个开发者用自己的 XOps 令牌。** `FLW-026③`（职责分离）整个压在"事件载荷里的发起者就是那个真人"上，而发起者 = 调用 `submit_approval_request` 所用令牌的持有人。**一旦团队共用一个令牌，这条会整体失效且无声**：要么所有请求都算那个人发起的（他的任务永远不能结算节点），要么所有请求都不算（自发自批的门形同虚设）。

## 范围

### 包含

- **两个形状定死的 tool**：`submit_approval_request` · `poll_approval`
- **登记**：policyId → 哪条流程，以及**结果列映射**（哪一列是 decision、哪一列是 reason）
- **approver 的解析**：从结算行的 `writtenBy` 解析（人 → 就是他；执行 → 私有任务的所有者；插件求值 → 安装该插件的维护者），role 取该人在本项目的角色
- 按 **governingDigest 幂等**发起流程实例，**立即返回**
- 原样保存 revision 各字段
- gitHead 作为事件载荷里的**主体修订**
- **XForge 侧配套四样的交付与维护**
- **角色的自校验**（在告诉人类"你的批准生效了"之前就失败）

### 明确不包含

- **流程引擎本身**——归 RP-14 与 RP-15。
- **approvals 模板**——归 RP-18。**本包适配它，不定义它。**
- **仓绑定**——归 RP-08。登记**挂在它的仓绑定上，不另开一套对象**。
- **往任何仓库写东西**——**永不**。回执由 XForge CLI 自己写进仓里（`XFG-017`）。
- **"理由必填"的校验**——归 RP-18 的 approvals 插件。**平台不认识"理由"这个概念。**

本包认领 [`requirements.md`](requirements.md) 的 **24 条**。**条目正文以 `requirements.md` 为准**，下表只是索引与一句话摘要。

### XFG XForge 适配（24 条）

| ID | 摘要 |
|---|---|
| `XFG-001` | 链路成立需要五件事：① 在 XOps 建一个项目；② 绑定 XForge 所在的那个仓库；③ 登记 policyId → 哪条流程并声明结果列映射；④ 每个… |
| `XFG-002` | ②③ 任一找不到时明确失败，绝不静默创建。 |
| `XFG-003` | 结果列映射声明：结算表的哪一列是 decision（approve/reject）、哪一列是 reason。 |
| `XFG-004` | approver 由结算行的 writtenBy 解析：是人 → 就是他；是执行 → 取那个私有任务的所有者；是插件求值 → 取安装该插件的维护者。 |
| `XFG-005` | ④ 不是运维建议，是一条安全前提。 |
| `XFG-006` | ⑤ 只影响自动化，不影响人。 |
| `XFG-007` | 实现名为 submit_approval_request 的 tool，参数与 XForge 规格完全一致：change · flow · stage · … |
| `XFG-008` | 实现名为 poll_approval 的 tool，参数只有 governingDigest。 |
| `XFG-009` | 两个 tool 的返回值是一个 text 类型的 content item，其 text 是一段 JSON 字符串——不使用 structuredConte… |
| `XFG-010` | 两个 tool 的名字与参数由 XForge 定死，XOps 没有任何设计自由度，只有实现义务（MCP-004 的窄接口纪律方向一致但来源不同）。 |
| `XFG-011` | submit_approval_request 的处理：由仓绑定定位 XOps 项目（找不到 → 明确失败）→ 由 policyId + roles 映射到… |
| `XFG-012` | 原样保存 revision 各字段（人做决定时要看清"我批的是哪一版"）。 |
| `XFG-013` | poll_approval 必须立即返回，绝不阻塞：未决 → pending；已决 → decided + decision + approver{id, … |
| `XFG-014` | poll_approval 是纯读操作，无副作用，可安全重复调用。 |
| `XFG-015` | approver.role 必须是 XForge 侧认得的角色名，且同时在 provider 条目与被满足的 policy 角色列表内。 |
| `XFG-016` | reason 是不可信自由文本（G7），原样保存与展示，不解析。 |
| `XFG-017` | XOps 从不写任何仓库。 |
| `XFG-018` | Gate 拿不到凭据：XForge 的 Gate 子进程会过滤掉一切凭据形状的环境变量。 |
| `XFG-019` | 角色以 XOps 为准：XOps 返回自己的项目角色名，XForge 侧去对齐。 |
| `XFG-020` | 不可用时可重试，不是放行。 |
| `XFG-021` | 四样，缺一样门就不存在，由 XOps 提供并维护：① 一份 McpServer 资源（transport: http · url · authTokenEn… |
| `XFG-022` | ①② 缺了会加载失败，③④ 缺了会静默失效——后者更危险：xforge doctor 对未被引用的扩展资源只警告、从不阻塞，于是 provider 装好了、… |
| `XFG-023` | 两侧的版本对齐责任在 XOps 这边。 |
| `XFG-024` | 验收必须用真实的 xforge approve --provider xops 跑通，不能只做孤立单测。 |

## 关键不变量

`G6`（XOps 不进入开发的关键路径；Gate 只读仓内文件）· `I-G`（不持有、不请求仓库写权限）· `I-O`（职责分离——它压在"令牌按人签发"上）。

## 接口面

**MCP tools（形状由 XForge 定死）**：

```text
submit_approval_request
    change / flow / stage / transition / policyId
    revision{stateRevision, contentRevision, policySnapshotDigest, gitBase, gitHead}
    governingDigest / roles / reason
poll_approval
    参数只有 governingDigest，**必须立即返回**
    未决        → pending
    已决        → decided + decision + approver{id, role} + reason (+ expiresAt)
    从未提交过  → **明确的未知状态，不是报错**
```

**另有登记 tools**：登记"本项目对应哪个仓" · 登记 "policyId → 哪条流程 + 结果列映射"。

**契约元素**（基线在 [`../contracts/`](../contracts/README.md)，正文由本包自己的变更逐条添加）：

```text
api:mcp.tool.xforge.submit-approval-request
api:mcp.tool.xforge.poll-approval
                                    ⚠️ **形状由 XForge 定死，XOps 没有设计自由度**（XFG-010）。
                                    这两条元素的正文照抄 XForge 的定义，**任何"优化"都是破坏性变更**，
                                    要走 DECISIONS.yaml
api:mcp.tool.xforge.register.*      登记本项目对应哪个仓 · policyId → 流程 + 结果列映射
rust:xops-xforge#*
```

## 验收标准

- **`xforge approve --provider xops` 真实跑通**（端到端，不是 mock）。
- **断网测试**：关停 XOps 后跑 `xforge approve`，**报连接失败可重试，不是放行**。**这条必须实际断网测一次。**
- **两个 tool 的返回值形状**：一个 `text` 类型的 content item，其 `text` 是 JSON 字符串，**不使用 `structuredContent`**。
- **`poll_approval` 立即返回**：未决时立刻回 `pending`，不阻塞等待。
- **从未提交过**：返回**明确的未知状态，不是报错**——XForge 会整轮重试（连接+提交+轮询），**必须对重试安全**。
- **幂等**：同一 `governingDigest` 重复提交，**不发起第二个实例**。
- **②③ 找不到时明确失败，绝不静默创建**：没绑仓、或没登记 policyId → 明确失败。
- **approver 三种解析**都要实际构造：人写的行 · 私有任务写的行 · 插件判定。
- **角色自校验**：配一个 XOps 不会返回的角色名，**在提交阶段就失败**，不是等到"告诉人类他的批准生效了"之后。
- **gitHead 未推送**：任务的工作区准备失败 → `FLW-026⑥` 不过 → 不结算、归为工作区错误、**通知写入者**；调用方看到 `pending`。**这条失败必须真的发出通知**，否则 XForge 那边会看到一个查不出原因的挂起。
- **人工写入不受影响**：gitHead 未推送时，人看 `revision` 各字段照样能批。
- **XOps 从不写任何仓库**：枚举全仓代码路径。
- **配套四样齐全**：`xforge doctor` 之外，**要额外验证 ④（Flow 里确实引用了这个 provider）**——因为 doctor 对它只警告不阻塞。

## 内部工作包建议

一次 **Major Change**：

```text
WP-A 登记（仓绑定上的 policyId → 流程 + 结果列映射）+ 角色自校验
        │
        ▼
WP-B submit_approval_request（幂等发起、原样保存 revision、
     gitHead 作主体修订、立即返回）
        │
        ▼
WP-C poll_approval（立即返回、三种状态、approver 三种解析、重试安全）
        │
        ▼
WP-D XForge 侧配套四样的交付 + 端到端与断网测试
```

**WP-D 不是文档工作**，它是本包的一半价值——**缺 ③④ 的后果是"门看起来在，其实不存在"**。

## 风险与待定

- **④ 那条静默失效是本包最危险的一处。** `xforge doctor` 只警告不阻塞，所以**必须有一个我们自己的检查**去验证"某条 Flow 确实引用了这个 provider"。建议把它做成 XOps 侧交付物的一部分，而不是一句叮嘱。
- **Q12（`expiresAt` 由谁设、默认多久）**未定，影响 `XFG-013`。首版可以先不返回 `expiresAt`，**但要确认 XForge 侧接受它缺席**。
- **版本对齐是持续责任**（D40）。XForge 的两个 tool 形状由它定死，**它变了我们就得跟**。建议把"XForge 版本升级时重跑端到端"列入常规。
- **"共用一个令牌"会让职责分离整体失效且无声。** 这条不能只写在文档里——**建议在提交阶段做一次检测**：同一个令牌短时间内代表多个不同开发者提交，至少要留一条警示痕迹。
