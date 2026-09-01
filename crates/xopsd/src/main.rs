//! `xopsd`：把 XOps 跑起来。
//!
//! ```text
//! xopsd                 按环境变量起两个服务面
//! xopsd --generate-key  生成一把加密密钥，然后退出
//! xopsd --check         装配一遍、打印横幅、**不监听**，然后退出
//! ```
//!
//! `--check` 那一条是给部署用的：它回答"这份配置装得起来吗"，
//! **而且顺带把降级项打出来**——比起起服务之后再翻日志，这一步在部署脚本里更好使。

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::Duration;

use xops_core::log;
use xopsd::{Config, assemble, banner};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--generate-key") => generate_key(),
        Some("--check") => run(true),
        Some("--issue-token") => issue_token(args.get(1).map(String::as_str)),
        Some("--help" | "-h") => {
            println!("{}", help());
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("不认识的参数：{other}\n\n{}", help());
            ExitCode::FAILURE
        }
        None => run(false),
    }
}

/// 停止 accept 之后，给在途请求多久收尾。
const DRAIN: Duration = Duration::from_secs(3);
/// 多久扫一遍"跑完但还没落账"的执行。
///
/// ⚠️ 它是**账变得可见的延迟上限**:执行结束到 `_runs` 出现那一行之间就是这么久。
const REAP_EVERY: Duration = Duration::from_millis(500);

fn help() -> &'static str {
    "xopsd —— XOps 服务端\n\
     \n\
     用法：\n  \
       xopsd                 起两个服务面\n  \
       xopsd --check         装配一遍并打印横幅，不监听\n  \
       xopsd --generate-key  生成一把加密密钥\n  \
       xopsd --issue-token <账号>\n        \
                             给这个账号签一把 MCP 令牌（不在就先建）。\n        \
                             **引导用**：第一把令牌只能这样来 —— 签令牌本身要令牌\n\
     \n\
     环境变量：\n  \
       XOPS_SECRET_KEY        **必填**，只读仓凭据与插件配置的加密密钥\n  \
       XOPS_DB                数据库路径，默认 :memory:\n  \
       XOPS_MCP_ADDR          MCP 写入面，默认 127.0.0.1:8765\n  \
       XOPS_WEB_ADDR          只读 Web 面，默认 127.0.0.1:8766\n  \
       XOPS_ASSETS            前端产物目录，不给就用嵌进二进制的那一份\n  \
       XOPS_WORKSPACES        工作区根目录\n  \
       XOPS_MODEL_KEY         模型 API key。**不给就跑桩引擎**\n  \
       XOPS_MODEL             默认模型，默认 claude-sonnet-4-6\n  \
       XOPS_MODEL_BASE_URL    模型服务地址（兼容 Anthropic Messages 的任何一个）\n  \
       XOPS_LOG               off / error / warn / info / debug，默认 info\n"
}

/// 给一个账号签一把 MCP 令牌。**第一把令牌只能这样来。**
///
/// # 为什么它是一条命令，不是一个接口
///
/// 签令牌经 MCP 要先有令牌（`MCP-002`：**每次调用都要带令牌，握手也不例外**），
/// 于是第一把无处可来。开一个"引导端点"是错的答案——**那是一个免认证的、
/// 能签出任意权限凭据的网络入口**，它会一直在那里。
///
/// 这条命令的授权来自**能不能读到这个库文件**，而那正是该有的那一级:
/// 能碰数据库的人本来就能拿到一切。
///
/// ⚠️ **令牌原文只在这里出现一次**（`TOK-002`：系统内任何持久化位置都不存在它）。
/// 丢了就再签一把，找不回来。
fn issue_token(account: Option<&str>) -> ExitCode {
    let Some(account) = account.filter(|name| !name.trim().is_empty()) else {
        eprintln!("要给哪个账号签？用法：xopsd --issue-token <账号>");
        return ExitCode::FAILURE;
    };
    log::set_level(log::level_from_env());
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    if config.in_memory() {
        eprintln!("库是 :memory: —— 签出来的令牌随进程一起消失。设 XOPS_DB 再来一次。");
        return ExitCode::FAILURE;
    }
    let assembled = match assemble(&config) {
        Ok(assembled) => assembled,
        Err(error) => {
            eprintln!("装配失败：{error}");
            return ExitCode::FAILURE;
        }
    };
    match assembled.directory.bootstrap_token(account) {
        Ok(secret) => {
            // ⚠️ 印到 stdout，横幅那些走 stderr —— 这样 `$(xopsd --issue-token me)`
            // 拿到的就只有令牌本身。
            println!("{}", secret.into_string());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("签不出来：{error}");
            ExitCode::FAILURE
        }
    }
}

