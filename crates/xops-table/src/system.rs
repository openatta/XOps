//! 五张系统表（`TBL-005`～`TBL-010`）。
//!
//! **恰好五张，schema 固定，只有平台能写。** 它们不受用户列类型集合限制这一条，
//! 在实现上表现为：它们照样用同一组列类型，但**由平台建、不经建表 tool**——
//! 需要一种用户没有的类型时，扩的是这里，不是用户能声明的那个集合。
//!
//! ⚠️ **`retainUntil` 不在任何一张表的列声明里**：它是平台自动补的列位（`TBL-014`），
//! 声明它反而会被 [`crate::column::check_column_name`] 拒掉。

use xops_core::{Result, Timestamp};
use xops_identity::ProjectId;

use crate::column::{BLOB_MAX, Column, ColumnType, LONG_TEXT_MAX, TEXT_MAX};
use crate::table::{Kind, Protection, TableId, TableSchema};

/// 每次任务执行一行（`TBL-006`）。**全系统最热的表**（`CON-010`）。
pub const RUNS: &str = "_runs";
/// 每个流程实例一行（`TBL-007`）。
pub const FLOWS: &str = "_flows";
/// 每个实例的每个节点一行（`TBL-008`）。
pub const FLOW_NODES: &str = "_flow_nodes";
/// 每个插件的每个版本一行（`TBL-009`）。
pub const PLUGINS: &str = "_plugins";
/// 每条通知一行（`TBL-010`）。**平台全局表**，因而能跨项目聚合。
pub const NOTICES: &str = "_notices";

/// 项目级的四张。`_notices` 不在其中——它是全局的。
pub const PER_PROJECT: [&str; 4] = [RUNS, FLOWS, FLOW_NODES, PLUGINS];

fn text(name: &str, required: bool) -> Result<Column> {
    Column::new(name, ColumnType::Text { max_len: TEXT_MAX }, required)
}

fn long_text(name: &str) -> Result<Column> {
    Column::new(
        name,
        ColumnType::LongText {
            max_len: LONG_TEXT_MAX,
        },
        false,
    )
}

fn time(name: &str, required: bool) -> Result<Column> {
    Column::new(name, ColumnType::Timestamp, required)
}

fn integer(name: &str) -> Result<Column> {
    Column::new(name, ColumnType::Integer, false)
}

fn enumerated(name: &str, values: &[&str], required: bool) -> Result<Column> {
    Column::new(
        name,
        ColumnType::Enum {
            values: values.iter().map(|value| (*value).to_owned()).collect(),
        },
        required,
    )
}

