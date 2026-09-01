//! QuickJS 载体。
//!
//! > **一次调用新建一个 runtime，调用结束整个扔掉**——调用之间不共享任何状态（`PLG-001`）。
//!
//! 三件防故障（**不是防恶意**）的事（`PLG-013`）：
//!
//! ```text
//! 超时      **超时视为「节点未通过」，绝不视为通过**
//! 错误隔离  插件抛异常不能拖垮平台；**故障只花一次调用**，不是一个 runtime 的持续降级
//! 不递归    交回、由平台代写的行不再触发插件求值（见 [`crate::positions::triggers_evaluation`]）
//! ```
//!
//! ⚠️ **死循环由载体的字节码级中断兜住**——光靠超时停不下一个不让出的循环；
//! 它的表现必须是"这次求值超时"，**不是"一个线程转死了"**。
//!
//! # 绑定只按声明注入
//!
//! **能力默认为零，未声明即没有——不是"调用时被拒绝"，是那个函数不存在**（`I-Z`）。
//! 所以这个文件里没有一处"检查权限然后返回错误"的宿主函数：
//! **没声明的那一样，`globalThis` 上根本没有它。**

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use xops_core::{Error, Result};

use crate::capability::{Capabilities, Position};
use crate::net::{Net, Request};

/// 流转插件的硬超时默认值（`CON-009`）。
///
/// ⚠️ **求值在写串行区间的锁内**：这个上限直接决定它所在结算表的最大写吞吐。
/// QuickJS 让一次求值退化成毫秒级的纯计算，**但那不构成把它调大的理由**。
pub const TRANSITION_TIMEOUT: Duration = Duration::from_millis(200);
/// 输出插件的硬超时。它在锁外，可以宽一些。
pub const OUTPUT_TIMEOUT: Duration = Duration::from_secs(10);
/// 一次调用的内存上限。
pub const MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
/// 一次读表最多几行。
pub const READ_TABLE_LIMIT: usize = 500;

/// 宿主这一侧。**只有声明过的那几样会被调用到**——没声明的那一样连绑定都不注入。
pub trait Host: Send + Sync + 'static {
    /// 读**它自己**那一份配置（`PLG-012` ②）。
    ///
    /// # Errors
    /// 底层不可用。
    fn config(&self) -> Result<BTreeMap<String, String>>;

    /// 读一张表（`PLG-012` ③）。
    ///
    /// # Errors
    /// 表不存在或底层不可用。**"不许读"不从这里出去**——那种表压根不在绑定的可达范围里。
    fn read_table(&self, table: &str, limit: usize) -> Result<Value>;

    /// 出网后端。
    fn net(&self) -> &dyn Net;
}

/// 这次调用给了什么。
pub struct Grant {
    pub capabilities: Capabilities,
    /// 没有宿主就等于三样都给不了——**此时连声明过的绑定也不注入**。
    pub host: Option<Arc<dyn Host>>,
}

impl std::fmt::Debug for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grant")
            .field("capabilities", &self.capabilities)
            .field("host", &self.host.is_some())
            .finish()
    }
}

impl Grant {
    /// 一样都不给。**流转插件永远是这个**。
    #[must_use]
    pub fn none() -> Self {
        Self {
            capabilities: Capabilities::none(),
            host: None,
        }
    }
}

/// 载体给出的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 跑完了，这是它返回的东西。
    Returned(Value),
    /// 超时（含死循环被中断）。**视为未通过。**
    TimedOut,
    /// 抛了异常。**视为未通过，并留痕。**
    Threw(String),
}

impl Outcome {
    /// 拿到返回值，或者说清为什么没有。
    #[must_use]
    pub const fn value(&self) -> Option<&Value> {
        match self {
            Self::Returned(value) => Some(value),
            _ => None,
        }
    }

    /// 出了什么事，一句话。**给留痕用。**
    #[must_use]
    pub fn note(&self) -> String {
        match self {
            Self::Returned(_) => String::new(),
            Self::TimedOut => "插件求值超时".to_owned(),
            Self::Threw(error) => format!("插件抛异常：{error}"),
        }
    }
}

/// 宿主函数统一的回话格式：JS 那一侧只看得到值或者一个异常。
fn ok(value: &Value) -> String {
    serde_json::json!({"ok": true, "value": value}).to_string()
}

fn failed(message: &str) -> String {
    serde_json::json!({"ok": false, "error": message}).to_string()
}

