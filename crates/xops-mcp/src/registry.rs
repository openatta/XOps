//! 注册骨架。**本包的全部意义在这个文件里。**
//!
//! 注册一个 tool 时必须交出五样：固定形状的输入 schema · 需要的角色 · 是否幂等 ·
//! 幂等键从哪来 · 留痕形状。**交不出的注册不进来**——这是纪律的落点，不是文档里的一句话。
//!
//! 它一旦留了"可以先不声明、以后补"的口子，后面八个包会把这个口子用满。所以
//! [`ToolSpec`] 没有公开的构造方式，只有一个会当场拒绝你的 builder。

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;
use xops_audit::{AuditEnvelope, AuditLog, EventKind};
use xops_core::{Actor, Error, Id, Result, Role};
use xops_identity::{Action, Identity, ProjectId, can_in};

use crate::schema::Schema;

/// tool 的名字：`<域>.<动作>`。
///
/// `MCP-011` 说"tool 目录本身是可扩展的，**扩展方式必须统一**"——所以这里校验的是形状，
/// 不是一张写死的域清单。写死清单的话，每加一个域都要回来改这个文件，
/// 而那正是这条要避免的事。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolName(String);

/// **由外部规格定死的 tool 名。恰好这两个。**
///
/// 它们不合 `<域>.<动作>` 的形状，也带着下划线——但 `XFG-010` 写得很直白：
///
/// > 两个 tool 的名字与参数由 XForge 定死，**XOps 没有任何设计自由度，只有实现义务**。
///
/// ⚠️ **这是一张写死的白名单，不是一个开关。** 它挡住的正是"以后再往里加一个"：
/// 加一条要有一份同样级别的外部规格，而那件事在代码审查里看得见。
pub const EXTERNAL_NAMES: [&str; 2] = ["submit_approval_request", "poll_approval"];

impl ToolName {
    pub const MAX_LEN: usize = 64;

    /// # Errors
    /// 不是 `<域>.<动作>` 的形状，**且不在 [`EXTERNAL_NAMES`] 里**。
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if EXTERNAL_NAMES.contains(&name.as_str()) {
            return Ok(Self(name));
        }
        let segments: Vec<&str> = name.split('.').collect();
        let shaped = name.len() <= Self::MAX_LEN
            && segments.len() >= 2
            && segments.iter().all(|segment| {
                !segment.is_empty()
                    && segment.starts_with(|c: char| c.is_ascii_lowercase())
                    && segment
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            });
        if !shaped {
            return Err(Error::invalid(format!("tool 名要写成 <域>.<动作>：{name}")));
        }
        Ok(Self(name))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn domain(&self) -> &str {
        self.0.split('.').next().unwrap_or(&self.0)
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 调这个 tool 要什么权限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requirement {
    /// 平台级：不针对某个项目（查自己是谁、能力发现、令牌管理）。**只要令牌有效即可。**
    Platform,
    /// 项目级：参数里必须带 `project`，且调用者在那个项目里要能做这个动作。
    InProject(Action),
}

/// 幂等性（`MCP-006`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Idempotency {
    /// 只读，天然幂等。
    ReadOnly,
    /// 有副作用，**接受幂等键**：同一个键重复调用不产生第二次副作用，且返回与首次相同的结果。
    Keyed,
    /// 有副作用但不接受幂等键。**必须写清为什么**——这条路存在是为了让"忘了做幂等"
    /// 与"想清楚了不做"在代码里看起来不一样。
    NotIdempotent { reason: &'static str },
}

impl Idempotency {
    #[must_use]
    pub const fn has_effects(&self) -> bool {
        !matches!(self, Self::ReadOnly)
    }
}

/// 一个 tool 的全部声明。**没有公开构造方式**，只能经 [`ToolSpec::builder`]。
#[derive(Debug, Clone)]
pub struct ToolSpec {
    name: ToolName,
    summary: String,
    input: Schema,
    requirement: Requirement,
    idempotency: Idempotency,
    audit: EventKind,
    /// 只属于某个项目的 tool（表专属的那些）。`None` 表示到处都在。
    project: Option<ProjectId>,
    /// 回话**只给一个 `text` 类型的 content item，不带 `structuredContent`**。
    ///
    /// 这一位存在的唯一理由是 `XFG-009`：那两个 tool 的返回值形状由 XForge 定死。
    /// **别的 tool 一律不要打开它**——`structuredContent` 是调用方少写一次
    /// `JSON.parse` 的地方。
    text_only: bool,
}

impl ToolSpec {
    #[must_use]
    pub fn builder(name: &str) -> ToolSpecBuilder {
        ToolSpecBuilder {
            name: name.to_owned(),
            summary: None,
            input: None,
            requirement: None,
            idempotency: None,
            audit: None,
            project: None,
            text_only: false,
        }
    }

    #[must_use]
    pub fn name(&self) -> &ToolName {
        &self.name
    }

    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    #[must_use]
    pub fn input(&self) -> &Schema {
        &self.input
    }

    #[must_use]
    pub fn requirement(&self) -> Requirement {
        self.requirement
    }

    /// 回话只给一个 `text` content item（`XFG-009`）。
    #[must_use]
    pub const fn text_only(&self) -> bool {
        self.text_only
    }

    #[must_use]
    pub fn idempotency(&self) -> &Idempotency {
        &self.idempotency
    }

    #[must_use]
    pub fn audit(&self) -> &EventKind {
        &self.audit
    }

    /// 这个 tool 属于哪个项目。表专属 tool 只在它自己那个项目里出现。
    #[must_use]
    pub fn project(&self) -> Option<ProjectId> {
        self.project
    }

    /// 在这个项目的能力发现里该不该出现。
    #[must_use]
    pub fn visible_in(&self, project: Option<ProjectId>) -> bool {
        match (self.project, project) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(mine), Some(asked)) => mine == asked,
        }
    }