fn generate_key() -> ExitCode {
    match xops_repo::Sealer::generate_key() {
        Ok(key) => {
            println!("{}={key}", xops_repo::KEY_ENV);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("生成不出来：{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(check_only: bool) -> ExitCode {
    // 先定级别再干别的 —— 装配阶段出的错也要记得下来。
    log::set_level(log::level_from_env());
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let assembled = match assemble(&config) {
        Ok(assembled) => assembled,
        Err(error) => {
            eprintln!("装配失败：{error}");
            return ExitCode::FAILURE;
        }
    };
    print!("{}", banner::render(&config, &assembled));

    if check_only {
        println!("\n装得起来。（--check：没有监听任何端口）");
        return ExitCode::SUCCESS;
    }

    // ——— 停机信号 ———
    //
    // ⚠️ **这不是为了数据安全。** 每次写都是一条原子语句，库开着 WAL——
    // 直接 `kill -9` 也不会写坏东西。优雅停机换来的是两样别的：
    // 在途请求能把回话写完，以及 systemd / 编排器看到的是一次干净的退出，
    // 而不是"等超时再 SIGKILL"。
    let stopping = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        if let Err(error) = signal_hook::flag::register(signal, Arc::clone(&stopping)) {
            eprintln!("接不上停机信号：{error}");
            return ExitCode::FAILURE;
        }
    }

    // 两个服务面各一个线程。**它们各监听各的端口，共用同一份状态。**
    let mcp_listener = match xops_mcp::transport::http::listen(&config.mcp_addr) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("MCP 面听不起来：{error}");
            return ExitCode::FAILURE;
        }
    };
    let web_listener = match xops_web::server::listen(&config.web_addr) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("Web 面听不起来：{error}");
            return ExitCode::FAILURE;
        }
    };

    let mcp_thread = {
        let server = Arc::clone(&assembled.mcp);
        let stopping = Arc::clone(&stopping);
        thread::spawn(move || {
            xops_mcp::transport::http::serve_listener_until(server, &mcp_listener, &stopping)
        })
    };
    let web_thread = {
        let server = Arc::clone(&assembled.web);
        let stopping = Arc::clone(&stopping);
        thread::spawn(move || server.serve_listener_until(&web_listener, &stopping))
    };

    // 落账循环。**执行跑完之后是它把 `_runs` 那一行写下来**——
    // 触发那条路非阻塞（`EXE-021`），所以没有别人在等着做这件事。
    {
        let reaper = Arc::clone(&assembled.reaper);
        let stopping = Arc::clone(&stopping);
        thread::spawn(move || {
            while !stopping.load(std::sync::atomic::Ordering::Relaxed) {
                match reaper.sweep() {
                    Ok(landed) if landed > 0 => {
                        log::info("xopsd.landed", &[("runs", &landed.to_string())]);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        log::error("xopsd.reaper", &[("error", &format!("{error}"))]);
                    }
                }
                thread::sleep(REAP_EVERY);
            }
        });
    }

    log::info(
        "xopsd.started",
        &[
            ("version", env!("CARGO_PKG_VERSION")),
            ("mcp", &config.mcp_addr),
            ("web", &config.web_addr),
            ("engine", assembled.engine_kind),
        ],
    );
    println!("\n起来了。Ctrl-C 停。");

    for (what, handle) in [("MCP", mcp_thread), ("Web", web_thread)] {
        match handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                log::error(
                    "xopsd.surface",
                    &[("face", what), ("error", &format!("{error}"))],
                );
                return ExitCode::FAILURE;
            }
            Err(_) => {
                log::error("xopsd.surface", &[("face", what), ("error", "线程崩了")]);
                return ExitCode::FAILURE;
            }
        }
    }

    // ⚠️ **在途请求在各自的线程上，这里给它们一个有界的窗口**——
    // 不是"等它们全部结束"，因为没有人在数还剩几个。窗口过了就走。
    log::info(
        "xopsd.stopping",
        &[("drain_millis", &DRAIN.as_millis().to_string())],
    );
    thread::sleep(DRAIN);
    log::info("xopsd.stopped", &[]);
    ExitCode::SUCCESS
}
