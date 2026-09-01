# crate 接缝

## Purpose

crate 与 crate 之间**已定型的 trait 与读模型**。需求包总览 §3 第 2 条"接缝先于实现"
说的就是这份记录：一个包依赖另一个包时，依赖的是这里的元素，不是它的内部结构。

**crate 内部的类型不进这里。** 判据是：换一个实现进去，上层要不要改。要改的才是接缝。

## 谁往这里加元素

| 前缀 | 归属 | 硬验收 |
|---|---|---|
| `rust:xops-core#*` `rust:xops-store#*` | RP-01 | 换一个内存实现进去，写入路径与它上面的一切不改一行（`CON-012`） |
| `rust:xops-exec#*` | RP-07 WP-A | 换成桩引擎不改契约（`EXE-014`）；契约里不出现引擎的任何类型 |
| `rust:xops-identity#*` `rust:xops-audit#*` | RP-02 | |
| `rust:xops-table#*` | RP-04 | |
| `rust:xops-read#*` | RP-05 | 读模型是前端唯一能看见的东西；它同时以 `api:http.*` 的形态出现在 api.md |
| `rust:xops-repo#*` | RP-08 | |
| `rust:xops-skill#*` `rust:xops-task#*` `rust:xops-dispatch#*` | RP-09 · RP-10 · RP-11 · RP-12 · RP-13 | |
| `rust:xops-flow#*` | RP-14 WP-C | RP-15 只经它驱动迁移，**不得自己改 `_flows` / `_flow_nodes`** |
| `rust:xops-settle#*` `rust:xops-script#*` | RP-15 · RP-16 | |
| `rust:xops-notice#*` `rust:xops-template#*` `rust:xops-xforge#*` | RP-17 · RP-18 · RP-19 | |

## 命名

```
rust:<crate>#<路径>        例：rust:xops-store#Store::put
                                rust:xops-exec#ExecContract::submit
                                rust:xops-flow#Instance::settle
```

方言文件（有实现之后）：`rust/<crate>.api.txt`，由 `cargo-public-api` 生成。
**手写 trait 签名只是第一阶段**——代码一落地就切成快照当权威，因为手写的接口文档必然漂移。

## Elements

(none)
