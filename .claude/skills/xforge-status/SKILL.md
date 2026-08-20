---
name: xforge-status
description: 把 xforge state 的机器状态解释为可读进度——既可以是"在飞的 Change 有哪些、各自到了哪个 Stage"的全局视图，也可以是单个 Change 的详情；用于用户询问在飞情况、做到哪、为何阻塞、剩余工作包、Evidence 是否当前或能否 Verify/Archive 时。
allowed-tools: Read, Grep, Glob, Bash(xforge:*)
---

# 不变量

- 全局视图运行 `xforge state`；单个 Change 详情运行 `xforge state --change <id>`。State 是唯一状态事实源。
- 在飞清单从 `activeChanges` 读取，它已排除归档 Change。**不得自己遍历 `xforge/changes` 重建这份清单**，也不得从目录内容、提交信息或会话记忆推断 Stage——记录在案的 Stage 只有 State 报告的那个。
- Flow 或 Stage 为 null 的 Change 表示解析失败，**必须作为"无法解析"报出来，不得省略**——加载不了的 Change 恰恰最需要被注意，静默丢掉它比露出一个缺口更糟。
- 严格只读，不维护第二份进度，不顺便继续、修复或勾选任务。

# 权限

- 可以查询、筛选和解释 State、在飞 Change 全局清单、work packages、deliveries、diagnostics 与 Evidence freshness。
- 不得修改任何项目文件、生成 Evidence、执行 ready Action 或归档。

# 执行

1. **全局视图（未指定 Change 时的默认）。** 按 `activeChanges` 报出每个在飞 Change 的 id、Flow、当前 Stage 与 risk；按 Stage 排序使最接近完成的排在前面，并给出总数。清单为空时直接说明——**空清单是一个答案，不是一次失败**。
2. **单 Change 视图。** 解析 Change ID（归属不唯一时请求用户选择），输出 Flow、当前 Stage/state revision、ready/blocked Transitions、pending Approvals、Rule 的 instructed/guarded/verified/approved/uncovered coverage、Policy/Hook active coverage、Audit chain/remote pending/gaps、工作包/deliveries、Evidence freshness、Verify/Archive readiness。
3. 给出下一合法 Action、对应 Skill 和为何尚未 ready。**只报出 Skill 名称，不得代为执行**——报告就绪与迈出这一步是两种不同的权限。
4. Requirement ID 确定性索引不可用时明确标记为启发式，不从 Markdown 搜索结果过度推断状态。

# 证据

- 所有进度结论引用同一次 State revision 与具体诊断/Evidence 路径。
- 当用户问的是某个 Change 该不该通过当前审批、而不是它进行到哪一步时，运行 `xforge brief --change <id> --text` 并逐字返回其输出。它回答的是另一个问题，而且回答得更好。

# 停止与返工

- ID 歧义、State 错误或 Evidence 无法验证时停止并说明缺失信息；不得用会话记忆补齐。
