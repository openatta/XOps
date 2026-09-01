//! 求值链：**把"一行写进了结算表"接到"这个节点算不算通过"上**。
//!
//! # 它补的是哪个口子
//!
//! `CON-002` 把一次写圈成四步同一区间，③ 就是节点求值。存储那一层为它留了
//! [`Evaluate`] 这个位——**而在这之前没有任何生产实现填过它**。
//!
//! 后果不是"慢"或"偶尔漏",是**整个流程引擎惰性**:`flow.settle` 把行写进去了，
//! 没有任何东西去求值——节点永不结算、实例永不推进、"未被采纳"的通知永不发出。
//! 七条判定各有验收、保护列有验收、插件求值有验收，**全都是对的，而且全都跑不到**。
//!
//! ⚠️ **这一类断点单元测试发现不了**:每个单元自己被构造出来、注入桩、断言行为，
//! 那证明不了它在成品里被接上了。
//!
//! # 为什么求值在锁内
//!
//! 求值读的是"这张表此刻有哪些行"。挪到锁外，两个并发的结算行会同时被判为
//! "该节点的最后一票"（`CON-002` ③ 的全部意义）。所以它在这里，不在别处。

use std::sync::Arc;

use serde_json::Value;
use xops_core::{Actor, Clock, Error, Result, RowId, TableName, WriteOp};
use xops_flow::definition::Evaluation;
use xops_flow::instance::InstanceId;
use xops_flow::{Definition, Flows};
use xops_store::{EvalScope, Evaluate, RowView, WriteRequest, Writeback};
use xops_table::{TableId, Tables, WrittenBy};

use crate::evaluate::{Evaluator, Written};
use crate::protection::INSTANCE_COLUMN;
use crate::verdict::Verdict;

/// 「插件求值」的注入位。RP-16 填它。
///
/// 本 crate 不认识载体，所以留一个位——与别处那些接缝同形。
/// **没接就等于"指定了流转插件的节点求不了值"**,而那会明确失败，不会静默通过。
pub trait PluginEvaluator: Send + Sync + 'static {
    /// 跑一次流转插件。
    ///
    /// # Errors
    /// 插件不在、位置不对、或者它交回了平台不肯代写的东西。
    ///
    /// ⚠️ **超时与异常不是 `Err`**——它们是"这个节点没过"（`PLG-013`）。
    fn transition(&self, call: &TransitionCall<'_>) -> Result<PluginVerdict>;
}

/// 一次流转插件求值要的那几样。
///
/// 打成一个结构而不是七个参数：这几样是**一次调用的完整描述**，
/// 拆开之后调用点会开始只传一部分。
#[derive(Debug, Clone)]
pub struct TransitionCall<'a> {
    pub project: xops_identity::ProjectId,
    pub plugin: &'a str,
    /// `PLG-002` 的三样输入，**都由平台在调用前查好**。
    pub instance: &'a Value,
    pub row: &'a Value,
    pub related: &'a Value,
    /// 平台只肯代写这两张（`CON-003`）。
    pub settlement: &'a TableId,
    pub subject: Option<&'a TableId>,
}

/// 插件给出的结论。
#[derive(Debug, Clone, PartialEq)]
pub struct PluginVerdict {
    /// `pass` / `fail` / `reject`。
    pub verdict: String,
    /// 要平台代写的那些行。
    pub writes: Vec<(TableId, Option<String>, Value)>,
    pub note: String,
}

/// 求值链。**把它接进 `WriteEngine::with_evaluate`。**
pub struct Chain {
    flows: Arc<Flows>,
    tables: Arc<Tables>,
    evaluator: Arc<Evaluator>,
    clock: Arc<dyn Clock>,
    plugins: Option<Arc<dyn PluginEvaluator>>,
    /// 「这一行没被采纳」要通知写入者（`FLW-027`）。**没接就等于不通知**——
    /// 而那正是"自动化失灵是静默的"要挡的事。
    notices: Option<Arc<dyn NotSettledNotifier>>,
}

/// 「这一行没被采纳，告诉写它的人」的注入位。RP-17 填它。
pub trait NotSettledNotifier: Send + Sync + 'static {
    fn not_settled(
        &self,
        project: xops_identity::ProjectId,
        instance: &str,
        table: &str,
        row: &str,
        writer: xops_identity::UserId,
        why: &str,
    );
}

impl std::fmt::Debug for Chain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chain")
            .field("plugins", &self.plugins.is_some())
            .field("notices", &self.notices.is_some())
            .finish_non_exhaustive()
    }
}