    /// MCP `tools/list` 里的一条。
    #[must_use]
    pub fn describe(&self) -> Value {
        serde_json::json!({
            "name": self.name.as_str(),
            "description": self.summary,
            "inputSchema": self.input.to_json_schema(),
        })
    }
}

/// 交不齐五样就 `build` 不出来的 builder。
#[derive(Debug, Clone)]
pub struct ToolSpecBuilder {
    name: String,
    summary: Option<String>,
    input: Option<Schema>,
    requirement: Option<Requirement>,
    idempotency: Option<Idempotency>,
    audit: Option<String>,
    project: Option<ProjectId>,
    text_only: bool,
}

impl ToolSpecBuilder {
    #[must_use]
    pub fn summary(mut self, summary: &str) -> Self {
        self.summary = Some(summary.to_owned());
        self
    }

    #[must_use]
    pub fn input(mut self, schema: Schema) -> Self {
        self.input = Some(schema);
        self
    }

    #[must_use]
    pub fn requires(mut self, requirement: Requirement) -> Self {
        self.requirement = Some(requirement);
        self
    }

    #[must_use]
    pub fn idempotency(mut self, idempotency: Idempotency) -> Self {
        self.idempotency = Some(idempotency);
        self
    }

    /// 这个 tool 的留痕形状（`xops_audit::kinds` 里的常量）。
    #[must_use]
    pub fn audits(mut self, kind: &str) -> Self {
        self.audit = Some(kind.to_owned());
        self
    }

    /// 只在某个项目里出现（表专属 tool）。
    #[must_use]
    pub fn scoped_to(mut self, project: ProjectId) -> Self {
        self.project = Some(project);
        self
    }

    /// # Errors
    /// 五样里少了任何一样，或者名字 / 事件类型不合形状。
    /// 回话**不带 `structuredContent`**（`XFG-009`）。
    ///
    /// ⚠️ **只给那两个形状定死的 tool 用。**
    #[must_use]
    pub const fn text_only(mut self) -> Self {
        self.text_only = true;
        self
    }

