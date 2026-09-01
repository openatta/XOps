//! 实例与节点的状态机。
//!
//! **状态机操作在本包，判断"该不该调用它"在 RP-15。** 这条分工要写死——
//! RP-15 **不得自己去改 `_flows` / `_flow_nodes`**，它经这里的接口驱动迁移。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Id, Result, Timestamp};
use xops_identity::{ProjectId, UserId};

use crate::definition::FlowId;

/// 实例标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct InstanceId(Id);

impl InstanceId {
    #[must_use]
    pub fn generate() -> Self {
        Self(Id::generate())
    }

    #[must_use]
    pub const fn from_id(id: Id) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_id(self) -> Id {
        self.0
    }
}

impl std::fmt::Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 主体 = (类型, 标识, 修订)（`FLW-012`）。**类型开放，平台不解释。**
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub kind: String,
    pub id: String,
    pub revision: Option<String>,
}

/// 实例状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstanceState {
    Running,
    Approved,
    Rejected,
    Cancelled,
    Expired,
}

impl InstanceState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// 节点状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NodeState {
    Inactive,
    Active,
    Approved,
    Rejected,
    /// **已作废。** 实例被拒绝或取消时，其余节点转成这个，**不停在"未激活"**——
    /// 停在未激活会让人以为它还会被激活。
    Void,
}

/// 一个节点在某个实例里的那一行（`_flow_nodes`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRun {
    pub instance: InstanceId,
    /// 第几步。
    pub step: usize,
    pub node: String,
    pub state: NodeState,
    pub activated_at: Option<Timestamp>,
    pub settled_at: Option<Timestamp>,
    /// 结算它的那些行的 ID。
    pub settled_by: Vec<String>,
}

/// 一个实例（`_flows`）。
///
/// ⚠️ **没有 `currentNode`**：当前激活的节点可能有多个（并行组），
/// 权威是 `_flow_nodes` 里 state=激活中的那些行（`TBL-007`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub id: InstanceId,
    pub project: ProjectId,
    pub flow: FlowId,
    /// **发起时的版本。实例始终按它走完**（`FLW-007`）。
    pub version: u32,
    pub subject: Subject,
    pub started_by: UserId,
    pub state: InstanceState,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    /// 过期时刻（`FLW-017`）。⚠️ `expiresAt` 由谁设、默认多久是 **Q12**，还没定。
    pub expires_at: Option<Timestamp>,
    /// 当前走到第几步。
    pub step: usize,
    pub nodes: Vec<NodeRun>,
}

impl Instance {
    /// 此刻激活着的那些节点。**"卡在哪"问的就是它。**
    #[must_use]
    pub fn active(&self) -> Vec<&NodeRun> {
        self.nodes
            .iter()
            .filter(|node| node.state == NodeState::Active)
            .collect()
    }

    /// 这一步的节点是不是全通过了。**并行组要全部通过才推进**（`FLW-002`）。
    #[must_use]
    pub fn step_complete(&self, step: usize) -> bool {
        let this_step: Vec<&NodeRun> = self.nodes.iter().filter(|node| node.step == step).collect();
        !this_step.is_empty()
            && this_step
                .iter()
                .all(|node| node.state == NodeState::Approved)
    }

    /// 标记一个节点通过。
    ///
    /// # Errors
    /// 实例已终态 · 没有这个节点 · 它此刻不是激活中。
    pub fn approve(&mut self, node: &str, by: &[String], at: Timestamp) -> Result<()> {
        self.settle(node, NodeState::Approved, by, at)
    }

    /// 标记一个节点被拒。**整个实例立即进入拒绝终态，其余节点转为已作废。**
    ///
    /// # Errors
    /// 同上。
    pub fn reject(&mut self, node: &str, by: &[String], at: Timestamp) -> Result<()> {
        self.settle(node, NodeState::Rejected, by, at)?;
        self.state = InstanceState::Rejected;
        self.ended_at = Some(at);
        self.void_remaining();
        Ok(())
    }

