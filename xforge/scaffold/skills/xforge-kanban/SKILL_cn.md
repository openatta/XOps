---
name: xforge-kanban
description: 仅根据 Git 提交元数据,为当前仓库生成活跃度看板——按贡献者统计活跃度与代码量、提交时段热力图,以及 feature/fix/其它分类;用于用户要求项目看板、贡献报告、活跃度热力图或提交历史汇总时。
allowed-tools: Read, Grep, Glob, Write, Bash(git:*), Bash(node:*)
---

# 不变量

- 本 Skill 是只读的项目报告能力,独立于 Change/Flow/Gate 生命周期状态;绝不得为它查询带 `--change <id>` 的 `xforge state`,也不得涉及 `xforge/changes`、`xforge/specs`、Evidence 或 Approval。内置脚本可以调用不带 `--change` 的 `xforge state` 来读取 `project.modules` 做分组,读不到时会退化为单一隐式模块——这是项目结构查询,不是 Change/Flow/Gate 治理。
- 以 `git log`(以及用于模块分组的、来自 `xforge state` 的 `project.modules`)作为唯一事实来源。绝不编造脚本产出之外的提交、作者、日期、计数或模块边界。
- 运行 `scripts/git-activity.mjs` 提取数据;不得凭部分 `git log` 输出或记忆手工计数。
- 按邮箱而非显示名对贡献者分组——同一个人可能用不同姓名提交(脚本已处理此逻辑,不要自行按姓名重新分组)。
- 只有提交标题中出现明确的 Conventional Commits 类型前缀(如 `feat:`、`fix(scope):`)才归为 `feat`/`fix`;没有可识别前缀的提交归为 `unclassified`,不得从 diff 或正文猜测意图。
- 浅克隆或加了 `--since`/`--until`/`--author` 过滤条件时,历史覆盖是不完整的;必须明确说明,不得把局部数据当作完整数据呈现。
- 有些项目需要 Git 之外的信息(例如通过 MCP 把提交关联到 issue)。这属于对本 Skill 脚本的项目本地扩展,不属于本 Skill 的不变量——见"停止与返工"。

# 权限

- 唯一允许的动作:运行内置脚本(只读,脚本本身不写入任何内容);只有用户明确要求保存副本时,才把渲染后的报告写到项目内、且不受版本控制的路径。
- 不得因为本 Skill 而 commit、push、修改历史,或写入任何被 Git 跟踪的文件。
- 不得为了让报告"好看"而改写、过滤或"清理" Git 历史。

# 执行

1. 确认当前目录在 Git 仓库内(`git rev-parse --is-inside-work-tree`)。不是,或 `git` 不可用时,停止并说明原因。
2. 在本 Skill 目录下运行 `node scripts/git-activity.mjs [--root <path>] [--since <date>] [--until <date>] [--author <pattern>]`,解析其 JSON 标准输出。用户要求的时间/作者范围要原样透传给脚本,不要事后自己再过滤。
3. 脚本非零退出,或返回 `shallow: true` 时,必须先原样呈现这个情况——浅克隆会低估历史,用户需要在信任这些数字之前知道这一点。
4. 把 JSON 渲染成 Markdown 看板:
   - 贡献者表格:提交数、增/删行数、活跃天数、首末次提交日期(每个邮箱一行;同一邮箱对应多个显示名时在同一行内列出);
   - 从 `activity` 直方图生成一个紧凑的"星期 × 小时"活跃度热力图(文本/emoji 网格或 Markdown 表格,选当前输出场景下表现最好的形式);
   - 从 `typeBreakdown` 生成 `feat`/`fix`/其它的分类汇总,把每类下的提交标题也列出来,让用户能看到每个 feature/fix 具体是什么,而不只是个数字;
   - `modules` 数组超过一项时,按模块各生成一段(结构与上面全局部分相同,只是范围收窄到该模块),避免 monorepo 的活跃度被拍扁成一个失真的全局排名;`modules` 只有一项时跳过这部分——全局数字已经覆盖了它,重复展示没有意义;
   - 存在多个模块时,还要把 `unscoped`(不落在任何已声明模块路径下的活动,例如根目录文档或 CI 配置)和 `crossModuleCommits`(改动跨越多个模块的提交)各自单独列一小节;不得把它们的行数悄悄并入某个模块的合计里。
5. 默认直接在回复中展示看板。只有用户明确要求保存副本时才写文件,且要写到项目内、不受版本控制的路径下(例如 `.xforge-kanban/<name>.md`),并提醒用户在提交前确认该路径已被 `.gitignore` 排除。

# 证据

- 报告脚本输出中的确切提交范围(`range.from`–`range.to`)、提交总数和 `shallow` 标记。
- 报告 `moduleResolution`:明确说明模块分组是来自项目自己的 `project.modules`(`xforge-state`),还是因为 XForge 不可用或当前不是 XForge 托管项目而退化成的单一隐式模块(`implicit-root`)。
- 所有数字都必须原样照搬脚本输出,不得四舍五入、估算或外推。

# 停止与返工

- 当前目录不是 Git 仓库、`git` 不可用,或脚本报错时必须停止——不得退回到凭文件列表或记忆猜测。
- 写入任何文件前必须先停下来征求用户同意。
- 用户想要脚本产生不了的数据(issue 关联、PR 元数据、不同的分类方式、额外的 MCP 信号来源)时,不得编造。引导用户使用 `xforge-scaffold` 在自己项目的副本里扩展本 Skill 的私有脚本 `scripts/git-activity.mjs`——本 Skill 特意保留在 scaffold 中,就是为了让每个项目都能按需定制。
