//! 看板域的三个 tool。**看板的定义经 MCP**（`BRD-001`）——Web 那一侧一个写路由都没有。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Error, Id, Result};
use xops_identity::{Action, ProjectId};
use xops_mcp::registry::{CallContext, Idempotency, Registry, Requirement, Tool, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};
use xops_table::TableId;

use crate::board::{BoardId, BoardSpec, Direction, Filter};
use crate::model::{ReadModel, kinds};

fn project_field() -> Field {
    Field::required("project", FieldType::Id, "项目标识")
}

fn filter_record() -> FieldType {
    FieldType::Record {
        fields: vec![
            Field::required(
                "op",
                FieldType::Enum {
                    values: vec!["equals".into(), "present".into()],
                },
                "筛选方式。**只有等值与非空两种**——再多就开始像查询语言了",
            ),
            Field::required("column", FieldType::Text { max_len: 48 }, "哪一列"),
            Field::optional("value", FieldType::Text { max_len: 256 }, "等值筛选的值"),
        ],
    }
}

pub struct DefineBoard {
    spec: ToolSpec,
    model: Arc<ReadModel>,
}

impl DefineBoard {
    /// # Errors
    /// 声明不合形状。
    pub fn new(model: Arc<ReadModel>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("board.define")
                .summary("定义一个看板：显示哪张表、按什么筛选、按什么排序、显示哪几列")
                .input(
                    Schema::new()
                        .field(project_field())
                        .field(Field::required(
                            "name",
                            FieldType::Text { max_len: 64 },
                            "看板名",
                        ))
                        .field(Field::required(
                            "table",
                            FieldType::Text {
                                max_len: TableId::MAX_LEN,
                            },
                            "显示哪张表。**_notices 不行**（BRD-004）",
                        ))
                        .field(Field::optional(
                            "filters",
                            FieldType::List {
                                of: Box::new(filter_record()),
                                max_len: 16,
                            },
                            "按什么筛选",
                        ))
                        .field(Field::optional(
                            "sort",
                            FieldType::Text { max_len: 48 },
                            "按哪一列排序",
                        ))
                        .field(Field::optional(
                            "direction",
                            FieldType::Enum {
                                values: vec!["asc".into(), "desc".into()],
                            },
                            "升序还是降序",
                        ))
                        .field(Field::optional(
                            "columns",
                            FieldType::List {
                                of: Box::new(FieldType::Text { max_len: 48 }),
                                max_len: 64,
                            },
                            "显示哪几列。不给就是全部",
                        )),
                )
                .requires(Requirement::InProject(Action::ManageBusinessObject))
                .idempotency(Idempotency::Keyed)
                .audits(kinds::BOARD_DEFINED)
                .build()?,
            model,
        })
    }
}

impl Tool for DefineBoard {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let project = require_project(context)?;
        let name = context.text("table")?;
        let table = if name.starts_with('_') {
            TableId::system(name)?
        } else {
            TableId::user(name)?
        };
        let filters = context
            .arg("filters")
            .and_then(Value::as_array)
            .map(|items| items.iter().map(parse_filter).collect::<Result<Vec<_>>>())
            .transpose()?
            .unwrap_or_default();
        let direction = match context.arg("direction").and_then(Value::as_str) {
            Some("desc") => Direction::Desc,
            _ => Direction::Asc,
        };
        let columns = context
            .arg("columns")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let board = self.model.define_board(
            context.identity.user.id,
            project,
            BoardSpec {
                name: context.text("name")?.to_owned(),
                table,
                filters,
                sort: context
                    .arg("sort")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                direction,
                columns,
            },
        )?;
        Ok(json!({"board": board.id.to_string(), "name": board.name}))
    }
}

fn parse_filter(value: &Value) -> Result<Filter> {
    let column = value["column"].as_str().unwrap_or_default().to_owned();
    match value["op"].as_str().unwrap_or_default() {
        "present" => Ok(Filter::Present { column }),
        "equals" => Ok(Filter::Equals {
            column,
            value: value
                .get("value")
                .cloned()
                .ok_or_else(|| Error::invalid("等值筛选要给 value"))?,
        }),
        other => Err(Error::invalid(format!("不认识的筛选方式：{other}"))),
    }
}

pub struct ListBoards {
    spec: ToolSpec,
    model: Arc<ReadModel>,
}

impl ListBoards {
    /// # Errors
    /// 声明不合形状。
    pub fn new(model: Arc<ReadModel>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("board.list")
                .summary("列出项目里的看板")
                .input(Schema::new().field(project_field()))
                .requires(Requirement::InProject(Action::ReadProject))
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::BOARD_DEFINED)
                .build()?,
            model,
        })
    }
}

impl Tool for ListBoards {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        let project = require_project(context)?;
        let boards = self.model.boards(context.identity.user.id, project)?;
        Ok(json!({"boards": boards}))
    }
}

pub struct ShowBoard {
    spec: ToolSpec,
    model: Arc<ReadModel>,
}

impl ShowBoard {
    /// # Errors
    /// 声明不合形状。
    pub fn new(model: Arc<ReadModel>) -> Result<Self> {
        Ok(Self {
            spec: ToolSpec::builder("board.show")
                .summary("看一个看板")
                .input(
                    Schema::new()
                        .field(project_field())
                        .field(Field::required("board", FieldType::Id, "看板标识"))
                        .field(Field::optional("limit", FieldType::Integer, "最多几行")),
                )
                .requires(Requirement::InProject(Action::ReadProject))
                .idempotency(Idempotency::ReadOnly)
                .audits(kinds::BOARD_DEFINED)
                .build()?,
            model,
        })
    }
}

impl Tool for ShowBoard {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        require_project(context)?;
        let board = BoardId::from_id(context.id("board")?);
        let limit = context
            .arg("limit")
            .and_then(Value::as_i64)
            .and_then(|limit| usize::try_from(limit).ok())
            .unwrap_or(100)
            .min(1_000);
        let view = self.model.board(context.identity.user.id, board, limit)?;
        serde_json::to_value(view)
            .map_err(|error| Error::internal(format!("看板视图装不下：{error}")))
    }
}

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| Error::internal("项目级 tool 却没有项目"))
}

/// 注册看板域。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut Registry, model: &Arc<ReadModel>) -> Result<()> {
    registry.register(Arc::new(DefineBoard::new(Arc::clone(model))?))?;
    registry.register(Arc::new(ListBoards::new(Arc::clone(model))?))?;
    registry.register(Arc::new(ShowBoard::new(Arc::clone(model))?))?;
    Ok(())
}

/// 让 `Id` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _IdLink = Id;
