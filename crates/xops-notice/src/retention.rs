//! `_notices` 自己的保留期（`RET-008`）。
//!
//! > **平台级配置，默认 3 个月，与任务无关**——否则通知会无限增长。
//!
//! ⚠️ 它**不受任务保留期约束**（`RET-006` ④）：一条通知的去留与产生它的那次执行
//! 保留多久没有关系。清理**整批按时间进行**（`RET-005`）。

use xops_core::Timestamp;

/// 一天多少毫秒。
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1_000;

/// 通知保留多久。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    pub keep_days: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Self::PLATFORM_DEFAULT
    }
}

impl Retention {
    /// **3 个月**（`RET-008`）。
    pub const PLATFORM_DEFAULT: Self = Self { keep_days: 90 };

    /// 这一刻写下的通知，留到什么时候。
    #[must_use]
    pub fn retain_until(self, now: Timestamp) -> Timestamp {
        Timestamp::from_millis(now.as_millis() + i64::from(self.keep_days) * DAY_MILLIS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 默认三个月() {
        assert_eq!(Retention::default().keep_days, 90, "RET-008");
    }

    #[test]
    fn 到期时刻按写入当时算() {
        let now = Timestamp::from_millis(1_000);
        let until = Retention::default().retain_until(now);
        assert_eq!(until.as_millis(), 1_000 + 90 * DAY_MILLIS);
    }
}
