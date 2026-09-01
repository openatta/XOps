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
use std::thread;

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
       XOPS_ATTACORE_SOCKET   AttaCore 的 socket，**不给就跑桩引擎**\n"
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

    // 两个服务面各一个线程。**它们各监听各的端口，共用同一份状态。**
    let mcp = Arc::clone(&assembled.mcp);
    let mcp_addr = config.mcp_addr.clone();
    let mcp_thread = thread::spawn(move || xops_mcp::transport::http::serve(mcp, mcp_addr));

    let web = Arc::clone(&assembled.web);
    let web_addr = config.web_addr.clone();
    let web_thread = thread::spawn(move || web.serve(web_addr));

    println!("\n起来了。Ctrl-C 停。");
    for (what, handle) in [("MCP", mcp_thread), ("Web", web_thread)] {
        match handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                eprintln!("{what} 服务面停了：{error}");
                return ExitCode::FAILURE;
            }
            Err(_) => {
                eprintln!("{what} 服务面的线程崩了");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}
