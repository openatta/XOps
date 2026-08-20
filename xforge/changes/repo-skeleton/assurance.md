# 验证记录 — repo-skeleton

- Change revision：`contentRevision a1a405d6…`
- 实现 HEAD：`0b91fd71a7735e811b05f23f7f117c835dcb0804`
- 实现提交：`c263e88`（骨架）、`0b91fd7`（补自动化验证）

## Requirement → 实现 → 验证 映射

| Requirement / Scenario | 实现 | 验证 | 结论 |
|---|---|---|---|
| **SKEL-001** 新增包被自动纳入 | `tsconfig.json` include glob、`eslint.config.js`、`vitest.config.ts` include glob | `test/skeleton.test.ts` 前三个测试：建探针包后**注入错误**，断言 typecheck 报 `TS2322` 且指出该文件、lint 命中 `no-unused-vars`、vitest 收集到 `probe.test.ts` | 通过 |
| **SKEL-001** 布局外目录不被纳入 | `pnpm-workspace.yaml` 仅声明 `apps/*`、`packages/*` | 同文件第四个测试：建 `stray-tmp/` 后 typecheck 仍为 0，且 `pnpm ls -r` 输出不含它 | 通过 |
| **SKEL-002** 干净 clone 三条命令通过 | `package.json` scripts | Gate `unit-tests` 于 `0b91fd7` 运行 `pnpm run test` 退出 0；typecheck 与 lint 在同一 HEAD 人工执行退出 0 | 通过 |
| **SKEL-002** 类型错误使命令失败 | — | 由 SKEL-001 第一个测试同时覆盖（注入 `TS2322` 后退出非 0） | 通过 |
| **SKEL-002** 失败测试使命令失败 | — | **未自动化**。人工验证：探针加入必败测试后 `pnpm run test` 退出 1、报 `1 failed`。未固化的原因是断言它需要在测试内调用测试命令自身，构成递归 | 人工 |
| **SKEL-002** 非交互式行为一致 | — | **未自动化**。人工验证：stdin 取自 `/dev/null`、stdout 非 TTY 时三条命令均退出 0 且无交互提示 | 人工 |
| **SKEL-003** Node 版本声明 | `package.json` `engines.node = ">=20"` | `test/skeleton.test.ts` 断言下限主版本 ≥ 20 | 通过 |
| **SKEL-004** CI 执行同一套命令 | `.github/workflows/ci.yml` | **未验证** — 见下 | **未验证** |
| **SKEL-005** 不含业务内容 | 无 `apps/`、无 `packages/`、无 `dependencies` | `test/skeleton.test.ts` 断言 `dependencies` 为空；目录不存在经人工确认 | 通过 |
| **SKEL-006** 产物不入库 | `.gitignore` | `test/skeleton.test.ts` 断言忽略项齐全；人工确认安装并跑完三条命令后 `git status --porcelain` 为 0 行 | 通过 |

## 断言有效性

自动化测试通过不等于断言有意义。对三处根配置各做一次独立变异，观察是否**恰好**杀掉对应的那个测试：

| 变异 | 结果 |
|---|---|
| `tsconfig.json` 移除 `packages/**` include | 1 个测试失败 |
| `package.json` 加入一个运行时依赖 | 1 个测试失败 |
| `engines.node` 降为 `>=16` | 1 个测试失败 |

每次只杀掉一个而非全部，说明断言彼此独立且确实在区分。恢复后 7/7 通过。

## Gates

| Gate | 命令 | 退出码 | digest | gitHead |
|---|---|---|---|---|
| structure | `builtin:structure` | 0 | `1bafbafa…` | `0b91fd71` |
| unit-tests | `builtin:declared:unit-tests`（声明为 `pnpm run test`，`declaredBy: xbitshans`） | 0 | `2fd50cc6…` | `0b91fd71` |

## 未解决项

**SKEL-004 的三个场景没有任何执行证据。**

工作流文件的结构经人工核对（触发器含 `push` 与 `pull_request`、三条命令齐全、Node 矩阵 20/24），但**从未真实运行过一次**——本仓库的提交尚未推送到远端，CI 因此没有被触发过。

这不是实现缺陷，退回 apply 也解决不了：缺的是一次真实的 CI 执行环境。在拿到一次真实运行记录之前，SKEL-004 只能算作静态核对，不能算作已验证。**一个从未跑过的 CI 配置，与没有 CI 的差别小于它看起来的样子**——`pnpm/action-setup` 的版本解析、`cache: pnpm` 的可用性、矩阵在 Node 20 上的实际结果，全都只有跑一次才知道。

## 结论

除 SKEL-004 外，全部 Requirement 均已验证，两个 mandatory Gate 于当前 revision 通过。

**本次为 verify-only，未请求归档。** 是否在 SKEL-004 仍为静态核对的状态下推进关闭，属于人的决定。
