//! 插件的读写面、安装治理、以及那份**不落表**的配置。
//!
//! 这个文件里最要紧的两条：
//!
//! > **安装时必须逐条披露它声明了哪些能力，披露不可跳过**（`PLG-007`）——
//! > 所以 [`Plugins::install`] 要调用方把披露原文交回来，交不出就装不上。
//!
//! > **配置不落在 `_plugins` 表里**（`PLG-015`）——那张表可查询，把凭据放进去等于公开。
//! > **这是全系统唯一一份不以表的形式存在的状态**（`I-A`）。

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};
use xops_audit::{AuditEnvelope, AuditLog};
use xops_core::{Actor, Clock, Error, Result, RowId, Timestamp};
use xops_identity::{Action, Directory, ProjectId, UserId};
use xops_repo::{Sealer, Secret};
use xops_store::{Store, space};
use xops_table::{TableId, Tables, WrittenBy, system};

use crate::capability::{Capabilities, Position};
use crate::carrier::Host;
use crate::net::Net;
use crate::pipeline::Generated;
use crate::plugin::{Case, CaseResult, Plugin, State};

/// 插件落在这张系统表上（`TBL-009`）。
pub const PLUGINS_TABLE: &str = system::PLUGINS;

/// 配置在 KV 里的空间。**不是一张表**——查询面够不到这里。
const CONFIG_SPACE: &str = space::META;

/// 事件类型（`PLG-011`）。
pub mod kinds {
    pub const PLUGIN_GENERATED: &str = "plugin.generated";
    pub const PLUGIN_INSTALLED: &str = "plugin.installed";
    pub const PLUGIN_DISABLED: &str = "plugin.disabled";
    pub const PLUGIN_CONFIG_WRITTEN: &str = "plugin.config.written";
}

/// 插件资产。
pub struct Plugins {
    tables: Arc<Tables>,
    store: Arc<dyn Store>,
    audit: Arc<AuditLog>,
    directory: Arc<Directory>,
    sealer: Arc<Sealer>,
    net: Arc<dyn Net>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Plugins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugins").finish_non_exhaustive()
    }
}

/// 构造它要的那几样。
pub struct Deps {
    pub tables: Arc<Tables>,
    pub store: Arc<dyn Store>,
    pub audit: Arc<AuditLog>,
    pub directory: Arc<Directory>,
    pub sealer: Arc<Sealer>,
    /// 出网后端。**没有就传 [`crate::net::Denied`]**——那样声明了出网的插件也发不出去，
    /// 而这件事在部署层面是看得见的。
    pub net: Arc<dyn Net>,
    pub clock: Arc<dyn Clock>,
}

impl Plugins {
    #[must_use]
    pub fn new(deps: Deps) -> Self {
        Self {
            tables: deps.tables,
            store: deps.store,
            audit: deps.audit,
            directory: deps.directory,
            sealer: deps.sealer,
            net: deps.net,
            clock: deps.clock,
        }
    }

    /// 收下一次生成的产出，落成一个**候选**（`PLG-005`、`PLG-006`）。
    ///
    /// 造插件任务的创建与手动触发是项目成员的事（`PLG-008`），所以这里要的是
    /// 成员级的写权限。**过不了那三样的产出根本到不了这里**——它在
    /// [`crate::pipeline::generate`] 那一步就返回了错误。
    ///
    /// # Errors
    /// 没权限 · 名字不合法 · 这个版本已经有了。
    pub fn record_candidate(&self, actor: UserId, generated: Generated) -> Result<Plugin> {
        let plugin = generated.plugin;
        self.directory
            .authorize(actor, plugin.project, Action::WriteSkill)?;
        if plugin.name.is_empty() || plugin.name.len() > 64 {
            return Err(Error::invalid("插件名要 1–64 字节"));
        }
        if self
            .find(plugin.project, &plugin.name, plugin.version)?
            .is_some()
        {
            return Err(Error::invalid(format!(
                "{}#{} 已经有了。**已安装的版本不可变**——改就是一个新版本（PLG-009）",
                plugin.name, plugin.version
            )));
        }
        self.tables.insert(
            &WrittenBy::Platform,
            Some(plugin.project),
            &Self::table()?,
            Self::to_row(&plugin)?,
        )?;
        self.record(
            actor,
            &plugin,
            kinds::PLUGIN_GENERATED,
            json!({
                "plugin": plugin.name,
                "version": plugin.version,
                "position": plugin.position,
                "capabilities": plugin.capabilities.disclose(),
                "generatedBy": plugin.generated_by,
                "cases": plugin.cases.len(),
            }),
        )?;
        Ok(plugin)
    }

