//! 装配：把 19 个 crate 接成一个进程。
//!
//! **这个文件里没有业务判断，一句都没有。** 它只做三件事：
//! 建对象 · 按依赖顺序接起来 · 把两个服务面交出去。
//! 任何一处"顺手在这里判一下"都是把语义搬出它该在的包。
//!
//! # 两个服务面是分开的
//!
//! ```text
//! MCP 写入面   POST /mcp          唯一的写入通道（I-L）
//! 只读 Web 面  GET /… + 三条例外   两个凭据路由 + 一条 webhook，**不写业务对象**（G2）
//! ```
//!
//! 它们**各监听各的端口，共用同一份状态**。分开不是为了好看：
//! `xops-web` 里结构性地不存在写业务对象的路由，而那件事只有在两个面分开时才成立。

use std::path::PathBuf;
use std::sync::Arc;

use xops_audit::AuditLog;
use xops_core::{Clock, Error, Result, SystemClock, TableName};
use xops_dispatch::Dispatcher;
use xops_dispatch::event::Whitelist;
use xops_exec::provider::IsolationLevel;
use xops_exec::{EmbeddedEngine, Engine, ExecContract, Runtime, StubEngine};
use xops_flow::Flows;
use xops_identity::{Directory, ProjectId};
use xops_mcp::McpServer;
use xops_mcp::tools::project::ProjectHook;
use xops_notice::Notices;
use xops_read::ReadModel;
use xops_repo::{Repos, Sealer};
use xops_script::Plugins;
use xops_skill::Skills;
use xops_store::{MemoryRelations, MemoryStore, Relations, SqliteStore, Store, WriteEngine};
use xops_table::Tables;
use xops_table::engine::Catalog;
use xops_template::Templates;
use xops_web::{Assets, Sessions, WebServer};
use xops_xforge::XForge;

use crate::config::Config;

/// 装配好的两个服务面，以及后台要用的那几样。
#[allow(missing_debug_implementations, reason = "装的全是 Arc<dyn …>")]
pub struct Assembled {
    pub mcp: Arc<McpServer>,
    pub web: Arc<WebServer>,
    /// 引擎是真的还是桩。**启动横幅要说出来。**
    pub engine_kind: &'static str,
    /// 隔离级别没兑现的那些需求（`D58`、`EXE-029`）。**不静默降级。**
    pub unsatisfied: &'static [(&'static str, &'static str)],
    /// 引擎那一侧的已知缺口。**与隔离无关**，但同样要在启动时说出来。
    pub engine_gaps: &'static [(&'static str, &'static str)],
    pub notices: Arc<Notices>,
    pub dispatcher: Arc<Dispatcher>,
    /// 把跑完的执行落成账。**没有它 `_runs` 就是空的**——
    /// 触发那条路是非阻塞的，执行跑完之后没有人在等着写账。
    pub reaper: Arc<xops_dispatch::dispatch::Reaper>,
    /// 到点了去点定时任务（`TRG-009`）。
    pub ticker: Arc<xops_dispatch::schedule_store::Ticker>,
    /// 保留期（`RET-003` · `AUD-010` · `RET-008`）。
    pub keeper: Arc<xops_task::keeper::Keeper>,
    pub flows: Arc<Flows>,
    pub clock: Arc<dyn Clock>,
    /// 目录。**给引导用**——第一个用户与第一份令牌要从这里来。
    pub directory: Arc<Directory>,
}

/// Git webhook → 一个事件 → 订阅了它的那些任务（`TRG-011`）。
///
/// ⚠️ **端点内不做任何拉取或执行**（`TRG-014`）:这里只解载荷、按投递标识幂等地入队。
/// 平台的 webhook 都有超时并会因超时重投，慢一点就是放大问题。
///
/// ⚠️ **错误一律映射成"不存在"**（`TRG-012`）——端点不能成为探测器:
/// 验签失败、载荷不合形状、项目不存在，回的东西必须一模一样。
struct GitWebhooks {
    repos: Arc<Repos>,
    tasks: Arc<xops_task::Tasks>,
    dispatcher: Arc<Dispatcher>,
    clock: Arc<dyn Clock>,
}