    pub fn build(self) -> Result<ToolSpec> {
        let missing = |what: &str| {
            Error::invalid(format!(
                "tool {} 没有声明{what}——注册骨架不接受'先不声明、以后补'",
                self.name
            ))
        };
        Ok(ToolSpec {
            name: ToolName::new(self.name.clone())?,
            summary: self.summary.ok_or_else(|| missing("说明"))?,
            input: self.input.ok_or_else(|| missing("输入 schema"))?,
            requirement: self.requirement.ok_or_else(|| missing("需要的角色"))?,
            idempotency: self.idempotency.ok_or_else(|| missing("幂等性"))?,
            audit: EventKind::new(self.audit.ok_or_else(|| missing("留痕形状"))?)?,
            project: self.project,
            text_only: self.text_only,
        })
    }
}

/// 已认证的调用上下文（`MCP-012` 四件里的第二件）。
pub struct CallContext<'a> {
    /// 调用者。**由令牌解析得出**（`TOK-007`、`I-B`）。
    pub identity: &'a Identity,
    /// 目标项目（项目级 tool 才有）。
    pub project: Option<ProjectId>,
    /// 调用者在那个项目里的角色。
    pub role: Option<Role>,
    /// 调用方给的幂等键。
    pub idempotency_key: Option<String>,
    /// 已经过 schema 校验的参数。
    pub args: Value,
    /// 目录本身。**能力发现要用它**——而 tool 拿不到 `Arc<Registry>`：
    /// 那会让目录里装着一个装着目录的东西。
    pub registry: &'a Registry,
    audit: &'a AuditLog,
}

impl<'a> CallContext<'a> {
    #[must_use]
    pub fn new(
        identity: &'a Identity,
        project: Option<ProjectId>,
        role: Option<Role>,
        idempotency_key: Option<String>,
        args: Value,
        registry: &'a Registry,
        audit: &'a AuditLog,
    ) -> Self {
        Self {
            identity,
            project,
            role,
            idempotency_key,
            args,
            registry,
            audit,
        }
    }

    /// 写入时署的名。**`I-B`：它来自令牌，不来自请求体。**
    #[must_use]
    pub fn actor(&self) -> Actor {
        self.identity.actor()
    }

    /// 取一个参数（schema 已经保证过形状）。
    #[must_use]
    pub fn arg(&self, name: &str) -> Option<&Value> {
        self.args.get(name).filter(|value| !value.is_null())
    }

    /// 取一个字符串参数。
    ///
    /// # Errors
    /// 没给或者不是字符串——schema 校验过之后这只会发生在 tool 自己写错字段名时。
    pub fn text(&self, name: &str) -> Result<&str> {
        self.arg(name)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::internal(format!("tool 要的参数 {name} 不在 schema 里")))
    }

    /// 取一个标识参数。
    ///
    /// # Errors
    /// 同上，或者不是合法标识。
    pub fn id(&self, name: &str) -> Result<Id> {
        Id::parse(self.text(name)?)
    }

    /// 构造一条属于当前项目的留痕（`MCP-012` 四件里的第四件）。
    ///
    /// # Errors
    /// 这是个平台级调用，没有项目；或者事件类型不合形状。
    pub fn envelope(&self, kind: &str, target: Id, data: Value) -> Result<AuditEnvelope> {
        let project = self
            .project
            .ok_or_else(|| Error::internal("平台级调用没有项目，留痕请用 platform_envelope"))?;
        AuditEnvelope::project_scoped(kind, project.as_id(), target, data)
    }

    /// 构造一条平台级留痕。
    ///
    /// # Errors
    /// 事件类型不合形状。
    pub fn platform_envelope(&self, kind: &str, target: Id, data: Value) -> Result<AuditEnvelope> {
        AuditEnvelope::platform(kind, self.identity.user.id.as_id(), target, data)
    }

    /// 追加一条没有业务行的留痕。
    ///
    /// # Errors
    /// 底层写失败。
    pub fn record(&self, envelope: &AuditEnvelope) -> Result<()> {
        self.audit.append(&self.actor(), envelope).map(|_| ())
    }
}

