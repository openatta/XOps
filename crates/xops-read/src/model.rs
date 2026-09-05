//! 读模型。**这是前端唯一能看见的东西。**
//!
//! 前端不直连库、不拼 SQL、不调 MCP——它只认这里的这几个视图。所以这份接口的完备性
//! 是 RP-06 能不能并行开工的全部前提：**它需要的每一样数据都要在这里，
//! 不能让它去开第二条数据通路。**
//!
//! 三件明确不做（`BRD-002`、`BRD-003`、`TBL-023`）：**没有聚合、没有指标、没有 join。**
//! 一个主体的完整时间线横跨两张表，这里就给两个视图，**不在后端把它们拼起来**（`BRD-006`）。

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use xops_audit::{AuditEnvelope, AuditLog};
use xops_core::{Actor, Clock, Error, Id, Result, Role, RowId, TableName, Timestamp, WriteOp};
use xops_identity::{Action, Directory, ProjectId, UserId};
use xops_notice::Notices;
use xops_store::{Store, WriteEngine, WriteRequest};
use xops_table::{Filter, MAX_SCAN, TableId, Tables};

use crate::board::{Board, BoardId, BoardSpec, Direction, check_boardable};

/// 看板定义落在这张平台表上。用户看不到它。
pub const BOARDS_TABLE: &str = "_boards";

/// 事件类型。
pub mod kinds {
    pub const BOARD_DEFINED: &str = "board.defined";
}

/// 我是谁（`BRD-011`：**明确展示当前用户身份**）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentityView {
    pub user: String,
    pub display_name: String,
    pub provider: String,
    pub account: String,
}

/// 一个我参与的项目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectView {
    pub project: String,
    pub slug: String,
    pub display_name: String,
    pub role: String,
    pub archived: bool,
}

/// 看板清单里的一条。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardSummary {
    pub board: String,
    pub name: String,
    pub table: String,
}

/// 看板视图：一张表的一个视图。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardView {
    pub board: String,
    pub name: String,
    pub table: String,
    /// 显示哪几列，**按看板定义的顺序**。
    pub columns: Vec<String>,
    pub rows: Vec<RowView>,
    /// 这一页从第几行开始。
    pub offset: usize,
    /// 后面还有没有。
    ///
    /// ⚠️ **给的是"还有没有"，不是"一共几行"。** 一个总数会被读成一个指标
    /// （"缺陷 42 条"），而 `BRD-002` 说平台不内建任何报表、`BRD-003` 的判据是
    /// "哪天要在平台代码里写什么是缺陷密度就越界了"。**翻页需要的只是这一个布尔。**
    pub has_more: bool,
}

/// 一行。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RowView {
    pub row: String,
    pub values: Value,
}

/// 单行历史（`BRD-006` 的前一半：**状态怎么变的、谁改的、什么时候**）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RowHistoryView {
    pub table: String,
    pub row: String,
    pub versions: Vec<VersionView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VersionView {
    pub seq: u64,
    pub op: WriteOp,
    pub at: i64,
    pub written_by: Option<Value>,
    pub values: Value,
}

/// 同一个实例上的结算行（`BRD-006` 的后一半：**为什么这么变、谁表的态**）。
///
/// **它与单行历史是两个视图，两次查询**——平台不做 join。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettlementView {
    pub table: String,
    pub row: String,
    pub at: i64,
    pub written_by: Option<Value>,
    pub values: Value,
}

/// 长文本的原文（`BRD-010`：**供不信任渲染的人自行查看**）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LongTextView {
    pub table: String,
    pub row: String,
    pub column: String,
    pub text: String,
}

/// 个人看板上的一条（`NTF-001`）。
///
/// ⚠️ **`text` 是指针不是内容**（`NTF-006`：不含凭据、令牌或产物原文），
/// 由确定性代码生成、不经模型（`NTF-003`），里面的自由文本原样引用或截断（`NTF-004`）。
/// **这三条在 RP-17 那一侧兑现，本视图不复核**——同一条规则写两遍，两遍迟早不一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NoticeView {
    pub notice: String,
    /// 五类之一（`NTF-007`）。
    pub kind: String,
    /// 哪个项目。**可以是 `None`**——`_notices` 是平台全局表（`NTF-014`）。
    pub project: Option<String>,
    pub subject: String,
    pub text: String,
    pub created_at: i64,
}

/// 一个项目成员。
///
/// 显示名在这里解出来：前端**没有第二条数据通路**去按 id 换名字，
/// 给 id 不给名字的视图等于逼它去开一条。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberView {
    pub user: String,
    pub display_name: String,
    pub role: String,
    pub added_at: i64,
}

