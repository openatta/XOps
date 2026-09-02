//! **把引擎嵌进来:一个进程**(`D61`)。
//!
//! # 为什么不是两个进程了
//!
//! `EXE-014` 原本写的是"XOps 与执行引擎是两个分立的进程"。**`D61` 把它改成一个。**
//! 改的理由与代价都记在那条决策里,这里只说这个文件承担的那一半:
//!
//! ```text
//! 仍然成立   EXE-014 的硬验收 —— 引擎在 `Engine` trait 后面,换成 StubEngine 不改一行
//!            EXE-016 一次执行一个会话、用完即销毁
//!            EXE-019 超时强制终止
//!            EXE-021 提交即返回
//!
//! 拿不到了   EXE-017 的一半 —— 引擎 abort / OOM 时 XOps 跟着一起死,
//!            **没有一个活着的进程去把在途执行归类**。panic 还接得住(catch_unwind),
//!            abort 接不住。这一条在 D61 里是明写的代价,不是疏漏
//! ```
//!
//! # `EXE-014` 那道墙在哪
//!
//! ⚠️ **AttaCore 的类型只允许出现在这个文件里。** 契约（[`crate::contract`]）与
//! 派工单（[`crate::worksheet`]）里一个都不能有——那条是 `EXE-014` 没被 `D61` 改掉的
//! 那一半:**引擎的概念不得泄漏进契约**。有一条枚举全 crate 的测试守着它。
//!
//! # 会话隔离比 daemon 那条路更硬
//!
//! `EXE-016` 特意点过 daemon 那侧的会话池:"**看起来干净比没有池子时更容易骗过人**"。
//! 库模式没有池子——**一次 `run` 造一个 `Agent`,跑完就掉**,
//! 会话隔离是结构上的,不是靠淘汰策略。

use std::sync::Arc;
use std::time::Duration;

use attacore_core::event::AgentEvent;
use attacore_core::interface::model::{Model, Usage};
use attacore_core::interface::scene::AgentScene;
use attacore_core::settings::Settings;
use attacore_runtime::agent::Builder;

use crate::engine::{Cancel, Completed, Engine};
use crate::failure::FailureKind;
use crate::worksheet::Worksheet;

/// 轮多久看一次取消信号。**它决定超时的反应时间上限。**
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// 嵌进来的 AttaCore。
pub struct EmbeddedEngine {
    scene: Arc<dyn AgentScene>,
    model: Arc<dyn Model>,
    settings: Arc<Settings>,
    /// 引擎那一侧是 async 的,而 [`Engine::run`] 是同步的——异步由
    /// [`crate::runtime::Runtime`] 负责（`EXE-021`）。这个 runtime 就是那道桥。
    tokio: tokio::runtime::Runtime,
}

impl std::fmt::Debug for EmbeddedEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedEngine")
            .field("scene", &self.scene.id())
            .finish_non_exhaustive()
    }
}

impl EmbeddedEngine {
    /// XOps 该用的那一份引擎配置。
    ///
    /// 从引擎的默认值出发，**关掉两样对无人值守的一次性执行不成立的东西**：
    ///
    /// ```text
    /// memory_enabled = false
    ///     引擎在一个回合结束之后会**再发一次模型调用**去提取记忆。
    ///     两条都不行：① 那次调用的 token **不在 `TurnOutcome.usage` 里**，
    ///        而 `TSK-005` 的预算是按它记的账——预算会悄悄超；
    ///        ② 它在我们已经返回之后才发生，`EXE-019` 的强制终止管不到它。
    ///     ⚠️ 这一条是实测撞出来的：脚本里排了两个回合，跑一次就少了一个。
    /// ```
    ///
    /// 调用方当然可以自己造一份 `Settings` 传进来——**但把 `memory_enabled` 打开
    /// 就等于接受上面那两条**。
    #[must_use]
    pub fn settings(default_model: &str) -> Settings {
        let mut settings = Settings::defaults_for(default_model);
        settings.memory_enabled = false;
        settings
    }

