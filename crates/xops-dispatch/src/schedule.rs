//! 定时触发（`TRG-009` / `TRG-010`）。
//!
//! 两类表达就够了：**每天某时** 与 **每隔 N 小时**，都带明确的时区。
//!
//! ⚠️ `TRG-010`：**错过的窗口不补跑。**
//! > 补跑会在恢复瞬间产生一批并发执行，风险大于收益。
//!
//! 但**错过这件事要留痕**——静默跳过与"它本来就没到点"在外面看起来一模一样。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Result, Timestamp};
use xops_identity::UserId;
use xops_task::TaskId;

/// 一天的毫秒数。
const DAY: i64 = 24 * 60 * 60 * 1_000;
/// 一小时的毫秒数。
const HOUR: i64 = 60 * 60 * 1_000;

/// 怎么定时。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Cadence {
    /// 每天某时。`hour` / `minute` 是**那个时区里的**钟点。
    Daily { hour: u8, minute: u8 },
    /// 每隔 N 小时。
    EveryHours { hours: u8 },
}

/// 一条调度。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub task: TaskId,
    pub cadence: Cadence,
    /// 时区相对 UTC 的偏移，分钟。**必须明确**（`TRG-009`）——
    /// "每天 02:00"不说时区等于没说。
    pub utc_offset_minutes: i16,
    /// 配置它的人。**触发者记为系统，但必须能追溯到他。**
    pub configured_by: UserId,
    /// 上一次真正触发的时刻。
    pub last_fired_at: Option<Timestamp>,
}

impl Schedule {
    /// # Errors
    /// 钟点越界、间隔为 0 或超过 24、时区偏移离谱。
    pub fn new(
        task: TaskId,
        cadence: Cadence,
        utc_offset_minutes: i16,
        configured_by: UserId,
    ) -> Result<Self> {
        match cadence {
            Cadence::Daily { hour, minute } => {
                if hour > 23 || minute > 59 {
                    return Err(Error::invalid("钟点越界"));
                }
            }
            Cadence::EveryHours { hours } => {
                if hours == 0 || hours > 24 {
                    return Err(Error::invalid("间隔要在 1–24 小时之间"));
                }
            }
        }
        if !(-720..=840).contains(&utc_offset_minutes) {
            return Err(Error::invalid("时区偏移离谱"));
        }
        Ok(Self {
            task,
            cadence,
            utc_offset_minutes,
            configured_by,
            last_fired_at: None,
        })
    }

    /// `after` 之后的下一个触发时刻（`TRG-009` 要求可查）。
    #[must_use]
    pub fn next_after(&self, after: Timestamp) -> Timestamp {
        let offset = i64::from(self.utc_offset_minutes) * 60 * 1_000;
        match self.cadence {
            Cadence::Daily { hour, minute } => {
                let local = after.as_millis() + offset;
                let day_start = local.div_euclid(DAY) * DAY;
                let target = day_start + i64::from(hour) * HOUR + i64::from(minute) * 60 * 1_000;
                let local_next = if target > local { target } else { target + DAY };
                Timestamp::from_millis(local_next - offset)
            }
            Cadence::EveryHours { hours } => {
                let step = i64::from(hours) * HOUR;
                let base = self
                    .last_fired_at
                    .map_or(after.as_millis(), Timestamp::as_millis);
                let elapsed = after.as_millis() - base;
                let steps = elapsed.div_euclid(step) + 1;
                Timestamp::from_millis(base + steps * step)
            }
        }
    }

    /// 从上次触发到现在，**错过了几个窗口**。
    ///
    /// 它们**不补跑**（`TRG-010`），但要留痕——所以这个数要算得出来。
    #[must_use]
    pub fn missed_windows(&self, now: Timestamp) -> Vec<Timestamp> {
        let Some(last) = self.last_fired_at else {
            return Vec::new();
        };
        let mut missed = Vec::new();
        let mut cursor = self.next_after(last);
        // 只数**已经过去**的窗口，且不含此刻这一个——此刻这个是要正常触发的那个。
        while cursor.as_millis() < now.as_millis() && missed.len() < 1_000 {
            missed.push(cursor);
            cursor = self.next_after(cursor);
        }
        // 最后那一个是"现在该触发的"，不算错过。
        missed.pop();
        missed
    }

    /// 现在该不该触发。
    #[must_use]
    pub fn due(&self, now: Timestamp) -> bool {
        match self.last_fired_at {
            None => true,
            Some(last) => self.next_after(last).as_millis() <= now.as_millis(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule(cadence: Cadence) -> Schedule {
        Schedule::new(TaskId::generate(), cadence, 8 * 60, UserId::generate()).unwrap()
    }

    #[test]
    fn 每天某时算得出下一次() {
        // 东八区的 02:00 = UTC 前一天 18:00。
        let daily = schedule(Cadence::Daily { hour: 2, minute: 0 });
        let now = Timestamp::from_millis(0); // UTC 1970-01-01 00:00 = 本地 08:00
        let next = daily.next_after(now);
        // 本地明天 02:00 = UTC 今天 18:00。
        assert_eq!(next.as_millis(), 18 * HOUR);
    }

    #[test]
    fn 时区必须说清楚() {
        assert!(
            Schedule::new(
                TaskId::generate(),
                Cadence::Daily {
                    hour: 25,
                    minute: 0
                },
                0,
                UserId::generate()
            )
            .is_err()
        );
        assert!(
            Schedule::new(
                TaskId::generate(),
                Cadence::EveryHours { hours: 0 },
                0,
                UserId::generate()
            )
            .is_err()
        );
        assert!(
            Schedule::new(
                TaskId::generate(),
                Cadence::Daily { hour: 2, minute: 0 },
                9_999,
                UserId::generate()
            )
            .is_err()
        );
    }

    #[test]
    fn 错过的窗口不补跑但数得出来() {
        let mut every = schedule(Cadence::EveryHours { hours: 1 });
        every.last_fired_at = Some(Timestamp::from_millis(0));
        // 停服三个多小时。
        let now = Timestamp::from_millis(3 * HOUR + 10 * 60 * 1_000);
        let missed = every.missed_windows(now);
        assert_eq!(
            missed.len(),
            2,
            "TRG-010：错过两个窗口，各有一条痕迹，但都不补跑"
        );
        assert!(every.due(now), "而现在这一个照常触发");
    }

    #[test]
    fn 没跑过就该跑一次() {
        let every = schedule(Cadence::EveryHours { hours: 6 });
        assert!(every.due(Timestamp::from_millis(0)));
        assert!(
            every.missed_windows(Timestamp::from_millis(0)).is_empty(),
            "没有历史就没有错过"
        );
    }

    #[test]
    fn 追得到配置它的人() {
        let who = UserId::generate();
        let schedule =
            Schedule::new(TaskId::generate(), Cadence::EveryHours { hours: 1 }, 0, who).unwrap();
        assert_eq!(
            schedule.configured_by, who,
            "TRG-009：触发者记为系统，但要追得到人"
        );
    }
}
