//! 插件域的八个 tool（`PLG-016`）。
//!
//! 权限分层照 `PLG-008` 分成三档，**每一档由 [`Requirement`] 直接表达**，
//! 不在 tool 体里再判一次：
//!
//! ```text
//! 项目成员    列出 · 查看候选的源码/能力声明/测试结果 · 读已安装的源码与能力声明 · 版本历史
//! 维护者      安装（含能力披露）· 停用
//! 所有者      读写插件配置 —— **且"读"只读得到键名**
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Error, Result};
use xops_identity::{Action, ProjectId};
use xops_mcp::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};

use crate::plugin::{Plugin, State};
use crate::service::{Plugins, kinds};

fn project_field() -> Field {
    Field::required("project", FieldType::Id, "项目标识")
}

fn name_field() -> Field {
    Field::required("plugin", FieldType::Text { max_len: 64 }, "插件名")
}

fn version_field() -> Field {
    Field::required(
        "version",
        FieldType::Integer,
        "版本号。**引用的是固定版本，不跟随最新**",
    )
}

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| Error::internal("项目级 tool 却没有项目"))
}

fn version_of(context: &CallContext<'_>) -> Result<u32> {
    let raw = context
        .arg("version")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::invalid("要给版本号"))?;
    u32::try_from(raw).map_err(|_| Error::invalid("版本号不合法"))
}

/// 一个插件在回话里长什么样。
///
/// **源码与能力声明一起给**（`PLG-010`、`I-T`）：能力声明是版本的一部分，
/// 单给源码等于让人看一半。
fn describe(plugin: &Plugin) -> Value {
    json!({
        "plugin": plugin.name,
        "version": plugin.version,
        "state": plugin.state,
        "position": plugin.position,
        "entry": plugin.entry,
        "source": plugin.source,
        "capabilities": plugin.capabilities,
        "disclosure": plugin.capabilities.disclose(),
        "cases": plugin.cases,
        "caseResults": plugin.case_results,
        "generatedBy": plugin.generated_by,
        "installedBy": plugin.installed_by.map(|user| user.to_string()),
    })
}

macro_rules! plugin_tool {
    ($name:ident, $tool:expr, $summary:expr, $schema:expr, $requires:expr, $idempotency:expr, $audits:expr, $call:expr) => {
        pub struct $name {
            spec: ToolSpec,
            plugins: Arc<Plugins>,
        }

        impl $name {
            /// # Errors
            /// 声明不合形状。
            pub fn new(plugins: Arc<Plugins>) -> Result<Self> {
                Ok(Self {
                    spec: ToolSpec::builder($tool)
                        .summary($summary)
                        .input($schema)
                        .requires($requires)
                        .idempotency($idempotency)
                        .audits($audits)
                        .build()?,
                    plugins,
                })
            }
        }

        impl Tool for $name {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }

            fn call(&self, context: &CallContext<'_>) -> Result<Value> {
                #[allow(clippy::redundant_closure_call, reason = "宏把 tool 体摊平在这里")]
                ($call)(&self.plugins, context)
            }
        }
    };
}

plugin_tool!(
    ListPlugins,
    "plugin.list",
    "列出这个项目里已安装的插件",
    Schema::new().field(project_field()),
    Requirement::InProject(Action::ReadProject),
    Idempotency::ReadOnly,
    kinds::PLUGIN_INSTALLED,
    |plugins: &Arc<Plugins>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let installed = plugins.list(context.identity.user.id, project, State::Installed)?;
        Ok(json!({"plugins": installed.iter().map(describe).collect::<Vec<_>>()}))
    }
);

plugin_tool!(
    ListCandidates,
    "plugin.candidates",
    "列出候选插件（还没生效的那些）",
    Schema::new().field(project_field()),
    Requirement::InProject(Action::ReadProject),
    Idempotency::ReadOnly,
    kinds::PLUGIN_GENERATED,
    |plugins: &Arc<Plugins>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let candidates = plugins.list(context.identity.user.id, project, State::Candidate)?;
        Ok(json!({"plugins": candidates.iter().map(describe).collect::<Vec<_>>()}))
    }
);

plugin_tool!(
    ShowPlugin,
    "plugin.show",
    "看一个版本的源码、能力声明与测试结果。**候选与已安装一视同仁——项目成员都读得到**",
    Schema::new()
        .field(project_field())
        .field(name_field())
        .field(version_field()),
    Requirement::InProject(Action::ReadProject),
    Idempotency::ReadOnly,
    kinds::PLUGIN_GENERATED,
    |plugins: &Arc<Plugins>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let plugin = plugins.read(
            context.identity.user.id,
            project,
            context.text("plugin")?,
            version_of(context)?,
        )?;
        Ok(describe(&plugin))
    }
);

