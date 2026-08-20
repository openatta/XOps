---
name: xforge-upgrade-scaffold
description: 把已暂存的新版 XForge 脚手架合并进本项目自己的脚手架，保住项目的适配，并报告需要人来决定的事项；用于 `xforge upgrade-scaffold` 已暂存某个版本并写出 MERGE.md 之后。
allowed-tools: Read, Grep, Glob, Write, Edit, Bash(xforge:*)
---

# 不变量

- 先读 `xforge/scaffold-<version>/MERGE.md` 和 `plan.json`。它们已经把整个工作面说清楚，不需要靠翻脚手架去发现。
- `identical` 的文件已成定局，**不要打开它们**——计划存在的意义，就是让工作量是那几个有差异的文件，而不是那七十八个没差异的。
- `xforge/scaffold/**` 是项目的，`xforge/scaffold-<version>/**` 是发行版的。**默认谁也不压过谁。**
- `xforge/.rollback/**` 是还原点，绝不写入。
- 用 `xforge state --kind skills`（以及 `--kind rules`）读取本项目当前选中了什么，不要去解析 `xforge/manifest.yaml`。选中了什么是 CLI 报告的**已解析事实**，而那个文件只是它的输入之一。
- `manifest.scaffold.version` 跟随的是脚手架的**内容**，只有 `upgrade-scaffold --complete` 会推进它；因此「CLI 比脚手架新」是正常状态，不是故障。若 `xforge upgrade-scaffold` 因声明的 CLI 与运行的 CLI 不一致而拒绝，先跑 `xforge update`：它只动 CLI 那个版本号，脚手架的版本号仍停在文件所在的位置。
- `XFORGE_UPGRADE_VERSION_PIN_UNRELIABLE` 表示版本号声称脚手架已经是即将安装的这个版本，而文件并非如此——这是旧版 `update` 在没有合并任何东西的情况下推进了版本号留下的。起始版本已无法恢复，因此报告出的版本跨度没有意义；合并本身是按文件内容算的，不受影响。说明一次即可继续。

# 权限

- 可写 `xforge/scaffold/**`；`xforge/manifest.yaml` 仅在记录人**明确批准过**的选择时可写。
- 不得触碰 `xforge/changes/**`、`xforge/specs/**`、审计链、审批、`xforge/constitution.md`、`xforge/architecture.md`。脚手架可以重新生成，治理记录不能——一条能被重建的审计链，本来就不值得保留。
- **绝不删除 `project-only` 文件。** 没有任何依据能区分"上游删掉的资产"和"本项目自己写的资产"，按前一种理解去删，就是凭猜测销毁别人的工作。

# 执行

1. 每个 `added` 文件：逐字拷入。**不要**把它加进 Manifest 选择——文件随发行版到达，不等于决定要运行它。
2. 每个 `changed` 文件：两份都读。吸收新版**规定**的东西，保住本项目**知道**的东西。一个带着真实测试命令的 Gate、一段本项目选定的 Skill 措辞、一个有人调过的阈值——那些是关于这个项目的事实，要活过升级。
3. 英文与 `_cn` 两份 Skill 必须保持等价。只合并一种语言，会让项目留下两份互相矛盾的 Skill，而 Agent 读到哪一份就变成了 Manifest 语言设置的问题，而不是项目决定的问题。
4. 运行 `xforge upgrade-scaffold --complete`，然后 `xforge install`，然后 `xforge doctor`。

# 证据

- 逐个 `changed` 文件报告你取了哪一边、为什么，一行即可。"采纳上游"和"保留本项目的"都是答案；**不报告的合并不是答案。**
- 列出计划中标为"随发行版到达但未被选中"的每一项资产，并明说选不选是**用户**的决定，不是你的。
- 逐字引用 `xforge upgrade-scaffold --complete` 的采纳计数。它报告的是"计划中有多少个文件现在与发行版一致"，**它不给合并打分**；把它复述成一个评分，就是替 CLI 编造了一个它没有做出的判断。

# 停止与返工

- 当某个 `changed` 文件的两份内容**不可能同时成立**时停下——比如新版删掉了本项目依赖的某条规则，或改名了本项目引用的东西。那是关于这个项目的决定，不是关于合并的。
- 宁可停下，也不要靠"整份采纳新版"来化解冲突。**偏向上游是唯一永远可用、而几乎永远不对的解法**；项目就是这样悄悄失去那些脚手架本来就邀请它做的适配的。
- 当暂存目录不存在、或它的 `plan.json` 无法解析时停下：去运行 `xforge upgrade-scaffold`，而不是靠翻目录把计划重建出来。

# 判断要点

- 只是措辞不同的文件，同样值得问一句。上游重写一段 Skill 正文，往往正是因为旧措辞把 Agent 带偏了——所以"意思一样"恰恰是这次重写要反驳的那个说法。
- **选择与内容是两个决定，而改变行为的是前者。** 把 `xforge-architect` 拷进来什么也没改变；把它加进 `scaffold.skills`，改变的是项目里每一个 Agent 被告知要做什么。把文件带进来，把选择报上去。
- 没有冲突的合并是正常结果，不是可疑结果。多数发行版改动的文件是没有项目动过的；为了显得尽责而编造一个难点，只会把读者的注意力从真正要紧的那一个上面挪开。
