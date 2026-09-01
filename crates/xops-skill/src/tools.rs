//! 技能域的 tool。
//!
//! ⚠️ **「发起测试执行」这个 tool 注册在本包，链路在 RP-11。** 本包持有的是
//! "这个版本有没有一次成功的测试执行"这个事实与它的门；真去跑一次是 RP-11 的事。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Error, Id, Result};
use xops_identity::{Action, ProjectId};
use xops_mcp::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};

use crate::declaration::{Declaration, Input, InputType, OutputShape};
use crate::service::{Skills, kinds};
use crate::skill::{Ownership, SkillId};

fn project_field() -> Field {
    Field::required("project", FieldType::Id, "项目标识")
}

fn skill_field() -> Field {
    Field::required("skill", FieldType::Id, "技能标识")
}

/// 声明在参数里的形状。**四样，逐个字段写死**（`SKL-007`、`I-I`）。
fn declaration_record() -> FieldType {
    FieldType::Record {
        fields: vec![
            Field::optional(
                "inputs",
                FieldType::List {
                    of: Box::new(FieldType::Record {
                        fields: vec![
                            Field::required("name", FieldType::Text { max_len: 48 }, "参数名"),
                            Field::required(
                                "type",
                                FieldType::Enum {
                                    values: ["text", "integer", "bool", "id"]
                                        .iter()
                                        .map(|value| (*value).to_owned())
                                        .collect(),
                                },
                                "参数类型。**可机读**，不是一段说明文字",
                            ),
                            Field::optional("required", FieldType::Bool, "必填吗"),
                            Field::optional(
                                "description",
                                FieldType::Text { max_len: 256 },
                                "说明",
                            ),
                        ],
                    }),
                    max_len: 32,
                },
                "输入契约",
            ),
            Field::required(
                "output",
                FieldType::Enum {
                    values: ["report", "rows", "plugin-source"]
                        .iter()
                        .map(|value| (*value).to_owned())
                        .collect(),
                },
                "产出形态",
            ),
            Field::optional("needsRepository", FieldType::Bool, "要不要读代码仓"),
            Field::optional(
                "network",
                FieldType::List {
                    of: Box::new(FieldType::Text { max_len: 128 }),
                    max_len: 32,
                },
                "出网白名单。**不给就是不出网**",
            ),
            Field::required("maxDurationMillis", FieldType::Integer, "预计时长上限"),
        ],
    }
}