impl Chain {
    #[must_use]
    pub fn new(
        flows: Arc<Flows>,
        tables: Arc<Tables>,
        evaluator: Arc<Evaluator>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            flows,
            tables,
            evaluator,
            clock,
            plugins: None,
            notices: None,
        }
    }

    #[must_use]
    pub fn with_plugins(mut self, plugins: Arc<dyn PluginEvaluator>) -> Self {
        self.plugins = Some(plugins);
        self
    }

    #[must_use]
    pub fn with_notices(mut self, notices: Arc<dyn NotSettledNotifier>) -> Self {
        self.notices = Some(notices);
        self
    }

    /// 从物理表名倒回「哪个项目的哪张表」。
    ///
    /// 物理名的形状是 `p<项目>.<表名>`（平台全局表没有项目那一段）。
    fn split(physical: &TableName) -> Option<(xops_identity::ProjectId, TableId)> {
        let raw = physical.as_str();
        let rest = raw.strip_prefix('p')?;
        let (project, table) = rest.split_once('.')?;
        let project = xops_core::Id::parse(project)
            .ok()
            .map(xops_identity::ProjectId::from_id)?;
        let table = if table.starts_with('_') {
            TableId::system(table).ok()?
        } else {
            TableId::user(table).ok()?
        };
        Some((project, table))
    }

    /// 这张表是哪几条流程的结算表。
    fn settling(&self, physical: &TableName) -> Vec<Definition> {
        let Some((project, table)) = Self::split(physical) else {
            return Vec::new();
        };
        self.flows
            .referencing(project, &table)
            .unwrap_or_default()
            .into_iter()
            .filter(|definition| definition.settlement_table == table)
            .collect()
    }
}

impl Evaluate for Chain {
    fn scope(&self, table: &TableName) -> EvalScope {
        // 锁集合要在**开锁之前**知道（`CON-004`）。可写回的只有本流程的结算表与主体表，
        // 第三张表平台不代写——**正因为如此，锁集合在流程定义时就是已知且有限的**。
        let mut writeback = Vec::new();
        let mut update_only = Vec::new();
        for definition in self.settling(table) {
            let Some((project, _)) = Self::split(table) else {
                continue;
            };
            writeback.push(table.clone());
            if let Some(subject) = &definition.subject_table
                && let Ok(physical) = xops_table::table::physical_name(Some(project), subject)
            {
                writeback.push(physical.clone());
                // **主体表只能 update**:insert 等于让插件自己开出新实例（`I-R`）。
                update_only.push(physical);
            }
        }
        writeback.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        writeback.dedup_by(|a, b| a.as_str() == b.as_str());
        update_only.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        update_only.dedup_by(|a, b| a.as_str() == b.as_str());
        EvalScope {
            writeback_tables: writeback,
            update_only,
        }
    }

    fn evaluate(&self, request: &WriteRequest, _view: &dyn RowView) -> Result<Vec<Writeback>> {
        // 只有 insert 参与求值（D45）——update / delete 不触发、不结算。
        // 这一条由 `WriteEngine` 在调进来之前就判过了，这里不重复。
        let definitions = self.settling(&request.table);
        if definitions.is_empty() {
            return Ok(Vec::new());
        }
        let values = xops_audit::AuditEnvelope::from_payload(&request.payload)
            .map_or_else(|| request.payload.clone(), |envelope| envelope.data);

        // ① 这一行冲着哪个实例来（`FLW-022`）。没有 `_instance` 就不是冲谁来的。
        let Some(instance_id) = values
            .get(INSTANCE_COLUMN)
            .and_then(Value::as_str)
            .and_then(|raw| xops_core::Id::parse(raw).ok())
            .map(InstanceId::from_id)
        else {
            return Ok(Vec::new());
        };
        let Ok(mut instance) = self.flows.instance(instance_id) else {
            return Ok(Vec::new());
        };
        let Ok(definition) = self.flows.definition(instance.flow, instance.version) else {
            return Ok(Vec::new());
        };

        let written_by = written_by_of(&values);
        let written = Written {
            values: values.clone(),
            written_by: written_by.clone(),
            row: request.row.to_string(),
        };

        let mut writebacks = Vec::new();
        // 此刻激活着的那些节点，一个一个问。
        let active: Vec<String> = instance
            .active()
            .into_iter()
            .map(|run| run.node.clone())
            .collect();
        for name in active {
            let Some(node) = definition.node(instance.step, &name) else {
                continue;
            };
            // ②～⑦ 由平台先判完（`FLW-028`）——**不满足的行根本不会被交给插件**。
            let verdict = self
                .evaluator
                .judge(&definition, &instance, node, &written)?;
            let verdict = match (&verdict, &node.evaluation) {
                // ① 的"满足筛选"那一半由插件替代。
                (Verdict::Settle, Evaluation::Plugin { plugin, inputs }) => {
                    let (settled, mut writes) =
                        self.by_plugin(&definition, &instance, plugin, inputs, &values)?;
                    writebacks.append(&mut writes);
                    settled
                }
                _ => verdict,
            };

            if let Verdict::NotSettled { failed } = &verdict {
                // `FLW-027`：**行照常留在表里**，只是不结算——留一条痕迹并通知写入者。
                if let (Some(notices), Some(writer)) =
                    (&self.notices, crate::writers::responsible(&written_by))
                {
                    notices.not_settled(
                        definition.project,
                        &instance_id.to_string(),
                        definition.settlement_table.as_str(),
                        &written.row,
                        writer.user,
                        failed.why(),
                    );
                }
                continue;
            }
            self.evaluator
                .apply(&mut instance, node, &verdict, &written, self.clock.now())?;
        }
        Ok(writebacks)
    }
}

