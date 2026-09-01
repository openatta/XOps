# XOps

**现在这个仓只有设计文档，没有一行实现。** 三层文档各管一件事：

```
docs/concepts-and-architecture.md   产品的根：是什么、为什么、用什么做（第 11 版）
docs/requirements/requirements.md   必须满足什么：309 条，编号按域
docs/requirements/RP-01..RP-19.md   谁跟谁一起做、动哪些 crate、做完怎么验收
docs/contracts/                     接口在上一次被批准时长什么样
```

## 接口以 docs/contracts/ 为准

三份基线：`api.md`（MCP 写入面 + 只读 HTTP）· `rust.md`（crate 接缝）· `data.md`（数据库）。

- **实现引用契约，不复述契约。** 一处能被读错，好过两处互相矛盾。
- **不要直接编辑基线文件。** 它是被批准过的记录。改接口要在
  `docs/contracts/deltas/<变更名>/<面>.md` 写一份 delta（五节齐全，无变化写 `(none)`），
  再跑 `node scripts/contracts.mjs sync` 并入基线。
- **实现与契约不一致时，改实现。** 改契约是一个需要具名人拍板的决定：
  在 `docs/contracts/DECISIONS.yaml` 记一条，四个字段齐全。
- 提交前跑 `node scripts/contracts.mjs check`，有未合并的 delta 就再跑一次 `sync`。
  **实现与基线的变化进同一次提交。**
- 细节与命名规则看 [docs/contracts/README.md](docs/contracts/README.md)，动手前读它。

## 需求与包

- **需求条目编号一条不动。** 重新拆包只改归属，不改编号——这是这个仓最贵的一笔资产。
- 条目正文只在 `requirements.md` 里有一份，包文档不复制。
- **两个包不同时改同一个 crate**；三处例外（`xops-task` · `xops-dispatch` · `xops-web`）
  在需求包总览 §4 里写死了先后。
- 没有对应需求条目、没有包归属的实现，不要写。

## 身份

台账具名身份（`DECISIONS.yaml` 的 `decidedBy`）写 **Git author `xbitsmaster`**。
一开始就写真实的，事后改会改到一条已经被引用过的决策上。

## 引擎是子模块，那个仓只读

`vendor/attacore` 是上游的仓（`D61`，固定在 `v0.2.0`）。**改那边的代码是明令禁止的**——
改动会被上游清理掉，而一次被清理掉的修改是查不出来的。需求变更**走 ISSUE 提过去**。

⚠️ **格式化用 `./scripts/fmt.sh`，不要用 `cargo fmt --all`。**
后者顺着 path 依赖走进子模块，**一次格式化了 75 个文件**（踩过）。
`[workspace] exclude` 拦不住它:那张表是给依赖解析看的，不是给 rustfmt 看的。
有一条测试盯着子模块干不干净。

## 工作方式

- 并行开工用 git worktree 时，**工作树不要建在 `/tmp` 下**——外部清理会整批抹掉它们。
- 接口没定型之前不要动实现：各面的定型点是各包自己的第一件工作包（`docs/contracts/README.md` §6）。
