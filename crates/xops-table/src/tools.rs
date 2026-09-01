//! 表域的 tool，以及**表专属 tool 的派发**（`MCP-005`）。
//!
//! > "任意表 CRUD"不构成对 `MCP-004` 的破例：每张表建好之后，平台为它派发一组专属的
//! > 读写 tool，各自带**由该表 schema 生成的固定形状输入 schema**。
//! > **不存在 `{table, values: 任意形状}` 的通用写 tool。**
//!
//! 派发出来的 tool 与静态注册的走同一条路：同样交出五样、同样过 schema 校验、
//! 同样按角色裁剪。列名与类型在协议层是**被声明过的**，不是运行时随便塞的。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Error, Id, Result, RowId, WriteOp};
use xops_identity::{Action, ProjectId};
use xops_mcp::registry::{CallContext, Idempotency, Requirement, Tool, ToolSource, ToolSpec};
use xops_mcp::{Field, FieldType, Schema};

use crate::column::{BLOB_MAX, Column, ColumnType, LONG_TEXT_MAX, TEXT_MAX};
use crate::engine::{Tables, kinds};
use crate::query::Query;
use crate::table::{Kind, Protection, TableId, TableSchema};
use crate::writtenby::WrittenBy;

fn project_field() -> Field {
    Field::required("project", FieldType::Id, "项目标识")
}

fn table_field() -> Field {
    Field::required(
        "table",
        FieldType::Text {
            max_len: TableId::MAX_LEN,
        },
        "表名",
    )
}

/// 一列的声明在 tool 参数里的形状。**逐个字段声明死**，不是一个任意对象。
fn column_record() -> FieldType {
    FieldType::Record {
        fields: vec![
            Field::required("name", FieldType::Text { max_len: 48 }, "列名"),
            Field::required(
                "type",
                FieldType::Enum {
                    values: [
                        "text",
                        "long-text",
                        "integer",
                        "decimal",
                        "bool",
                        "timestamp",
                        "enum",
                        "sequence",
                        "row-ref",
                        "blob",
                        "derived",
                    ]
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect(),
                },
                "列类型。**穷举的十一种，没有 json**（TBL-021）",
            ),
            Field::optional("required", FieldType::Bool, "必填吗"),
            Field::optional("maxLen", FieldType::Integer, "文本 / 二进制的上限"),
            Field::optional(
                "enumValues",
                FieldType::List {
                    of: Box::new(FieldType::Text { max_len: 64 }),
                    max_len: 64,
                },
                "枚举的取值集合",
            ),
            Field::optional(
                "template",
                FieldType::Text { max_len: TEXT_MAX },
                "派生文本的模板，如 {project.slug}-{seq}",
            ),
        ],
    }
}