    fn settle(&mut self, node: &str, state: NodeState, by: &[String], at: Timestamp) -> Result<()> {
        if self.state.is_terminal() {
            return Err(Error::invalid("实例已经是终态了"));
        }
        let step = self.step;
        let target = self
            .nodes
            .iter_mut()
            .find(|candidate| candidate.node == node && candidate.step == step)
            .ok_or_else(|| Error::not_found("不存在"))?;
        if target.state != NodeState::Active {
            return Err(Error::invalid(format!("节点 {node} 此刻不是激活中")));
        }
        target.state = state;
        target.settled_at = Some(at);
        target.settled_by = by.to_vec();
        Ok(())
    }

    /// 这一步全通过了就推进到下一步；已经是最后一步就进"已通过"终态。
    ///
    /// 返回**这次新激活的节点名**（调用方要为它们各发一个「节点被激活」事件）。
    ///
    /// # Errors
    /// 实例已终态。
    pub fn advance(&mut self, total_steps: usize, at: Timestamp) -> Result<Vec<String>> {
        if self.state.is_terminal() {
            return Err(Error::invalid("实例已经是终态了"));
        }
        if !self.step_complete(self.step) {
            return Ok(Vec::new());
        }
        let next = self.step + 1;
        if next >= total_steps {
            self.state = InstanceState::Approved;
            self.ended_at = Some(at);
            return Ok(Vec::new());
        }
        self.step = next;
        Ok(self.activate_step(next, at))
    }

    /// 激活某一步的全部节点。
    pub fn activate_step(&mut self, step: usize, at: Timestamp) -> Vec<String> {
        let mut activated = Vec::new();
        for node in self.nodes.iter_mut().filter(|node| node.step == step) {
            if node.state == NodeState::Inactive {
                node.state = NodeState::Active;
                node.activated_at = Some(at);
                activated.push(node.node.clone());
            }
        }
        activated
    }

    /// 取消。
    ///
    /// # Errors
    /// 已经是终态。
    pub fn cancel(&mut self, at: Timestamp) -> Result<()> {
        self.terminate(InstanceState::Cancelled, at)
    }

    /// 过期。
    ///
    /// # Errors
    /// 已经是终态。
    pub fn expire(&mut self, at: Timestamp) -> Result<()> {
        self.terminate(InstanceState::Expired, at)
    }

    fn terminate(&mut self, state: InstanceState, at: Timestamp) -> Result<()> {
        if self.state.is_terminal() {
            return Err(Error::invalid("实例已经是终态了"));
        }
        self.state = state;
        self.ended_at = Some(at);
        self.void_remaining();
        Ok(())
    }

