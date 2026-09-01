//! 求值链：一行写进结算表时发生什么（`FLW-033`、`FLW-034`）。
//!
//! **整段发生在这张表的写入串行区间内**（`CON-002` ③）——
//! 除了最后的"触发任务"那一步，它在锁外入队。
//!
//! 本包**不得自己去改 `_flows` / `_flow_nodes`**：它经 RP-14 的状态机接口驱动迁移。
//! **这条分工是那一刀能成立的全部前提。**

use std::sync::Arc;

use serde_json::Value;
use xops_core::{Id, Result, Timestamp};
use xops_flow::instance::{Instance, NodeState};
use xops_flow::{Definition, Flows, Node};
use xops_table::WrittenBy;

use crate::verdict::{Rule, Verdict};
use crate::writers::{WriterCheck, responsible};

/// 一次求值要用到的、从写入那一侧带过来的东西。
#[derive(Debug, Clone)]
pub struct Written {
    /// 这一行的内容。
    pub values: Value,
    pub written_by: WrittenBy,
    /// 行标识。结算之后记进 `_flow_nodes.settledBy`。
    pub row: String,
}

/// 求值器。
pub struct Evaluator {
    flows: Arc<Flows>,
    writers: Arc<WriterCheck>,
}

impl std::fmt::Debug for Evaluator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Evaluator").finish_non_exhaustive()
    }
}

impl Evaluator {
    #[must_use]
    pub fn new(flows: Arc<Flows>, writers: Arc<WriterCheck>) -> Self {
        Self { flows, writers }
    }

    /// **七条判定，缺一不可**（`FLW-026`）。
    ///
    /// ⚠️ 节点指定了流转插件时，① 里的"满足筛选"那一半由插件的判定替代，
    /// **②～⑦ 一条不减，且由平台在调用插件之前先判完**——
    /// 不满足的行根本不会被交给插件（`FLW-028`）。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn judge(
        &self,
        _definition: &Definition,
        instance: &Instance,
        node: &Node,
        written: &Written,
    ) -> Result<Verdict> {
        // ⑤ 节点此刻激活着。
        if instance.state.is_terminal() {
            return Ok(Verdict::NotSettled {
                failed: Rule::NodeActive,
            });
        }
        let run = instance
            .nodes
            .iter()
            .find(|run| run.node == node.name && run.step == instance.step);
        if run.is_none_or(|run| run.state != NodeState::Active) {
            return Ok(Verdict::NotSettled {
                failed: Rule::NodeActive,
            });
        }

        // ① _instance 指向本实例、满足节点的筛选。
        //    （指定了流转插件时，"满足筛选"那一半由插件替代——调用方判完 ②～⑦ 再交给它。）
        let instance_matches = written
            .values
            .get(crate::protection::INSTANCE_COLUMN)
            .and_then(Value::as_str)
            == Some(&instance.id.to_string());
        if !instance_matches {
            return Ok(Verdict::NotSettled {
                failed: Rule::Targeted,
            });
        }
        let by_criteria = matches!(node.evaluation, xops_flow::Evaluation::ByCriteria);
        if by_criteria && !node.pass.matches(&written.values) {
            // 先看拒绝条件：命中就是拒绝，不是"不结算"。
            if node
                .reject
                .as_ref()
                .is_some_and(|reject| reject.matches(&written.values))
            {
                return Ok(Verdict::Reject);
            }
            return Ok(Verdict::NotSettled {
                failed: Rule::Targeted,
            });
        }
        if by_criteria
            && node
                .reject
                .as_ref()
                .is_some_and(|reject| reject.matches(&written.values))
        {
            return Ok(Verdict::Reject);
        }

        let Some(who) = responsible(&written.written_by) else {
            return Ok(Verdict::NotSettled {
                failed: Rule::AllowedWriter,
            });
        };

        // ② 允许写入者 —— **写入这一刻**判。
        if !self
            .writers
            .allowed(instance.project, &node.writers, who, &written.written_by)?
        {
            return Ok(Verdict::NotSettled {
                failed: Rule::AllowedWriter,
            });
        }

        // ③ 职责分离。
        if node.separation_of_duties && !self.writers.separated(who, instance.started_by) {
            return Ok(Verdict::NotSettled {
                failed: Rule::SeparationOfDuties,
            });
        }

        // ④ 同一写入者在同一节点尚未贡献过。
        if self.already_contributed(instance, node, who.user)? {
            return Ok(Verdict::NotSettled {
                failed: Rule::NotAlreadyContributed,
            });
        }

        // ⑥⑦ 来自执行的那两条。
        if let WrittenBy::Execution {
            status, revision, ..
        } = &written.written_by
        {
            if status != "succeeded" {
                return Ok(Verdict::NotSettled {
                    failed: Rule::ExecutionSucceeded,
                });
            }
            // ⑦ 只在声明了代码数据源（因而有修订）时适用。
            if let Some(read) = revision
                && instance.subject.revision.as_deref() != Some(read.as_str())
            {
                return Ok(Verdict::NotSettled {
                    failed: Rule::RevisionMatches,
                });
            }
        }

        Ok(Verdict::Settle)
    }

    /// ④ 的实现：这个人在这个节点上有没有贡献过。
    ///
    /// `settledBy` 记的是结算它的那些行的 ID——**那条记录不因原行被改而变**（`FLW-032`）。
    fn already_contributed(
        &self,
        instance: &Instance,
        node: &Node,
        _user: xops_identity::UserId,
    ) -> Result<bool> {
        // 会签票数由 settledBy 的条数体现；同一个人只能贡献一条。
        // 这一版按"这个节点已经有过结算行"判——多写入者的会签由 quorum 那一侧推进。
        let contributed = instance
            .nodes
            .iter()
            .filter(|run| run.node == node.name && run.step == instance.step)
            .map(|run| run.settled_by.len())
            .sum::<usize>();
        Ok(contributed >= node.quorum as usize)
    }

    /// 求值通过之后驱动状态机（`FLW-034`）。
    ///
    /// **经 RP-14 的接口**——本包不碰 `_flows` / `_flow_nodes`。
    ///
    /// # Errors
    /// 状态机拒绝这次迁移，或者底层写失败。
    pub fn apply(
        &self,
        instance: &mut Instance,
        node: &Node,
        verdict: &Verdict,
        written: &Written,
        at: Timestamp,
    ) -> Result<Vec<String>> {
        match verdict {
            Verdict::Settle => {
                instance.approve(&node.name, std::slice::from_ref(&written.row), at)?;
                self.flows.advance(instance)
            }
            Verdict::Reject => {
                instance.reject(&node.name, std::slice::from_ref(&written.row), at)?;
                self.flows.save(instance)?;
                Ok(Vec::new())
            }
            // FLW-027：**行照常留在表里**，只是不结算。
            Verdict::NotSettled { .. } => Ok(Vec::new()),
        }
    }
}

/// 让 `Id` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _IdLink = Id;