impl Chain {
    /// 指定了流转插件的那一半。
    fn by_plugin(
        &self,
        definition: &Definition,
        instance: &xops_flow::Instance,
        plugin: &str,
        inputs: &[xops_flow::definition::RowQuery],
        row: &Value,
    ) -> Result<(Verdict, Vec<Writeback>)> {
        let plugins = self.plugins.as_ref().ok_or_else(|| {
            Error::unavailable(format!(
                "节点指定了流转插件 {plugin}，而这个部署没有接插件载体——\
                 **明确失败，不当作通过**"
            ))
        })?;
        // `PLG-002`：三样输入**都由平台在调用前查好**，按 JSON 喂进去。
        let mut related = Vec::new();
        for query in inputs {
            let rows = self
                .tables
                .query_all(
                    Some(definition.project),
                    &query.table,
                    &[],
                    xops_table::MAX_SCAN,
                )
                .unwrap_or_default();
            for (_, values) in rows.into_iter().take(query.limit) {
                if query.criteria.matches(&values) {
                    related.push(values);
                }
            }
        }
        let instance_json = serde_json::json!({
            "instance": instance.id.to_string(),
            "subjectRow": instance.subject.id,
            "state": instance.state,
            "step": instance.step,
        });
        let outcome = plugins.transition(&TransitionCall {
            project: definition.project,
            plugin,
            instance: &instance_json,
            row,
            related: &Value::Array(related),
            settlement: &definition.settlement_table,
            subject: definition.subject_table.as_ref(),
        })?;

        let verdict = match outcome.verdict.as_str() {
            "pass" => Verdict::Settle,
            "reject" => Verdict::Reject,
            // `PLG-013`：**超时与异常一律是未通过，绝不视为通过。**
            // 归到 ①:插件替代的正是"满足筛选"那一半，所以没过就是没过在 ① 上。
            _ => Verdict::NotSettled {
                failed: crate::verdict::Rule::Targeted,
            },
        };

        let mut writebacks = Vec::new();
        if verdict != Verdict::Settle && verdict != Verdict::Reject {
            return Ok((verdict, writebacks));
        }
        for (table, row_id, values) in outcome.writes {
            let physical = xops_table::table::physical_name(Some(definition.project), &table)?;
            let (op, target) = match row_id
                .as_deref()
                .and_then(|id| xops_core::Id::parse(id).ok())
            {
                Some(id) => (WriteOp::Update, RowId::from_id(id)),
                None => (WriteOp::Insert, RowId::generate()),
            };
            let envelope = xops_audit::AuditEnvelope::project_scoped(
                "flow.writeback",
                definition.project.as_id(),
                instance.id.as_id(),
                values,
            )?;
            writebacks.push(Writeback {
                table: physical,
                op,
                row: target,
                payload: envelope.to_payload()?,
                // 署名是**那次插件求值**，四项全内联（`TBL-016`）。
                actor: Actor::Plugin {
                    plugin: plugin.to_owned(),
                },
            });
        }
        Ok((verdict, writebacks))
    }
}

/// 从行里把 `writtenBy` 解回来。解不出来就当平台写的。
fn written_by_of(values: &Value) -> WrittenBy {
    values
        .get("writtenBy")
        .cloned()
        .and_then(|raw| serde_json::from_value(raw).ok())
        .unwrap_or(WrittenBy::Platform)
}
