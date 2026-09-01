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

/// 到点了去点它（`TRG-009`）。
///
/// # 它补的是哪个口子
///
/// `schedule.configure` 存得进去、`schedule.next` 算得出下一次——
/// **而没有任何东西到点去触发**。定时任务因此永远不会跑，而且它是静默的:
/// 配置在那儿、时间也对，就是什么都不发生。
pub struct Ticker {
    schedules: Arc<Schedules>,
    tasks: Arc<xops_task::Tasks>,
    dispatcher: Arc<crate::Dispatcher>,
    clock: Arc<dyn xops_core::Clock>,
}

impl std::fmt::Debug for Ticker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ticker").finish_non_exhaustive()
    }
}

impl Ticker {
    #[must_use]
    pub fn new(
        schedules: Arc<Schedules>,
        tasks: Arc<xops_task::Tasks>,
        dispatcher: Arc<crate::Dispatcher>,
        clock: Arc<dyn xops_core::Clock>,
    ) -> Self {
        Self {
            schedules,
            tasks,
            dispatcher,
            clock,
        }
    }

    /// 扫一遍，把到点的都点了。返回点了几个。
    ///
    /// # Errors
    /// 底层不可用。**单个触发失败不中断整轮。**
    pub fn tick(&self) -> Result<usize> {
        let now = self.clock.now();
        let mut fired = 0;
        for mut schedule in self.schedules.all()? {
            if !schedule.due(now) {
                continue;
            }
            let Ok(task) = self.tasks.read_internal(schedule.task) else {
                continue;
            };
            let event = crate::Event {
                kind: crate::EventKind::Scheduled,
                project: task.project,
                // ⚠️ **外部标识用「这一次的窗口」**:同一个窗口重复扫到时
                // `TRG-013` 的幂等会把它挡成 Duplicate，而不是跑第二遍。
                external_id: Some(format!(
                    "schedule\u{0}{}\u{0}{}",
                    schedule.task,
                    now.as_millis() / 1000
                )),
                triggered_by: crate::Trigger::Schedule {
                    configured_by: schedule.configured_by,
                },
                revision: None,
                at: now,
                payload: serde_json::json!({}),
            };
            match self.dispatcher.trigger(&task, &event) {
                Ok(_) => {
                    // **先记下已经点过，再算下一次**——顺序反了会重复点。
                    schedule.last_fired_at = Some(now);
                    if let Err(error) = self.schedules.put(&schedule) {
                        xops_core::log::error(
                            "schedule.mark",
                            &[
                                ("task", &schedule.task.to_string()),
                                ("error", &format!("{error}")),
                            ],
                        );
                    }
                    fired += 1;
                }
                Err(error) => xops_core::log::warn(
                    "schedule.fire",
                    &[
                        ("task", &schedule.task.to_string()),
                        ("error", &format!("{error}")),
                    ],
                ),
            }
        }
        Ok(fired)
    }
}
