//! 启动横幅。
//!
//! **它存在的唯一理由是不让降级悄悄发生**（`EXE-029`）：
//!
//! ```text
//! 引擎是桩            "跑得通、什么也没真跑" —— 查起来很慢的一种错
//! 裸跑没兑现的八条    D58 的代价可枚举，接容器那天它要缩短
//! 出网后端没接        声明了出网的插件也发不出去
//! ```
//!
//! 三条都写出来，而不是等人自己发现。

use crate::assemble::Assembled;
use crate::config::Config;

/// 一段拼好的横幅。**返回字符串而不是直接打印**——这样测试问得出它说了什么。
#[must_use]
pub fn render(config: &Config, assembled: &Assembled) -> String {
    let mut out = String::new();
    out.push_str(&format!("XOps {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str(&format!(
        "  存储      {}\n",
        if config.in_memory() {
            "内存（进程退出就没了）".to_owned()
        } else {
            config.db.clone()
        }
    ));
    out.push_str(&format!("  MCP 写入面  http://{}/mcp\n", config.mcp_addr));
    out.push_str(&format!("  只读 Web 面 http://{}/\n", config.web_addr));
    out.push_str(&format!(
        "  tool        {} 个\n",
        assembled.mcp.registry().len()
    ));
    out.push_str(&format!(
        "  日志        {:?}（{}）\n",
        xops_core::log::level(),
        xops_core::log::LEVEL_ENV
    ));

    if assembled.engine_kind == "stub" {
        out.push_str(
            "\n⚠️  执行引擎是**桩**：它跑得通，什么也没真跑。\n    \
             接真引擎设 XOPS_ATTACORE_SOCKET。\n",
        );
    } else {
        out.push_str("\n  执行引擎    attacore\n");
    }

    if !assembled.unsatisfied.is_empty() {
        out.push_str(&format!(
            "\n⚠️  **裸跑**（D58）：以下 {} 条没有兑现，不是「以后补」，是现在就没有——\n",
            assembled.unsatisfied.len()
        ));
        for (id, why) in assembled.unsatisfied {
            out.push_str(&format!("      {id}  {why}\n"));
        }
        out.push_str("    接上容器后端那天，这张表要缩短。**缩短这件事是看得见的。**\n");
    }

    out.push_str("\n⚠️  插件的出网后端没接（Denied）：声明了出网的插件也发不出去。\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 桩引擎与裸跑都要说出来() {
        // 这个测试盯的不是文案，是**这三件事没被吞掉**。
        let config = Config {
            secret_key: "0a".repeat(32),
            ..Config::default()
        };
        let assembled = crate::assemble(&config).unwrap();
        let banner = render(&config, &assembled);
        assert!(banner.contains(env!("CARGO_PKG_VERSION")), "横幅要带版本号");
        assert!(banner.contains("桩"), "引擎是桩这件事要说出来");
        assert!(banner.contains("裸跑"), "D58 的代价要说出来");
        assert!(banner.contains("出网后端没接"));
        assert!(banner.contains("/mcp"));
    }
}
