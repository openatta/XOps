//! 后台那几件事：**没有人在等它们，所以必须有人定期去做。**
//!
//! # 它补的是同一类口子
//!
//! 这个仓里每个单元自己都是对的、测试也绿，而好几条链**从来没有谁调用**——
//! 定时到点了没人点、webhook 掉进地里、实例永不过期、保留期从不生效。
//! 单元测试证明不了"这个对象在成品里被接上了",**装配层是唯一知道全貌的地方**。
//!
//! ```text
//! 每 500ms   落账      跑完的执行 → `_runs` 那一行 + 一条通知
//! 每 5s      定时      到点的调度 → 触发（TRG-009）
//! 每 60s     到期      实例过期（FLW-017）
//! 每 1h      保留期    _runs / 审计 / 通知 各按各的清（RET-003 / AUD-010 / RET-008）
//! ```
//!
//! ⚠️ **每一轮的失败都只记一条、不中断**:一件事做不成不该让别的也停下，
//! 而"一直做不成"要看得见。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use xops_core::log;

use crate::assemble::Assembled;

/// 落账扫一遍的间隔。**它是账变得可见的延迟上限。**
pub const REAP_EVERY: Duration = Duration::from_millis(500);
/// 定时器的分辨率。**比它更细的 cron 表达式没有意义。**
pub const TICK_EVERY: Duration = Duration::from_secs(5);
/// 多久看一次有没有实例过期。
pub const EXPIRE_EVERY: Duration = Duration::from_secs(60);
/// 多久清一次到期数据。**整批离线进行**（`CON-006`）。
pub const PRUNE_EVERY: Duration = Duration::from_secs(60 * 60);

/// 起后台维护线程。`stop` 一置起就收工。
pub fn spawn(assembled: &Assembled, stop: &Arc<AtomicBool>) -> Vec<thread::JoinHandle<()>> {
    let mut handles = Vec::new();

    // ① 落账。执行跑完之后是它把 `_runs` 那一行写下来。
    {
        let reaper = Arc::clone(&assembled.reaper);
        let stop = Arc::clone(stop);
        handles.push(thread::spawn(move || {
            every(&stop, REAP_EVERY, || match reaper.sweep() {
                Ok(landed) if landed > 0 => {
                    log::info("bg.landed", &[("runs", &landed.to_string())]);
                }
                Ok(_) => {}
                Err(error) => log::error("bg.reap", &[("error", &format!("{error}"))]),
            });
        }));
    }

    // ② 定时。**到点了得有人去点它**——不然 `schedule.configure` 存得进去、永不触发。
    {
        let ticker = Arc::clone(&assembled.ticker);
        let stop = Arc::clone(stop);
        handles.push(thread::spawn(move || {
            every(&stop, TICK_EVERY, || match ticker.tick() {
                Ok(fired) if fired > 0 => {
                    log::info("bg.scheduled", &[("fired", &fired.to_string())]);
                }
                Ok(_) => {}
                Err(error) => log::error("bg.tick", &[("error", &format!("{error}"))]),
            });
        }));
    }

    // ③ 实例过期（`FLW-017`）。
    {
        let flows = Arc::clone(&assembled.flows);
        let clock = Arc::clone(&assembled.clock);
        let stop = Arc::clone(stop);
        handles.push(thread::spawn(move || {
            every(&stop, EXPIRE_EVERY, || {
                match flows.expire_due(clock.now()) {
                    Ok(expired) if expired > 0 => {
                        log::info("bg.expired", &[("instances", &expired.to_string())]);
                    }
                    Ok(_) => {}
                    Err(error) => log::error("bg.expire", &[("error", &format!("{error}"))]),
                }
            });
        }));
    }

    // ④ 保留期。**整批按时间，不挑行**（`RET-005`）。
    {
        let keeper = Arc::clone(&assembled.keeper);
        let stop = Arc::clone(stop);
        handles.push(thread::spawn(move || {
            every(&stop, PRUNE_EVERY, || match keeper.prune() {
                Ok(swept) if swept > 0 => {
                    log::info("bg.pruned", &[("rows", &swept.to_string())]);
                }
                Ok(_) => {}
                Err(error) => log::error("bg.prune", &[("error", &format!("{error}"))]),
            });
        }));
    }

    handles
}

/// 每隔一段做一次，直到 `stop`。
///
/// ⚠️ **等待切成小段**:直接 `sleep(1 小时)` 会让停机等上一个小时。
fn every(stop: &AtomicBool, period: Duration, mut work: impl FnMut()) {
    const SLICE: Duration = Duration::from_millis(200);
    while !stop.load(Ordering::Relaxed) {
        work();
        let deadline = Instant::now() + period;
        while Instant::now() < deadline {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(SLICE.min(deadline.saturating_duration_since(Instant::now())));
        }
    }
}
