//! 实例化。
//!
//! > **建表、建流程、装插件一步完成；之后它们就是普通对象**（`TPL-002`、`TPL-004`）。
//!
//! **本包只调用别人**：`xops-table` 建表、`xops-flow` 建流程、`xops-script` 装插件。
//! 一处都不绕——**模板带的插件走正常的候选与安装流程，不走后门直接装**（`I-K`）。
//!
//! # "中途失败不留下半套东西"是怎么做到的
//!
//! **靠预检，不靠事务。** 存储契约只有基本增删改查（`CON-012`），
//! **跨表事务这个东西不存在**，所以这里不假装有：
//!
//! ```text
//! ① 预检   表名 · 流程名 · 插件名全部先查一遍，撞了就**在动手之前**失败
//! ② 动手   建表 → 建流程 → 装插件
//! ③ 兜底   ② 里任何一步出错，**把这次已经建出来的表软删掉**，再把原错误抛出去
//! ```
//!
//! ⚠️ **③ 是尽力而为的**：软删本身也可能失败。真发生了就两件事都写在错误消息里——
//! **说清楚比假装干净有用**。

use std::sync::Arc;

use xops_core::{Error, Result, Timestamp};
use xops_flow::definition::{Definition, Node, Step};
use xops_flow::{FlowId, Flows};
use xops_identity::{Directory, ProjectId, UserId};
use xops_script::plugin::State;
use xops_script::{Plugins, generate};
use xops_table::Tables;
use xops_table::table::TableId;

use crate::catalog;
use crate::template::{FlowSpec, NodeSpec, StepSpec, Template};

/// 实例化出来的东西。**它们已经是普通对象了。**
#[derive(Debug, Clone, PartialEq)]
pub struct Instantiated {
    pub template: String,
    pub tables: Vec<TableId>,
    pub flow: Option<FlowId>,
    /// 装上的插件：名字与版本。
    pub plugins: Vec<(String, u32)>,
}

/// 模板。
pub struct Templates {
    tables: Arc<Tables>,
    flows: Arc<Flows>,
    plugins: Arc<Plugins>,
    directory: Arc<Directory>,
}

impl std::fmt::Debug for Templates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Templates").finish_non_exhaustive()
    }
}

impl Templates {
    #[must_use]
    pub fn new(
        tables: Arc<Tables>,
        flows: Arc<Flows>,
        plugins: Arc<Plugins>,
        directory: Arc<Directory>,
    ) -> Self {
        Self {
            tables,
            flows,
            plugins,
            directory,
        }
    }

    /// 列出可用模板（`TPL-002`）。
    #[must_use]
    pub fn list(&self) -> Vec<Template> {
        catalog::ALL()
    }

    /// 看一个模板的内容。
    ///
    /// # Errors
    /// 没有这个模板。
    pub fn show(&self, name: &str) -> Result<Template> {
        catalog::find(name).ok_or_else(|| Error::not_found("不存在"))
    }

    /// 在本项目实例化一个自带模板。
    ///
    /// # Errors
    /// 没有这个模板 · 没权限 · 名字撞了 · 中途出错（此时已建出来的表会被尽力撤掉）。
    pub fn instantiate(
        &self,
        actor: UserId,
        project: ProjectId,
        name: &str,
    ) -> Result<Instantiated> {
        let template = self.show(name)?;
        self.instantiate_template(actor, project, &template)
    }

    /// 实例化一个给定的模板。
    ///
    /// **这条路径与 [`Self::instantiate`] 完全一样**——自带模板没有任何捷径。
    ///
    /// # Errors
    /// 同上。
    pub fn instantiate_template(
        &self,
        actor: UserId,
        project: ProjectId,
        template: &Template,
    ) -> Result<Instantiated> {
        self.preflight(actor, project, template)?;

        let mut built: Vec<TableId> = Vec::new();
        let result = self.build(actor, project, template, &mut built);
        match result {
            Ok(instantiated) => Ok(instantiated),
            Err(error) => Err(self.undo(actor, project, &built, error)),
        }
    }

    // ——————————————————————————————— 内部 ———————————————————————————————

