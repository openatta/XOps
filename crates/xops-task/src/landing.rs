//! 产出落地：把一次执行的结果写成账。
//!
//! 顺序是定死的（`TSK-006` ③ / `CON-011`）：**先落 `_runs`，再写产出行。**
//! 理由不是洁癖——`FLW-026⑥` 要读 `_runs.status` 才知道产出行算不算结算。
//!
//! 它**不是跨表事务**（D43）。两者之间崩溃是可接受的失败形态：`_runs` 行完整、
//! 产出行可能缺失；**反过来（产出行在、执行状态未定）是不可接受的**，顺序就是为了排除它。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Clock, Error, Id, Result, RowId, Timestamp};
use xops_identity::ProjectId;
use xops_table::{TableId, Tables, WrittenBy};

use crate::retention::Retention;
use crate::task::Task;

/// 单次执行产物的体量上限（`EXE-025`），**按字符数**。
///
/// ⚠️ 它必须留出标注的位置，且**不能超过 `_runs.output` 这一列的上限**——
/// 超了的话截断出来的东西照样写不进去，而那时候报的是"列超长"，
/// 看起来像是别的问题。踩过一次。
pub const MAX_OUTPUT_CHARS: usize = xops_table::column::LONG_TEXT_MAX - TRUNCATION_MARK.len();
/// 超限时贴的标注。**不静默丢弃**——调用方要看得见这件事发生过。
pub const TRUNCATION_MARK: &str = "\n\n〔产物超过体量上限，已截断〕";

/// 两层拒绝（`EXE-024`）。**必须分清。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// schema 校验不过 → **整批行不入表**，执行归为技能错误类失败。
    SchemaFailed { reason: String },
    /// schema 过、节点判定不过 → **行入表**，只是不结算。
    NotSettled { reason: String },
}

impl Rejection {
    /// 行入不入表。
    #[must_use]
    pub const fn rows_landed(&self) -> bool {
        matches!(self, Self::NotSettled { .. })
    }
}

/// 一次落地的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Landed {
    /// `_runs` 那一行。
    pub run_row: RowId,
    /// 产出行。
    pub rows: Vec<RowId>,
    /// 产物被截断了吗（`EXE-025`）。
    pub truncated: bool,
    /// 这次落地里的拒绝（如果有）。
    pub rejection: Option<Rejection>,
}

/// 「自动化失灵不能是静默的」的出口（`EXE-024`：**两种情况都要通知**）。
///
/// **RP-17 填它。** 没接的时候两层拒绝照样发生，只是没人被告知——
/// 而那正是这条要防的，所以本包在没接时会在审计里留一条。
pub trait Notifier: Send + Sync + 'static {
    fn notify(&self, task: &Task, rejection: &Rejection);
}

/// 产出落地。
pub struct Landing {
    tables: Arc<Tables>,
    clock: Arc<dyn Clock>,
    notifier: Option<Arc<dyn Notifier>>,
}

impl std::fmt::Debug for Landing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Landing")
            .field("notifier", &self.notifier.is_some())
            .finish_non_exhaustive()
    }
}

/// 一次执行交回来的东西，落账时要用的那几样。
#[derive(Debug, Clone)]
pub struct Completion {
    pub run: Id,
    pub status: String,
    pub failure_kind: Option<String>,
    pub tokens_used: u64,
    pub token_budget: u64,
    pub output: String,
    pub trace: String,
    pub revision: Option<String>,
    pub skill: String,
    pub skill_version: String,
    pub trigger: String,
    pub triggered_by: String,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    /// 技能产出的那些行（`OutputShape::Rows` 时才有）。
    pub rows: Vec<Value>,
}

impl Landing {
    #[must_use]
    pub fn new(tables: Arc<Tables>, clock: Arc<dyn Clock>) -> Self {
        Self {
            tables,
            clock,
            notifier: None,
        }
    }

    /// 接上通知出口。RP-17 用。
    #[must_use]
    pub fn with_notifier(mut self, notifier: Arc<dyn Notifier>) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// 落一次执行的账。
    ///
    /// # Errors
    /// `_runs` 写不进去。**产出行写不进去不是 `Err`**——那是一次两层拒绝，
    /// 它已经被记在 `_runs` 上了。
    pub fn land(
        &self,
        task: &Task,
        retention: Retention,
        written_by: &WrittenBy,
        completion: &Completion,
    ) -> Result<Landed> {
        let now = self.clock.now();
        let (output, truncated) = truncate(&completion.output);

        // ① 先校验每一行是否符合目标表的 schema（`EXE-023`）。
        //    **不过就整批不入表**——不是"过的那些先写进去"。
        let rejection = self.validate_rows(task, completion)?;

        // ② 先落 _runs（CON-011 / TSK-006 ③）。
        let run_row = self.write_run_row(
            task,
            retention,
            completion,
            &output,
            truncated,
            rejection.as_ref(),
            now,
        )?;

        // ③ 再写产出行。schema 不过时这一步整批跳过。
        let mut rows = Vec::new();
        if rejection.as_ref().is_none_or(Rejection::rows_landed) {
            let target = task.writes.first();
            if let Some(target) = target {
                for row in &completion.rows {
                    let mut values = row.clone();
                    if let Some(object) = values.as_object_mut() {
                        // RET-002：**取写入当时的配置**，不回查任务。
                        object.insert(
                            "retainUntil".into(),
                            json!(retention.retain_until(now).as_millis()),
                        );
                    }
                    rows.push(self.tables.insert(
                        written_by,
                        Some(task.project),
                        target,
                        values,
                    )?);
                }
            }
        }

        if let Some(rejection) = &rejection
            && let Some(notifier) = &self.notifier
        {
            // EXE-024：**两种情况都要通知——自动化失灵不能是静默的。**
            notifier.notify(task, rejection);
        }

        Ok(Landed {
            run_row,
            rows,
            truncated,
            rejection,
        })
    }

