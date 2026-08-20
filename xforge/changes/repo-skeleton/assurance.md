# 验证记录 — repo-skeleton

- Change revision：`contentRevision a1a405d6…`
- 实现 HEAD：`1197042c41645c7e068a5569334d71357584589e`
- 实现提交：`c263e88`（骨架）、`0b91fd7`（补自动化验证）、`1197042`（修正 Node 下限）

## Requirement → 实现 → 验证 映射

| Requirement / Scenario | 实现 | 验证 | 结论 |
|---|---|---|---|
| **SKEL-001** 新增包被自动纳入 | `tsconfig.json` include glob、`eslint.config.js`、`vitest.config.ts` include glob | `test/skeleton.test.ts` 前三个测试：建探针包后**注入错误**，断言 typecheck 报 `TS2322` 且指出该文件、lint 命中 `no-unused-vars`、vitest 收集到 `probe.test.ts` | 通过 |
| **SKEL-001** 布局外目录不被纳入 | `pnpm-workspace.yaml` 仅声明 `apps/*`、`packages/*` | 同文件第四个测试：建 `stray-tmp/` 后 typecheck 仍为 0，且 `pnpm ls -r` 输出不含它 | 通过 |
| **SKEL-002** 干净 clone 三条命令通过 | `package.json` scripts | Gate `unit-tests` 于 `1197042` 运行 `pnpm run test` 退出 0；三条命令在 CI 的两个 Node 版本上均 success | 通过 |
| **SKEL-002** 类型错误使命令失败 | — | 由 SKEL-001 第一个测试同时覆盖（注入 `TS2322` 后退出非 0） | 通过 |
| **SKEL-002** 失败测试使命令失败 | — | **未自动化**。人工验证：探针加入必败测试后 `pnpm run test` 退出 1、报 `1 failed`。未固化的原因是断言它需要在测试内调用测试命令自身，构成递归 | 人工 |
| **SKEL-002** 非交互式行为一致 | — | **未自动化**。人工验证：stdin 取自 `/dev/null`、stdout 非 TTY 时三条命令均退出 0 且无交互提示 | 人工 |
| **SKEL-003** Node 版本声明 | `package.json` `engines.node = ">=22.13"` | `test/skeleton.test.ts` 断言下限主版本 ≥ 20，并断言 **CI 矩阵最低版本 == 声明下限** | 通过 |
| **SKEL-004** CI 执行同一套命令 | `.github/workflows/ci.yml` | GitHub Actions run `32366304635` 于 `1197042`：Node 22 与 24 两个 job 均 success，`typecheck`/`lint`/`test` 三步逐一 success；触发方式为 push | 通过 |
| **SKEL-005** 不含业务内容 | 无 `apps/`、无 `packages/`、无 `dependencies` | `test/skeleton.test.ts` 断言 `dependencies` 为空；目录不存在经人工确认 | 通过 |
| **SKEL-006** 产物不入库 | `.gitignore` | `test/skeleton.test.ts` 断言忽略项齐全；人工确认安装并跑完三条命令后 `git status --porcelain` 为 0 行 | 通过 |

## 断言有效性

自动化测试通过不等于断言有意义。对三处根配置各做一次独立变异，观察是否**恰好**杀掉对应的那个测试：

| 变异 | 结果 |
|---|---|
| `tsconfig.json` 移除 `packages/**` include | 1 个测试失败 |
| `package.json` 加入一个运行时依赖 | 1 个测试失败 |
| `engines.node` 降为 `>=16` | 1 个测试失败 |
| CI 矩阵改回 `[20, 24]` | 1 个测试失败 |

每次只杀掉一个而非全部，说明断言彼此独立且确实在区分。恢复后 8/8 通过。

## Gates

| Gate | 命令 | 退出码 | gitHead |
|---|---|---|---|
| structure | `builtin:structure` | 0 | `1197042c` |
| unit-tests | `builtin:declared:unit-tests`（声明为 `pnpm run test`，`declaredBy: xbitshans`） | 0 | `1197042c` |

**digest 不在此处抄录**，以 `evidence/verification-receipt.yaml` 为准。抄一份到这里会形成
死循环：改动本文件即改变 contentRevision，使刚记下的 digest 当场失效。

## 验证过程中发现并修复的缺陷

**首次真实 CI 运行直接推翻了一条已交付的实现。**

`packageManager` 钉的 pnpm 11.22.0 要求 Node ≥ 22.13，而 `engines.node` 当时声明
`>=20`，CI 也照此跑了 Node 20 —— 该下限**根本不是这套工具链能达到的**，job 在
`pnpm/action-setup` 一步即失败（`ERR_UNKNOWN_BUILTIN_MODULE: node:sqlite`）。
Node 20 亦已于 2026 年 4 月 EOL。

这条缺陷**任何静态核对都发现不了**：工作流文件语法正确、三条命令齐全、矩阵写法
合法，本地 Node 26 上一切正常。它只在真实运行里暴露。

修正为 `engines.node: ">=22.13"`、CI 矩阵 `[22, 24]`，并补一条测试断言
"CI 矩阵最低版本 == engines 声明下限" —— 声明的下限如果没有被 CI 真正跑到，
它就只是一句没人验证的话。

同期还修掉一个测试自身的破坏性缺陷：清理逻辑曾无条件删除整个 `packages/`，
一旦项目真有了包，跑一次测试就会把它们删掉。已改为仅在目录为空时删除。

## 结论

全部 6 条 Requirement 均已验证，两个 mandatory Gate 于当前 revision（HEAD `1197042`）通过。
SKEL-002 的两个场景（失败测试使命令失败、非交互式行为一致）为人工验证，原因已在映射表中说明。

**本次为 verify-only，未请求归档。**
