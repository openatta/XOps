//! 把这个二进制里注册着的 tool 目录打出来。
//!
//! ```text
//! cargo run -p xopsd --example tools
//! ```
//!
//! **它不联网、不监听、不需要令牌**——问的是"这份代码里有哪些 tool"，
//! 不是"我现在能调哪些"。后者是 `mcp.capabilities` 那个 tool 的活，
//! 而那个要带令牌、要看角色。

use xopsd::{Config, assemble};

fn main() {
    let config = Config {
        // 只是为了装配起得来。这个进程不写任何东西。
        secret_key: "00".repeat(32),
        ..Config::default()
    };
    let assembled = match assemble(&config) {
        Ok(assembled) => assembled,
        Err(error) => {
            eprintln!("装配失败：{error}");
            std::process::exit(1);
        }
    };
    let mut names: Vec<String> = assembled
        .mcp
        .registry()
        .specs()
        .map(|spec| spec.name().as_str().to_owned())
        .collect();
    names.sort();
    for name in &names {
        println!("{name}");
    }
    eprintln!("\n共 {} 个。", names.len());
    eprintln!(
        "⚠️ 表专属 tool（row.<表>.{{insert,update,delete,select}}）**不在这张表里**：\
         \n   它们按项目里的表在运行时派发，基线记的是生成规则，不是实例。"
    );
}