    fn validate_rows(&self, task: &Task, completion: &Completion) -> Result<Option<Rejection>> {
        let Some(target) = task.writes.first() else {
            return Ok(None);
        };
        // TSK-004：**未声明的表写不了。** 这里只可能写进声明过的第一张。
        let schema = match self.tables.describe_internal(Some(task.project), target) {
            Ok(schema) => schema,
            Err(error) => {
                return Ok(Some(Rejection::SchemaFailed {
                    reason: error.message().to_owned(),
                }));
            }
        };
        for row in &completion.rows {
            let Some(object) = row.as_object() else {
                return Ok(Some(Rejection::SchemaFailed {
                    reason: "产出行不是对象".into(),
                }));
            };
            for (name, value) in object {
                if xops_table::AUTO_COLUMNS.contains(&name.as_str()) {
                    continue;
                }
                let Some(column) = schema.column(name) else {
                    return Ok(Some(Rejection::SchemaFailed {
                        reason: format!("列 {name} 不在 {target} 的 schema 里"),
                    }));
                };
                if let Err(error) = column.ty.check(name, value) {
                    return Ok(Some(Rejection::SchemaFailed {
                        reason: error.message().to_owned(),
                    }));
                }
            }
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments, reason = "一次落账要写的东西就是这么多")]
    fn write_run_row(
        &self,
        task: &Task,
        retention: Retention,
        completion: &Completion,
        output: &str,
        truncated: bool,
        rejection: Option<&Rejection>,
        now: Timestamp,
    ) -> Result<RowId> {
        let status = if matches!(rejection, Some(Rejection::SchemaFailed { .. })) {
            // EXE-024：schema 不过 → 执行归为**技能错误类**失败。
            "failed"
        } else {
            completion.status.as_str()
        };
        let failure_kind = if matches!(rejection, Some(Rejection::SchemaFailed { .. })) {
            Some("skill".to_owned())
        } else {
            completion.failure_kind.clone()
        };
        let runs = TableId::system(xops_table::system::RUNS)?;
        let mut values = json!({
            "run": completion.run.to_string(),
            "task": task.id.to_string(),
            "skill": completion.skill,
            "skillVersion": completion.skill_version,
            "trigger": completion.trigger,
            "triggeredBy": completion.triggered_by,
            "status": status,
            "tokensUsed": i64::try_from(completion.tokens_used).unwrap_or(i64::MAX),
            "tokenBudget": i64::try_from(completion.token_budget).unwrap_or(i64::MAX),
            "output": if truncated { format!("{output}{TRUNCATION_MARK}") } else { output.to_owned() },
            "trace": completion.trace,
            "startedAt": completion.started_at.as_millis(),
            // RET-004：过程记录**先过期**，行本身按输出保留期走。
            "traceRetainUntil": retention.trace_retain_until(now).as_millis(),
            // RET-002：取写入**当时**的配置。
            "retainUntil": retention.retain_until(now).as_millis(),
        });
        if let Some(object) = values.as_object_mut() {
            if let Some(kind) = failure_kind {
                object.insert("failureKind".into(), json!(kind));
            }
            if let Some(finished) = completion.finished_at {
                object.insert("finishedAt".into(), json!(finished.as_millis()));
            }
        }
        // EXE-026：**每次执行一条 `_runs` 行，永不被后续执行覆盖**——这是 insert，不是 update。
        //
        // ⚠️ 署名是 `Platform` 而不是传进来的那个 `written_by`：`_runs` 是系统表，
        // **只有平台能写**（`TBL-003`）。这一行是平台在记账，不是技能在写东西——
        // "这次是谁跑的"由它自己的 `triggeredBy` / `task` / `skill` 几列回答，
        // 那几列比一个 `writtenBy` 说得更全。产出行那一侧才用 `written_by`。
        self.tables
            .insert(&WrittenBy::Platform, Some(task.project), &runs, values)
    }
}

/// 超限就截断并标注（`EXE-025`）。**不静默丢弃。**
#[must_use]
pub fn truncate(output: &str) -> (String, bool) {
    if output.chars().count() <= MAX_OUTPUT_CHARS {
        return (output.to_owned(), false);
    }
    // 按字符截 —— 半个汉字会让整段读不回来。
    (output.chars().take(MAX_OUTPUT_CHARS).collect(), true)
}

/// 让 `ProjectId` 与 `Error` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _Links = (ProjectId, Error);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 超限截断并标注不静默丢弃() {
        let short = "短的";
        let (text, truncated) = truncate(short);
        assert_eq!(text, short);
        assert!(!truncated);

        let long = "啊".repeat(MAX_OUTPUT_CHARS + 10);
        let (text, truncated) = truncate(&long);
        assert!(truncated, "EXE-025");
        assert!(text.chars().count() <= MAX_OUTPUT_CHARS);
        // 截在字符边界上 —— 半个汉字会让整段 JSON 都读不回来。
        assert!(text.chars().all(|c| c == '啊'));
    }

    #[test]
    fn 两层拒绝分得清行入不入表() {
        assert!(
            !Rejection::SchemaFailed { reason: "x".into() }.rows_landed(),
            "整批不入表"
        );
        assert!(
            Rejection::NotSettled { reason: "x".into() }.rows_landed(),
            "行入表，只是不结算"
        );
    }
}
