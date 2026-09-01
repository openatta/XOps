//! 一层薄的结构化日志。
//!
//! # 它为什么不是格式化字符串
//!
//! ```rust,ignore
//! log::info("mcp.call", &[("tool", name), ("outcome", "ok")]);   // 这样
//! log::info(&format!("调了 {name}，参数 {args:?}"));              // **不这样**
//! ```
//!
//! 第二种写法迟早会有人把一个带令牌的东西插进去——`token.issue` 的回话里有令牌原文，
//! 插件配置的值是凭据，派工单里有仓库凭据。**键值对让"要记什么"是一次显式选择**，
//! 而格式化字符串让它成了一次顺手。
//!
//! # 隐去是一张网，不是一个保证
//!
//! [`redact`] 会把长得像凭据的值换成 `<已隐去>`。**它挡不住所有形态**——
//! 真正的规矩仍然是**不要把密文传进来**。它存在的意义是：
//! 万一有人传了，这条日志不至于把凭据落在磁盘上。
//!
//! # 为什么不引一个日志库
//!
//! 这一层要的东西就这么多：一个级别、一个事件名、一串键值、一个时刻。
//! 引一套 `tracing` 换回来的是 subscriber、span、字段类型系统——
//! 而这个进程是单实例的、路由只有两条。**不值。**

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};

/// 日志级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// 一条也不记。
    Off = 0,
    Error = 1,
    Warn = 2,
    Info = 3,
    /// 每一次请求。
    Debug = 4,
}

impl Level {
    /// 从 `XOPS_LOG` 的值解析。不认识的当 `info`。
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Self::Off,
            "error" => Self::Error,
            "warn" | "warning" => Self::Warn,
            "debug" | "trace" => Self::Debug,
            _ => Self::Info,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }
}

/// 环境变量名。
pub const LEVEL_ENV: &str = "XOPS_LOG";

/// 隐去之后留下的东西。
pub const REDACTED: &str = "<已隐去>";

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

/// 设一个级别。**进程启动时调一次。**
pub fn set_level(level: Level) {
    LEVEL.store(level as u8, Ordering::Relaxed);
}

/// 按 `XOPS_LOG` 设级别，返回设成了什么。
#[must_use]
pub fn level_from_env() -> Level {
    let level = std::env::var(LEVEL_ENV).map_or(Level::Info, |text| Level::parse(&text));
    set_level(level);
    level
}

/// 现在是什么级别。
#[must_use]
pub fn level() -> Level {
    match LEVEL.load(Ordering::Relaxed) {
        0 => Level::Off,
        1 => Level::Error,
        2 => Level::Warn,
        4 => Level::Debug,
        _ => Level::Info,
    }
}

/// 长得像凭据的值换掉。
///
/// ⚠️ **这是一张网，不是一个保证。** 对照的是那几个已知前缀与词根——
/// 一段没有前缀的随机十六进制它认不出来。**规矩仍然是不要把密文传进来。**
///
/// 与 `xops_dispatch::looks_like_credential` 用的是同一批标记，
/// 但那一处是**拦下派工单**（拦不住就不发），这一处是**擦掉日志**（擦不掉也得记）——
/// 两处的失败后果不同，所以没有合成一个。
#[must_use]
pub fn redact(value: &str) -> String {
    const MARKERS: [&str; 9] = [
        "xops_",
        "xsess_",
        "ghp_",
        "github_pat_",
        "Authorization",
        "authToken",
        "password",
        "secret",
        "BEGIN PRIVATE KEY",
    ];
    if MARKERS.iter().any(|marker| {
        value
            .to_ascii_lowercase()
            .contains(&marker.to_ascii_lowercase())
    }) {
        return REDACTED.to_owned();
    }
    value.to_owned()
}