    /// 这一次执行的配置:**把备好的只读工作区变成 agent 的工作目录**。
    ///
    /// ⚠️ **这条线在 `D61` 把引擎搬进程时断过。** 两进程那版是把工作区当
    /// `session.create` 的 `project_root` 传过去的（见 `attacore.rs`）;
    /// 搬进程之后这一步没跟过来,于是 agent 的工作目录一直是
    /// `Settings.paths.local_data_dir` 的默认值 `"."`——**xopsd 进程自己的 cwd**。
    /// 后果是声明了 `needsRepository` 的技能读的是 XOps 的源码目录,
    /// **而且不报错**:它确实读到了东西,只是读错了地方。
    ///
    /// 引擎那一侧的工作目录是 `local_data_dir` 的**上一级**,所以这里放的是
    /// `<工作区>/.atta`——那个位置由引擎定,不由我们定。
    fn settings_for(&self, worksheet: &Worksheet) -> Arc<Settings> {
        let Some(workspace) = worksheet.capabilities.workspace.as_ref() else {
            return Arc::clone(&self.settings);
        };
        let mut settings = (*self.settings).clone();
        settings.paths.local_data_dir = workspace.join(".atta");
        Arc::new(settings)
    }

    /// # Errors
    /// tokio runtime 建不起来。
    pub fn new(
        scene: Arc<dyn AgentScene>,
        model: Arc<dyn Model>,
        settings: Arc<Settings>,
    ) -> xops_core::Result<Self> {
        let tokio = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                xops_core::Error::internal(format!("建不起 tokio runtime：{error}"))
            })?;
        Ok(Self {
            scene,
            model,
            settings,
            tokio,
        })
    }
}

impl Engine for EmbeddedEngine {
    fn healthy(&self) -> bool {
        // 嵌进来之后"引擎在不在"退化成"这个进程在不在"——它在,不然这行代码不会跑。
        //
        // ⚠️ `EXE-030`（引擎不可用绝不就地跑）因此在这条路上**是空的**:
        // 没有"别处"可连,也就没有"连不上就在本地跑"这个降级。
        // 它对 [`crate::attacore::AttaCoreEngine`] 那条路仍然有意义。
        true
    }

    fn run(
        &self,
        worksheet: &Worksheet,
        cancel: &Cancel,
    ) -> std::result::Result<Completed, (FailureKind, String)> {
        // ⚠️ **先看一眼再动手。** 取消信号在开跑之前就置起时（看门狗先于我们到），
        // 靠下面那个轮询任务是来不及的：一次回合可能在轮询线程被调度之前就跑完了，
        // 于是"已经要求取消"变成了"照样跑完并计费"。
        if cancel.requested() {
            return Err((FailureKind::Timeout, "开跑之前就已经要求取消了".to_owned()));
        }
        let token = tokio_util_cancel();
        // 把我们的取消信号桥到引擎那一侧。**`EXE-019` 全靠它**:
        // 超时之后不把这个信号递进去,就会留下一个还在烧模型额度的会话。
        let watcher = {
            let cancel = cancel.clone();
            let token = token.clone();
            self.tokio.spawn(async move {
                while !cancel.requested() {
                    if token.is_cancelled() {
                        return;
                    }
                    tokio::time::sleep(CANCEL_POLL).await;
                }
                token.cancel();
            })
        };

        let outcome = self.tokio.block_on(self.turn(worksheet, token.clone()));
        watcher.abort();
        outcome
    }
}

