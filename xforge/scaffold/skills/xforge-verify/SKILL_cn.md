---
name: xforge-verify
description: 用当前证据核验 Change 的完整性、正确性、一致性与 Gates，并在用户明确授权时预览后归档；用于验收 readiness、验证并关闭 Change，或归档一个已有当前验证回执的 Change。
allowed-tools: Read, Grep, Glob, Write, Edit, Bash(xforge:*)
---

# 不变量

- 先运行 `xforge state --change <id>`，解析用户意图为 `verify-only`、`verify-and-archive` 或 `archive-current`；没有明确归档授权时只验证。
- 重读当前 revision 的 Proposal、delta/main Specs、可选 Clarifications/Design/Check report、实现 diff、工作包/deliveries、Constitution、Rules 和 Gates。
- 默认不修产品代码，不手写或篡改 Gate Evidence；实现变化会使旧验证回执失效。
- Archive 是独立的 `archive-write` 协议动作，不代表 deploy/release 权限。
- Reviewer/Agent 只能形成 assurance，不能签发 Approval；Machine Gate 只接受 CLI runner 生成并绑定当前 revision 的 Evidence。

# 权限

- 可写 Verify Stage `produces` 的 Artifact——assurance——以及 `evidence/verification-receipt.yaml`，后者是本 Stage 的 exit condition 而不是 Artifact。Gate Evidence（`evidence/*.json`）只能由 `xforge check` 生成；receipt 只引用这些 digest，不得把它们改写成自己的结论。
- 只有 `verify-and-archive` 或 `archive-current` 的明确用户授权允许调用 `xforge archive`；先 dry-run，再执行原子同步与移动。
- 失败时只报告并返回 Apply rework；除非用户另行明确授权，不修改实现。

# 执行

1. 解析唯一 Change 和模式；若 `archive-current` 的 receipt 不属于当前 revision/Git HEAD/Flow/Gate versions，先重新 Verify。
2. 按完整性、正确性和一致性审查：把每个 Requirement/Scenario 映射到实现与自动化测试，把 Design/Constitution/Rules 映射到最终 diff。
3. 若存在工作包，要求每个包有有效完成 delivery，核对依赖 commit、实际写入边界、验证命令，并确认每项 `done_when` 都有精确一次的非空证据映射；高风险或跨系统结果使用独立 Reviewer。Reviewer 只读，无法自行写证据文件——必须由你逐字转录它的结论后再确认（见 `xforge-apply` 第 8 步）。
4. 运行 `xforge check --change <id>`，重新执行工作包验证和所有 mandatory Gates；重开 Evidence，核对 Change、命令、时间、退出状态、digest 与当前 revision。
5. 生成 assurance。然后写 verification receipt——必须在第 4 步的 Gate 全部通过**之后**再写，绝不能提前，因为它要引用那次运行产生的 digest。`evidence/verification-receipt.yaml` 不是内容 Artifact，而是本 Stage 的 `verificationReceipt` exit condition，由 CLI 对照磁盘上的 Evidence 判定：

   ```yaml
   status: passed
   contentRevision: <取自 `xforge state --change <id>` 的 governance.revision.contentRevision>
   gitHead: <Gate Evidence 上记录的 gitHead，不是当前 HEAD>
   gates:
     - gate: unit-tests
       evidence: <evidence/tests.json 的 digest 字段>
       status: passed
   ```

   本 Stage 每个通过的 Gate 都要引用一次，且必须是当前 digest——不得遗漏、不得引用其它 Stage 的 Gate、不得使用早先运行的 digest。`gates` 只放 Gate；work-package 交付写在 `workPackageDeliveries`（`package`、`delivery`、`dispatch`、`status`、`verifyCommand`、`exitCode`），写成 `gates` 的一行会被以 `gate-unverifiable-<name>` 拒绝。之后若再改动任何 Artifact，必须重跑 Gate 并重写 receipt。任一 mandatory Gate、Requirement 或关键约束未验证时请求 `apply` rework Transition；不得手写 Gate PASS。