    /// **在动手之前**把能查的都查一遍。
    fn preflight(&self, actor: UserId, project: ProjectId, template: &Template) -> Result<()> {
        // 权限：建表 + 定义流程 + 安装插件。**装插件要维护者**（`PLG-008`），
        // 所以一次实例化整体上要维护者及以上。这里逐条问，错误消息才说得清缺哪一样。
        self.directory
            .authorize(actor, project, xops_identity::Action::CreateTable)?;
        if template.flow.is_some() {
            self.directory
                .authorize(actor, project, xops_identity::Action::DefineFlow)?;
        }
        if !template.plugins.is_empty() {
            self.directory
                .authorize(actor, project, xops_identity::Action::InstallPlugin)?;
        }

        for spec in &template.tables {
            let id = TableId::user(&spec.name)?;
            if self.tables.describe_internal(Some(project), &id).is_ok() {
                return Err(Error::invalid(format!(
                    "{} 已经有了。**不覆盖**——同一个项目里实例化两套时，撞名就明确失败",
                    spec.name
                )));
            }
        }
        if let Some(flow) = &template.flow
            && self
                .flows
                .list(actor, project)?
                .iter()
                .any(|existing| existing.name == flow.name)
        {
            return Err(Error::invalid(format!("流程「{}」已经有了", flow.name)));
        }
        for plugin in &template.plugins {
            if self
                .plugins
                .history(actor, project, &plugin.name)?
                .iter()
                .any(|existing| existing.version == 1)
            {
                return Err(Error::invalid(format!("插件「{}」已经有了", plugin.name)));
            }
        }
        Ok(())
    }

    fn build(
        &self,
        actor: UserId,
        project: ProjectId,
        template: &Template,
        built: &mut Vec<TableId>,
    ) -> Result<Instantiated> {
        // ① 表。
        for spec in &template.tables {
            let id = TableId::user(&spec.name)?;
            let columns = spec
                .columns
                .iter()
                .map(crate::template::ColumnSpec::to_column)
                .collect::<Result<Vec<_>>>()?;
            self.tables
                .create(actor, project, id.clone(), spec.protection, columns)?;
            built.push(id);
        }

        // ② 插件。**先于流程**——流程的节点要引用它，引用一个还没装上的插件说不通。
        let mut plugins = Vec::new();
        for spec in &template.plugins {
            let generated = generate(
                project,
                &spec.name,
                1,
                spec.position,
                &spec.entry,
                &spec.source,
                spec.capabilities.clone(),
                spec.cases.clone(),
                None,
                Some(format!("template:{}", template.name)),
            )?;
            let candidate = self.plugins.record_candidate(actor, generated)?;
            debug_assert_eq!(candidate.state, State::Candidate);
            // **含能力披露，一条都不能少**（`PLG-007`）——模板不是绕过它的路。
            let disclosure = candidate.capabilities.disclose();
            self.plugins
                .install(actor, project, &spec.name, 1, &disclosure)?;
            plugins.push((spec.name.clone(), 1));
        }

        // ③ 流程。
        let flow = match &template.flow {
            Some(spec) => Some(self.define_flow(actor, project, spec)?),
            None => None,
        };

        Ok(Instantiated {
            template: template.name.clone(),
            tables: built.clone(),
            flow,
            plugins,
        })
    }

    fn define_flow(&self, actor: UserId, project: ProjectId, spec: &FlowSpec) -> Result<FlowId> {
        let definition = Definition {
            flow: FlowId::generate(),
            project,
            version: 0,
            name: spec.name.clone(),
            settlement_table: TableId::user(&spec.settlement_table)?,
            subject_table: spec
                .subject_table
                .as_deref()
                .map(TableId::user)
                .transpose()?,
            start: spec.start,
            status_columns: spec.status_columns.clone(),
            steps: spec.steps.iter().map(to_step).collect(),
            state: xops_flow::definition::State::Published,
            created_by: actor,
            created_at: Timestamp::from_millis(0),
        };
        Ok(self.flows.define(actor, definition)?.flow)
    }

    /// 尽力把这次已经建出来的表撤掉。
    fn undo(&self, actor: UserId, project: ProjectId, built: &[TableId], error: Error) -> Error {
        let mut stuck = Vec::new();
        for table in built {
            if self.tables.drop_table(actor, project, table).is_err() {
                stuck.push(table.to_string());
            }
        }
        if stuck.is_empty() {
            return error;
        }
        // **说清楚比假装干净有用。**
        Error::internal(format!(
            "{error}。撤销时这几张表没删掉，需要人来收拾：{}",
            stuck.join("、")
        ))
    }
}

fn to_step(spec: &StepSpec) -> Step {
    match spec {
        StepSpec::Single { node } => Step::Single {
            node: to_node(node),
        },
        StepSpec::Parallel { nodes } => Step::Parallel {
            nodes: nodes.iter().map(to_node).collect(),
        },
    }
}

fn to_node(spec: &NodeSpec) -> Node {
    Node {
        name: spec.name.clone(),
        pass: spec.pass.clone(),
        quorum: spec.quorum,
        reject: spec.reject.clone(),
        writers: spec.writers.clone(),
        separation_of_duties: spec.separation_of_duties,
        evaluation: spec.evaluation.clone(),
    }
}