/// 从一条列声明里读出 [`Column`]。
///
/// # Errors
/// 类型名不认识，或者该给的选项没给。
pub fn parse_column(value: &Value) -> Result<Column> {
    let name = value["name"].as_str().unwrap_or_default();
    let required = value["required"].as_bool().unwrap_or(false);
    let max_len = usize::try_from(value["maxLen"].as_i64().unwrap_or(0)).unwrap_or(0);
    let ty = match value["type"].as_str().unwrap_or_default() {
        "text" => ColumnType::Text {
            max_len: if max_len == 0 { TEXT_MAX } else { max_len },
        },
        "long-text" => ColumnType::LongText {
            max_len: if max_len == 0 { LONG_TEXT_MAX } else { max_len },
        },
        "integer" => ColumnType::Integer,
        "decimal" => ColumnType::Decimal,
        "bool" => ColumnType::Bool,
        "timestamp" => ColumnType::Timestamp,
        "enum" => {
            let values: Vec<String> = value["enumValues"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            if values.is_empty() {
                return Err(Error::invalid(format!("列 {name} 是枚举，要声明取值集合")));
            }
            ColumnType::Enum { values }
        }
        "sequence" => ColumnType::Sequence,
        "row-ref" => ColumnType::RowRef,
        "blob" => ColumnType::Blob {
            max_bytes: if max_len == 0 { BLOB_MAX } else { max_len },
        },
        "derived" => {
            let template = value["template"]
                .as_str()
                .ok_or_else(|| Error::invalid(format!("列 {name} 是派生文本，要声明模板")))?;
            ColumnType::Derived {
                template: template.to_owned(),
            }
        }
        other => return Err(Error::invalid(format!("不认识的列类型：{other}"))),
    };
    Column::new(name, ty, required)
}

macro_rules! table_tool {
    ($name:ident, $tool:expr, $summary:expr, $input:expr, $action:expr, $idem:expr, $audit:expr, $body:expr) => {
        pub struct $name {
            spec: ToolSpec,
            tables: Arc<Tables>,
        }

        impl $name {
            /// # Errors
            /// 声明不合形状——只可能是这个文件被改坏了。
            pub fn new(tables: Arc<Tables>) -> Result<Self> {
                Ok(Self {
                    spec: ToolSpec::builder($tool)
                        .summary($summary)
                        .input($input)
                        .requires(Requirement::InProject($action))
                        .idempotency($idem)
                        .audits($audit)
                        .build()?,
                    tables,
                })
            }
        }

        impl Tool for $name {
            fn spec(&self) -> &ToolSpec {
                &self.spec
            }

            fn call(&self, context: &CallContext<'_>) -> Result<Value> {
                #[allow(clippy::redundant_closure_call)]
                ($body)(&self.tables, context)
            }
        }
    };
}

table_tool!(
    CreateTable,
    "table.create",
    "建一张表：声明列、列类型与保护级别",
    Schema::new()
        .field(project_field())
        .field(table_field())
        .field(Field::optional(
            "protection",
            FieldType::Enum {
                values: vec!["normal".into(), "protected".into()]
            },
            "保护级别。**建表时声明，之后不可降级**（TBL-004）",
        ))
        .field(Field::required(
            "columns",
            FieldType::List {
                of: Box::new(column_record()),
                max_len: 128
            },
            "有哪些列",
        )),
    Action::CreateTable,
    Idempotency::Keyed,
    kinds::TABLE_CREATED,
    |tables: &Arc<Tables>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let name = TableId::user(context.text("table")?)?;
        let protection = match context.arg("protection").and_then(Value::as_str) {
            Some("protected") => Protection::Protected,
            _ => Protection::Normal,
        };
        let columns = context
            .arg("columns")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::invalid("缺少必填字段 columns"))?
            .iter()
            .map(parse_column)
            .collect::<Result<Vec<_>>>()?;
        let schema = tables.create(context.identity.user.id, project, name, protection, columns)?;
        Ok(describe(&schema))
    }
);

table_tool!(
    AddColumn,
    "table.add-column",
    "加一列。新列对历史行为空；改列类型、删列、改列名都不做",
    Schema::new()
        .field(project_field())
        .field(table_field())
        .field(Field::required("column", column_record(), "要加的列")),
    Action::CreateTable,
    Idempotency::Keyed,
    kinds::TABLE_COLUMN_ADDED,
    |tables: &Arc<Tables>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let name = TableId::user(context.text("table")?)?;
        let column = parse_column(
            context
                .arg("column")
                .ok_or_else(|| Error::invalid("缺少必填字段 column"))?,
        )?;
        let schema = tables.add_column(context.identity.user.id, project, &name, column)?;
        Ok(describe(&schema))
    }
);

table_tool!(
    DescribeTable,
    "table.describe",
    "查一张表的结构",
    Schema::new().field(project_field()).field(table_field()),
    Action::ReadProject,
    Idempotency::ReadOnly,
    kinds::TABLE_CREATED,
    |tables: &Arc<Tables>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let name = table_id(context.text("table")?)?;
        let schema = tables.describe(context.identity.user.id, project, &name)?;
        Ok(describe(&schema))
    }
);

table_tool!(
    ListTables,
    "table.list",
    "列出项目里的表。**软删过的不在其中**",
    Schema::new().field(project_field()),
    Action::ReadProject,
    Idempotency::ReadOnly,
    kinds::TABLE_CREATED,
    |tables: &Arc<Tables>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let listed = tables.list(context.identity.user.id, project)?;
        Ok(json!({"tables": listed.iter().map(describe).collect::<Vec<_>>()}))
    }
);

table_tool!(
    DropTable,
    "table.drop",
    "删表。**软删**：从列出结果中消失、专属 tool 停止派发，行与事件仍在、单行历史仍可查",
    Schema::new().field(project_field()).field(table_field()),
    Action::ManageBusinessObject,
    Idempotency::Keyed,
    kinds::TABLE_DROPPED,
    |tables: &Arc<Tables>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let name = TableId::user(context.text("table")?)?;
        tables.drop_table(context.identity.user.id, project, &name)?;
        Ok(json!({"dropped": name.as_str()}))
    }
);