/// 跑一次插件。
///
/// **每次调用新建一个 `Runtime`**——这是"调用之间不共享任何状态"的实现方式，
/// 不是靠清理全局对象。
///
/// # Errors
/// 载体本身建不起来。**插件自己的失败不是 `Err`**——它是 [`Outcome::TimedOut`]
/// 或 [`Outcome::Threw`]，因为那是一次正常的求值结果。
pub fn invoke(
    source: &str,
    entry: &str,
    input: &Value,
    position: Position,
    grant: &Grant,
) -> Result<Outcome> {
    let timeout = match position {
        Position::Transition => TRANSITION_TIMEOUT,
        Position::Output => OUTPUT_TIMEOUT,
    };
    let runtime = rquickjs::Runtime::new()
        .map_err(|error| Error::internal(format!("建不起 QuickJS runtime：{error}")))?;
    runtime.set_memory_limit(MEMORY_LIMIT_BYTES);

    // 字节码级中断：**光靠超时停不下一个不让出的循环。**
    let deadline = Instant::now() + timeout;
    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let interrupted = Arc::clone(&interrupted);
        runtime.set_interrupt_handler(Some(Box::new(move || {
            if Instant::now() >= deadline {
                interrupted.store(true, Ordering::SeqCst);
                return true;
            }
            false
        })));
    }

    let context = rquickjs::Context::full(&runtime)
        .map_err(|error| Error::internal(format!("建不起 QuickJS context：{error}")))?;
    let prelude = bind(&context, grant)?;

    let program = format!(
        "{prelude}\n{source}\n;(function(){{ \
         return JSON.stringify(({entry})(JSON.parse(__xops_input))); }})()"
    );
    let input_text = input.to_string();

    let outcome = context.with(|ctx| {
        if ctx
            .globals()
            .set("__xops_input", input_text.clone())
            .is_err()
        {
            return Outcome::Threw("喂不进输入".to_owned());
        }
        match ctx.eval::<String, _>(program.as_str()) {
            Ok(text) => serde_json::from_str(&text).map_or_else(
                |error| Outcome::Threw(format!("返回值不是 JSON：{error}")),
                Outcome::Returned,
            ),
            Err(error) => {
                if interrupted.load(Ordering::SeqCst) {
                    Outcome::TimedOut
                } else {
                    Outcome::Threw(describe(&ctx, &error))
                }
            }
        }
    });
    Ok(outcome)
}

fn describe(ctx: &rquickjs::Ctx<'_>, error: &rquickjs::Error) -> String {
    if matches!(error, rquickjs::Error::Exception) {
        ctx.catch().as_exception().map_or_else(
            || "未知异常".to_owned(),
            |exception| exception.message().unwrap_or_else(|| "未知异常".to_owned()),
        )
    } else {
        format!("{error}")
    }
}

/// 注入绑定，**只注入声明过的那几样**，并返回要拼在源码前面的那段 JS。
fn bind(context: &rquickjs::Context, grant: &Grant) -> Result<String> {
    let Some(host) = grant.host.clone() else {
        // 没有宿主 —— 一样都注入不了。流转插件走的永远是这一条。
        return Ok(String::new());
    };
    let capabilities = grant.capabilities.clone();
    let mut granted: Vec<&str> = Vec::new();

    context
        .with(|ctx| -> rquickjs::Result<()> {
            let globals = ctx.globals();
            if capabilities.own_config {
                let host = Arc::clone(&host);
                globals.set(
                    "__xops_config",
                    rquickjs::function::Func::from(move || match host.config() {
                        Ok(config) => ok(&serde_json::json!(config)),
                        Err(error) => failed(&format!("{error}")),
                    }),
                )?;
            }
            if !capabilities.tables.is_empty() {
                let host = Arc::clone(&host);
                let allowed = capabilities.clone();
                globals.set(
                    "__xops_read_table",
                    rquickjs::function::Func::from(move |name: String, limit: i64| {
                        // 声明之外的表**不在这个绑定够得到的范围里**。
                        let known = allowed
                            .tables
                            .iter()
                            .any(|table| table.as_str() == name.as_str());
                        if !known {
                            return failed(&format!(
                                "{name} 不在这个插件声明过的表里——它没有这条路"
                            ));
                        }
                        let limit = usize::try_from(limit.max(0))
                            .unwrap_or(READ_TABLE_LIMIT)
                            .clamp(1, READ_TABLE_LIMIT);
                        match host.read_table(&name, limit) {
                            Ok(rows) => ok(&rows),
                            Err(error) => failed(&format!("{error}")),
                        }
                    }),
                )?;
            }
            if !capabilities.network.is_empty() {
                let host = Arc::clone(&host);
                let allowed = capabilities.clone();
                globals.set(
                    "__xops_fetch",
                    rquickjs::function::Func::from(
                        move |url: String, method: String, body: String| {
                            let request = Request {
                                url,
                                method,
                                body: (!body.is_empty()).then_some(body),
                            };
                            match crate::net::fetch(host.net(), &allowed, request) {
                                Ok(response) => ok(&serde_json::json!({
                                    "status": response.status,
                                    "body": response.body,
                                })),
                                Err(error) => failed(&format!("{error}")),
                            }
                        },
                    ),
                )?;
            }
            Ok(())
        })
        .map_err(|error| Error::internal(format!("注入绑定时出错：{error}")))?;

    if grant.capabilities.own_config {
        granted.push("config: function () { return __xops_unwrap(__xops_config()); }");
    }
    if !grant.capabilities.tables.is_empty() {
        granted.push(
            "readTable: function (name, limit) { \
             return __xops_unwrap(__xops_read_table(String(name), limit || 100)); }",
        );
    }
    if !grant.capabilities.network.is_empty() {
        granted.push(
            "fetch: function (url, options) { options = options || {}; \
             return __xops_unwrap(__xops_fetch(String(url), \
             String(options.method || 'GET'), String(options.body || ''))); }",
        );
    }
    if granted.is_empty() {
        return Ok(String::new());
    }
    Ok(format!(
        "function __xops_unwrap(text) {{ var reply = JSON.parse(text); \
         if (!reply.ok) {{ throw new Error(reply.error); }} return reply.value; }}\n\
         globalThis.xops = {{ {} }};\n",
        granted.join(", ")
    ))
}

