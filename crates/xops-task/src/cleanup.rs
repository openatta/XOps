//! 到期清理。**全系统唯一一处硬删除**（`RET-010`）。
//!
//! 三条不能松：
//!
//! ```text
//! 整批按时间   不得选择性删除个别行（RET-005）
//! 豁免优先     豁免清单先判，任务保留期后判（RET-006/007）
//! 清理留痕     删除这件事本身要有事件可查（RET-010）
//! ```
//!
//! ⚠️ **它在锁外整批离线进行**（`CON-006`）。

use std::sync::Arc;

use serde_json::Value;
use xops_audit::{AuditEnvelope, AuditLog};
use xops_core::{Actor, Error, Result, RowId, TableName, Timestamp};
use xops_identity::ProjectId;
use xops_store::{Row, Store, keys, space};
use xops_table::{TableId, Tables};

use crate::retention::{Exemption, exemption};

/// 事件类型。
pub mod kinds {
    pub const RETENTION_SWEPT: &str = "retention.swept";
}

/// 一次清理的账。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Swept {
    /// 整行删掉了几行。
    pub rows: usize,
    /// 只清了 `trace` 这一列的有几行。
    pub traces: usize,
    /// 因为豁免而留下的有几行。
    pub exempted: usize,
}

/// 清理作业。
pub struct Cleanup {
    tables: Arc<Tables>,
    store: Arc<dyn Store>,
    audit: Arc<AuditLog>,
}

impl std::fmt::Debug for Cleanup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cleanup").finish_non_exhaustive()
    }
}

impl Cleanup {
    #[must_use]
    pub fn new(tables: Arc<Tables>, store: Arc<dyn Store>, audit: Arc<AuditLog>) -> Self {
        Self {
            tables,
            store,
            audit,
        }
    }

    /// 扫一张表，把到期的整批清掉。
    ///
    /// **没有"删除某一行"的入口**——这个函数只接受一个时刻（`RET-005`）。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn sweep(&self, project: ProjectId, table: &TableId, now: Timestamp) -> Result<Swept> {
        let schema = self.tables.describe_internal(Some(project), table)?;
        let physical = schema.physical()?;
        let prefix = keys::table_prefix(&physical);
        let mut swept = Swept::default();
        let mut cursor: Option<Vec<u8>> = None;

        loop {
            let page = self
                .store
                .scan(space::ROW, &prefix, cursor.as_deref(), 256)?;
            if page.is_empty() {
                break;
            }
            cursor = page.last().map(|(key, _)| key.clone());
            for (key, bytes) in page {
                let row: Row = serde_json::from_slice(&bytes)
                    .map_err(|error| Error::internal(format!("投影读不回来：{error}")))?;
                // 豁免先判 —— RET-007：两条规则都命中时豁免赢。
                if exemption(table.as_str(), &row.payload).is_some() {
                    swept.exempted += 1;
                    continue;
                }
                let retain_until = read_millis(&row.payload, "retainUntil");
                let trace_until = read_millis(&row.payload, "traceRetainUntil");

                if retain_until.is_some_and(|until| until <= now.as_millis()) {
                    // 整行删除（`RET-003`）。这是全系统唯一一处硬删除。
                    self.store.delete(space::ROW, &key)?;
                    self.store
                        .delete(space::EVENT, &keys::event(&physical, row.seq))?;
                    swept.rows += 1;
                } else if trace_until.is_some_and(|until| until <= now.as_millis())
                    && row
                        .payload
                        .get("trace")
                        .is_some_and(|trace| !trace.is_null())
                {
                    // RET-004：过程记录到期**只清这一列**，行本身按输出保留期走。
                    self.clear_trace(&physical, row.row, &row)?;
                    swept.traces += 1;
                }
            }
        }

        // RET-010：删除这件事本身留痕。
        let envelope = AuditEnvelope::project_scoped(
            kinds::RETENTION_SWEPT,
            project.as_id(),
            project.as_id(),
            serde_json::json!({
                "table": table.as_str(),
                "before": now.as_millis(),
                "rows": swept.rows,
                "traces": swept.traces,
                "exempted": swept.exempted,
            }),
        )?;
        self.audit.append(&Actor::Platform, &envelope)?;
        Ok(swept)
    }

    fn clear_trace(&self, physical: &TableName, row: RowId, stored: &Row) -> Result<()> {
        let mut payload = stored.payload.clone();
        if let Some(object) = payload.as_object_mut() {
            object.insert("trace".into(), Value::Null);
        }
        let updated = Row {
            payload,
            ..stored.clone()
        };
        self.store.put(
            space::ROW,
            &keys::row(physical, row),
            &serde_json::to_vec(&updated)
                .map_err(|error| Error::internal(format!("投影装不下：{error}")))?,
        )
    }
}

fn read_millis(payload: &Value, field: &str) -> Option<i64> {
    payload.get(field).and_then(Value::as_i64)
}

/// 让 `Exemption` 在文档链接里可见。
#[allow(dead_code, reason = "文档链接用")]
type _ExemptionLink = Exemption;