/// 从参数里读出声明。
///
/// # Errors
/// 类型名不认识。
pub fn parse_declaration(value: &Value) -> Result<Declaration> {
    let inputs = value["inputs"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    Ok(Input {
                        name: item["name"].as_str().unwrap_or_default().to_owned(),
                        ty: match item["type"].as_str().unwrap_or_default() {
                            "text" => InputType::Text,
                            "integer" => InputType::Integer,
                            "bool" => InputType::Bool,
                            "id" => InputType::Id,
                            other => {
                                return Err(Error::invalid(format!("不认识的参数类型：{other}")));
                            }
                        },
                        required: item["required"].as_bool().unwrap_or(false),
                        description: item["description"].as_str().unwrap_or_default().to_owned(),
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(Declaration {
        inputs,
        output: match value["output"].as_str().unwrap_or_default() {
            "report" => OutputShape::Report,
            "rows" => OutputShape::Rows,
            "plugin-source" => OutputShape::PluginSource,
            other => return Err(Error::invalid(format!("不认识的产出形态：{other}"))),
        },
        needs_repository: value["needsRepository"].as_bool().unwrap_or(false),
        network: value["network"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        max_duration_millis: u64::try_from(value["maxDurationMillis"].as_i64().unwrap_or(0))
            .unwrap_or(0),
    })
}

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| Error::internal("项目级 tool 却没有项目"))
}

macro_rules! skill_tool {
    ($name:ident, $tool:expr, $summary:expr, $input:expr, $action:expr, $idem:expr, $audit:expr, $body:expr) => {
        pub struct $name {
            spec: ToolSpec,
            skills: Arc<Skills>,
        }

        impl $name {
            /// # Errors
            /// 声明不合形状。
            pub fn new(skills: Arc<Skills>) -> Result<Self> {
                Ok(Self {
                    spec: ToolSpec::builder($tool)
                        .summary($summary)
                        .input($input)
                        .requires(Requirement::InProject($action))
                        .idempotency($idem)
                        .audits($audit)
                        .build()?,
                    skills,
                })
            }
        }

        impl Tool for $name {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }

            fn call(&self, context: &CallContext<'_>) -> Result<Value> {
                #[allow(clippy::redundant_closure_call)]
                ($body)(&self.skills, context)
            }
        }
    };
}

skill_tool!(
    CreateSkill,
    "skill.create",
    "建一个技能。**上传不执行**——这条路径上没有任何提交执行的调用",
    Schema::new()
        .field(project_field())
        .field(Field::required(
            "name",
            FieldType::Text { max_len: 64 },
            "技能名"
        ))
        .field(Field::required(
            "content",
            FieldType::LongText {
                max_len: 256 * 1024
            },
            "技能内容，自然语言。**平台不解析它的语义**",
        ))
        .field(Field::required(
            "declaration",
            declaration_record(),
            "四样声明"
        ))
        .field(Field::optional(
            "private",
            FieldType::Bool,
            "建成个人私有的"
        )),
    Action::WriteSkill,
    Idempotency::Keyed,
    kinds::SKILL_CREATED,
    |skills: &Arc<Skills>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let ownership = if context
            .arg("private")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            Ownership::Private {
                owner: context.identity.user.id,
            }
        } else {
            Ownership::Public
        };
        let declaration = parse_declaration(
            context
                .arg("declaration")
                .ok_or_else(|| Error::invalid("缺少 declaration"))?,
        )?;
        let resolved = skills.create(
            context.identity.user.id,
            project,
            context.text("name")?,
            ownership,
            context.text("content")?,
            declaration,
        )?;
        Ok(json!({
            "skill": resolved.skill.id.to_string(),
            "version": resolved.version.version,
            "state": resolved.version.state,
        }))
    }
);

skill_tool!(
    UpdateSkill,
    "skill.update",
    "改内容或声明。**产生新版本**，旧版本原样可查",
    Schema::new()
        .field(project_field())
        .field(skill_field())
        .field(Field::required(
            "content",
            FieldType::LongText {
                max_len: 256 * 1024
            },
            "内容"
        ))
        .field(Field::required(
            "declaration",
            declaration_record(),
            "四样声明"
        )),
    Action::WriteSkill,
    Idempotency::Keyed,
    kinds::SKILL_VERSIONED,
    |skills: &Arc<Skills>, context: &CallContext<'_>| {
        let declaration = parse_declaration(
            context
                .arg("declaration")
                .ok_or_else(|| Error::invalid("缺少 declaration"))?,
        )?;
        let version = skills.update(
            context.identity.user.id,
            SkillId::from_id(context.id("skill")?),
            context.text("content")?,
            declaration,
        )?;
        Ok(json!({"version": version.version, "state": version.state}))
    }
);

skill_tool!(
    PublishSkill,
    "skill.publish",
    "发布一个版本。**没有过一次成功的测试执行就发布不了**",
    Schema::new()
        .field(project_field())
        .field(skill_field())
        .field(Field::required("version", FieldType::Integer, "版本号",)),
    Action::WriteSkill,
    Idempotency::Keyed,
    kinds::SKILL_PUBLISHED,
    |skills: &Arc<Skills>, context: &CallContext<'_>| {
        let version = version_of(context)?;
        let record = skills.publish(
            context.identity.user.id,
            SkillId::from_id(context.id("skill")?),
            version,
        )?;
        Ok(json!({"version": record.version, "state": record.state}))
    }
);

