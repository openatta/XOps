//! 项目与成员域的 tool。**语义在 RP-02，这里只是壳。**

use std::sync::Arc;

use serde_json::{Value, json};
use xops_audit::kinds;
use xops_core::{Id, Result, Role};
use xops_identity::{Action, Directory, ProjectId, Slug, UserId};

use crate::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use crate::schema::{Field, FieldType, Schema};

/// 项目建好之后要接着做的事。
///
/// **平台建那四张系统表就挂在这里**（`TBL-005`）——而"什么是表"是 RP-04 的事，
/// 它的 crate 在 `xops-identity` 之上。所以这里只留一个位，由 `xopsd` 把 RP-04 接进来。
pub trait ProjectHook: Send + Sync + 'static {
    /// # Errors
    /// 后续动作失败。
    fn after_create(&self, project: ProjectId, slug: &str) -> Result<()>;
}

/// 什么都不做。M1 之前项目建完就完了。
#[derive(Debug, Default)]
pub struct NoHook;

impl ProjectHook for NoHook {
    fn after_create(&self, _project: ProjectId, _slug: &str) -> Result<()> {
        Ok(())
    }
}

fn describe(project: &xops_identity::Project, role: Option<Role>) -> Value {
    json!({
        "project": project.id.to_string(),
        "slug": project.slug.as_str(),
        "displayName": project.display_name,
        "createdAt": project.created_at.as_millis(),
        "archived": project.is_archived(),
        "role": role.map(|role| role.as_str()),
    })
}

pub struct CreateProject {
    spec: ToolSpec,
    directory: Arc<Directory>,
    hook: Arc<dyn ProjectHook>,
}

impl CreateProject {
    /// # Errors
    /// 声明不合形状。
    pub fn new(directory: Arc<Directory>, hook: Arc<dyn ProjectHook>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("project.create")
                .summary("建一个项目。任何用户都可以建，无需申请或审批；创建者自动成为所有者")
                .input(
                    Schema::new()
                        .field(Field::required(
                            "slug",
                            FieldType::Text {
                                max_len: Slug::MAX_LEN,
                            },
                            "短名。**全平台唯一、创建后不可变**——一旦分配就再也改不了",
                        ))
                        .field(Field::required(
                            "displayName",
                            FieldType::Text { max_len: 128 },
                            "显示名。可变，且不得作为关联键使用",
                        )),
                )
                // 建项目不需要项目内角色 —— 那时候还没有项目。
                .requires(Requirement::Platform)
                .idempotency(Idempotency::Keyed)
                .audits(kinds::PROJECT_CREATED)
                .build()?,
            directory,
            hook,
        })
    }
}

impl Tool for CreateProject {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let slug = Slug::new(context.text("slug")?)?;
        let project = self.directory.create_project(
            context.identity.user.id,
            slug.clone(),
            context.text("displayName")?,
        )?;
        self.hook.after_create(project.id, slug.as_str())?;
        Ok(describe(&project, Some(Role::Owner)))
    }
}

macro_rules! simple_tool {
    ($name:ident, $tool:expr, $summary:expr, $input:expr, $req:expr, $idem:expr, $audit:expr, $body:expr) => {
        pub struct $name {
            spec: ToolSpec,
            directory: Arc<Directory>,
        }

        impl $name {
            /// # Errors
            /// 声明不合形状。
            pub fn new(directory: Arc<Directory>) -> Result<Self> {
                Ok(Self {
                    spec: ToolSpec::builder($tool)
                        .summary($summary)
                        .input($input)
                        .requires($req)
                        .idempotency($idem)
                        .audits($audit)
                        .build()?,
                    directory,
                })
            }
        }

        impl Tool for $name {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }

            fn call(&self, context: &CallContext<'_>) -> Result<Value> {
                #[allow(clippy::redundant_closure_call)]
                ($body)(&self.directory, context)
            }
        }
    };
}

fn project_field() -> Field {
    Field::required("project", FieldType::Id, "项目标识")
}

fn user_field() -> Field {
    Field::required("user", FieldType::Id, "目标用户标识")
}

simple_tool!(
    MyProjects,
    "project.mine",
    "查询我参与的项目",
    Schema::new(),
    Requirement::Platform,
    Idempotency::ReadOnly,
    kinds::PROJECT_CREATED,
    |directory: &Arc<Directory>, context: &CallContext<'_>| {
        let listed = directory.my_projects(context.identity.user.id)?;
        Ok(json!({
            "projects": listed
                .iter()
                .map(|(project, role)| describe(project, Some(*role)))
                .collect::<Vec<_>>(),
        }))
    }
);

