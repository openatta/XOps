---
name: xforge-check
description: 对 Major Change 做实现前跨 Artifact 语义审查，检查完整性、一致性、可测试性、风险与可实施性；用于 State 返回 ready Check Action 或 Major 规划需要正式质量门时。
allowed-tools: Read, Grep, Glob, Write, Edit, Bash(xforge:*)
---

# 不变量

- 先运行 `xforge state --change <id>`，只消费当前 revision 的 ready Check Action，重读 Proposal、Specs、Clarifications、Design、Constitution、Rules 与代码事实。
- `xforge-check` 做语义审查；`xforge check` 提供 schema、路径、Gate 和 Evidence 的确定性输入，二者不能互相替代。
- 默认只读 governing artifacts；发现问题时报告 rework，不在审查中悄悄改写上游。
- Check report 是 LLM Review Evidence，不是 Gate Evidence；即使写出 `PASS` 也不能通过 Machine Gate、Transition 或 Approval。
- Gate Evidence 绑定 Gate 运行当刻的 content revision。**必须在最后一次写入之后**、一次性运行 Gate。先跑一个 Gate、再改 Artifact、再跑下一个，会让先跑的 Gate 变陈旧：所有 Gate 都报 `passed`，Stage 却仍然出不去。

# 权限

- 只可写 Check Stage `produces` 的三个 Artifact：`check-report.md`、`evidence/check-findings.yaml` 和 `evidence/constitution-check.yaml`。两个台账都由 Agent 撰写——没有任何 CLI 命令会生成它们，而 Stage 缺少它们就无法退出。
- 不得写产品代码、Proposal/Specs/Clarifications/Design、工作包或 Archive。
- "Gate Evidence" 指只由 `xforge check` 写入的 `evidence/*.json`（`structure.json`、`check-findings.json`、`constitution-check.json` 等），绝不手写或修改。上面两个 YAML 台账是 Gate 读取的 Artifact，不是 Gate Evidence。

# 执行

1. 检查 Proposal/Specs 是否完整、明确、可测试，关键问题是否 resolved。
2. 检查 Design 是否覆盖所有 Requirement、约束、trust boundaries、失败场景、兼容性、迁移和回滚。
3. 核对测试、rollout、monitoring、stop signals、owner、path scope、依赖与并行边界是否匹配重大影响。
4. 运行 `xforge check --change <id>`，把确定性诊断作为证据输入。
5. 写 `evidence/check-findings.yaml`：按 blocker、warning、suggestion 记录每项 finding，指出 Artifact/Requirement 位置、原因、`refs`，blocker 未解决时还要写 `reworkTo` Stage；审查没有发现问题时写出显式空列表。标记为 `resolved` 的 blocker 必须写 `resolvedBy`，且必须是本 Change 某份 receipt 上的审批人或它的某个 Git author——与下面 `approvedBy` 同一条标准，因为无人认领的「已解决」不算解决。要清楚这条校验能做什么、不能做什么：当 Change 尚无提交也无 receipt 时，没有任何东西可供比对，于是任何名字都通过，Gate 会给出一条 warning 说明这一点。**那次通过是暂时的**：Change 的第一次提交建立了这个集合，此后同样的名字如果对不上就会让刚刚还是绿色的 Gate 失败，而那时本报告已经写完。所以一开始就写真实身份，不要写一个打算以后再改的。
6. 写 `evidence/constitution-check.yaml`：按文档顺序为 `xforge/constitution.md` 的每个 `## ` 标题写一条，`status` 为 `compliant`、`violation` 或 `not-applicable`，并至少给出一条机器可定位的 `references`——本 Change delta Specs 中的 Requirement id、真实存在的路径，或 `gate:<name>`（该 Change 已有通过的 Gate Evidence）。「真实存在的路径」指仓库里任意路径：先按 Change 相对解析，再按项目相对解析——`xforge/constitution.md` 和 `xforge/architecture.md` 都是合法引用，而且对架构类与治理类原则往往正是最恰当的引用。不要把自己限制在 Change 目录内的路径。`violation` 还需要 `justification` 和具名 `approvedBy`（必须是真实审批人或 Git author；该 Change 已有 approval receipt 时必须是 receipt 上的人）；`not-applicable` 需要 `justification`。只写 `compliant` 而不引用任何东西，正是本 Gate 要拒绝的笼统声明；approval receipt 也不能顶替：receipt 记录的是有人批准了某次 transition，而不是本 Change 为何满足该原则，因此只引用 receipt 会被拒绝。这一点在治理原则上最容易踩到——那里 receipt 是最顺手的证据——应当引用本 Change 实际做过的事（材料性问题台账、Clarifications、某个 Requirement id），要附上 receipt 可以放在它们旁边。每条 `justification` 都用块标量书写（`justification: >-`，正文缩进另起一行）：普通标量遇到「冒号加空格」或以 `[`、`{` 开头即失效。
7. 在 `check-report.md` 与两个台账都写完之后，再运行一次 `xforge check --change <id>`，它会对最终内容重新运行并刷新当前 Stage 的整个 Gate 集合；`--all-gates` 还会运行 Change 尚未到达的 Stage 所属的 Gate，那些 Gate 不可能通过，Stage 中途通常不需要这样做。
8. 刷新 State；有 blocker 时请求 State 指定的 rework Transition；无 blocker 时仍由 CLI Gate 与 Approval 决定是否可运行 `xforge transition --change <id> --to apply`。

# 证据

- 报告跨 Artifact 映射、CLI 检查结果、未覆盖 Requirement/风险和可实施性结论。
- 只有 blocker 为零且 Action `doneWhen` 满足时才能声明 Check satisfied。
- 在放行实现的那次审批之前，运行 `xforge brief --change <id> --text` 并把输出**逐字**交给用户。不得转述、重排或概括：简报把 CLI 算出的事实与原文引用分开呈现，用你自己的话复述会毁掉读者区分二者的唯一依据。其 reconciliation 条目陈述的是本 Stage 自己的账本与文件之间的差异——回应它们，不要与它们争辩。

# 停止与返工

- 在材料性遗漏、矛盾、范围漂移、不可测试 Requirement、缺少 rollback 或路径/owner 冲突时停止。
- 按最早受影响点返回 Propose、Clarify 或 Design，**经由 `xforge-revise`**——它是修改上游 Artifact 的正规路径：一致地修订受影响的 Artifact，并让 digest 链使依赖它们的 Evidence 失效。直接改上游 Artifact 会让 Change 的其余部分静默地与它不一致。
- 不检查不存在的长期任务计划。

# 判断要点

- "评审通过"和"CLI Gate 是绿的"是两句不同的话。一份 Design 完全可以内部自洽、写得很好，却因为某条 Requirement 完全没有测试策略而在 Check 里不通过——单个 Artifact 内部一致，不代表所有 Artifact 之间彼此覆盖。
- 缺失的反面场景（失败路径、边界条件、兼容性破坏）最容易被漏掉，因为一份看起来干净的 Design 里，没有任何东西会主动指出"这里本该有、但没有"。要检查的是本该存在却不存在的东西，不只是已经存在但写错的东西。