/// 编译 + 入口导出检查（`PLG-006` 的第一件、`PLG-017` 的 ①）。
///
/// # Errors
/// 编译不过，或者入口不在。
pub fn compile_check(source: &str, entry: &str) -> Result<()> {
    let runtime = rquickjs::Runtime::new()
        .map_err(|error| Error::internal(format!("建不起 QuickJS runtime：{error}")))?;
    runtime.set_memory_limit(MEMORY_LIMIT_BYTES);
    let context = rquickjs::Context::full(&runtime)
        .map_err(|error| Error::internal(format!("建不起 QuickJS context：{error}")))?;
    context.with(|ctx| {
        ctx.eval::<(), _>(source)
            .map_err(|error| Error::invalid(format!("编译不过：{}", describe(&ctx, &error))))?;
        let exists: bool = ctx
            .eval(format!("typeof ({entry}) === 'function'").as_str())
            .map_err(|error| Error::invalid(format!("查入口时出错：{error}")))?;
        if !exists {
            return Err(Error::invalid(format!(
                "入口 {entry} 不在，或者不是一个函数"
            )));
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capabilities;
    use serde_json::json;
    use xops_table::TableId;

    struct Fake;

    impl Net for Fake {
        fn send(&self, request: &Request) -> Result<crate::net::Response> {
            Ok(crate::net::Response {
                status: 200,
                location: None,
                body: format!("发给了 {}", request.url),
            })
        }
    }

    impl Host for Fake {
        fn config(&self) -> Result<BTreeMap<String, String>> {
            Ok(BTreeMap::from([("token".to_owned(), "s3cr3t".to_owned())]))
        }

        fn read_table(&self, table: &str, limit: usize) -> Result<Value> {
            Ok(json!([{"表": table, "上限": limit}]))
        }

        fn net(&self) -> &dyn Net {
            self
        }
    }

    fn bare(source: &str, entry: &str, input: Value, position: Position) -> Outcome {
        invoke(source, entry, &input, position, &Grant::none()).unwrap()
    }

    fn granted(source: &str, capabilities: Capabilities) -> Outcome {
        invoke(
            source,
            "run",
            &json!({}),
            Position::Output,
            &Grant {
                capabilities,
                host: Some(Arc::new(Fake)),
            },
        )
        .unwrap()
    }

    #[test]
    fn 跑得通一个正常插件() {
        let outcome = bare(
            "function decide(input) { return { pass: input.votes >= 2 }; }",
            "decide",
            json!({"votes": 3}),
            Position::Transition,
        );
        assert_eq!(outcome.value().unwrap()["pass"], json!(true));
    }

    #[test]
    fn 死循环被字节码级中断停下() {
        let started = Instant::now();
        let outcome = bare(
            "function decide() { while (true) {} }",
            "decide",
            json!({}),
            Position::Transition,
        );
        assert_eq!(outcome, Outcome::TimedOut, "表现是「这次求值超时」");
        assert!(
            started.elapsed() < TRANSITION_TIMEOUT * 8,
            "不是「一个线程转死了」——它真的停下来了"
        );
    }

    #[test]
    fn 抛异常是未通过而且故障只花一次调用() {
        let outcome = bare(
            "function decide() { throw new Error('炸了'); }",
            "decide",
            json!({}),
            Position::Transition,
        );
        assert!(matches!(outcome, Outcome::Threw(_)));
        assert!(outcome.note().contains("抛异常"));
        let next = bare(
            "function decide() { return {pass: true}; }",
            "decide",
            json!({}),
            Position::Transition,
        );
        assert_eq!(next.value().unwrap()["pass"], json!(true), "下一次照常");
    }

    #[test]
    fn 跨调用不共享任何状态() {
        let leave = "globalThis.__mark = '我来过'; \
                     function decide() { return {mark: globalThis.__mark ?? null}; }";
        let read = "function decide() { return {mark: globalThis.__mark ?? null}; }";
        bare(leave, "decide", json!({}), Position::Transition);
        let second = bare(read, "decide", json!({}), Position::Transition);
        assert_eq!(
            second.value().unwrap()["mark"],
            json!(null),
            "一次调用一个 runtime，调用结束整个扔掉"
        );
    }

    #[test]
    fn 没声明就没有那个函数() {
        // `I-Z`：**不是「调用时被拒绝」，是那个函数不存在。**
        for probe in [
            "fetch",
            "require",
            "process",
            "setTimeout",
            "globalThis.xops",
        ] {
            let source = format!("function decide() {{ return {{ kind: typeof ({probe}) }}; }}");
            let outcome = bare(&source, "decide", json!({}), Position::Output);
            let kind = outcome
                .value()
                .and_then(|value| value["kind"].as_str())
                .unwrap_or("");
            assert_eq!(kind, "undefined", "{probe} 不该存在");
        }
    }

    #[test]
    fn 声明了才有那个函数() {
        let probe = "function run() { return { \
                     config: typeof (globalThis.xops && xops.config), \
                     readTable: typeof (globalThis.xops && xops.readTable), \
                     fetch: typeof (globalThis.xops && xops.fetch) }; }";
        let only_config = granted(
            probe,
            Capabilities {
                own_config: true,
                ..Capabilities::none()
            },
        );
        let value = only_config.value().unwrap().clone();
        assert_eq!(value["config"], json!("function"));
        assert_eq!(value["readTable"], json!("undefined"), "没声明读表");
        assert_eq!(value["fetch"], json!("undefined"), "没声明出网");
    }

    #[test]
    fn 读得到自己那份配置() {
        let outcome = granted(
            "function run() { return xops.config(); }",
            Capabilities {
                own_config: true,
                ..Capabilities::none()
            },
        );
        assert_eq!(outcome.value().unwrap()["token"], json!("s3cr3t"));
    }

    #[test]
    fn 声明之外的表读不到() {
        let capabilities = Capabilities {
            tables: vec![TableId::user("bugs").unwrap()],
            ..Capabilities::none()
        };
        let mine = granted(
            "function run() { return xops.readTable('bugs', 10); }",
            capabilities.clone(),
        );
        assert_eq!(mine.value().unwrap()[0]["表"], json!("bugs"));
        let other = granted(
            "function run() { return xops.readTable('salaries', 10); }",
            capabilities,
        );
        assert!(matches!(other, Outcome::Threw(_)), "声明之外的表读不到");
    }

    #[test]
    fn 出网只到声明过的主机() {
        let capabilities = Capabilities {
            network: vec!["ok.example".into()],
            ..Capabilities::none()
        };
        let allowed = granted(
            "function run() { return xops.fetch('https://ok.example/x'); }",
            capabilities.clone(),
        );
        assert_eq!(allowed.value().unwrap()["status"], json!(200));
        let denied = granted(
            "function run() { return xops.fetch('https://evil.example/x'); }",
            capabilities,
        );
        assert!(matches!(denied, Outcome::Threw(_)));
    }

    #[test]
    fn date还在但别的时钟没有() {
        let outcome = bare(
            "function decide() { return { \
             hasDate: typeof Date === 'function', \
             hasTimer: typeof setInterval }; }",
            "decide",
            json!({}),
            Position::Transition,
        );
        let value = outcome.value().unwrap().clone();
        assert_eq!(value["hasDate"], json!(true), "除 Date 之外没有时钟");
        assert_eq!(value["hasTimer"], json!("undefined"));
    }

    #[test]
    fn 吃内存的插件被限住而不拖垮进程() {
        let outcome = bare(
            "function decide() { const a = []; for (;;) { a.push(new Array(100000).fill(1)); } }",
            "decide",
            json!({}),
            Position::Output,
        );
        assert!(!matches!(outcome, Outcome::Returned(_)), "它不该成功返回");
    }

    #[test]
    fn 入口不在就编译不过() {
        assert!(compile_check("function decide() { return 1; }", "decide").is_ok());
        assert!(compile_check("function other() { return 1; }", "decide").is_err());
        assert!(compile_check("function ( broken", "decide").is_err());
    }
}