/// 一个 tool。
pub trait Tool: Send + Sync + 'static {
    fn spec(&self) -> &ToolSpec;

    /// 干活。**认证、鉴权、schema 校验、幂等、留痕都已经在外面做完了**——
    /// `MCP-012`：注册一个 tool 即自动获得全套纪律，各域不需要自己写这些。
    ///
    /// # Errors
    /// 业务上的失败。
    fn call(&self, context: &CallContext<'_>) -> Result<Value>;
}

/// 运行时才知道有哪些 tool 的那一类来源。
///
/// **`MCP-005` 的落点**：每张表建好之后平台为它派发一组专属的读写 tool，
/// 而"现在有哪些表"是运行时的事。派发**机制**归 RP-04，这里只提供它必须落在其上的位。
///
/// ⚠️ 派发出来的 tool 与静态注册的**走同一条路**——同样要交出五样、同样过 schema 校验、
/// 同样按角色裁剪。这正是 `MCP-005` 不构成对 `MCP-004` 破例的原因。
pub trait ToolSource: Send + Sync + 'static {
    /// 此刻有哪些。
    ///
    /// # Errors
    /// 目录读不出来。
    fn tools(&self) -> Result<Vec<Arc<dyn Tool>>>;
}

/// tool 目录。
#[derive(Default)]
pub struct Registry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
    sources: Vec<Arc<dyn ToolSource>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("tools", &self.tools.keys())
            .finish()
    }
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// # Errors
    /// 重名。
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        let name = tool.spec().name().as_str().to_owned();
        if self.tools.contains_key(&name) {
            return Err(Error::conflict(format!("tool {name} 已经注册过了")));
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    /// 接一个动态来源（`MCP-005`）。
    pub fn add_source(&mut self, source: Arc<dyn ToolSource>) {
        self.sources.push(source);
    }

    /// 按名字找。**先找静态的，再问动态来源**——派发出来的 tool 盖不掉注册过的。
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        if let Some(tool) = self.tools.get(name) {
            return Some(Arc::clone(tool));
        }
        self.sources
            .iter()
            .filter_map(|source| source.tools().ok())
            .flatten()
            .find(|tool| tool.spec().name().as_str() == name)
    }

    /// 静态注册的加上此刻派发出来的。
    ///
    /// # Errors
    /// 某个来源读不出目录。
    pub fn all(&self) -> Result<Vec<Arc<dyn Tool>>> {
        let mut out: Vec<Arc<dyn Tool>> = self.tools.values().map(Arc::clone).collect();
        for source in &self.sources {
            out.extend(source.tools()?);
        }
        Ok(out)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// 静态注册的那些的声明。枚举验收用它——动态那部分见 [`Self::all`]。
    pub fn specs(&self) -> impl Iterator<Item = &ToolSpec> {
        self.tools.values().map(|tool| tool.spec())
    }

    /// 能力发现（`MCP-009`）：**按调用者在这个项目里的角色裁剪**。
    ///
    /// ⚠️ **裁剪不是只藏起来。** 看不到的那些调用也会失败——两处用的是同一个判定
    /// （[`allows`]），不是两份各写一遍的逻辑。
    ///
    /// # Errors
    /// 某个动态来源读不出目录。
    pub fn visible_to(
        &self,
        role: Option<Role>,
        archived: bool,
        project: Option<ProjectId>,
    ) -> Result<Vec<Arc<dyn Tool>>> {
        Ok(self
            .all()?
            .into_iter()
            .filter(|tool| allows(tool.spec(), role, archived))
            .filter(|tool| tool.spec().visible_in(project))
            .collect())
    }
}