    /// 安装一个候选（`PLG-007`）。**维护者及以上。**
    ///
    /// `acknowledged` 是调用方交回来的披露原文，必须与 [`Capabilities::disclose`]
    /// **逐条一致**。这条不是形式：**"不看披露直接装"必须在接口上不可表达**，
    /// 而不是靠人自觉去看一眼。
    ///
    /// # Errors
    /// 没权限 · 不是候选 · 用例没全过 · **披露对不上**。
    pub fn install(
        &self,
        actor: UserId,
        project: ProjectId,
        name: &str,
        version: u32,
        acknowledged: &[String],
    ) -> Result<Plugin> {
        self.directory
            .authorize(actor, project, Action::InstallPlugin)?;
        let (row, mut plugin) = self.require(project, name, version)?;
        plugin.check_installable()?;

        let disclosure = plugin.capabilities.disclose();
        if acknowledged != disclosure.as_slice() {
            return Err(Error::invalid(format!(
                "披露对不上，装不了。这个版本声明的是：{}（PLG-007：逐条披露，不可跳过）",
                disclosure.join("；")
            )));
        }

        plugin.state = State::Installed;
        plugin.installed_by = Some(actor);
        plugin.installed_at = Some(self.clock.now());
        self.tables.update(
            &WrittenBy::Platform,
            Some(project),
            &Self::table()?,
            row,
            Self::to_row(&plugin)?,
        )?;
        // PLG-011：谁装的、哪次执行产出的、声明了哪些能力、用例是什么、结果如何。
        self.record(
            actor,
            &plugin,
            kinds::PLUGIN_INSTALLED,
            json!({
                "plugin": plugin.name,
                "version": plugin.version,
                "installedBy": actor.to_string(),
                "generatedBy": plugin.generated_by,
                "capabilities": disclosure,
                "cases": plugin.cases,
                "caseResults": plugin.case_results,
            }),
        )?;
        Ok(plugin)
    }

    /// 停用一个版本。**维护者及以上。** 历史记录完整保留。
    ///
    /// # Errors
    /// 没权限 · 没有这个版本。
    pub fn disable(
        &self,
        actor: UserId,
        project: ProjectId,
        name: &str,
        version: u32,
    ) -> Result<Plugin> {
        self.directory
            .authorize(actor, project, Action::InstallPlugin)?;
        let (row, mut plugin) = self.require(project, name, version)?;
        plugin.state = State::Disabled;
        self.tables.update(
            &WrittenBy::Platform,
            Some(project),
            &Self::table()?,
            row,
            Self::to_row(&plugin)?,
        )?;
        self.record(
            actor,
            &plugin,
            kinds::PLUGIN_DISABLED,
            json!({
                "plugin": plugin.name,
                "version": plugin.version,
            }),
        )?;
        Ok(plugin)
    }