skill_tool!(
    DisableSkill,
    "skill.disable",
    "停用一个版本。不再被触发，历史执行记录完整保留",
    Schema::new()
        .field(project_field())
        .field(skill_field())
        .field(Field::required("version", FieldType::Integer, "版本号",)),
    Action::WriteSkill,
    Idempotency::Keyed,
    kinds::SKILL_DISABLED,
    |skills: &Arc<Skills>, context: &CallContext<'_>| {
        let version = version_of(context)?;
        let record = skills.disable(
            context.identity.user.id,
            SkillId::from_id(context.id("skill")?),
            version,
        )?;
        Ok(json!({"version": record.version, "state": record.state}))
    }
);

skill_tool!(
    DerivePrivate,
    "skill.derive",
    "从一份技能派生一份私有副本。**是一次拷贝而不是引用**",
    Schema::new().field(project_field()).field(skill_field()),
    Action::WriteSkill,
    Idempotency::Keyed,
    kinds::SKILL_DERIVED,
    |skills: &Arc<Skills>, context: &CallContext<'_>| {
        let copy = skills.derive_private(
            context.identity.user.id,
            SkillId::from_id(context.id("skill")?),
        )?;
        Ok(json!({"skill": copy.skill.id.to_string(), "version": copy.version.version}))
    }
);

skill_tool!(
    ReadSkill,
    "skill.read",
    "查技能内容与版本历史",
    Schema::new().field(project_field()).field(skill_field()),
    Action::ReadProject,
    Idempotency::ReadOnly,
    kinds::SKILL_CREATED,
    |skills: &Arc<Skills>, context: &CallContext<'_>| {
        let skill = SkillId::from_id(context.id("skill")?);
        let resolved = skills.read(context.identity.user.id, skill)?;
        Ok(json!({
            "skill": resolved.skill.id.to_string(),
            "name": resolved.skill.name,
            "ownership": resolved.skill.ownership,
            "latest": resolved.version.version,
            "content": resolved.version.content,
            "declaration": resolved.version.declaration,
            "versions": skills
                .versions(skill)?
                .iter()
                .map(|version| json!({
                    "version": version.version,
                    "state": version.state,
                    "tested": version.tested_run.is_some(),
                }))
                .collect::<Vec<_>>(),
        }))
    }
);

skill_tool!(
    ListSkills,
    "skill.list",
    "列出我看得见的技能。**别人的私有技能不在其中**",
    Schema::new().field(project_field()),
    Action::ReadProject,
    Idempotency::ReadOnly,
    kinds::SKILL_CREATED,
    |skills: &Arc<Skills>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let listed = skills.list(context.identity.user.id, project)?;
        Ok(json!({
            "skills": listed
                .iter()
                .map(|resolved| json!({
                    "skill": resolved.skill.id.to_string(),
                    "name": resolved.skill.name,
                    "ownership": resolved.skill.ownership,
                    "state": resolved.version.state,
                }))
                .collect::<Vec<_>>(),
        }))
    }
);

fn version_of(context: &CallContext<'_>) -> Result<u32> {
    let raw = context
        .arg("version")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::invalid("缺少 version"))?;
    u32::try_from(raw).map_err(|_| Error::invalid("版本号不合法"))
}

/// 注册技能域。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, skills: &Arc<Skills>) -> Result<()> {
    registry.register(Arc::new(CreateSkill::new(Arc::clone(skills))?))?;
    registry.register(Arc::new(UpdateSkill::new(Arc::clone(skills))?))?;
    registry.register(Arc::new(PublishSkill::new(Arc::clone(skills))?))?;
    registry.register(Arc::new(DisableSkill::new(Arc::clone(skills))?))?;
    registry.register(Arc::new(DerivePrivate::new(Arc::clone(skills))?))?;
    registry.register(Arc::new(ReadSkill::new(Arc::clone(skills))?))?;
    registry.register(Arc::new(ListSkills::new(Arc::clone(skills))?))?;
    Ok(())
}

/// 让 `Id` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _IdLink = Id;