table_tool!(
    RowHistory,
    "table.history",
    "查一行的完整历史：谁、何时、改了什么",
    Schema::new()
        .field(project_field())
        .field(table_field())
        .field(Field::required("row", FieldType::Id, "行标识")),
    Action::ReadProject,
    Idempotency::ReadOnly,
    kinds::TABLE_CREATED,
    |tables: &Arc<Tables>, context: &CallContext<'_>| {
        let project = require_project(context)?;
        let name = table_id(context.text("table")?)?;
        tables.describe(context.identity.user.id, project, &name)?;
        let row = RowId::from_id(context.id("row")?);
        let history = tables.history(Some(project), &name, row)?;
        Ok(json!({
            "versions": history
                .iter()
                .map(|version| json!({
                    "seq": version.seq,
                    "op": version.op,
                    "at": version.at.as_millis(),
                    "writtenBy": version.written_by,
                    "values": version.values,
                }))
                .collect::<Vec<_>>(),
        }))
    }
);

fn require_project(context: &CallContext<'_>) -> Result<ProjectId> {
    context
        .project
        .ok_or_else(|| Error::internal("项目级 tool 却没有项目"))
}

fn table_id(name: &str) -> Result<TableId> {
    if name.starts_with(TableId::SYSTEM_PREFIX) {
        TableId::system(name)
    } else {
        TableId::user(name)
    }
}

fn describe(schema: &TableSchema) -> Value {
    json!({
        "table": schema.name.as_str(),
        "kind": schema.kind,
        "protection": schema.protection,
        "columns": schema.columns,
        "createdAt": schema.created_at.as_millis(),
        "droppedAt": schema.dropped_at.map(xops_core::Timestamp::as_millis),
    })
}

// ——————————————————————————————— 表专属 tool 的派发 ———————————————————————————————

/// 一张表的一个专属 tool。
pub struct RowTool {
    spec: ToolSpec,
    tables: Arc<Tables>,
    schema: TableSchema,
    op: Option<WriteOp>,
}

impl RowTool {
    fn build(
        tables: &Arc<Tables>,
        schema: &TableSchema,
        action: &str,
        op: Option<WriteOp>,
    ) -> Result<Self> {
        let project = schema
            .project
            .ok_or_else(|| Error::internal("全局表不派发专属 tool"))?;
        // 项目级 tool 一律要有 project：鉴权那一步从参数里取它。
        let mut input = Schema::new().field(project_field());
        if matches!(op, Some(WriteOp::Update | WriteOp::Delete)) {
            input = input.field(Field::required("row", FieldType::Id, "行标识"));
        }
        if let Some(WriteOp::Insert | WriteOp::Update) = op {
            for column in schema.writable_columns() {
                // 列名与类型在协议层是**被声明过的**（MCP-005）。
                // update 时全部可选：没给的列保持原样。
                let field = if op == Some(WriteOp::Insert) && column.required {
                    column.field()
                } else {
                    Field::optional(
                        &column.name,
                        column.ty.field_type(),
                        &format!("列 {}", column.name),
                    )
                };
                input = input.field(field);
            }
        }
        if op.is_none() {
            input = input
                .field(Field::optional(
                    "limit",
                    FieldType::Integer,
                    "这一页最多几行。**按写入序**，最老的在前",
                ))
                .field(Field::optional(
                    "after",
                    FieldType::Text { max_len: 26 },
                    "游标：从这一行**之后**接着取。把上一页回话里的 `next` 原样传回来",
                ));
        }
        let requirement = match op {
            Some(_) => Requirement::InProject(Tables::write_action(schema)),
            None => Requirement::InProject(Action::ReadProject),
        };
        Ok(Self {
            spec: ToolSpec::builder(&format!("row.{}.{action}", schema.name.slug()))
                .summary(&format!("表 {} 的{}", schema.name, summary(op)))
                .input(input)
                .requires(requirement)
                .idempotency(match op {
                    Some(_) => Idempotency::Keyed,
                    None => Idempotency::ReadOnly,
                })
                .audits(kinds::TABLE_CREATED)
                .scoped_to(project)
                .build()?,
            tables: Arc::clone(tables),
            schema: schema.clone(),
            op,
        })
    }
}

fn summary(op: Option<WriteOp>) -> &'static str {
    match op {
        Some(WriteOp::Insert) => "插入",
        Some(WriteOp::Update) => "更新",
        Some(WriteOp::Delete) => "删除（软删）",
        None => "查询",
    }
}

