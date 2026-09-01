# XOps

> 一个给 agent 用的运营平台：**写入只经 MCP，看的那一面只读**。

**状态：`0.1.0`,内部预览。** 能跑起来、有日志、停得干净;
但**执行是裸跑的、默认引擎是桩**——先读[这个能拿它做什么](#这个能拿它做什么)。

## 它是什么

```text
表      用户经 MCP 建的命名表,11 种列类型,平台一张业务表都不预置
技能    一段提示词 + 一份声明。发布前必须成功测试过一次
任务    技能 + 输入 + 触发方式。手动、定时、订阅事件
流程    结算表上的表态推动实例前进。七条判定各挡各的
插件    技能生成的 JS,跑在 QuickJS 里,**能力默认为零**
看板    一张表的一个视图。**平台不内建任何报表**
通知    `_notices` 上属于我的那些行。没有渠道、没有重试
```

两个服务面,**各监听各的端口**:

| | |
|---|---|
| **MCP 写入面** `POST /mcp` | **唯一的写入通道**。67 个 tool |
| **只读 Web 面** | 后端**结构性地不存在写业务对象的路由**——不是"有但不给用" |

## 跑起来

```bash
git submodule update --init   # AttaCore（执行引擎，嵌进来的）
cargo build --release

# 生成一把加密密钥。**它没有默认值**——写死的默认密钥看起来是加密的,实际不是
export $(./target/release/xopsd --generate-key)

export XOPS_DB=/var/lib/xops/xops.db
./target/release/xopsd --check   # 装配一遍、打印横幅、不监听
./target/release/xopsd
```

| 环境变量 | |
|---|---|
| `XOPS_SECRET_KEY` | **必填**。只读仓凭据与插件配置的加密密钥 |
| `XOPS_DB` | 数据库路径,默认 `:memory:`(进程退出就没了) |
| `XOPS_MCP_ADDR` | 默认 `127.0.0.1:8765` |
| `XOPS_WEB_ADDR` | 默认 `127.0.0.1:8766` |
| `XOPS_ASSETS` | 前端产物目录。不给就用嵌进二进制的那一份 |
| `XOPS_MODEL_KEY` | 模型 API key。**不给就跑桩引擎** |
| `XOPS_MODEL` | 默认模型,默认 `claude-sonnet-4-6` |
| `XOPS_MODEL_BASE_URL` | 模型服务地址(兼容 Anthropic Messages 的任何一个) |
| `XOPS_LOG` | `off`/`error`/`warn`/`info`/`debug`,默认 `info` |

**存活探针**:`GET /healthz` → `{"status":"ok"}`。不认证、不查库、**不带任何信息**。

**限流不做**(`MCP-015`):交给部署侧的反向代理。这是明写的不做。

**备份**要连 `-wal` 与 `-shm` 两个附属文件一起,或者先跑一次 checkpoint。

## 这个能拿它做什么

> ⚠️ **执行是裸跑的**(`D58`)。技能**直接在宿主上跑**,没有容器:
> 没有网络白名单强制、没有资源上限、两次执行互相看得见文件系统。
> 启动横幅会把没兑现的八条逐条列出来——**它不静默降级**。
>
> **所以:只跑你自己写的、或者你完全信任的技能。**

| | |
|---|---|
| 你自己 + 几个人,技能都是自己写的,单机 | 可以 |
| 让别人往里放技能 | **不行**——那是在你的宿主上跑他的代码 |
| 生产 | **不行** |

**默认引擎是桩**:不设 `XOPS_MODEL_KEY` 就是 `StubEngine`——跑得通,什么也没真跑。

**引擎是嵌进来的**(`D61`):AttaCore 以 git 子模块固定在 `v0.2.0`,**一个进程**,
没有 `attacored`。克隆之后要 `git submodule update --init`。

## 设计文档

三层各管一件事,**接口以 `docs/contracts/` 为准**:

```text
docs/concepts-and-architecture.md   是什么、为什么、用什么做
docs/requirements/requirements.md   必须满足什么:309 条,编号按域
docs/requirements/RP-01..RP-19.md   谁跟谁一起做、动哪些 crate、怎么验收
docs/contracts/                     接口在上一次被批准时长什么样
```

改接口要先写 delta,再 `node scripts/contracts.mjs sync` 并入基线。
细节见 [`docs/contracts/README.md`](docs/contracts/README.md)。

## 开发

```bash
cargo test --workspace          # 656 个测试
cargo clippy --all-targets      # -D all
./scripts/fmt.sh                # ⚠️ 不要用 cargo fmt --all，见下
cd web && npm ci && npm test    # 11 个前端测试
node scripts/contracts.mjs check
```

20 个 crate。`rusqlite` **只允许出现在 `xops-store/src/sqlite.rs` 一个文件里**,
有一条枚举全仓的测试守着这条线。

⚠️ **`vendor/attacore` 是只读的子模块**(执行引擎)。改那边的代码会被上游清理掉,
需求变更走 ISSUE。**格式化用 `./scripts/fmt.sh`**——`cargo fmt --all` 会顺着
path 依赖走进子模块(踩过一次,75 个文件),`[workspace] exclude` 拦不住它。

## 许可

Apache-2.0,见 [LICENSE](LICENSE)。
