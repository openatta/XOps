# XForge 侧的配套四样

> **缺一样门就不存在**（`XFG-021`）。这是 XOps 的**持续交付物**，不是一次性配置说明——
> **版本对齐责任在 XOps 这边**（`XFG-023`、`D40`）。

```text
① xops-approvals.mcpserver.yaml   一份 McpServer 资源
② manifest.snippet.yaml           它在 manifest.yaml 的 scaffold.mcpServers 里的登记
③ approvals.snippet.yaml          一条 approvals.providers[] 条目
④ flow.snippet.yaml               某条 Flow 的 approvalPolicies[].providers 里引用它
```

## 为什么 ③④ 比 ①② 危险

**①② 缺了会加载失败，③④ 缺了会静默失效**——后者更危险：

> `xforge doctor` 对未被引用的扩展资源**只警告、从不阻塞**，于是 provider 装好了、
> 连得上、却没有任何一条 Flow 引用它，**这道审批门等于不存在，而一切看起来都正常**。

所以本仓有**自己的检查**：`xops_xforge::scaffold::missing`。它的口径写在那个模块的
开头，一句话说清：**它检的是"这几个名字在不在文本里"，证明得了缺失，证明不了结构完全正确。**
后者由 `xforge doctor` 加上真实的 `xforge approve --provider xops` 一起回答（`XFG-024`）。

## 装之前要知道的三条

| | |
|---|---|
| **每个开发者用自己的 XOps 令牌** | 不是运维建议，是一条**安全前提**（`XFG-005`）。职责分离整个压在"事件载荷里的发起者就是那个真人"上，而发起者 = 调用 `submit_approval_request` 所用令牌的持有人。**一旦团队共用一个令牌，这条会整体失效且无声。** |
| **角色只有三个** | `owner` / `maintainer` / `member`（`XFG-019`）。若某条 policy 要求 `verifier`，校验将永远失败且无法绕过。**不要为此把 XOps 改成可配置角色系统**——约定 XForge 侧只用这三个名字，或日后在绑定上加一张三五行的映射表。 |
| **XOps 挂了不放行** | 关停 XOps 之后跑 `xforge approve`，必须报**连接失败可重试**（`XFG-020`）。XOps 侧一处降级都没有；**"断网时到底发生什么"由 XForge 侧的传输层决定，那件事必须实际断网测一次**。 |

## 还欠着的两条

```text
XFG-024  真实的 `xforge approve --provider xops` 端到端
XFG-020  关停 XOps 之后的断网测试
```

两条都要**一个跑起来的 XOps 服务**加**一份装好的 XForge**：本仓还没有可执行入口，
XForge 的契约治理版本也还没发布。**写在这里而不是当作通过。**