impl Tool for RowTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn call(&self, context: &CallContext<'_>) -> Result<Value> {
        // I-B：**署名来自令牌**。参数里就算带了 writtenBy，也在写入前被盖掉。
        let written_by = WrittenBy::Person {
            user: context.identity.user.id,
        };
        let project = self.schema.project;
        let name = &self.schema.name;
        let values = || {
            let mut object = serde_json::Map::new();
            if let Some(args) = context.args.as_object() {
                for column in self.schema.writable_columns() {
                    if let Some(value) = args.get(&column.name) {
                        object.insert(column.name.clone(), value.clone());
                    }
                }
            }
            Value::Object(object)
        };
        match self.op {
            Some(WriteOp::Insert) => {
                let row = self.tables.insert(&written_by, project, name, values())?;
                Ok(json!({"row": row.to_string()}))
            }
            Some(WriteOp::Update) => {
                let row = RowId::from_id(context.id("row")?);
                self.tables
                    .update(&written_by, project, name, row, values())?;
                Ok(json!({"row": row.to_string()}))
            }
            Some(WriteOp::Delete) => {
                let row = RowId::from_id(context.id("row")?);
                self.tables.delete(&written_by, project, name, row)?;
                Ok(json!({"row": row.to_string()}))
            }
            None => {
                let limit = context
                    .arg("limit")
                    .and_then(Value::as_i64)
                    .and_then(|limit| usize::try_from(limit).ok())
                    .unwrap_or(100);
                // **按写入序翻页。** 回话里带 `next`：把它原样传回来就是下一页。
                //
                // ⚠️ 一页给的是**最老的那几行**，不是最新的。想看最新的就一直翻到
                // `next` 为 null——**这一层有意不做倒序**：倒序要么是一次全表读，
                // 要么要一条索引，两样都不该由一个 tool 悄悄替调用方决定。
                let after = context
                    .arg("after")
                    .and_then(Value::as_str)
                    .map(|cursor| Id::parse(cursor).map(RowId::from_id))
                    .transpose()?;
                let page = self.tables.query(
                    project,
                    name,
                    &Query {
                        filters: Vec::new(),
                        limit: limit.min(1_000),
                        after,
                    },
                )?;
                Ok(json!({
                    "rows": page.rows
                        .into_iter()
                        .map(|(row, values)| json!({"row": row.to_string(), "values": values}))
                        .collect::<Vec<_>>(),
                    "next": page.next.map(|row| row.to_string()),
                }))
            }
        }
    }
}

/// 表专属 tool 的派发源（`MCP-005`）。
///
/// **建表即派发、删表即停派**——它每次被问的时候现算，所以不需要谁去通知它。
pub struct TableTools {
    tables: Arc<Tables>,
}

impl TableTools {
    #[must_use]
    pub fn new(tables: Arc<Tables>) -> Self {
        Self { tables }
    }
}

impl ToolSource for TableTools {
    fn tools(&self) -> Result<Vec<Arc<dyn Tool>>> {
        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        for schema in self.tables.catalog().live()? {
            if schema.project.is_none() {
                // 全局表（_notices）只有两个专属 tool，且在 RP-17（NTF-009）。
                continue;
            }
            out.push(Arc::new(RowTool::build(
                &self.tables,
                &schema,
                "select",
                None,
            )?));
            if schema.kind == Kind::System {
                // 系统表**只有平台能写**（TBL-003），所以一个写 tool 都不派发。
                continue;
            }
            for (action, op) in [
                ("insert", WriteOp::Insert),
                ("update", WriteOp::Update),
                ("delete", WriteOp::Delete),
            ] {
                out.push(Arc::new(RowTool::build(
                    &self.tables,
                    &schema,
                    action,
                    Some(op),
                )?));
            }
        }
        Ok(out)
    }
}

/// 把表域的六个 tool 与派发源都接上。
///
/// # Errors
/// 声明不合形状或重名。
pub fn register(registry: &mut xops_mcp::Registry, tables: &Arc<Tables>) -> Result<()> {
    registry.register(Arc::new(CreateTable::new(Arc::clone(tables))?))?;
    registry.register(Arc::new(AddColumn::new(Arc::clone(tables))?))?;
    registry.register(Arc::new(DescribeTable::new(Arc::clone(tables))?))?;
    registry.register(Arc::new(ListTables::new(Arc::clone(tables))?))?;
    registry.register(Arc::new(DropTable::new(Arc::clone(tables))?))?;
    registry.register(Arc::new(RowHistory::new(Arc::clone(tables))?))?;
    registry.add_source(Arc::new(TableTools::new(Arc::clone(tables))));
    Ok(())
}

/// 未使用的导入占位，保持 `Id` 在 doc link 里可见。
#[allow(dead_code, reason = "文档链接用")]
type _IdLink = Id;
