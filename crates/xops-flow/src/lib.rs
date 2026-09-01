//! 流程定义、校验与实例状态机。
//!
//! > **流程吃掉审批。** 节点结算的唯一形式是"结算表上出现满足条件的行"——
//! > 人写就是审批，任务或插件判定就是自动化。
//!
//! 归属：RP-14。**判断"该不该结算"在 RP-15**，本包只提供状态机；
//! RP-15 **不得自己去改 `_flows` / `_flow_nodes`**。

pub mod definition;
pub mod instance;
pub mod service;
pub mod tools;
pub mod validate;

pub use definition::{
    Criteria, Definition, Evaluation, Filter, FlowId, Node, RowQuery, Start, State, Step, Writers,
};
pub use instance::{Instance, InstanceId, InstanceState, NodeRun, NodeState, Subject};
pub use service::{FLOWS_TABLE, Flows};
pub use validate::{Finding, require_valid, validate};