impl EmbeddedEngine {
    /// 一次执行 = **一个新建的 `Agent`**,跑完就掉（`EXE-016`）。
    async fn turn(
        &self,
        worksheet: &Worksheet,
        token: tokio_util::sync::CancellationToken,
    ) -> std::result::Result<Completed, (FailureKind, String)> {
        let run = worksheet.run.to_string();
        let (mut agent, mut events, _input) = Builder::new()
            .scene(Arc::clone(&self.scene))
            .model(Arc::clone(&self.model))
            .settings(self.settings_for(worksheet))
            .session_id(run.clone())
            .build()
            .map_err(|error| (FailureKind::Engine, format!("会话建不起来：{error}")))?;

        // 事件流是**产出与过程记录的唯一来源**（`EXE-022`）:
        // 正文从 TextDelta 攒出来,过程记录把每一条都记下。
        //
        // ⚠️ **边跑边收,不是跑完再收。** 早先这里是"另起一个任务 `while let Some(..)
        // = events.recv()`,等通道关掉就收工"——那个写法**挂死了**:通道要等 `Agent`
        // 连同它内部持有的每一份 sender 全部掉光才会关,而那不在我们手上。
        // **不要拿「通道会关」当收尾条件。**
        let mut collected = Collected::default();
        let outcome = {
            let turn = agent.run_turn(worksheet.prompt(), run, token.clone());
            let mut turn = std::pin::pin!(turn);
            loop {
                tokio::select! {
                    // 先看回合有没有结束 —— 结束了就不再等事件。
                    biased;
                    done = &mut turn => break done,
                    event = events.recv() => match event {
                        Some(event) => collected.take(&event),
                        // 通道先关了也不算错:回合的结果才是结论。
                        None => break (&mut turn).await,
                    },
                }
            }
        };
        // 回合结束之后,把已经排在通道里的那些收干净。**只收现成的,不等新的。**
        while let Ok(event) = events.try_recv() {
            collected.take(&event);
        }
        drop(agent);

        match outcome {
            Ok(done) => Ok(Completed {
                output: collected.output,
                trace: collected.trace,
                tokens_used: tokens(&done.usage),
            }),
            Err(error) => {
                let kind = if token.is_cancelled() {
                    FailureKind::Timeout
                } else {
                    classify(&error)
                };
                Err((kind, format!("{error}\n{}", collected.trace)))
            }
        }
    }
}

/// 一路收下来的产出与过程记录。
#[derive(Debug, Default)]
struct Collected {
    output: String,
    trace: String,
}

impl Collected {
    fn take(&mut self, event: &AgentEvent) {
        if let AgentEvent::TextDelta { text, .. } = event {
            self.output.push_str(text);
        }
        self.trace.push_str(&describe(event));
        self.trace.push('\n');
    }
}

/// 引擎那一侧的失败归到我们这一侧的哪一类。
///
/// ⚠️ **归类是我们的事,不是引擎的。** 让引擎的错误枚举直接变成我们的失败分类，
/// 就是 `EXE-014` 那句"引擎的概念不得泄漏进契约"最容易破的地方。
fn classify(error: &attacore_runtime::turn::TurnError) -> FailureKind {
    match error {
        attacore_runtime::turn::TurnError::Model(_) => FailureKind::ModelService,
        attacore_runtime::turn::TurnError::Shutdown => FailureKind::Timeout,
        attacore_runtime::turn::TurnError::Internal(_) => FailureKind::Engine,
    }
}

/// 一条事件在过程记录里长什么样。
///
/// ⚠️ **只记形状与计数,不记载荷**:事件里带着模型的完整回话与工具参数,
/// 而过程记录会落进 `_runs.trace`——`I-F` 说过程记录中不出现任何凭据。
fn describe(event: &AgentEvent) -> String {
    match event {
        AgentEvent::TextDelta { text, .. } => format!("text +{}", text.len()),
        AgentEvent::TurnComplete {
            stop_reason,
            api_calls,
            tool_calls,
            ..
        } => format!("turn-complete stop={stop_reason} api={api_calls} tools={tool_calls}"),
        AgentEvent::Error { code, message, .. } => format!("error {code} {message}"),
        other => variant(other),
    }
}

