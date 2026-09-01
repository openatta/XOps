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
use xops_core::{Clock, Result, SystemClock, TableName};
use xops_dispatch::Dispatcher;
use xops_dispatch::event::Whitelist;
use xops_exec::attacore::AttaCoreEngine;
use xops_exec::provider::IsolationLevel;
use xops_exec::{Engine, ExecContract, Runtime, StubEngine};
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
    pub notices: Arc<Notices>,
    pub dispatcher: Arc<Dispatcher>,
    /// 目录。**给引导用**——第一个用户与第一份令牌要从这里来。
    pub directory: Arc<Directory>,
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
    // 后者是缓存不是账——需要按别的列找的表独立成一张真表,索引交给数据库,
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
    let engine = Arc::new(
        WriteEngine::new(Arc::clone(&store), Arc::clone(&clock))
            .with_pre_write(Arc::clone(&catalog) as Arc<dyn xops_store::PreWrite>)
            .with_schema_check(Arc::clone(&catalog) as Arc<dyn xops_store::SchemaCheck>),
    );

    // ③ 审计。**每个包的平台表都要登记**，否则重建索引时走不到它的事件流。
    let mut audit = AuditLog::new(Arc::clone(&engine), Arc::clone(&store))?;
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
        Arc::clone(&clock),
    ));
    let skills = Arc::new(Skills::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&audit),
        Arc::clone(&directory),
        Arc::clone(&clock),
    ));

    // ④ 执行。**引擎不可用绝不就地跑**（`EXE-030`）——这里只决定接哪一个。
    let (engine_impl, engine_kind): (Arc<dyn Engine>, &'static str) = match &config.attacore_socket
    {
        Some(socket) => (Arc::new(AttaCoreEngine::at(socket.clone())), "attacore"),
        // ⚠️ **不给 socket 就是桩**。它跑得通、什么也没真跑——
        // 启动横幅必须说出来。
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
    let dispatcher = Arc::new(Dispatcher::new(
        Arc::clone(&tasks),
        Arc::clone(&skills),
        Arc::clone(&exec),
        Arc::clone(&audit),
        Arc::clone(&store),
        Arc::clone(&clock),
    ));
    let schedules = Arc::new(xops_dispatch::schedule_store::Schedules::new(
        Arc::clone(&store),
        Arc::clone(&audit),
    ));

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

    // ⑨ 只读 Web 面。**前端产物随二进制发行**（`D55`），部署方不需要 Node。
    let sessions = Arc::new(Sessions::new(Arc::clone(&store), Arc::clone(&clock)));
    let assets = config
        .assets
        .clone()
        .map_or_else(Assets::embedded, Assets::at);
    let web = Arc::new(WebServer::new(
        Arc::clone(&model),
        Arc::clone(&directory),
        sessions,
        assets,
    ));

    Ok(Assembled {
        mcp: Arc::new(mcp),
        web,
        engine_kind,
        unsatisfied: isolation.unsatisfied(),
        notices,
        dispatcher,
        directory,
    })
}

/// 让 `PathBuf` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _PathLink = PathBuf;