impl xops_web::WebhookSink for GitWebhooks {
    fn deliver(
        &self,
        signature: Option<&str>,
        event: Option<&str>,
        delivery: Option<&str>,
        body: &[u8],
    ) -> Result<()> {
        let reject = xops_dispatch::webhook::rejection;
        let (Some(event), Some(delivery)) = (event, delivery) else {
            return Err(reject());
        };
        let payload: serde_json::Value = serde_json::from_slice(body).map_err(|_| reject())?;
        let git =
            xops_dispatch::webhook::extract(&payload, delivery, event).map_err(|_| reject())?;

        // ⚠️ **验签这一步同时回答了"哪个项目"。**
        // 密钥按项目存（`TRG-012`），所以签得过的那一把密钥就指出了项目——
        // 不需要先按仓名去找项目，而"先找再验"会开一条探测信道：
        // 同一个仓名，绑过的与没绑过的走的分支不一样。
        //
        // ⚠️ **一次投递最多命中一个项目。** 平台级一把密钥的那版是发给所有绑过仓的
        // 项目的——A 仓的一次 push 会触发 B 项目的任务。
        let Some(binding) = self
            .repos
            .webhook_source(body, signature.unwrap_or_default())
            .map_err(|_| reject())?
        else {
            return Err(reject());
        };

        let subscribers = self
            .tasks
            .subscribers(binding.project, "git")
            .map_err(|_| reject())?;
        for task in subscribers {
            let event = xops_dispatch::Event {
                kind: xops_dispatch::EventKind::Git,
                project: binding.project,
                // `TRG-013`：**按投递标识幂等**——平台超时重投不该跑第二遍。
                external_id: Some(git.delivery.clone()),
                triggered_by: xops_dispatch::Trigger::External {
                    source: "git".to_owned(),
                },
                // `TRG-017`：**这次要它看的那一版**，覆盖任务里写死的。
                revision: Some(git.revision.clone()),
                at: self.clock.now(),
                payload: serde_json::json!({
                    "branch": git.branch, "event": git.event,
                }),
            };
            let _ = self.dispatcher.trigger(&task, &event);
        }
        // ⚠️ **没有订阅者也算收下了。** 回"不存在"会让调用方能探出
        // "这个仓有没有被订阅"——那是 `TRG-012` 要挡的同一件事。
        Ok(())
    }
}

/// 「按修订备一份只读工作区」那条链（`RP-08` → `RP-11`）。
///
/// ⚠️ **这条以前一处实现都没有。** `WorkspaceSource` 是个注入位，
/// 而全仓没有任何东西填它——于是声明了 `needsRepository` 的技能
/// **两条路都跑不了**:正式触发拿不到工作区，试跑也拿不到。
/// 那条枚举验收当时没抓住它，因为它数的是字符串 `GitPlatform`,
/// 而那个词因为别的原因也在装配层里出现——**数名字不如数接口**。
struct Workspaces {
    repos: Arc<Repos>,
}

/// 一份还活着的工作区。**攥着它的人放手，那个目录就没了**（`RPO-009`）。
struct Held(xops_repo::Workspace);

impl xops_dispatch::PreparedWorkspace for Held {
    fn path(&self) -> &std::path::Path {
        self.0.root()
    }
}

impl xops_dispatch::WorkspaceSource for Workspaces {
    fn prepare(
        &self,
        project: ProjectId,
        revision: Option<&str>,
    ) -> Result<Arc<dyn xops_dispatch::PreparedWorkspace>> {
        // 没指定修订就解出此刻的确切 sha,**解完就钉住**:
        // `RPO-010` 要回答的是"这份报告针对哪版代码",而分支名明天指向别处。
        let revision = match revision {
            Some(revision) => revision.to_owned(),
            None => self.repos.head_revision(project)?,
        };
        Ok(Arc::new(Held(
            self.repos.prepare_workspace(project, &revision)?,
        )))
    }
}