plugin_tool!(
    PluginHistory,
    "plugin.history",
    "一个插件的全部版本",
    Schema::new().field(project_field()).field(name_field()),
    Requirement::InProject(Action::ReadProject),
    Idempotency::ReadOnly,
    kinds::PLUGIN_INSTALLED,
    |plugins: &Arc<Plugins>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let versions =
            plugins.history(context.identity.user.id, project, context.text("plugin")?)?;
        Ok(json!({"versions": versions.iter().map(describe).collect::<Vec<_>>()}))
    }
);

plugin_tool!(
    InstallPlugin,
    "plugin.install",
    "把一个候选装进项目。**必须把它声明的能力逐条交回来，披露不可跳过**",
    Schema::new()
        .field(project_field())
        .field(name_field())
        .field(version_field())
        .field(Field::required(
            "acknowledged",
            FieldType::List {
                of: Box::new(FieldType::Text { max_len: 256 }),
                max_len: 64,
            },
            "把 plugin.show 给出的 disclosure 逐条抄回来。\
             **对不上就装不了**——这条让「不看披露直接装」在接口上不可表达（PLG-007）",
        )),
    Requirement::InProject(Action::InstallPlugin),
    Idempotency::Keyed,
    kinds::PLUGIN_INSTALLED,
    |plugins: &Arc<Plugins>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let acknowledged: Vec<String> = context
            .arg("acknowledged")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let plugin = plugins.install(
            context.identity.user.id,
            project,
            context.text("plugin")?,
            version_of(context)?,
            &acknowledged,
        )?;
        Ok(describe(&plugin))
    }
);

plugin_tool!(
    DisablePlugin,
    "plugin.disable",
    "停用一个版本。历史记录完整保留",
    Schema::new()
        .field(project_field())
        .field(name_field())
        .field(version_field()),
    Requirement::InProject(Action::InstallPlugin),
    Idempotency::Keyed,
    kinds::PLUGIN_DISABLED,
    |plugins: &Arc<Plugins>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let plugin = plugins.disable(
            context.identity.user.id,
            project,
            context.text("plugin")?,
            version_of(context)?,
        )?;
        Ok(describe(&plugin))
    }
);

plugin_tool!(
    WritePluginConfig,
    "plugin.config.set",
    "写一份插件配置。**加密存储，不落在 _plugins 表里，任何接口都读不出原文**",
    Schema::new()
        .field(project_field())
        .field(name_field())
        .field(Field::required(
            "config",
            FieldType::List {
                of: Box::new(FieldType::Record {
                    fields: vec![
                        Field::required("key", FieldType::Text { max_len: 64 }, "键"),
                        Field::required("value", FieldType::Text { max_len: 4096 }, "值"),
                    ],
                }),
                max_len: 64,
            },
            "整份覆盖写。**写进去就读不出来了**——它只在调用那一刻注入给这个插件自己",
        )),
    Requirement::InProject(Action::WritePluginConfig),
    Idempotency::Keyed,
    kinds::PLUGIN_CONFIG_WRITTEN,
    |plugins: &Arc<Plugins>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let mut config = BTreeMap::new();
        for item in context
            .arg("config")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::invalid("要给一份配置"))?
        {
            let key = item
                .get("key")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::invalid("配置项要有键"))?;
            let value = item
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::invalid("配置项要有值"))?;
            config.insert(key.to_owned(), value.to_owned());
        }
        let name = context.text("plugin")?;
        plugins.write_config(context.identity.user.id, project, name, &config)?;
        // 回话里也没有值。**"任何接口都读不出原文"包括这一条回话。**
        Ok(json!({"plugin": name, "keys": config.keys().collect::<Vec<_>>()}))
    }
);

plugin_tool!(
    ReadPluginConfigKeys,
    "plugin.config.keys",
    "看这份配置有哪几个键。**只有键名，没有值——包括所有者自己也读不出原文**",
    Schema::new().field(project_field()).field(name_field()),
    Requirement::InProject(Action::WritePluginConfig),
    Idempotency::ReadOnly,
    kinds::PLUGIN_CONFIG_WRITTEN,
    |plugins: &Arc<Plugins>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let name = context.text("plugin")?;
        let keys = plugins.config_keys(context.identity.user.id, project, name)?;
        Ok(json!({"plugin": name, "keys": keys}))
    }
);

/// 注册插件域。**恰好八个**（`PLG-016`）。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, plugins: &Arc<Plugins>) -> Result<()> {
    registry.register(Arc::new(ListPlugins::new(Arc::clone(plugins))?))?;
    registry.register(Arc::new(ListCandidates::new(Arc::clone(plugins))?))?;
    registry.register(Arc::new(ShowPlugin::new(Arc::clone(plugins))?))?;
    registry.register(Arc::new(PluginHistory::new(Arc::clone(plugins))?))?;
    registry.register(Arc::new(InstallPlugin::new(Arc::clone(plugins))?))?;
    registry.register(Arc::new(DisablePlugin::new(Arc::clone(plugins))?))?;
    registry.register(Arc::new(WritePluginConfig::new(Arc::clone(plugins))?))?;
    registry.register(Arc::new(ReadPluginConfigKeys::new(Arc::clone(plugins))?))?;
    Ok(())
}