/// 记一条。
///
/// `event` 是一个**固定的事件名**（`mcp.call` / `web.request` 这样），不是一句话——
/// 它要能被 grep、被计数。变化的东西进 `fields`。
pub fn log(level: Level, event: &str, fields: &[(&str, &str)]) {
    if level > self::level() || level == Level::Off {
        return;
    }
    let mut line = format!("{} {:<5} {event}", now(), level.label());
    for (key, value) in fields {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(&quote(&redact(value)));
    }
    line.push('\n');
    // 写不出去就算了 —— **日志失败不能让请求失败**。
    let _ = std::io::stderr().write_all(line.as_bytes());
}

pub fn error(event: &str, fields: &[(&str, &str)]) {
    log(Level::Error, event, fields);
}

pub fn warn(event: &str, fields: &[(&str, &str)]) {
    log(Level::Warn, event, fields);
}

pub fn info(event: &str, fields: &[(&str, &str)]) {
    log(Level::Info, event, fields);
}

pub fn debug(event: &str, fields: &[(&str, &str)]) {
    log(Level::Debug, event, fields);
}

/// 值里有空白或引号就括起来——**一行一条要能被机器切开**。
fn quote(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_owned();
    }
    if value
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '=')
    {
        return format!("\"{}\"", value.replace('"', "'"));
    }
    value.to_owned()
}

/// ISO 8601 的 UTC 时刻，毫秒。
///
/// 自己算而不是引一个时间库：**这一层只需要把一个毫秒数写成人看得懂的样子**。
fn now() -> String {
    let millis = <crate::SystemClock as crate::Clock>::now(&crate::SystemClock).as_millis();
    let (days, rest) = (millis.div_euclid(86_400_000), millis.rem_euclid(86_400_000));
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second, milli) = (
        rest / 3_600_000,
        (rest / 60_000) % 60,
        (rest / 1_000) % 60,
        rest % 1_000,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milli:03}Z")
}

/// 从 1970-01-01 起的天数换成年月日（Howard Hinnant 的那套算法）。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 级别按环境变量解析不认识的当info() {
        assert_eq!(Level::parse("off"), Level::Off);
        assert_eq!(Level::parse("ERROR"), Level::Error);
        assert_eq!(Level::parse("debug"), Level::Debug);
        assert_eq!(
            Level::parse("胡说八道"),
            Level::Info,
            "不认识的不该把日志关掉"
        );
        assert_eq!(Level::parse(""), Level::Info);
    }

    #[test]
    fn 像凭据的值被擦掉() {
        assert_eq!(redact("xops_abcdef"), REDACTED);
        assert_eq!(redact("xsess_abcdef"), REDACTED);
        assert_eq!(redact("ghp_abcdef"), REDACTED);
        assert_eq!(redact("Bearer ghp_x"), REDACTED);
        assert_eq!(redact("my-password=1"), REDACTED, "词根也算");
        // ⚠️ **它挡不住所有形态** —— 这条把那句话钉住，免得有人以为它是个保证。
        assert_eq!(
            redact("deadbeefcafe0123"),
            "deadbeefcafe0123",
            "没有前缀的随机串它认不出来：规矩仍然是不要把密文传进来"
        );
    }

    #[test]
    fn 一行一条能被机器切开() {
        assert_eq!(quote("ok"), "ok");
        assert_eq!(quote("有 空 格"), "\"有 空 格\"");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote("a=b"), "\"a=b\"", "等号也要括起来，不然切不开");
        assert_eq!(quote("说\"话\""), "\"说'话'\"");
    }

    #[test]
    fn 时刻是iso8601() {
        // 几个人工核对过的日子，含**闰日**——那是这套算法最容易错的地方。
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29), "2000 是闰年");
        assert_eq!(civil_from_days(19_601), (2023, 9, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
        let stamp = now();
        assert_eq!(stamp.len(), 24, "{stamp}");
        assert!(stamp.ends_with('Z') && stamp.contains('T'));
    }

    #[test]
    fn 关掉之后一条也不记() {
        set_level(Level::Off);
        assert_eq!(level(), Level::Off);
        // 记不记得出去这里看不到，但级别判定是同一条路。
        set_level(Level::Info);
        assert_eq!(level(), Level::Info);
    }
}