/// 求值链的接头。
///
/// ⚠️ **它存在的唯一理由是一个真实的环**:`WriteEngine` 要求值链，
/// 求值链要 `Flows` 与 `Tables`，而那两个都要 `WriteEngine`。
/// 先把空的接进去、链建好再填——**在填上之前它什么也不做，与没接一模一样**。
#[derive(Default)]
struct LateChain {
    inner: std::sync::RwLock<Option<Arc<dyn xops_store::Evaluate>>>,
}

impl LateChain {
    fn fill(&self, chain: Arc<dyn xops_store::Evaluate>) {
        if let Ok(mut slot) = self.inner.write() {
            *slot = Some(chain);
        }
    }

    fn get(&self) -> Option<Arc<dyn xops_store::Evaluate>> {
        self.inner.read().ok().and_then(|slot| slot.clone())
    }
}

impl xops_store::Evaluate for LateChain {
    fn scope(&self, table: &xops_core::TableName) -> xops_store::EvalScope {
        self.get()
            .map_or_else(xops_store::EvalScope::none, |chain| chain.scope(table))
    }

    fn evaluate(
        &self,
        request: &xops_store::WriteRequest,
        view: &dyn xops_store::RowView,
    ) -> Result<Vec<xops_store::Writeback>> {
        self.get()
            .map_or_else(|| Ok(Vec::new()), |chain| chain.evaluate(request, view))
    }
}

/// 流转插件的求值（`PLG-002`）。
struct TransitionPlugins {
    plugins: Arc<Plugins>,
}

impl xops_settle::PluginEvaluator for TransitionPlugins {
    fn transition(
        &self,
        call: &xops_settle::TransitionCall<'_>,
    ) -> Result<xops_settle::PluginVerdict> {
        // 引用的是**固定版本**（`PLG-009`）:先按名字取已安装的最新一版。
        let installed = self.plugins.resolve_latest(call.project, call.plugin)?;
        let settled = xops_script::evaluate_transition(
            &installed,
            &xops_script::TransitionInput {
                instance: call.instance.clone(),
                row: call.row.clone(),
                related: call.related.clone(),
            },
            call.settlement,
            call.subject,
        )?;
        Ok(xops_settle::PluginVerdict {
            verdict: match settled.verdict {
                xops_script::Verdict::Pass => "pass",
                xops_script::Verdict::Reject => "reject",
                xops_script::Verdict::Fail => "fail",
            }
            .to_owned(),
            writes: settled
                .writes
                .into_iter()
                .map(|write| (write.table, write.row, write.values))
                .collect(),
            note: settled.note,
        })
    }
}

/// 「这一行没被采纳」→ 通知写入者（`FLW-027`）。
struct NotSettled {
    notices: Arc<Notices>,
}

impl xops_settle::NotSettledNotifier for NotSettled {
    fn not_settled(
        &self,
        project: ProjectId,
        instance: &str,
        table: &str,
        row: &str,
        writer: xops_identity::UserId,
        why: &str,
    ) {
        let _ = self
            .notices
            .notify(&xops_notice::SourceEvent::RowNotSettled {
                project,
                instance: instance.to_owned(),
                table: table.to_owned(),
                row: row.to_owned(),
                writer,
                reason: why.to_owned(),
            });
    }
}

/// 审计留痕的保留期（`AUD-010`）。
///
/// ⚠️ **它只吃 `_audit` 上的留痕。** 有业务行的那些事件不是"留痕"，
/// 它们**是业务状态本身**——删了它们 `AUD-004` 当场落空。
struct PruneAudit {
    audit: Arc<AuditLog>,
}

impl xops_task::keeper::Prunable for PruneAudit {
    fn what(&self) -> &'static str {
        "审计留痕"
    }

    fn prune(&self, now: xops_core::Timestamp) -> Result<usize> {
        // 默认留一年。**保留期内不可删除**（`AUD-010`）。
        const KEEP_MILLIS: i64 = 365 * 24 * 60 * 60 * 1_000;
        self.audit.prune(xops_core::Timestamp::from_millis(
            now.as_millis() - KEEP_MILLIS,
        ))
    }
}