/// 一张系统表的固定 schema。
///
/// # Errors
/// 不是那五张之一，或者常量被改坏了。
pub fn schema(
    name: &str,
    project: Option<ProjectId>,
    project_slug: &str,
    created_at: Timestamp,
) -> Result<TableSchema> {
    let columns = match name {
        RUNS => vec![
            text("run", true)?,
            text("task", true)?,
            text("skill", true)?,
            text("skillVersion", true)?,
            text("trigger", true)?,
            text("triggeredBy", true)?,
            long_text("inputs")?,
            // `revision` 不在这里：它是平台自动补的列位（TBL-014），
            // 由 writtenBy 那一侧带进来 —— 声明它反而会被列名校验拒掉。
            time("startedAt", true)?,
            time("finishedAt", false)?,
            enumerated(
                "status",
                &["running", "succeeded", "failed", "cancelled"],
                true,
            )?,
            text("failureKind", false)?,
            integer("tokensUsed")?,
            integer("tokenBudget")?,
            // 过程记录比产出先过期（§4.9），所以它自己有一个到期时刻。
            time("traceRetainUntil", false)?,
            long_text("output")?,
            Column::new(
                "trace",
                ColumnType::Blob {
                    max_bytes: BLOB_MAX,
                },
                false,
            )?,
        ],
        FLOWS => vec![
            text("instance", true)?,
            text("flow", true)?,
            text("flowVersion", true)?,
            // 主体：主体表与行 ID，或外部主体标识。
            text("subjectTable", false)?,
            Column::new("subjectRow", ColumnType::RowRef, false)?,
            text("subjectExternal", false)?,
            text("subjectRevision", false)?,
            text("startedBy", true)?,
            enumerated(
                "state",
                &["running", "approved", "rejected", "cancelled", "expired"],
                true,
            )?,
            time("startedAt", true)?,
            time("endedAt", false)?,
        ],
        FLOW_NODES => vec![
            text("instance", true)?,
            text("node", true)?,
            // 实例被拒绝或取消时，其余节点转为"已作废"，**不停在"未激活"**。
            enumerated(
                "state",
                &["inactive", "active", "approved", "rejected", "void"],
                true,
            )?,
            time("activatedAt", false)?,
            time("settledAt", false)?,
            long_text("settledBy")?,
        ],
        PLUGINS => vec![
            text("plugin", true)?,
            text("version", true)?,
            enumerated("state", &["candidate", "installed", "disabled"], true)?,
            long_text("source")?,
            long_text("tests")?,
            long_text("testResult")?,
            text("generatedBy", false)?,
            text("installedBy", false)?,
            time("installedAt", false)?,
        ],
        NOTICES => vec![
            text("notice", true)?,
            text("user", true)?,
            text("project", false)?,
            text("kind", true)?,
            text("subject", false)?,
            // 只含指针，**不含凭据、令牌或产物原文**。
            long_text("text")?,
            time("createdAt", true)?,
            // 全系统唯一一个用户可改的系统表列（I-N 的例外，由专属 tool 代写）。
            time("readAt", false)?,
        ],
        other => {
            return Err(xops_core::Error::invalid(format!(
                "{other} 不是那五张系统表之一"
            )));
        }
    };
    TableSchema::new(
        project,
        project_slug,
        TableId::system(name)?,
        Kind::System,
        Protection::Normal,
        columns,
        created_at,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 恰好五张() {
        let all = [RUNS, FLOWS, FLOW_NODES, PLUGINS, NOTICES];
        assert_eq!(all.len(), 5, "TBL-005");
        for name in all {
            assert!(
                schema(
                    name,
                    Some(ProjectId::generate()),
                    "acme",
                    Timestamp::from_millis(0)
                )
                .is_ok(),
                "{name}"
            );
        }
        assert!(schema("_other", None, "acme", Timestamp::from_millis(0)).is_err());
    }

    #[test]
    fn runs带得动执行的全部记录() {
        let runs = schema(
            RUNS,
            Some(ProjectId::generate()),
            "acme",
            Timestamp::from_millis(0),
        )
        .unwrap();
        for column in [
            "run",
            "task",
            "skill",
            "trigger",
            "triggeredBy",
            "status",
            "tokensUsed",
            "tokenBudget",
            "output",
            "trace",
        ] {
            assert!(runs.column(column).is_some(), "少了 {column}");
        }
        for auto in ["retainUntil", "revision"] {
            assert!(
                runs.column(auto).is_none(),
                "{auto} 是自动补的列位，不该被声明"
            );
        }
    }

    #[test]
    fn 通知表是全局的() {
        let notices = schema(NOTICES, None, "", Timestamp::from_millis(0)).unwrap();
        assert!(
            notices.project.is_none(),
            "TBL-010 —— 平台全局表才能跨项目聚合"
        );
        assert!(notices.column("readAt").is_some());
    }

    #[test]
    fn 节点状态有作废这一档() {
        let nodes = schema(
            FLOW_NODES,
            Some(ProjectId::generate()),
            "acme",
            Timestamp::from_millis(0),
        )
        .unwrap();
        let state = nodes.column("state").unwrap();
        if let ColumnType::Enum { values } = &state.ty {
            assert!(
                values.contains(&"void".to_owned()),
                "实例被拒绝时其余节点转为已作废"
            );
        } else {
            panic!("state 该是枚举");
        }
    }

    #[test]
    fn 系统表都是系统那一类() {
        for name in [RUNS, FLOWS, FLOW_NODES, PLUGINS] {
            let schema = schema(
                name,
                Some(ProjectId::generate()),
                "acme",
                Timestamp::from_millis(0),
            )
            .unwrap();
            assert_eq!(schema.kind, Kind::System);
            assert!(schema.name.is_system());
        }
    }
}