/// 事件的变体名。`serde` 的 tag 就是它,不用自己维护一张表。
///
/// ⚠️ **tag 的键名是 `kind`,不是 `type`。** 早先这里读的是 `type`,永远读不到,
/// 于是每一条事件在过程记录里都写成字面量 `"event"`——**73 行 `event`**。
/// `EXE-022` 要的是"出了事能读的东西",而那样的记录一个字也没说。
/// 一个测试盯着这件事:上游改了 tag 名,这里要当场红。
const EVENT_TAG: &str = "kind";

fn variant(event: &AgentEvent) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|value| {
            value
                .get(EVENT_TAG)
                .and_then(|tag| tag.as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| "event".to_owned())
}

fn tokens(usage: &Usage) -> u64 {
    u64::from(usage.input_tokens) + u64::from(usage.output_tokens)
}

fn tokio_util_cancel() -> tokio_util::sync::CancellationToken {
    tokio_util::sync::CancellationToken::new()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn engine_for_test() -> EmbeddedEngine {
        let mock: Arc<dyn attacore_model::client::AnthropicClient> =
            Arc::new(attacore_model::mock::MockAnthropicClient::new());
        EmbeddedEngine::new(
            Arc::new(attacore_scene::scene::chat::ChatScene),
            Arc::new(attacore_model::adapter::AnthropicModel::new(mock)),
            Arc::new(EmbeddedEngine::settings("m")),
        )
        .unwrap()
    }

    fn sheet_for_test() -> Worksheet {
        Worksheet {
            run: crate::worksheet::RunId::generate(),
            instruction: "做点什么".into(),
            skill: "s".into(),
            skill_version: "1".into(),
            inputs: String::new(),
            revision: None,
            capabilities: crate::worksheet::Capabilities::default(),
            limits: crate::worksheet::Limits::default(),
        }
    }

    #[test]
    fn 事件在过程记录里说得出自己是什么() {
        // ⚠️ 这条盯的是**上游改了 tag 名**。早先读的是 `type`，而上游是 `kind`——
        // 读不到就回退成字面量 `"event"`，于是 `_runs.trace` 里是七十几行 `event`。
        // 那种记录不报错、不为空、**什么也没说**，只有拿真模型跑一遍才看得出来。
        let event = AgentEvent::ThinkingDelta {
            text: "想".into(),
            turn_id: "t".into(),
        };
        let described = describe(&event);
        assert_ne!(described, "event", "回退到字面量了 —— tag 键名对不上");
        assert_eq!(described, "thinking_delta");
    }

    #[test]
    fn 备好的工作区真的成了agent的工作目录() {
        // ⚠️ 这条盯的是 `D61` 搬进程时断掉的那根线。断掉的表现**不是报错**:
        // agent 照样跑，只是在 xopsd 自己的 cwd 里跑——读到的是 XOps 的源码。
        // 「读错了地方」和「读不到」是两件事，后者会喊，前者不会。
        let engine = engine_for_test();
        let mut sheet = sheet_for_test();
        sheet.capabilities.workspace = Some(PathBuf::from("/tmp/ws-1"));

        let settings = engine.settings_for(&sheet);
        assert_eq!(
            settings.paths.project_root(),
            PathBuf::from("/tmp/ws-1"),
            "工作目录该是那份工作区"
        );

        // 没有工作区的技能（不读代码仓）照旧 —— **不许顺手给它一个目录**。
        sheet.capabilities.workspace = None;
        assert_eq!(
            engine.settings_for(&sheet).paths.local_data_dir,
            engine.settings.paths.local_data_dir,
            "不声明代码仓的执行不该被塞一个工作目录（I-I）"
        );
    }

    #[test]
    fn 正文与结束事件另有说法() {
        assert_eq!(
            describe(&AgentEvent::TextDelta {
                text: "四个字".into(),
                turn_id: "t".into(),
            }),
            "text +9",
            "正文只记长度不记内容（I-F）"
        );
    }
}