/// 通知的保留期（`RET-008`，默认三个月，与任务无关）。
struct PruneNotices {
    notices: Arc<Notices>,
}

impl xops_task::keeper::Prunable for PruneNotices {
    fn what(&self) -> &'static str {
        "通知"
    }

    fn prune(&self, now: xops_core::Timestamp) -> Result<usize> {
        self.notices.prune(now)
    }
}

/// 执行结束 → 一条通知（`NTF-007`）。
///
/// ⚠️ **通知的失败不回滚落账**:`Notices::notify` 交回的是一组痕迹，
/// 不是一个能往上抛的错误（`NTF-008`）——这里因此只是把它丢掉，账已经落好了。
struct RunNotices {
    notices: Arc<Notices>,
}

impl xops_dispatch::dispatch::RunNotifier for RunNotices {
    fn finished(&self, task: &xops_task::Task, run: &str, status: &str) {
        let _ = self.notices.notify(&xops_notice::SourceEvent::RunFinished {
            project: task.project,
            run: run.to_owned(),
            task: task.id.to_string(),
            status: status.to_owned(),
            owner: task.created_by,
            after_failure: None,
        });
    }
}

/// 项目建好之后把那四张系统表建起来（`TBL-005`）。
struct SystemTables {
    tables: Arc<Tables>,
}

impl ProjectHook for SystemTables {
    fn after_create(&self, project: ProjectId, slug: &str) -> Result<()> {
        self.tables.ensure_system_tables(project, slug)
    }
}

/// 每个包各自的平台表，装配时一次登记齐。
///
/// **漏一张的后果是那张表的事件流不进审计索引**——查得到行，查不到"谁改的"。
fn watched_tables() -> Result<Vec<TableName>> {
    let mut out = xops_identity::directory::platform_tables()?;
    for name in [
        xops_table::CATALOG_TABLE,
        xops_read::BOARDS_TABLE,
        xops_repo::BINDINGS_TABLE,
        xops_skill::SKILLS_TABLE,
        xops_skill::VERSIONS_TABLE,
        xops_task::TASKS_TABLE,
        xops_flow::FLOWS_TABLE,
        xops_flow::service::INSTANCES_TABLE,
    ] {
        out.push(TableName::new(name)?);
    }
    Ok(out)
}