    /// 读一个版本：**源码与能力声明，对全体项目成员可读**（`PLG-010`、`I-T`）。
    ///
    /// # Errors
    /// 不是成员 · 没有这个版本。
    pub fn read(
        &self,
        viewer: UserId,
        project: ProjectId,
        name: &str,
        version: u32,
    ) -> Result<Plugin> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        Ok(self.require(project, name, version)?.1)
    }

    /// 列出这个项目里某一档状态的插件。
    ///
    /// # Errors
    /// 不是成员。
    pub fn list(&self, viewer: UserId, project: ProjectId, state: State) -> Result<Vec<Plugin>> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        Ok(self
            .all(project)?
            .into_iter()
            .filter(|plugin| plugin.state == state)
            .collect())
    }

    /// 一个插件的全部版本（`PLG-016`）。
    ///
    /// # Errors
    /// 不是成员。
    pub fn history(&self, viewer: UserId, project: ProjectId, name: &str) -> Result<Vec<Plugin>> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        let mut out: Vec<Plugin> = self
            .all(project)?
            .into_iter()
            .filter(|plugin| plugin.name == name)
            .collect();
        out.sort_by_key(|plugin| plugin.version);
        Ok(out)
    }

    /// 求值时要用的那个已安装版本。**引用它的流程节点用固定版本，不跟随最新**（`PLG-009`）。
    ///
    /// # Errors
    /// 没有这个版本 · 它不是"已安装"。
    pub fn resolve(&self, project: ProjectId, name: &str, version: u32) -> Result<Plugin> {
        let plugin = self.require(project, name, version)?.1;
        if !plugin.usable() {
            return Err(Error::invalid(format!(
                "{name}#{version} 不是已安装状态，引用不了"
            )));
        }
        Ok(plugin)
    }

    // ——————————————————————————————— 配置 ———————————————————————————————

    /// 写一份配置（`PLG-015`）。**项目所有者。**
    ///
    /// 整份加密后落在 KV 的一个非行空间里——**不是 `_plugins` 的一列，也不是任何一张表**。
    ///
    /// # Errors
    /// 没权限 · 没有这个插件 · 加密失败。
    pub fn write_config(
        &self,
        actor: UserId,
        project: ProjectId,
        name: &str,
        config: &BTreeMap<String, String>,
    ) -> Result<()> {
        self.directory
            .authorize(actor, project, Action::WritePluginConfig)?;
        if self.history(actor, project, name)?.is_empty() {
            return Err(Error::not_found("不存在"));
        }
        let plain = serde_json::to_string(config)
            .map_err(|error| Error::internal(format!("配置装不下：{error}")))?;
        let sealed = self.sealer.seal(&Secret::new(plain))?;
        let bytes = serde_json::to_vec(&sealed)
            .map_err(|error| Error::internal(format!("配置装不下：{error}")))?;
        self.store
            .put(CONFIG_SPACE, &Self::config_key(project, name), &bytes)?;
        // 留痕记的是**键名**，不是值 —— 值任何接口都读不出去，审计也不例外。
        let envelope = AuditEnvelope::project_scoped(
            kinds::PLUGIN_CONFIG_WRITTEN,
            project.as_id(),
            project.as_id(),
            json!({"plugin": name, "keys": config.keys().collect::<Vec<_>>()}),
        )?;
        self.audit.append(
            &Actor::User {
                user: actor.to_string(),
            },
            &envelope,
        )?;
        Ok(())
    }

    /// 这份配置有哪几个键。**只有键名，没有值**——
    /// **任何接口都读不出原文，包括所有者自己**（`PLG-015`）。
    ///
    /// # Errors
    /// 没权限 · 底层不可用。
    pub fn config_keys(
        &self,
        actor: UserId,
        project: ProjectId,
        name: &str,
    ) -> Result<Vec<String>> {
        self.directory
            .authorize(actor, project, Action::WritePluginConfig)?;
        Ok(self
            .open_config(project, name)?
            .map(|config| config.into_keys().collect())
            .unwrap_or_default())
    }

    /// 取出配置原文。**crate 内部专用，只在调用插件的那一刻用一次。**
    ///
    /// ⚠️ 它没有 `pub`。这不是风格问题：`PLG-015` 说"任何接口都读不出原文"，
    /// **让它跨出 crate 边界就等于开了一个接口。**
    fn open_config(
        &self,
        project: ProjectId,
        name: &str,
    ) -> Result<Option<BTreeMap<String, String>>> {
        let Some(bytes) = self
            .store
            .get(CONFIG_SPACE, &Self::config_key(project, name))?
        else {
            return Ok(None);
        };
        let sealed = serde_json::from_slice(&bytes)
            .map_err(|error| Error::internal(format!("配置读不回来：{error}")))?;
        let plain = self.sealer.open(&sealed)?;
        serde_json::from_str(plain.expose())
            .map(Some)
            .map_err(|error| Error::internal(format!("配置读不回来：{error}")))
    }

    fn config_key(project: ProjectId, name: &str) -> Vec<u8> {
        format!("plugin-config\0{project}\0{name}").into_bytes()
    }

    /// 给一个已安装插件配一份宿主。
    ///
    /// **配置只注入给该插件、且只在它声明了 `PLG-012` ② 时**——这两条都在
    /// [`PluginHost`] 里，不在调用方的自觉里。
    ///
    /// # Errors
    /// 没有这个插件 · 它不是已安装 · 它不是输出插件。
    pub fn host_for(&self, project: ProjectId, name: &str, version: u32) -> Result<Arc<dyn Host>> {
        let plugin = self.resolve(project, name, version)?;
        if plugin.position != Position::Output {
            return Err(Error::invalid("流转插件没有宿主——它一样都不能声明"));
        }
        let config = if plugin.capabilities.own_config {
            self.open_config(project, name)?.unwrap_or_default()
        } else {
            // 没声明就连读都不读一次 —— 拿不到的东西不会经过这里。
            BTreeMap::new()
        };
        Ok(Arc::new(PluginHost {
            project,
            capabilities: plugin.capabilities.clone(),
            config,
            tables: Arc::clone(&self.tables),
            net: Arc::clone(&self.net),
        }))
    }

    // ——————————————————————————————— 内部 ———————————————————————————————

    fn table() -> Result<TableId> {
        TableId::system(PLUGINS_TABLE)
    }

    fn require(&self, project: ProjectId, name: &str, version: u32) -> Result<(RowId, Plugin)> {
        self.find(project, name, version)?
            .ok_or_else(|| Error::not_found("不存在"))
    }

    fn find(
        &self,
        project: ProjectId,
        name: &str,
        version: u32,
    ) -> Result<Option<(RowId, Plugin)>> {
        for (row, values) in self.tables.rows(Some(project), &Self::table()?, 4_096)? {
            let plugin = Self::from_row(project, &values)?;
            if plugin.name == name && plugin.version == version {
                return Ok(Some((row, plugin)));
            }
        }
        Ok(None)
    }

    fn all(&self, project: ProjectId) -> Result<Vec<Plugin>> {
        self.tables
            .rows(Some(project), &Self::table()?, 4_096)?
            .into_iter()
            .map(|(_, values)| Self::from_row(project, &values))
            .collect()
    }

    fn to_row(plugin: &Plugin) -> Result<Value> {
        Ok(json!({
            "plugin": plugin.name,
            "version": plugin.version.to_string(),
            "state": match plugin.state {
                State::Candidate => "candidate",
                State::Installed => "installed",
                State::Disabled => "disabled",
            },
            "position": match plugin.position {
                Position::Transition => "transition",
                Position::Output => "output",
            },
            "entry": plugin.entry,
            "source": plugin.source,
            "capabilities": to_text(&plugin.capabilities)?,
            "tests": to_text(&plugin.cases)?,
            "testResult": to_text(&plugin.case_results)?,
            "generatedBy": plugin.generated_by,
            "installedBy": plugin.installed_by.map(|user| user.to_string()),
            "installedAt": plugin.installed_at.map(Timestamp::as_millis),
        }))
    }

    fn from_row(project: ProjectId, values: &Value) -> Result<Plugin> {
        let text = |key: &str| values.get(key).and_then(Value::as_str).unwrap_or_default();
        Ok(Plugin {
            project,
            name: text("plugin").to_owned(),
            version: text("version").parse().unwrap_or(0),
            position: match text("position") {
                "transition" => Position::Transition,
                _ => Position::Output,
            },
            entry: text("entry").to_owned(),
            source: text("source").to_owned(),
            capabilities: from_text(text("capabilities"))?,
            cases: from_text(text("tests"))?,
            case_results: from_text(text("testResult"))?,
            state: match text("state") {
                "installed" => State::Installed,
                "disabled" => State::Disabled,
                _ => State::Candidate,
            },
            generated_by: values
                .get("generatedBy")
                .and_then(Value::as_str)
                .map(str::to_owned),
            installed_by: values
                .get("installedBy")
                .and_then(Value::as_str)
                .and_then(|user| xops_core::Id::parse(user).ok())
                .map(UserId::from_id),
            installed_at: values
                .get("installedAt")
                .and_then(Value::as_i64)
                .map(Timestamp::from_millis),
        })
    }

    fn record(&self, actor: UserId, plugin: &Plugin, kind: &str, data: Value) -> Result<()> {
        let envelope = AuditEnvelope::project_scoped(
            kind,
            plugin.project.as_id(),
            plugin.project.as_id(),
            data,
        )?;
        self.audit.append(
            &Actor::User {
                user: actor.to_string(),
            },
            &envelope,
        )?;
        Ok(())
    }
}

