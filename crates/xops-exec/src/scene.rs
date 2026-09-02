//! 技能执行的场景。
//!
//! # 为什么不直接用引擎自带的那几个
//!
//! 场景决定 agent **手上有哪些工具**，而 `EXE-012` 说的是：
//!
//! > 两类数据源，一律显式声明，未声明的不提供：**代码**（只读挂载项目的 Git 仓，
//! > 定位到确切修订）· **外部网络**（默认拒绝，只放行声明的地址）。
//!
//! 配上 `I-I`——**一次执行的可见范围完全由其声明的数据源决定，不存在隐式扩权**——
//! 这一条就把工具集定死了：**只读那三样**。
//!
//! 装配层原先挂的是引擎的 `CodingScene`，它给 agent 配了 `Bash`、`Write` 与
//! **子代理**。三样各违一条：
//!
//! ```text
//! Bash    裸跑下它根本不在（D58），模型会试、会失败、会在回话里解释半天
//! Write   产出行由平台在容器外写（EXE-023）；技能自己写文件是绕过 schema 校验的一条路
//! Agent   一次执行 = 一个新建的 Agent，跑完就掉（EXE-016）。派出去的子代理
//!         不在这条账里：它的 token 不计入 TSK-005 的预算，它的产出也不回到
//!         这次执行的正文里
//! ```
//!
//! ⚠️ **最后那条是实测撞出来的**：让技能"读一个文件并把内容回复出来"，
//! 模型先试 `Bash`（不在）、再派子代理（产出没回来），最后回了一句
//! "我没有 shell 工具"。**执行是"成功"的，产出里一个字有用的都没有。**
//!
//! # 这个场景不做什么
//!
//! 不做出网。`WebFetch` / `WebSearch` 不在白名单里——
//! 网络白名单是**按技能声明**的（`EXE-012`），而工具白名单是按场景的：
//! 一个按场景放行的出网工具，等于让每个技能都隐式拿到出网能力。
//! 真要接出网，接的地方是执行后端按 `capabilities.network` 放行，不是这里。

use attacore_core::interface::prompt::PromptBlock;
use attacore_core::interface::scene::{AgentScene, ScenePromptContext, TokenBudget};

/// 跑一次技能用的场景。**只读三样工具，没有第四样。**
#[derive(Debug, Clone, Copy, Default)]
pub struct SkillScene;

/// 白名单。**改它等于改一次执行的可见范围**（`I-I`），
/// 所以它是一个具名常量，不是一段内联的 vec。
pub const TOOLS: [&str; 3] = ["Read", "Glob", "Grep"];

impl AgentScene for SkillScene {
    fn id(&self) -> &str {
        "xops-skill"
    }

    fn name(&self) -> &str {
        "XOps 技能执行"
    }

    fn description(&self) -> &str {
        "无人值守地跑一次技能：只读代码，不写文件，不派子代理"
    }

    /// ⚠️ **系统提示只说清楚"这是一次什么样的执行"，不说技能要做什么。**
    /// 技能内容由派工单从用户消息那一侧进来（`Worksheet::prompt`）——
    /// 平台不解释技能的语义（`SKL-007`），把它拌进系统提示就是解释了。
    fn build_system_prompt(&self, ctx: &ScenePromptContext) -> Vec<PromptBlock> {
        vec![
            PromptBlock::system_cached(
                "You run one skill, once, unattended. Nobody is watching and nobody will answer \
                 a question, so never ask one — if something is missing, say so in your final \
                 answer and stop.\n\
                 You have exactly three tools: Read, Glob, Grep. There is no shell, you cannot \
                 write files, and you cannot delegate to another agent. Do not plan around \
                 tools you do not have.\n\
                 The working directory is a read-only checkout of one repository at one exact \
                 revision. Everything outside it is off limits.\n\
                 Use the tools through the normal tool-call protocol. Never write a tool call \
                 out as text in your reply — text is stored as the deliverable, so a tool call \
                 written as text runs nothing and corrupts the result.\n\
                 Your final message IS the deliverable: it is stored verbatim as this run's \
                 output. Put the answer there and nothing else — no preamble, no narration of \
                 what you are about to do.",
            ),
            PromptBlock::system(format!(
                "Working directory: {}\nDate: {}\nOS: {}",
                ctx.cwd, ctx.date, ctx.os
            )),
        ]
    }

    fn tools(&self) -> Vec<String> {
        TOOLS.iter().map(|tool| (*tool).to_owned()).collect()
    }

    /// 明确禁掉那三样。**白名单已经不含它们了,这里再写一遍是有意的**——
    /// 白名单是和注册表求交集的,而"新注册的工具会不会漏进来"不该取决于
    /// 交集这一步的实现细节。
    fn disallowed_tools(&self) -> Vec<String> {
        [
            "Bash",
            "Write",
            "Edit",
            "Agent",
            "AskUser",
            "WebFetch",
            "WebSearch",
        ]
        .iter()
        .map(|tool| (*tool).to_owned())
        .collect()
    }

    fn token_budget(&self) -> TokenBudget {
        TokenBudget {
            compact_threshold: 100_000,
            compact_keep_recent: 20,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 工具集就是只读那三样() {
        // `EXE-012` + `I-I`:可见范围完全由声明的数据源决定。
        // 多一样都是隐式扩权 —— **这条测试就是那句话的落点**。
        assert_eq!(SkillScene.tools(), vec!["Read", "Glob", "Grep"]);
    }

    #[test]
    fn 那三样各违一条的工具被明确禁掉() {
        let 禁 = SkillScene.disallowed_tools();
        for tool in ["Bash", "Write", "Agent"] {
            assert!(禁.contains(&tool.to_owned()), "{tool} 该在禁用名单里");
        }
    }

    #[test]
    fn 提示里说得出没人在看这一件事() {
        // 无人值守:模型问一个问题就等于这次执行白跑 —— **而它会问**，
        // 除非提示里明说没人会答。
        let blocks = SkillScene.build_system_prompt(&ScenePromptContext {
            cwd: "/ws".into(),
            os: "linux".into(),
            shell: "".into(),
            home_dir: "".into(),
            date: "2026-09-02".into(),
            model_name: "m".into(),
            skills_text: None,
            mcp_instructions: None,
            session_memory: None,
            is_git: true,
            git_branch: None,
            is_worktree: false,
            git_status: None,
            language: None,
            scratchpad_dir: None,
            output_style_content: None,
            available_tools: None,
            tool_results_ever_cleared: false,
        });
        assert_eq!(blocks.len(), 2, "一块固定的、一块随环境的");
    }
}