/// 装配。
///
/// # Errors
/// 存储打不开 · 密钥不合法 · 任何一个 tool 的声明不合形状或重名。
#[allow(
    clippy::too_many_lines,
    reason = "装配就是一长串接线，拆开反而看不出顺序"
)]
pub fn assemble(config: &Config) -> Result<Assembled> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    // ① 存储。**换一个实现进去，上面的一切不改一行**（`CON-012`、`G12`）。
    //
    // 两条缝一起建：[`Store`] 管事件与键值投影，[`Relations`] 管**带索引的当前视图**。
    // 后者是缓存不是账——需要按别的列找的表独立成一张真表，索引交给数据库，
    // **而不是在键值里手写一条二级索引**。
    let (store, relations): (Arc<dyn Store>, Arc<dyn Relations>) = if config.in_memory() {
        (
            Arc::new(MemoryStore::new()),
            Arc::new(MemoryRelations::new()),
        )
    } else {
        let sqlite = Arc::new(SqliteStore::open(&config.db)?);
        let relations = sqlite.relations();
        (sqlite as Arc<dyn Store>, relations)
    };

    // ② 写入路径：目录 + 写引擎 + 四道钩子里的前两道。
    let catalog = Arc::new(Catalog::open(Arc::clone(&store), Arc::clone(&clock))?);
    // ⚠️ **求值链是后绑的。** `WriteEngine` 要它，而它要 `Flows` 与 `Tables`，
    // 而那两个又都要 `WriteEngine`——这是一个真实的环。
    // `LateChain` 是那个环的接头：先把空的接进去，等链建好了再填。
    //
    // 不这么做的替代方案是让 `Flows` / `Tables` 接受一个"稍后给"的引擎，
    // 那会把这个环推到每一个调用方身上。**接头只有一个地方，比到处都是好。**
    let late_chain = Arc::new(LateChain::default());
    let engine = Arc::new(
        WriteEngine::new(Arc::clone(&store), Arc::clone(&clock))
            .with_pre_write(Arc::clone(&catalog) as Arc<dyn xops_store::PreWrite>)
            .with_schema_check(Arc::clone(&catalog) as Arc<dyn xops_store::SchemaCheck>)
            .with_evaluate(Arc::clone(&late_chain) as Arc<dyn xops_store::Evaluate>),
    );

    // ③ 审计。**每个包的平台表都要登记**，否则重建索引时走不到它的事件流。
    let mut audit = AuditLog::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&relations),
    )?;
    for table in watched_tables()? {
        audit = audit.watching(table);
    }
    let audit = Arc::new(audit);

    let directory = Arc::new(Directory::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&audit),
        Arc::clone(&clock),
    ));
    let tables = Arc::new(Tables::new(
        Arc::clone(&engine),
        Arc::clone(&catalog),
        Arc::clone(&audit),
        Arc::clone(&directory),
        Arc::clone(&clock),
        Arc::clone(&store),
    ));
    // `_notices` 是**平台全局表**，不属于任何项目，所以它在这里建一次。
    tables.ensure_global_tables()?;

    let model = Arc::new(ReadModel::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&audit),
        Arc::clone(&directory),
        Arc::clone(&tables),
        Arc::clone(&clock),
    ));
    let flows = Arc::new(Flows::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&audit),
        Arc::clone(&directory),
        Arc::clone(&tables),
        Arc::clone(&relations),
        Arc::clone(&clock),
    )?);
    // ⚠️ 技能与「发起测试执行」互相要对方：`Skills` 要一条执行链才发得起测试执行，
    // 而那条链要 `Skills` 去解出技能版本。先建 `Skills`，再把链接上去——
    // 这是 `SKL-003` 那条门在装配层的形状。
    let skills = Arc::new(Skills::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&audit),
        Arc::clone(&directory),
        Arc::clone(&clock),
    ));

    // ④ 执行。**引擎嵌在这个进程里**（`D61`）——不再有 attacored 那个分立进程。
    //
    // 给了模型凭据就接真引擎，不给就跑桩。**这件事启动横幅会说出来**：
    // "以为接了真引擎、其实跑的是桩"是一种查起来很慢的错。
    let (engine_impl, engine_kind): (Arc<dyn Engine>, &'static str) = match &config.model_key {
        Some(key) => (embedded_engine(config, key)?, "attacore"),
        None => (Arc::new(StubEngine::new()), "stub"),
    };
    // `D58`：**裸跑**。没兑现的那些需求由 `unsatisfied()` 枚举着，不是一句"以后补"。
    let isolation = IsolationLevel::Bare;
    let exec: Arc<dyn ExecContract> =
        Arc::new(Runtime::new(engine_impl, Arc::clone(&clock), isolation));

    // ⑤ 任务与派发。订阅白名单挂上去 —— 没接就等于不校验订阅。
    let whitelist = Arc::new(Whitelist);
    let tasks = Arc::new(
        xops_task::Tasks::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            Arc::clone(&directory),
            Arc::clone(&skills),
            Arc::clone(&clock),
        )
        .with_subscription_check(Arc::clone(&whitelist) as Arc<dyn xops_task::SubscriptionCheck>),
    );
    // ⑥ 并发名额（`EXE-027`）。**发和收要是同一个**:
    // `Dispatcher` 提交时拿，`Reaper` 落账时还。分成两份就等于不限。
    let concurrency = Arc::new(xops_task::Concurrency::default());
    let slots = Arc::new(xops_dispatch::Slots::new(Arc::clone(&concurrency)));

    // ⑥ 仓绑定。密钥从环境来 —— **没有它这一步就起不来**，见 `Config::from_env`。
    let sealer = Arc::new(Sealer::from_hex(&config.secret_key)?);
    let repos = Arc::new(Repos::new(
        xops_repo::Deps {
            engine: Arc::clone(&engine),
            store: Arc::clone(&store),
            audit: Arc::clone(&audit),
            directory: Arc::clone(&directory),
            clock: Arc::clone(&clock),
        },
        Arc::clone(&sealer),
        Arc::new(xops_repo::GitHub) as Arc<dyn xops_repo::GitPlatform>,
        config.workspaces.clone(),
    ));

    // 备工作区那条链。**两条路都要接**:正式触发一条，技能试跑一条——
    // 试跑与正式执行走的是同一条路，少接一条就等于"能发布的技能跑不了"。
    let workspaces = Arc::new(Workspaces {
        repos: Arc::clone(&repos),
    }) as Arc<dyn xops_dispatch::WorkspaceSource>;

    // 接上测试执行那条链（`SKL-003`）。**没有它技能发布不了**——
    // 发布要一次成功的测试执行，而那次执行的入口在这里。
    let skills = Arc::new(
        Skills::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            Arc::clone(&directory),
            Arc::clone(&clock),
        )
        .with_test_runner(Arc::new(
            xops_dispatch::dispatch::TestRuns::new(Arc::clone(&exec), Arc::clone(&clock))
                .with_concurrency(Arc::clone(&concurrency))
                .with_workspaces(Arc::clone(&workspaces)),
        )),
    );

    let dispatcher = Arc::new(
        Dispatcher::new(
            Arc::clone(&tasks),
            Arc::clone(&skills),
            Arc::clone(&exec),
            Arc::clone(&audit),
            Arc::clone(&store),
            Arc::clone(&clock),
        )
        .with_concurrency(Arc::clone(&slots))
        .with_workspaces(Arc::clone(&workspaces)),
    );
    let schedules = Arc::new(xops_dispatch::schedule_store::Schedules::new(
        Arc::clone(&store),
        Arc::clone(&audit),
    ));
    // ② 到点了去点它。**没有它 `schedule.configure` 存得进去、永不触发。**
    let ticker = Arc::new(xops_dispatch::schedule_store::Ticker::new(
        Arc::clone(&schedules),
        Arc::clone(&tasks),
        Arc::clone(&dispatcher),
        Arc::clone(&clock),
    ));

    // ⑦ 插件。**出网后端没接就是 `Denied`**：声明了出网的插件也发不出去，
    //    而这件事在部署层面是看得见的（`PLG-004`）。
    let plugins = Arc::new(Plugins::new(xops_script::service::Deps {
        tables: Arc::clone(&tables),
        store: Arc::clone(&store),
        audit: Arc::clone(&audit),
        directory: Arc::clone(&directory),
        sealer: Arc::clone(&sealer),
        net: Arc::new(xops_script::net::Denied),
        clock: Arc::clone(&clock),
    }));

    let notices = Arc::new(Notices::new(
        Arc::clone(&tables),
        Arc::clone(&relations),
        Arc::clone(&directory),
        Arc::clone(&clock),
    )?);
    // 落账那一环（`EXE-026`）。**执行跑完之后是它把 `_runs` 那一行写下来**——
    // 触发那条路非阻塞（`EXE-021`），所以没有别人在等着做这件事。
    let reaper = Arc::new(
        xops_dispatch::dispatch::Reaper::new(
            Arc::clone(&dispatcher),
            Arc::clone(&tasks),
            Arc::new(xops_task::landing::Landing::new(
                Arc::clone(&tables),
                Arc::clone(&clock),
            )),
            Arc::clone(&store),
            Arc::clone(&exec),
        )
        .with_concurrency(Arc::clone(&slots))
        // 执行结束通知任务所有者（`NTF-007`）。**没接就等于不通知**，
        // 而"自动化失灵是静默的"正是这条要挡的。
        .with_notices(Arc::new(RunNotices {
            notices: Arc::clone(&notices),
        })),
    );

    // ④⑤ 保留期。三条一起做：`_runs` 与产出行、审计留痕、通知。
    let keeper = Arc::new(
        xops_task::keeper::Keeper::new(
            Arc::new(xops_task::cleanup::Cleanup::new(
                Arc::clone(&tables),
                Arc::clone(&store),
                Arc::clone(&audit),
            )),
            Arc::clone(&tables),
            Arc::clone(&directory),
            Arc::clone(&clock),
        )
        .with(Arc::new(PruneAudit {
            audit: Arc::clone(&audit),
        }))
        .with(Arc::new(PruneNotices {
            notices: Arc::clone(&notices),
        })),
    );

    let templates = Arc::new(Templates::new(
        Arc::clone(&tables),
        Arc::clone(&flows),
        Arc::clone(&plugins),
        Arc::clone(&directory),
    ));
    let xforge = Arc::new(XForge::new(
        Arc::clone(&repos),
        Arc::clone(&flows),
        Arc::clone(&tables),
        Arc::clone(&directory),
    ));

    // ⑧ MCP 写入面。**十六个域一次注册齐**——`MCP-011` 的那张目录就是这里。
    let mut mcp = McpServer::new(
        Arc::clone(&directory),
        Arc::clone(&audit),
        Arc::clone(&store),
    );
    {
        let registry = mcp.registry_mut();
        registry.register(Arc::new(xops_mcp::tools::WhoAmI::new()?))?;
        registry.register(Arc::new(xops_mcp::tools::Capabilities::new(Arc::clone(
            &directory,
        ))?))?;
        registry.register(Arc::new(xops_mcp::tools::MyPendingNodes::new(Arc::new(
            xops_flow::tools::PendingNodes::new(Arc::clone(&flows)),
        ))?))?;
        // 项目建好之后自动建那四张系统表。
        xops_mcp::tools::project::register_with_hook(
            registry,
            &directory,
            Arc::new(SystemTables {
                tables: Arc::clone(&tables),
            }),
        )?;
        xops_mcp::tools::token::register(registry, &directory)?;
        xops_mcp::tools::audit::register(registry, &directory, &audit)?;
        xops_table::tools::register(registry, &tables)?;
        xops_read::tools::register(registry, &model)?;
        xops_repo::tools::register(registry, &repos)?;
        xops_skill::tools::register(registry, &skills)?;
        xops_task::tools::register(registry, &tasks)?;
        xops_dispatch::tools::register(registry, &dispatcher, &tasks, &exec)?;
        xops_dispatch::tools::register_schedules(registry, &tasks, &schedules, &clock)?;
        xops_flow::tools::register(registry, &flows)?;
        xops_settle::tools::register(registry, &flows, &tables)?;
        xops_script::tools::register(registry, &plugins)?;
        xops_notice::tools::register(registry, &notices)?;
        xops_template::tools::register(registry, &templates)?;
        xops_xforge::tools::register(registry, &xforge)?;
    }

    // ① 求值链。**接上它，一行写进结算表才会真的去结算**——
    // 在这之前 `Evaluate` 没有任何生产实现，整个流程引擎是惰性的。
    late_chain.fill(Arc::new(
        xops_settle::Chain::new(
            Arc::clone(&flows),
            Arc::clone(&tables),
            Arc::new(xops_settle::Evaluator::new(
                Arc::clone(&flows),
                Arc::new(xops_settle::WriterCheck::new(
                    Arc::clone(&directory),
                    Arc::clone(&tables),
                )),
            )),
            Arc::clone(&clock),
        )
        .with_plugins(Arc::new(TransitionPlugins {
            plugins: Arc::clone(&plugins),
        }))
        .with_notices(Arc::new(NotSettled {
            notices: Arc::clone(&notices),
        })),
    ));

    // ⑦ 关系投影是缓存：**存量库升上来时它们是空的**，开机重放一次。
    //    新库上这三次都是 0 行，代价可忽略;而少了它，审计查不到、按主体找不到。
    for (what, count) in [
        ("audit", audit.rebuild_index()?),
        ("flow_instances", flows.rebuild_instances()?),
        ("notices", notices.rebuild()?),
    ] {
        if count > 0 {
            xops_core::log::info(
                "assemble.rebuilt",
                &[("relation", what), ("rows", &count.to_string())],
            );
        }
    }

    // ⑨ 只读 Web 面。**前端产物随二进制发行**（`D55`），部署方不需要 Node。
    let sessions = Arc::new(Sessions::new(Arc::clone(&store), Arc::clone(&clock)));
    let assets = config
        .assets
        .clone()
        .map_or_else(Assets::embedded, Assets::at);
    // ③ webhook 的落点。**没接就等于事件掉进地里**——路由在、验签在、什么也不发生。
    let web = Arc::new(
        WebServer::new(Arc::clone(&model), Arc::clone(&directory), sessions, assets).with_webhooks(
            Arc::new(GitWebhooks {
                repos: Arc::clone(&repos),
                tasks: Arc::clone(&tasks),
                dispatcher: Arc::clone(&dispatcher),
                clock: Arc::clone(&clock),
            }),
        ),
    );

    Ok(Assembled {
        mcp: Arc::new(mcp),
        web,
        engine_kind,
        unsatisfied: isolation.unsatisfied(),
        engine_gaps: xops_exec::IsolationLevel::engine_gaps(),
        notices,
        dispatcher,
        reaper,
        ticker,
        keeper,
        flows,
        clock,
        directory,
    })
}