/// 一张表。**不含任何一行数据。**
///
/// ⚠️ 它回答的是"有哪些表"，不是"表里有什么"。要看行就去看板那条路（`BRD-001`）——
/// 一个顺手加上"顺便回十行"的版本，就是绕过看板定义的第二条读数据通路。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableSummary {
    pub table: String,
    /// `system` 还是 `user`。
    pub kind: String,
    pub protection: String,
    pub columns: Vec<ColumnSummary>,
}

/// 一列：名字与它是什么类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ColumnSummary {
    pub column: String,
    pub kind: String,
    pub required: bool,
}

/// 读模型。
pub struct ReadModel {
    engine: Arc<WriteEngine>,
    store: Arc<dyn Store>,
    audit: Arc<AuditLog>,
    directory: Arc<Directory>,
    tables: Arc<Tables>,
    /// 个人看板读它（`NTF-001`）。**这条依赖是 L3 → L1，不成环**——
    /// `xops-notice` 不认识本 crate。
    notices: Arc<Notices>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ReadModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadModel").finish_non_exhaustive()
    }
}

impl ReadModel {
    #[must_use]
    pub fn new(
        engine: Arc<WriteEngine>,
        store: Arc<dyn Store>,
        audit: Arc<AuditLog>,
        directory: Arc<Directory>,
        tables: Arc<Tables>,
        notices: Arc<Notices>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            engine,
            store,
            audit,
            directory,
            tables,
            notices,
            clock,
        }
    }

    /// 我是谁。
    ///
    /// # Errors
    /// 这个人不存在。
    pub fn me(&self, viewer: UserId) -> Result<IdentityView> {
        let user = self
            .directory
            .user(viewer)?
            .ok_or_else(|| Error::not_found("不存在"))?;
        Ok(IdentityView {
            user: user.id.to_string(),
            display_name: user.display_name,
            provider: user.account.provider.as_str().to_owned(),
            account: user.account.account,
        })
    }

    /// 我参与的项目。**可见性完全遵循项目成员边界**（`BRD-011`）。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn projects(&self, viewer: UserId) -> Result<Vec<ProjectView>> {
        Ok(self
            .directory
            .my_projects(viewer)?
            .into_iter()
            .map(|(project, role)| ProjectView {
                project: project.id.to_string(),
                slug: project.slug.as_str().to_owned(),
                archived: project.is_archived(),
                display_name: project.display_name,
                role: role.as_str().to_owned(),
            })
            .collect())
    }

    /// 我的未读（个人看板，`NTF-001`）。跨项目一起排（`NTF-014`）。
    ///
    /// ⚠️ **签名里没有"看谁的"这个参数**，与 `Notices::unread` 同一条口径：
    /// `NTF-010` 的硬限定靠**调用方表达不出那个请求**兑现，不靠一次检查。
    ///
    /// 只回未读。**已读的从这一页消失，就是「标记已读」的意思**——
    /// 个人看板是一份待办，不是一条收件箱时间线。
    ///
    /// 回话里带 `truncated`：⚠️ **静默截断在这里的表现是"怎么没收到通知"**，
    /// 而那是查起来最慢的一种。所以多取一条来判断，判断完就丢掉。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn my_notices(&self, viewer: UserId, limit: usize) -> Result<(Vec<NoticeView>, bool)> {
        let mut rows = self.notices.unread(viewer, limit.saturating_add(1))?;
        let truncated = rows.len() > limit;
        rows.truncate(limit);
        Ok((
            rows.into_iter()
                .map(|notice| NoticeView {
                    notice: notice.id.to_string(),
                    kind: notice.kind.as_str().to_owned(),
                    project: notice.project.map(|project| project.to_string()),
                    subject: notice.subject,
                    text: notice.text,
                    created_at: notice.created_at.as_millis(),
                })
                .collect(),
            truncated,
        ))
    }

    /// 项目成员与各自的角色（`PRJ-007`）。
    ///
    /// # Errors
    /// 非成员看到的与项目不存在一致（`PRJ-008`）——授权在 `Directory::members` 里。
    pub fn members(&self, viewer: UserId, project: ProjectId) -> Result<Vec<MemberView>> {
        self.directory
            .members(viewer, project)?
            .into_iter()
            .map(|member| {
                // 显示名解不出来时**不编一个**：一个被删掉的用户显示成他的 id，
                // 比显示成"未知"更有用——id 还查得下去。
                let display_name = self
                    .directory
                    .user(member.user)?
                    .map_or_else(|| member.user.to_string(), |user| user.display_name);
                Ok(MemberView {
                    user: member.user.to_string(),
                    display_name,
                    role: member.role.as_str().to_owned(),
                    added_at: member.added_at.as_millis(),
                })
            })
            .collect()
    }

    /// 这个项目有哪些表。软删掉的不在里面（`TBL-026`）。
    ///
    /// ⚠️ **一行数据都不回。** 要看行就去看板那条路（`BRD-001`）——
    /// 一个顺手"再回十行"的版本，就是绕过看板定义的第二条读数据通路。
    ///
    /// # Errors
    /// 非成员看到的与项目不存在一致——授权在 `Tables::list` 里。
    pub fn tables(&self, viewer: UserId, project: ProjectId) -> Result<Vec<TableSummary>> {
        Ok(self
            .tables
            .list(viewer, project)?
            .into_iter()
            .map(|schema| TableSummary {
                table: schema.name.as_str().to_owned(),
                kind: if schema.name.is_system() {
                    "system".to_owned()
                } else {
                    "user".to_owned()
                },
                protection: match schema.protection {
                    xops_table::Protection::Protected => "protected".to_owned(),
                    xops_table::Protection::Normal => "normal".to_owned(),
                },
                columns: schema
                    .columns
                    .into_iter()
                    .map(|column| ColumnSummary {
                        column: column.name,
                        kind: column.ty.describe(),
                        required: column.required,
                    })
                    .collect(),
            })
            .collect())
    }

    /// 定义一个看板（经 MCP，`BRD-001`）。
    ///
    /// # Errors
    /// 没权限 / 项目不存在（同一个错）· 表不存在 · 这张表不允许建自由看板。
    pub fn define_board(
        &self,
        actor: UserId,
        project: ProjectId,
        spec: BoardSpec,
    ) -> Result<Board> {
        self.directory
            .authorize(actor, project, Action::ManageBusinessObject)?;
        check_boardable(&spec.table)?;
        // 表得真的存在。
        self.tables.describe(actor, project, &spec.table)?;
        let board = Board::new(project, spec, self.clock.now())?;
        let envelope = AuditEnvelope::project_scoped(
            kinds::BOARD_DEFINED,
            project.as_id(),
            board.id.as_id(),
            serde_json::to_value(&board)
                .map_err(|error| Error::internal(format!("看板装不下：{error}")))?,
        )?;
        let receipt = self.engine.write(WriteRequest {
            table: TableName::new(BOARDS_TABLE)?,
            op: WriteOp::Insert,
            row: RowId::from_id(board.id.as_id()),
            payload: envelope.to_payload()?,
            actor: Actor::User {
                user: actor.to_string(),
            },
        })?;
        self.audit.index(&receipt)?;
        Ok(board)
    }

    /// 列出一个项目的看板。
    ///
    /// # Errors
    /// 非成员看到的与项目不存在一致。
    pub fn boards(&self, viewer: UserId, project: ProjectId) -> Result<Vec<BoardSummary>> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        Ok(self
            .all_boards()?
            .into_iter()
            .filter(|board| board.project == project)
            .map(|board| BoardSummary {
                board: board.id.to_string(),
                name: board.name,
                table: board.table.as_str().to_owned(),
            })
            .collect())
    }

    /// 看一个看板。
    ///
    /// # Errors
    /// 非成员 / 看板不存在 —— 同一个错。
    pub fn board(
        &self,
        viewer: UserId,
        board: BoardId,
        offset: usize,
        limit: usize,
    ) -> Result<BoardView> {
        let definition = self
            .all_boards()?
            .into_iter()
            .find(|candidate| candidate.id == board)
            .ok_or_else(|| Error::not_found("不存在"))?;
        self.directory
            .authorize(viewer, definition.project, Action::ReadProject)?;

        // ⚠️ **筛选交给查询面，而不是「扫前一万行再过滤」。**
        //
        // 那个写法会安静地给出错误答案：行 ID 时间有序，截断留下的是**最老的一万条**，
        // 而排序发生在截断**之后**——于是一个「最新在前」的看板会稳定显示最老的那一批。
        // 排序要拿到全部命中才答得出来，所以这里用 `query_all`；
        // 扫不动时它**明确失败**，不截断（`xops_table::MAX_SCAN`）。
        let mut rows: Vec<(RowId, Value)> = self.tables.query_all(
            Some(definition.project),
            &definition.table,
            &definition.filters,
            MAX_SCAN,
        )?;
        if let Some(sort) = &definition.sort {
            rows.sort_by(|left, right| {
                let ordering = compare(left.1.get(sort), right.1.get(sort));
                match definition.direction {
                    Direction::Asc => ordering,
                    Direction::Desc => ordering.reverse(),
                }
            });
        }
        // 分页：**先排完序再切**。⚠️ 顺序不能反——排序要拿到全部命中才答得出来，
        // 先切再排就是"稳定地显示最老的那一批"，而它不报错。
        let has_more = rows.len() > offset.saturating_add(limit);
        let rows: Vec<(RowId, Value)> = rows.into_iter().skip(offset).take(limit).collect();

        let columns = if definition.columns.is_empty() {
            let schema = self
                .tables
                .describe(viewer, definition.project, &definition.table)?;
            schema
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect()
        } else {
            definition.columns.clone()
        };
        Ok(BoardView {
            board: definition.id.to_string(),
            name: definition.name,
            table: definition.table.as_str().to_owned(),
            rows: rows
                .into_iter()
                .map(|(row, values)| RowView {
                    row: row.to_string(),
                    values: project_columns(&values, &columns),
                })
                .collect(),
            columns,
            offset,
            has_more,
        })
    }

    /// 一行的完整历史。
    ///
    /// # Errors
    /// 非成员 / 表不存在 —— 同一个错。
    pub fn row_history(
        &self,
        viewer: UserId,
        project: ProjectId,
        table: &TableId,
        row: RowId,
    ) -> Result<RowHistoryView> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        let versions = self
            .tables
            .history(Some(project), table, row)?
            .into_iter()
            .map(|version| VersionView {
                seq: version.seq,
                op: version.op,
                at: version.at.as_millis(),
                written_by: version
                    .written_by
                    .and_then(|written| written.to_value().ok()),
                values: version.values,
            })
            .collect();
        Ok(RowHistoryView {
            table: table.as_str().to_owned(),
            row: row.to_string(),
            versions,
        })
    }

    /// 同一个实例上的结算行。**与单行历史分开查**（`BRD-006`）。
    ///
    /// # Errors
    /// 非成员 / 表不存在 —— 同一个错。
    pub fn settlements(
        &self,
        viewer: UserId,
        project: ProjectId,
        table: &TableId,
        instance: Id,
    ) -> Result<Vec<SettlementView>> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        // 同上：**这是一个谓词，不是「前一万行里凑巧命中的那些」**。
        // 一个新实例的结算行落在截断线之后时，旧写法会返回空——看起来像「没人表态」。
        Ok(self
            .tables
            .query_all(
                Some(project),
                table,
                &[Filter::equals("_instance", instance.to_string())],
                MAX_SCAN,
            )?
            .into_iter()
            .map(|(row, values)| SettlementView {
                table: table.as_str().to_owned(),
                row: row.to_string(),
                at: values.get("at").and_then(Value::as_i64).unwrap_or_default(),
                written_by: values.get("writtenBy").cloned(),
                values,
            })
            .collect())
    }

    /// 长文本的原文。
    ///
    /// # Errors
    /// 非成员 / 行或列不存在 —— 同一个错。
    pub fn long_text(
        &self,
        viewer: UserId,
        project: ProjectId,
        table: &TableId,
        row: RowId,
        column: &str,
    ) -> Result<LongTextView> {
        self.directory
            .authorize(viewer, project, Action::ReadProject)?;
        let values = self
            .tables
            .get(Some(project), table, row)?
            .ok_or_else(|| Error::not_found("不存在"))?;
        let text = values
            .get(column)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::not_found("不存在"))?;
        Ok(LongTextView {
            table: table.as_str().to_owned(),
            row: row.to_string(),
            column: column.to_owned(),
            text: text.to_owned(),
        })
    }

    /// 一个人在一个项目里的角色。Web 侧判可见性用它。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn role_of(&self, viewer: UserId, project: ProjectId) -> Result<Option<Role>> {
        self.directory.role_of(project, viewer)
    }

    fn all_boards(&self) -> Result<Vec<Board>> {
        Ok(
            xops_audit::projection::all_strict::<Board>(self.store.as_ref(), BOARDS_TABLE)?
                .into_iter()
                .map(|(_, board)| board)
                .collect(),
        )
    }
}

/// 只留看板声明要显示的那几列，外加平台自动补的来源字段。
///
/// `writtenBy` 总是留着：**看板上的来源标识读的就是它**（`TBL-016`）。
fn project_columns(values: &Value, columns: &[String]) -> Value {
    let Some(object) = values.as_object() else {
        return values.clone();
    };
    let mut out = serde_json::Map::new();
    for column in columns {
        if let Some(value) = object.get(column) {
            out.insert(column.clone(), value.clone());
        }
    }
    for auto in ["writtenBy", "at"] {
        if let Some(value) = object.get(auto) {
            out.insert(auto.to_owned(), value.clone());
        }
    }
    Value::Object(out)
}

fn compare(left: Option<&Value>, right: Option<&Value>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(Value::Number(left)), Some(Value::Number(right))) => left
            .as_f64()
            .unwrap_or_default()
            .partial_cmp(&right.as_f64().unwrap_or_default())
            .unwrap_or(std::cmp::Ordering::Equal),
        (Some(Value::String(left)), Some(Value::String(right))) => left.cmp(right),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        _ => std::cmp::Ordering::Equal,
    }
}

/// 让 `Timestamp` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _TimestampLink = Timestamp;
