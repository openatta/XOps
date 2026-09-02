//! 执行后端：四个执行契约，以及它们当前的隔离级别。
//!
//! D51 的原始设计是**一次性容器**：由 XOps 实现引擎四个执行契约的容器后端
//! （Process / FileSystem / Network / Sandbox），让 `EXE-002`～`EXE-011` 那一组
//! 靠**能力封锁**成立，而不是靠"技能内容里没有要求做这些事"。
//!
//! ⚠️ **本实现是裸跑（[`IsolationLevel::Bare`]），这是一个明写的决定，不是遗漏。**
//! 它的代价是可枚举的——[`IsolationLevel::unsatisfied`] 逐条列出它没兑现的需求，
//! 并且有测试盯着那张表。**把它写成数据而不是散在注释里，是为了让它不会悄悄消失**：
//! 哪天接容器后端进来，那张表要缩短，而缩短这件事是看得见的。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::worksheet::Capabilities;

/// 隔离到什么程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IsolationLevel {
    /// **裸跑**：执行直接在宿主上发生，没有一次性容器。
    Bare,
    /// 一次性容器（D51 的原始设计）。**还没有实现。**
    Container,
}

impl IsolationLevel {
    /// 这个级别**没有**兑现的需求，逐条列出来。
    ///
    /// 空表示全部兑现。
    #[must_use]
    pub const fn unsatisfied(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Bare => &[
                ("EXE-002", "没有一次性容器：执行直接在宿主上发生"),
                ("EXE-003", "文件与进程操作发生在宿主上，不在容器内"),
                (
                    "EXE-007",
                    "对外网络不是默认拒绝——白名单只被记录，没有被强制",
                ),
                ("EXE-008", "没有 CPU / 内存 / 磁盘上限，超限不会被终止"),
                ("EXE-009", "两次执行共用宿主文件系统，互相看得见"),
                ("EXE-011", "没有容器可销毁，残留由执行方自己收拾"),
                (
                    "EXE-028",
                    "隔离的主动攻击测试无从谈起：被攻击的那道墙还没有",
                ),
                ("EXE-029", "四个执行契约的容器后端没有实现"),
            ],
            Self::Container => &[],
        }
    }

    /// 引擎那一侧的已知缺口。**与隔离无关，所以不在 `unsatisfied` 里**，
    /// 但同样要在启动时说出来:一个看着像真数、实际少算的预算，
    /// 比没有预算更糟——**没有预算至少不会有人以为它在管事**。
    #[must_use]
    pub const fn engine_gaps() -> &'static [(&'static str, &'static str)] {
        &[(
            "TSK-005",
            concat!(
                "单次 token 用量是**少算的**：引擎只交回最后一次 API 调用的用量，",
                "一个回合里前几趟不在这个数里，预算因此咬不住。\n",
                "               上游拿不出累计数，**这条要走 ISSUE**——见 docs/upstream-issues/",
            ),
        )]
    }

    /// 仍然兑现的那几条，也写下来——它们不靠容器。
    #[must_use]
    pub const fn still_held(self) -> &'static [(&'static str, &'static str)] {
        &[
            (
                "EXE-004",
                "执行方没有任何写表的路径：它拿不到 MCP 令牌，也没有到 XOps 的网络路径",
            ),
            (
                "EXE-010",
                "派工单里不含任何凭据，执行方的环境是显式构造的，不继承宿主环境",
            ),
            (
                "EXE-015",
                "模型凭据在 attacored 那一侧，从结构上进不了执行方",
            ),
            (
                "EXE-013",
                "表数据不是数据源：派工单里没有表，只有调用方查好传进来的输入",
            ),
            ("EXE-014", "XOps 与引擎是两个分立进程，之间只有一条执行契约"),
            ("EXE-030", "引擎不可用时如实归入引擎错误类，绝不就地跑"),
        ]
    }
}

/// 进程契约。
pub trait ProcessProvider: Send + Sync + 'static {
    /// 这次执行的进程该在哪儿跑。
    ///
    /// AttaCore 把 `exec.process` 的用途原文写成"**which machine the work happens on**"——
    /// 换后端换的就是这个答案。
    fn placement(&self) -> IsolationLevel;
}

/// 文件系统契约。
pub trait FileSystemProvider: Send + Sync + 'static {
    /// 这次执行看得见哪些路径。**未声明的一律看不见**（`EXE-006`）。
    fn visible_paths(&self, capabilities: &Capabilities) -> Vec<PathBuf>;
}

/// 网络契约。
pub trait NetworkProvider: Send + Sync + 'static {
    /// 允不允许连这个主机。
    fn allows(&self, capabilities: &Capabilities, host: &str) -> bool;

    /// 这条判定是**被强制**的，还是只是被记录下来。
    ///
    /// 裸跑下它只是记录——把这件事写成一个方法，是为了让调用方问得出来，
    /// 而不是以为白名单生效了。
    fn enforced(&self) -> bool;
}

/// 沙箱契约。
pub trait SandboxProvider: Send + Sync + 'static {
    fn level(&self) -> IsolationLevel;

    /// 跑完之后要不要销毁什么。
    fn teardown(&self) -> bool;
}

/// 裸跑后端。
#[derive(Debug, Clone, Copy, Default)]
pub struct BareBackend;

impl ProcessProvider for BareBackend {
    fn placement(&self) -> IsolationLevel {
        IsolationLevel::Bare
    }
}

impl FileSystemProvider for BareBackend {
    fn visible_paths(&self, capabilities: &Capabilities) -> Vec<PathBuf> {
        // 声明了什么就是什么。裸跑下这只是一份**声明**，宿主上它拦不住越界读——
        // 那正是 IsolationLevel::Bare 的 unsatisfied 里写着 EXE-003 的原因。
        capabilities.workspace.iter().cloned().collect()
    }
}

impl NetworkProvider for BareBackend {
    fn allows(&self, capabilities: &Capabilities, host: &str) -> bool {
        capabilities.network.iter().any(|allowed| allowed == host)
    }

    fn enforced(&self) -> bool {
        false
    }
}

impl SandboxProvider for BareBackend {
    fn level(&self) -> IsolationLevel {
        IsolationLevel::Bare
    }

    fn teardown(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 裸跑没兑现的那几条是可枚举的() {
        let missing = IsolationLevel::Bare.unsatisfied();
        assert!(!missing.is_empty(), "裸跑当然有没兑现的");
        // 这一条最要紧：主动攻击测试没有对象。
        assert!(missing.iter().any(|(id, _)| *id == "EXE-028"));
        assert!(missing.iter().any(|(id, _)| *id == "EXE-002"));
        assert!(IsolationLevel::Container.unsatisfied().is_empty());
    }

    #[test]
    fn 不靠容器也成立的那几条也写下来了() {
        let held = IsolationLevel::Bare.still_held();
        for id in ["EXE-004", "EXE-010", "EXE-015", "EXE-014", "EXE-030"] {
            assert!(held.iter().any(|(held, _)| *held == id), "少了 {id}");
        }
    }

    #[test]
    fn 白名单在裸跑下只是记录() {
        let backend = BareBackend;
        let capabilities = Capabilities {
            workspace: None,
            network: vec!["api.example.com".into()],
        };
        assert!(backend.allows(&capabilities, "api.example.com"));
        assert!(!backend.allows(&capabilities, "evil.example"));
        assert!(
            !backend.enforced(),
            "裸跑下它拦不住谁。调用方问得出来这件事，就不会以为白名单生效了"
        );
    }
}
