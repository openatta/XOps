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

fn help() -> &'static str {
    "xopsd —— XOps 服务端\n\
     \n\
     用法：\n  \
       xopsd                 起两个服务面\n  \
       xopsd --check         装配一遍并打印横幅，不监听\n  \
       xopsd --generate-key  生成一把加密密钥\n\
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
