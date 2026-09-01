//! 调度的存放与到点扫描。
//!
//! ⚠️ **错过的窗口不补跑**（`TRG-010`）——但**每一个错过的窗口都留一条痕迹**。
//! 静默跳过与"它本来就没到点"在外面看起来一模一样，而那正是这条要防的。

use std::sync::Arc;

use xops_audit::{AuditEnvelope, AuditLog};
use xops_core::{Actor, Error, Result, Timestamp};
use xops_task::TaskId;

use crate::schedule::Schedule;

/// 调度记录的键空间。
const SPACE: &str = "schedule";

/// 事件类型。
pub mod kinds {
    /// 错过的窗口。**不补跑，但留痕。**
    pub const WINDOW_MISSED: &str = "schedule.window-missed";
}

/// 调度表。
pub struct Schedules {
    store: Arc<dyn xops_store::Store>,
    audit: Arc<AuditLog>,
}

impl std::fmt::Debug for Schedules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Schedules").finish_non_exhaustive()
    }
}

impl Schedules {
    #[must_use]
    pub fn new(store: Arc<dyn xops_store::Store>, audit: Arc<AuditLog>) -> Self {
        Self { store, audit }
    }

    /// 放一条。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn put(&self, schedule: &Schedule) -> Result<()> {
        self.store.put(
            SPACE,
            schedule.task.to_string().as_bytes(),
            &serde_json::to_vec(schedule)
                .map_err(|error| Error::internal(format!("调度装不下：{error}")))?,
        )
    }

    /// 取一条。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn get(&self, task: TaskId) -> Result<Option<Schedule>> {
        self.store
            .get(SPACE, task.to_string().as_bytes())?
            .map(|bytes| {
                serde_json::from_slice(&bytes)
                    .map_err(|error| Error::internal(format!("调度读不回来：{error}")))
            })
            .transpose()
    }

    /// 全部调度。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn all(&self) -> Result<Vec<Schedule>> {
        let mut out = Vec::new();
        let mut cursor: Option<Vec<u8>> = None;
        loop {
            let page = self.store.scan(SPACE, &[], cursor.as_deref(), 256)?;
            if page.is_empty() {
                break;
            }
            cursor = page.last().map(|(key, _)| key.clone());
            for (_, bytes) in page {
                if let Ok(schedule) = serde_json::from_slice::<Schedule>(&bytes) {
                    out.push(schedule);
                }
            }
        }
        Ok(out)
    }

    /// 此刻到点的那些。
    ///
    /// **顺带把错过的窗口逐个留痕**——它们不补跑，但要看得见。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn due(&self, project: xops_identity::ProjectId, now: Timestamp) -> Result<Vec<Schedule>> {
        let mut due = Vec::new();
        for schedule in self.all()? {
            if !schedule.due(now) {
                continue;
            }
            for missed in schedule.missed_windows(now) {
                let envelope = AuditEnvelope::project_scoped(
                    kinds::WINDOW_MISSED,
                    project.as_id(),
                    schedule.task.as_id(),
                    serde_json::json!({
                        "window": missed.as_millis(),
                        "why": "服务不可用期间错过。**不补跑**——补跑会在恢复瞬间产生一批并发执行",
                    }),
                )?;
                self.audit.append(&Actor::Platform, &envelope)?;
            }
            due.push(schedule);
        }
        Ok(due)
    }

    /// 记下这一次真的触发了。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn mark_fired(&self, task: TaskId, at: Timestamp) -> Result<()> {
        let Some(mut schedule) = self.get(task)? else {
            return Ok(());
        };
        schedule.last_fired_at = Some(at);
        self.put(&schedule)
    }
}