simple_tool!(
    DescribeProject,
    "project.describe",
    "查询项目详情。**非成员得到的与项目不存在完全一致**",
    Schema::new().field(project_field()),
    Requirement::InProject(Action::ReadProject),
    Idempotency::ReadOnly,
    kinds::PROJECT_CREATED,
    |directory: &Arc<Directory>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let record = directory.project(context.identity.user.id, project)?;
        Ok(describe(&record, context.role))
    }
);

simple_tool!(
    ArchiveProject,
    "project.archive",
    "归档项目。归档后转为只读：不再接受任何写操作，历史内容完整保留、可查询",
    Schema::new().field(project_field()),
    Requirement::InProject(Action::ManageProject),
    Idempotency::Keyed,
    kinds::PROJECT_ARCHIVED,
    |directory: &Arc<Directory>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let record = directory.archive_project(context.identity.user.id, project)?;
        Ok(describe(&record, context.role))
    }
);

simple_tool!(
    ListMembers,
    "member.list",
    "列出项目成员",
    Schema::new().field(project_field()),
    Requirement::InProject(Action::ReadProject),
    Idempotency::ReadOnly,
    kinds::MEMBER_ADDED,
    |directory: &Arc<Directory>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let members = directory.members(context.identity.user.id, project)?;
        Ok(json!({
            "members": members
                .iter()
                .map(|member| json!({
                    "user": member.user.to_string(),
                    "role": member.role.as_str(),
                    "addedAt": member.added_at.as_millis(),
                }))
                .collect::<Vec<_>>(),
        }))
    }
);

simple_tool!(
    SetMember,
    "member.set",
    "加成员或改角色。**一个项目必须始终至少有一个所有者**",
    Schema::new()
        .field(project_field())
        .field(user_field())
        .field(Field::required(
            "role",
            FieldType::Enum {
                values: vec!["member".into(), "maintainer".into(), "owner".into()],
            },
            "角色。**集合固定，不做可配置角色系统**",
        )),
    Requirement::InProject(Action::ManageMember),
    Idempotency::Keyed,
    kinds::MEMBER_ROLE_CHANGED,
    |directory: &Arc<Directory>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let user = UserId::from_id(context.id("user")?);
        let role = Role::parse(context.text("role")?)?;
        let member = directory.set_member(context.identity.user.id, project, user, role)?;
        Ok(json!({"user": member.user.to_string(), "role": member.role.as_str()}))
    }
);

simple_tool!(
    RemoveMember,
    "member.remove",
    "移除成员。**移除最后一个所有者被拒绝**",
    Schema::new().field(project_field()).field(user_field()),
    Requirement::InProject(Action::ManageMember),
    Idempotency::Keyed,
    kinds::MEMBER_REMOVED,
    |directory: &Arc<Directory>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let user = UserId::from_id(context.id("user")?);
        directory.remove_member(context.identity.user.id, project, user)?;
        Ok(json!({"removed": user.to_string()}))
    }
);

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| xops_core::Error::internal("项目级 tool 却没有项目"))
}

/// 注册项目与成员域。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, directory: &Arc<Directory>) -> Result<()> {
    register_with_hook(registry, directory, Arc::new(NoHook))
}

/// 带上"项目建好之后要做什么"。`xopsd` 用它把 RP-04 的建系统表接进来。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register_with_hook(
    registry: &mut Registry,
    directory: &Arc<Directory>,
    hook: Arc<dyn ProjectHook>,
) -> Result<()> {
    registry.register(Arc::new(CreateProject::new(Arc::clone(directory), hook)?))?;
    registry.register(Arc::new(MyProjects::new(Arc::clone(directory))?))?;
    registry.register(Arc::new(DescribeProject::new(Arc::clone(directory))?))?;
    registry.register(Arc::new(ArchiveProject::new(Arc::clone(directory))?))?;
    registry.register(Arc::new(ListMembers::new(Arc::clone(directory))?))?;
    registry.register(Arc::new(SetMember::new(Arc::clone(directory))?))?;
    registry.register(Arc::new(RemoveMember::new(Arc::clone(directory))?))?;
    Ok(())
}

/// 让 `Id` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _IdLink = Id;
