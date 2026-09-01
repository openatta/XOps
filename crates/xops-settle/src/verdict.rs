//! **七条判定，缺一不可**（`FLW-026`）。
//!
//! ```text
//! ① 行在节点的结算表上、_instance 指向本实例、满足节点的筛选
//! ② 写入者在允许写入者集合内 —— **写入这一刻**判定，不是事先
//! ③ 若要求职责分离：写入者 ≠ 实例发起人（写入者是任务时比**任务所有者**）
//! ④ 同一写入者在同一节点尚未贡献过结算行
//! ⑤ 该节点此刻处于激活状态
//! ⑥ 若来自执行：该执行正常完成（非超时/取消/失败）
//! ⑦ 若来自执行且声明了代码数据源：实际读取的修订 == 事件载荷里的主体修订
//! ```
//!
//! 每一条挡的是一件具体的事，写在各自的 [`Rule::why`] 上——
//! 那几句话是这个文件存在的理由，删掉之后这七条就成了七个看不出所以然的 if。

use serde::{Deserialize, Serialize};

/// 七条里的哪一条。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rule {
    /// ① 表、`_instance`、筛选。
    Targeted,
    /// ② 允许写入者。
    AllowedWriter,
    /// ③ 职责分离。
    SeparationOfDuties,
    /// ④ 同一写入者只结算一次。
    NotAlreadyContributed,
    /// ⑤ 节点激活中。
    NodeActive,
    /// ⑥ 执行正常完成。
    ExecutionSucceeded,
    /// ⑦ 修订对得上。
    RevisionMatches,
}

impl Rule {
    /// 七条，按编号。
    #[must_use]
    pub const fn all() -> [Self; 7] {
        [
            Self::Targeted,
            Self::AllowedWriter,
            Self::SeparationOfDuties,
            Self::NotAlreadyContributed,
            Self::NodeActive,
            Self::ExecutionSucceeded,
            Self::RevisionMatches,
        ]
    }

    /// **这一条挡的是什么。**
    #[must_use]
    pub const fn why(self) -> &'static str {
        match self {
            Self::Targeted => "这一行根本不是冲着这个节点来的",
            Self::AllowedWriter => {
                "名单表可以随时改：事件发出时 a 在名单里，任务跑了几分钟，\
                 期间 a 被移出——所以判定在**写入这一刻**，不是事先（FLW-029）"
            }
            Self::SeparationOfDuties => {
                "挡闭环自批：a 自己发起 → 触发 a 自己的任务 → 记为 a 的通过，\
                 全程一个人，审批唯一的价值（多一个人）当场归零。\
                 写入者是任务时比**任务所有者**——任务不是责任主体，人才是（FLW-030）"
            }
            Self::NotAlreadyContributed => "否则一个人连写 N 行就把会签凑齐了",
            Self::NodeActive => "实例可能已经被别处的拒绝终止了",
            Self::ExecutionSucceeded => {
                "挡产出异常：执行超时后部分产出仍被保留，那些残片不能算数（FLW-031）"
            }
            Self::RevisionMatches => {
                "挡「执行读的是仓库当前 HEAD 而不是这次要它看的那一版」（FLW-031）"
            }
        }
    }
}

/// 一次求值的结论。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Verdict {
    /// 结算这个节点。
    Settle,
    /// 不结算。**行照常留在表里**（它是一条正常数据），只是不算数（`FLW-027`）。
    NotSettled { failed: Rule },
    /// 拒绝：**整个实例立即进入拒绝终态。**
    Reject,
}

impl Verdict {
    #[must_use]
    pub const fn settles(&self) -> bool {
        matches!(self, Self::Settle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 七条一条不少() {
        assert_eq!(Rule::all().len(), 7, "FLW-026：缺一不可");
        let reasons: std::collections::BTreeSet<&str> =
            Rule::all().iter().map(|rule| rule.why()).collect();
        assert_eq!(reasons.len(), 7, "每一条挡的是不同的事");
    }

    #[test]
    fn 每一条都说得出它挡什么() {
        for rule in Rule::all() {
            assert!(rule.why().len() > 10, "{rule:?} 的理由太短，等于没写");
        }
    }

    #[test]
    fn 不结算不是拒绝() {
        assert!(
            !Verdict::NotSettled {
                failed: Rule::AllowedWriter
            }
            .settles()
        );
        assert!(!Verdict::Reject.settles());
        assert!(Verdict::Settle.settles());
    }
}
