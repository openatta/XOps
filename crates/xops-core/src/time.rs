//! 时间。
//!
//! 时间从 [`Clock`] 来，不从 `SystemTime::now()` 来——写入路径上的每一处时间戳都要能在
//! 测试里被摆布，否则"两个并发写的事件顺序确定"这类验收只能靠 sleep 去碰。

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// UTC 毫秒。**存储里一律存这个数**，不存字符串——换库时不需要关心日期方言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp(i64);

impl Timestamp {
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self(millis)
    }

    #[must_use]
    pub const fn as_millis(self) -> i64 {
        self.0
    }
}

/// 取当前时间的唯一途径。
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> Timestamp;
}

/// 真实时钟。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        Timestamp::from_millis(millis)
    }
}

/// 测试用时钟：停在某一刻，除非被推着走。
#[derive(Debug)]
pub struct FixedClock(AtomicI64);

impl FixedClock {
    #[must_use]
    pub fn new(millis: i64) -> Self {
        Self(AtomicI64::new(millis))
    }

    /// 往前推 `millis` 毫秒，返回推之后的时刻。
    pub fn advance(&self, millis: i64) -> Timestamp {
        Timestamp::from_millis(self.0.fetch_add(millis, Ordering::SeqCst) + millis)
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_millis(self.0.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 固定时钟不动除非被推() {
        let clock = FixedClock::new(1_000);
        assert_eq!(clock.now().as_millis(), 1_000);
        assert_eq!(clock.now().as_millis(), 1_000);
        assert_eq!(clock.advance(5).as_millis(), 1_005);
        assert_eq!(clock.now().as_millis(), 1_005);
    }

    #[test]
    fn 真实时钟给的是一个像样的年份() {
        // 2020-01-01 之后、2200 年之前。只是防呆，不是对精度的断言。
        let now = SystemClock.now().as_millis();
        assert!(now > 1_577_836_800_000, "{now}");
        assert!(now < 7_258_118_400_000, "{now}");
    }
}