/// 接真引擎。
///
/// ⚠️ **模型凭据到此为止。** 它进的是引擎的鉴权模式，不进派工单、不进过程记录
/// （`EXE-010`、`I-F`）。
fn embedded_engine(config: &Config, key: &str) -> Result<Arc<dyn Engine>> {
    let auth = attacore_model::client::AuthMode::ApiKey(key.to_owned());
    let client = match &config.model_base_url {
        Some(base) => {
            // ⚠️ **末尾那个斜杠不是可有可无的。**
            // 客户端拿 base 去 `Url::join("v1/messages")`，而 `join` 的语义是:
            // base 不以 `/` 结尾时**最后一段会被替换掉**——
            // `https://host/anthropic` 会变成 `https://host/v1/messages`，
            // 路径前缀被悄悄丢掉，表现是一个查不出原因的 404。
            // **实测撞的就是这个。**
            let normalized = if base.ends_with('/') {
                base.clone()
            } else {
                format!("{base}/")
            };
            let url = normalized
                .parse()
                .map_err(|error| Error::invalid(format!("模型地址不合法：{error}")))?;
            attacore_model::client::HttpAnthropicClient::with_base(auth, url)
        }
        None => attacore_model::client::HttpAnthropicClient::new(auth),
    }
    .map_err(|error| Error::unavailable(format!("模型客户端建不起来：{error}")))?;

    let model: Arc<dyn attacore_core::interface::model::Model> = Arc::new(
        attacore_model::adapter::AnthropicModel::new(Arc::new(client)),
    );
    // ⚠️ **场景决定 agent 手上有哪些工具，而那就是这次执行的可见范围**（`I-I`）。
    // 引擎自带的 `CodingScene` 配了 Bash / Write / 子代理，三样各违一条——
    // 理由写在 `xops_exec::scene` 的模块注释里，那是实测撞出来的。
    let scene: Arc<dyn attacore_core::interface::scene::AgentScene> =
        Arc::new(xops_exec::scene::SkillScene);
    let settings = Arc::new(EmbeddedEngine::settings(&config.model));
    Ok(Arc::new(EmbeddedEngine::new(scene, model, settings)?))
}

/// 让 `PathBuf` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _PathLink = PathBuf;