    /// 把还没结算的节点全部转成"已作废"。
    ///
    /// **不停在"未激活"**——停在那儿会让人以为它还会被激活。
    fn void_remaining(&mut self) {
        for node in &mut self.nodes {
            if matches!(node.state, NodeState::Inactive | NodeState::Active) {
                node.state = NodeState::Void;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(steps: &[&[&str]]) -> Instance {
        let id = InstanceId::generate();
        let mut nodes = Vec::new();
        for (step, names) in steps.iter().enumerate() {
            for name in *names {
                nodes.push(NodeRun {
                    instance: id,
                    step,
                    node: (*name).to_owned(),
                    state: NodeState::Inactive,
                    activated_at: None,
                    settled_at: None,
                    settled_by: Vec::new(),
                });
            }
        }
        let mut instance = Instance {
            id,
            project: ProjectId::generate(),
            flow: FlowId::generate(),
            version: 1,
            subject: Subject {
                kind: "bug".into(),
                id: "1".into(),
                revision: None,
            },
            started_by: UserId::generate(),
            state: InstanceState::Running,
            started_at: Timestamp::from_millis(0),
            ended_at: None,
            expires_at: None,
            step: 0,
            nodes,
        };
        instance.activate_step(0, Timestamp::from_millis(0));
        instance
    }

    #[test]
    fn 创建的同一步第一个节点随即激活() {
        let instance = instance(&[&["初审"], &["复审"]]);
        assert_eq!(instance.active().len(), 1);
        assert_eq!(instance.active()[0].node, "初审");
    }

    #[test]
    fn 并行组全部通过才推进() {
        let mut instance = instance(&[&["甲", "乙"], &["终审"]]);
        assert_eq!(instance.active().len(), 2, "并行组同时激活");

        instance
            .approve("甲", &[], Timestamp::from_millis(1))
            .unwrap();
        assert!(
            instance
                .advance(2, Timestamp::from_millis(1))
                .unwrap()
                .is_empty(),
            "还差一个"
        );
        assert_eq!(instance.step, 0);

        instance
            .approve("乙", &[], Timestamp::from_millis(2))
            .unwrap();
        let activated = instance.advance(2, Timestamp::from_millis(2)).unwrap();
        assert_eq!(activated, vec!["终审"]);
        assert_eq!(instance.step, 1);
    }

    #[test]
    fn 最后一步通过之后进已通过终态() {
        let mut instance = instance(&[&["唯一"]]);
        instance
            .approve("唯一", &[], Timestamp::from_millis(1))
            .unwrap();
        instance.advance(1, Timestamp::from_millis(1)).unwrap();
        assert_eq!(instance.state, InstanceState::Approved);
        assert!(instance.ended_at.is_some());
    }

    #[test]
    fn 拒绝即终态其余节点转为已作废() {
        let mut instance = instance(&[&["初审"], &["复审"], &["终审"]]);
        instance
            .reject("初审", &["行1".into()], Timestamp::from_millis(1))
            .unwrap();
        assert_eq!(instance.state, InstanceState::Rejected);
        let states: Vec<NodeState> = instance.nodes.iter().map(|node| node.state).collect();
        assert_eq!(states[0], NodeState::Rejected);
        assert!(
            states[1..].iter().all(|state| *state == NodeState::Void),
            "不停在未激活 —— 那会让人以为它还会被激活"
        );
    }

    #[test]
    fn 取消与过期各进各的终态() {
        let mut cancelled = instance(&[&["初审"]]);
        cancelled.cancel(Timestamp::from_millis(1)).unwrap();
        assert_eq!(cancelled.state, InstanceState::Cancelled);
        assert!(
            cancelled
                .nodes
                .iter()
                .all(|node| node.state == NodeState::Void)
        );

        let mut expired = instance(&[&["初审"]]);
        expired.expire(Timestamp::from_millis(1)).unwrap();
        assert_eq!(expired.state, InstanceState::Expired);
    }

    #[test]
    fn 终态之后什么都做不了() {
        let mut instance = instance(&[&["初审"]]);
        instance.cancel(Timestamp::from_millis(1)).unwrap();
        assert!(instance.cancel(Timestamp::from_millis(2)).is_err());
        assert!(
            instance
                .approve("初审", &[], Timestamp::from_millis(2))
                .is_err()
        );
    }

    #[test]
    fn 没激活的节点结算不了() {
        let mut instance = instance(&[&["初审"], &["复审"]]);
        assert!(
            instance
                .approve("复审", &[], Timestamp::from_millis(1))
                .is_err(),
            "它还没被激活"
        );
    }

    #[test]
    fn 卡在哪问的是激活中的那些行() {
        // _flows 没有 currentNode —— 权威是 _flow_nodes 里 state=激活中的那些。
        let instance = instance(&[&["甲", "乙"], &["终审"]]);
        let names: Vec<&str> = instance
            .active()
            .iter()
            .map(|node| node.node.as_str())
            .collect();
        assert_eq!(names, vec!["甲", "乙"], "并行组时可能是多个");
    }
}
