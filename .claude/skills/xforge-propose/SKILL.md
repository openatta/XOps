---
name: xforge-propose
description: 创建受治理的 Change，并仅生成 Propose Stage 允许的 change.yaml、proposal 与 delta Specs；用于用户要求把已足够明确的想法、缺陷或功能正式规格化，但尚未授权实现时。
allowed-tools: Read, Grep, Glob, Write, Edit, Bash(xforge:*)
---

# 不变量

- 先运行 `xforge state`，从 State 读取 Changes 路径、Flows、policy、Constitution、Rules、Specs 和项目模块。
- 只消费 `xforge-propose` 对应的 ready Action；每次写入前重读 Action inputs，写入后刷新 State。
- Flow 表达交付侧重点与治理量级：Quick 强调快速，限低风险、单模块、易回滚且无关键影响；Solid 强调稳定，适合常规产品与工程变更；Major 强调重大影响治理，用于高风险、跨系统或关键影响变更。不确定时升级或请求决定。
- Constitution 台账在 Check（实现之前）回答，而只有 Solid 与 Major 有 Check Stage。Quick 不带台账：它面向的是琐碎、单模块、低风险的工作，对一个「其全部意义就在于不承载原则级风险」的 Change 要求逐条原则认证并无所得。这不是绕开 Constitution 的路径——它照样约束着工作，而且 Quick 的 eligibility（`risk: low`、单模块、禁止 critical impact）由 CLI 强制，因此把一个真正需要台账的 Change 塞进 Quick，等于声明一份与事实不符的 classification。
- Specs 使用机器约定的 `ADDED|MODIFIED|REMOVED|RENAMED Requirements`、`Requirement`、`Scenario`、`WHEN`、`THEN` 标题。

# 权限

- 可以在 State 解析的 Changes 目录创建一个 kebab-case Change ID，写 `change.yaml` 以及 Propose Action 明确返回的 Proposal/delta Spec 路径。
- 不得写 Design、Clarifications、Check report、长期 Tasks、产品代码、主 Specs、Evidence 或 Archive。
- 不得替用户决定材料性兼容、数据、安全、隐私或范围问题。

# 执行

0. **想法仍然模糊时，先把它收敛，再创建任何东西。** 读代码、Spec 与约束，直到能陈述一个目标、它的边界、以及"做完"的判据。**调查本身不需要 Skill**，用普通的阅读与检索即可。这一步欠用户的是一个结论：要么给出一个有边界的目标，要么明确报告这个想法尚不可分离成一个目标。**不要为一个还界定不了的想法创建 Change**——一个没有边界的 Change，拆解它的代价远高于问一个问题的代价。
1. 解析唯一目标；若要新建 Change，检查是否已有覆盖同一问题的 active Change。
2. 将 `flow` 设为 State 解析出的 manifest 默认值，除非用户明确要求使用其他 Flow。仅当 classification（risk/security/privacy/publicApi/dataMigration）与该默认值明显冲突时才可主动偏离——此时应升级或请求决定，而不是静默改写。完整填写 classification、modules 和有边界的项目相对 path scope；仅当 Flow 被覆盖或被升级处理时才在 Proposal 中说明 Flow 选择，单纯继承默认值时无需说明。
3. 创建最小 `change.yaml` 后运行 `xforge state --change <id>`；该文件使用下列无包装对象结构，字段名和层级必须保持一致，再按项目事实替换值：

   ```yaml
   flow: solid
   classification:
     risk: medium
     security: false
     privacy: false
     publicApi: false
     dataMigration: false
   scope:
     modules: [root]
     paths: [src/**]
   ```

   只处理 State 返回给 Propose 的 ready Artifact/Action；Schema 诊断未清零前不得继续写 Artifact。
4. 从磁盘重读依赖，写 Why、Scope、Non-goals、Actors、Success criteria，并生成带稳定 Requirement ID 的成功、失败、边界和兼容性场景。不可把来源未声明的精确契约猜测写成规范事实；已有不可修改的验收测试定义了字段、输出形状或退出行为时必须逐项保持一致，测试与需求冲突则作为材料性歧义停止。
5. 每完成一个 Artifact 都刷新 State；当下一 Action 属于 Clarify、Design、Apply 或其他 Skill 时停止。
6. 运行 `xforge check --change <id>`，修复本 Stage 的结构问题，不把提示性文本称为已通过 Gate；只在 CLI 返回 ready Transition 时调用 `xforge transition --change <id> --to <stage>`。

# 证据

- 按 Action 的 `doneWhen` 与 `requiredEvidence` 报告 Change ID、Flow（默认值或被覆盖，被覆盖时说明原因）/classification、实际文件路径、假设和下一合法 Action。
- 只有当前 CLI 输出能证明结构、policy 与路径校验结果。

# 停止与返工

- 在未知模块、路径/身份/协议诊断、材料性歧义、Flow policy 不满足或权限边界处停止。
- 上游事实改变时交给 `xforge-revise`；不要在本 Skill 中顺便实现。

# 判断要点

- Flow 默认值存在的意义就是让常见情况不需要专门做风险分类推理；覆盖默认值才是例外路径，覆盖了却不在 Proposal 里说明，在后来者看来会像是疏漏，而不是一次深思熟虑的决定。
- 一条需求对作者本人读起来很清楚，但只有掌握了未写明的实现细节才能理解，对别人来说就不算可测试。要写出一个完全不了解这个 Change 背景的评审者，仍能对照实际运行系统去验证的场景。