6. Gate 和 Artifact 满足后调用 `xforge transition --change <id> --to ready-to-archive`；`verify-only` 到此停止，并报告 Closing Approval 与 Audit blockers。
7. 已获当前 revision 的人类/外部 Closing Approval 后运行 `xforge audit verify --change <id>` 和 `xforge archive --change <id> --dry-run`，展示完整 Specs merge/move 计划、冲突和显著兼容影响；仅在 Approval、Audit、Gate 全部当前且计划无错误时运行 `xforge archive --change <id>`。
8. 归档后运行 `xforge state`，确认 Change 离开 active set、主 Specs 可见且 Evidence 位于归档目录。

# 证据

- 输出 Requirement/Scenario、实现、测试、Design、工作包和 Gate 的可定位映射，以及 receipt 的 `contentRevision`、`gitHead` 和它引用的 Gate digest。
- 只有所有当前 mandatory Gate 成功且没有 blocker 时，才能声明 ready for archive；只有 CLI 原子事务成功才能声明 closed。
- 在关闭审批之前，运行 `xforge brief --change <id> --text` 并把输出**逐字**交给用户。不得转述、重排或概括：简报把 CLI 算出的事实与原文引用分开呈现，用你自己的话复述会毁掉读者区分二者的唯一依据。

# 停止与返工

- 在不完整实现、失败 Gate、无效 delivery、stale receipt、Spec 冲突、路径安全问题、目标碰撞或未授权归档时停止。
- 在 approval provider 配置失败（`XFORGE_APPROVAL_PROVIDER_FORBIDDEN`、`XFORGE_APPROVAL_MCP_SERVER_MISSING`、`XFORGE_APPROVAL_MCP_TOKEN_MISSING`、`XFORGE_APPROVAL_MCP_CONNECTION_FAILED`）时停止：provider 未配置，不是决定仍在等待。告知用户配置其 McpServer 与 token（见 `scaffold/mcp-servers/`），或改在终端本地审批；绝不对同一个 provider 反复重试。
- 审批命令一律从 `state.nextActions[].command` 里取，不要照 usage 字符串自己拼。`--for` 填的是该审批所解锁的那次 transition——Flow 里的 Stage id，绝不是 `stage` 这类字面词；填错过去会把真实的人类签字消耗在一份不会被计数的 receipt 上。`XFORGE_APPROVAL_TRANSITION_UNKNOWN` 与 `XFORGE_APPROVAL_TRANSITION_UNAPPROVABLE` 表示参数错了、且什么都没写入：改参数，不要重跑，更不要再请人签一次。`xforge approve ... --dry-run` 不需要终端、也不惊动审批人，就能把这些先校验一遍。
- 归档时出现 `audit:remote-pending` 要停止：远端 audit 投递被设为 required，而 `XFORGE_AUDIT_ENDPOINT` 未设置或不可达，`audit retry` 没有可投递的去处。应告知用户配置该 endpoint（以及 token/HMAC 环境变量），或不再对该 assurance level 要求远端投递；绝不反复重试。
- Verify 失败返回 Apply；governing artifact 自相矛盾时按 State 的 `reworkTo` 返回更早 Stage，**经由 `xforge-revise`**——它会一致地修订受影响的 Artifact，并让 digest 链使依赖它们的 Evidence 失效。直接改 governing artifact 会让 Change 的其余部分静默地与它不一致。

# 判断要点

- 一个 mandatory Gate 通过是可以归档的必要条件，不是充分条件——Gate 只检查它被写来检查的那件事。一条 Requirement 可以拥有完整测试覆盖、Gate 也是绿的，但测试断言的其实是错的行为；要核对 Scenario 的意图和测试实际检查的内容是否一致，不能只看它跑过且退出码为零。
- 一份每条 `done_when` 都填了引用的 `done_when_evidence` 映射看起来像证据，但引用本身可能和它要证明的结论无关——比如一条日志只证明某个函数执行过，不证明它产出了正确结果。接受这份映射之前要看引用实际展示了什么，不能只看每一项是不是都填了。