fn to_text<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| Error::internal(format!("装不下：{error}")))
}

fn from_text<T: serde::de::DeserializeOwned + Default>(text: &str) -> Result<T> {
    if text.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str(text).map_err(|error| Error::internal(format!("读不回来：{error}")))
}

/// 给一个具体插件的宿主。**它只认这一个插件的那一份配置与那几张表。**
struct PluginHost {
    project: ProjectId,
    capabilities: Capabilities,
    config: BTreeMap<String, String>,
    tables: Arc<Tables>,
    net: Arc<dyn Net>,
}

impl Host for PluginHost {
    fn config(&self) -> Result<BTreeMap<String, String>> {
        Ok(self.config.clone())
    }

    fn read_table(&self, table: &str, limit: usize) -> Result<Value> {
        let name = if table.starts_with('_') {
            TableId::system(table)?
        } else {
            TableId::user(table)?
        };
        // 这一层是**本项目的**那一条（`PLG-012` ③）：它读不到别的项目。
        if !self.capabilities.allows_table(&name) {
            return Err(Error::invalid(format!("{table} 不在声明之列")));
        }
        let rows = self
            .tables
            .rows(Some(self.project), &name, limit)?
            .into_iter()
            .map(|(_, values)| values)
            .collect::<Vec<_>>();
        Ok(Value::Array(rows))
    }

    fn net(&self) -> &dyn Net {
        self.net.as_ref()
    }
}

/// 让这几样在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _Links = (Case, CaseResult);