/// 这个角色能不能调这个 tool。**能力发现与调用鉴权共用它。**
#[must_use]
pub fn allows(spec: &ToolSpec, role: Option<Role>, archived: bool) -> bool {
    match spec.requirement() {
        Requirement::Platform => true,
        Requirement::InProject(action) => role.is_some_and(|role| can_in(role, action, archived)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Field, FieldType};

    fn full() -> ToolSpecBuilder {
        ToolSpec::builder("project.create")
            .summary("建一个项目")
            .input(Schema::new().field(Field::required(
                "slug",
                FieldType::Text { max_len: 24 },
                "短名",
            )))
            .requires(Requirement::Platform)
            .idempotency(Idempotency::Keyed)
            .audits(xops_audit::kinds::PROJECT_CREATED)
    }

    #[test]
    fn 五样齐了才注册得进来() {
        assert!(full().build().is_ok());
    }

    #[test]
    fn 忘了声明角色就build不出来() {
        let mut builder = full();
        builder.requirement = None;
        let error = builder.build().unwrap_err();
        assert!(
            error.message().contains("需要的角色"),
            "{}",
            error.message()
        );
        assert!(error.message().contains("先不声明、以后补"));
    }

    #[test]
    fn 五样里少任何一样都不行() {
        for (what, mut builder) in [
            ("说明", full()),
            ("输入 schema", full()),
            ("需要的角色", full()),
            ("幂等性", full()),
            ("留痕形状", full()),
        ] {
            match what {
                "说明" => builder.summary = None,
                "输入 schema" => builder.input = None,
                "需要的角色" => builder.requirement = None,
                "幂等性" => builder.idempotency = None,
                _ => builder.audit = None,
            }
            let error = builder.build().unwrap_err();
            assert!(
                error.message().contains(what),
                "少了{what}却没报出来：{}",
                error.message()
            );
        }
    }

    #[test]
    fn 名字要有域和动作() {
        assert!(ToolName::new("project.create").is_ok());
        assert!(ToolName::new("flow.node.settle").is_ok());
        assert!(ToolName::new("create").is_err());
        assert!(ToolName::new("Project.Create").is_err());
    }

    #[test]
    fn 域取得出来() {
        assert_eq!(ToolName::new("table.add-column").unwrap().domain(), "table");
    }

    #[test]
    fn 有副作用的看得出来() {
        assert!(!Idempotency::ReadOnly.has_effects());
        assert!(Idempotency::Keyed.has_effects());
        assert!(Idempotency::NotIdempotent { reason: "试" }.has_effects());
    }

    #[test]
    fn 裁剪与鉴权用的是同一个判定() {
        let spec = ToolSpec::builder("member.add")
            .summary("加成员")
            .input(Schema::new())
            .requires(Requirement::InProject(Action::ManageMember))
            .idempotency(Idempotency::Keyed)
            .audits(xops_audit::kinds::MEMBER_ADDED)
            .build()
            .unwrap();
        assert!(allows(&spec, Some(Role::Owner), false));
        assert!(!allows(&spec, Some(Role::Member), false));
        assert!(!allows(&spec, None, false), "不是成员就不该看见");
        assert!(!allows(&spec, Some(Role::Owner), true), "归档项目里写不了");
    }

    #[test]
    fn 外部规格定死的两个名字放行别的不放() {
        // `XFG-010`：XOps 对这两个名字**没有任何设计自由度，只有实现义务**。
        assert_eq!(EXTERNAL_NAMES.len(), 2, "白名单就这两条");
        for name in EXTERNAL_NAMES {
            assert!(ToolName::new(name).is_ok(), "{name}");
        }
        // **它是白名单，不是开关**：别的下划线名字照拒。
        assert!(ToolName::new("submit_anything").is_err());
        assert!(ToolName::new("poll").is_err());
        assert!(ToolName::new("project.create").is_ok());
    }

    #[test]
    fn 只有显式声明过的tool才不带structuredcontent() {
        let plain = ToolSpec::builder("project.create")
            .summary("建项目")
            .input(Schema::new())
            .requires(Requirement::Platform)
            .idempotency(Idempotency::Keyed)
            .audits("project.created")
            .build()
            .unwrap();
        assert!(!plain.text_only(), "默认带 structuredContent");

        let external = ToolSpec::builder("poll_approval")
            .summary("查审批")
            .input(Schema::new())
            .requires(Requirement::Platform)
            .idempotency(Idempotency::ReadOnly)
            .audits("xforge.approval.polled")
            .text_only()
            .build()
            .unwrap();
        assert!(external.text_only(), "XFG-009");
    }
}
