//! 嵌进来的引擎跑一次**真的回合**——离线。
//!
//! 这条测试是把引擎嵌进来最实在的一笔收获:模型客户端换成 `MockAnthropicClient`,
//! **agent 循环、工具注册、事件流、用量统计全都是真的**,不需要 API key、不需要网络、
//! 不需要一个跑着的 daemon。
//!
//! ⚠️ 对照 `XFG-024` 那条欠账:走 socket 的那条路要真的起一个 `attacored` 才验得了,
//! 所以它至今没被跑过。**这一条每次 `cargo test` 都跑。**

use std::sync::Arc;

use attacore_core::interface::model::Model;
use attacore_core::interface::scene::AgentScene;
use attacore_core::message::StopReason;
use attacore_model::client::AnthropicClient;
use attacore_model::mock::MockAnthropicClient;
use attacore_model::stream::{
    BlockDelta, ContentBlockStart, MessageDeltaPayload, MessageStartPayload, StreamEvent, Usage,
};
use xops_exec::worksheet::{Capabilities, Limits, RunId, Worksheet};
use xops_exec::{Cancel, EmbeddedEngine, Engine, FailureKind};

/// 一个只说一句话就收工的回合。
fn one_line_turn(text: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::MessageStart {
            message: MessageStartPayload {
                id: "msg_1".into(),
                model: "claude-sonnet-4-6".into(),
                role: "assistant".into(),
                usage: Usage {
                    input_tokens: 11,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
                stop_reason: None,
            },
        },
        StreamEvent::ContentBlockStart {
            index: 0,
            content_block: ContentBlockStart::Text {
                text: String::new(),
            },
        },
        StreamEvent::ContentBlockDelta {
            index: 0,
            delta: BlockDelta::TextDelta { text: text.into() },
        },
        StreamEvent::ContentBlockStop { index: 0 },
        StreamEvent::MessageDelta {
            delta: MessageDeltaPayload {
                stop_reason: Some(StopReason::EndTurn),
                stop_sequence: None,
            },
            usage: Some(Usage {
                input_tokens: 11,
                output_tokens: 7,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            }),
        },
        StreamEvent::MessageStop,
    ]
}

fn engine(script: Vec<Vec<StreamEvent>>) -> (EmbeddedEngine, Arc<MockAnthropicClient>) {
    build_engine(script, None)
}

fn build_engine(
    script: Vec<Vec<StreamEvent>>,
    failing: Option<attacore_model::error::AnthropicError>,
) -> (EmbeddedEngine, Arc<MockAnthropicClient>) {
    let mock = Arc::new(MockAnthropicClient::new());
    for turn in script {
        mock.push_turn(turn);
    }
    if let Some(error) = failing {
        mock.push_turn_with_errors(vec![Err(error)]);
    }
    let client: Arc<dyn AnthropicClient> = Arc::clone(&mock) as Arc<dyn AnthropicClient>;
    let model: Arc<dyn Model> = Arc::new(attacore_model::adapter::AnthropicModel::new(client));
    let scene: Arc<dyn AgentScene> = Arc::new(attacore_scene::scene::chat::ChatScene);
    let settings = Arc::new(EmbeddedEngine::settings("claude-sonnet-4-6"));
    (EmbeddedEngine::new(scene, model, settings).unwrap(), mock)
}

fn worksheet(instruction: &str) -> Worksheet {
    Worksheet {
        run: RunId::generate(),
        instruction: instruction.to_owned(),
        skill: "打个招呼".into(),
        skill_version: "1".into(),
        inputs: String::new(),
        revision: None,
        capabilities: Capabilities::default(),
        limits: Limits {
            token_budget: 10_000,
            timeout_millis: 30_000,
            ..Limits::default()
        },
    }
}

#[test]
fn 跑得通一个真回合而且产出与用量都对() {
    let (engine, mock) = engine(vec![one_line_turn("你好，我看过了。")]);
    let done = engine
        .run(&worksheet("说一句话"), &Cancel::new())
        .expect("这一回合该成");

    assert_eq!(done.output, "你好，我看过了。", "正文从 TextDelta 攒出来");
    assert_eq!(done.tokens_used, 18, "11 进 + 7 出");
    assert!(!done.trace.is_empty(), "过程记录不能是空的（EXE-022）");
    assert_eq!(mock.calls(), 1, "只调了模型一次");
}

#[test]
fn 派工单的输入与修订都喂进去了() {
    let (engine, mock) = engine(vec![one_line_turn("收到")]);
    let mut sheet = worksheet("按输入干活");
    sheet.inputs = "这是查好的那批数据".into();
    sheet.revision = Some("abc123".into());
    engine.run(&sheet, &Cancel::new()).unwrap();

    let request = mock.nth_request(0).expect("模型该收到一次请求");
    let sent = serde_json::to_string(&request).unwrap_or_default();
    assert!(sent.contains("按输入干活"), "指令");
    assert!(
        sent.contains("这是查好的那批数据"),
        "EXE-013 那条出路的输入"
    );
    assert!(sent.contains("abc123"), "读的哪个代码修订");
}

#[test]
fn 过程记录里不出现模型回话的原文() {
    // ⚠️ `I-F`：**过程记录中不出现任何凭据**。做法是过程记录只记形状与计数，
    // 不记事件载荷——载荷里带着模型的完整回话与工具参数，而它会落进 `_runs.trace`。
    let secret = "ghp_这是一个不该进过程记录的东西";
    let (engine, _) = engine(vec![one_line_turn(secret)]);
    let done = engine.run(&worksheet("说点什么"), &Cancel::new()).unwrap();

    assert_eq!(done.output, secret, "产出里有——那是它该在的地方");
    assert!(
        !done.trace.contains(secret),
        "过程记录里不该有：{}",
        done.trace
    );
    assert!(done.trace.contains("text +"), "记的是形状与长度");
}

#[test]
fn 两次执行之间不共享会话() {
    // `EXE-016`：一次执行一个会话、用完即销毁。
    // ⚠️ 库模式**没有会话池**——这一条是结构上的，不是靠淘汰策略。
    // `EXE-016` 特意点过 daemon 那侧的池子"看起来干净比没有池子时更容易骗过人"。
    let (engine, mock) = engine(vec![one_line_turn("第一次"), one_line_turn("第二次")]);
    let first = engine.run(&worksheet("第一次"), &Cancel::new()).unwrap();
    let second = engine.run(&worksheet("第二次"), &Cancel::new()).unwrap();
    assert_eq!(first.output, "第一次");
    assert_eq!(second.output, "第二次");

    // 第二次的请求里**不该带着第一次的对话**。
    let second_request = serde_json::to_string(&mock.nth_request(1).unwrap()).unwrap_or_default();
    assert!(
        !second_request.contains("第一次"),
        "第二次读到了第一次留下的东西：{second_request}"
    );
}

#[test]
fn 模型那一侧出错归到可重跑的那一类() {
    // 脚本里一个 turn 都没有 → mock 会报错。归类是**我们的事**，不是引擎的
    // ——让引擎的错误枚举直接变成我们的失败分类，正是 `EXE-014` 最容易破的地方。
    let (engine, _) = build_engine(
        vec![],
        Some(attacore_model::error::AnthropicError::Server {
            status: 500,
            body: "模型那边炸了".into(),
        }),
    );
    let (kind, detail) = engine
        .run(&worksheet("会失败"), &Cancel::new())
        .expect_err("模型报错这一回合该失败");
    assert!(
        matches!(kind, FailureKind::ModelService | FailureKind::Engine),
        "实际是 {kind:?}：{detail}"
    );
    assert!(kind.worth_retrying(), "这两类都值得重跑");
}

#[test]
fn 一次执行只发一次模型调用() {
    // ⚠️ 这一条盯的是**回合后的后台调用**：引擎默认会在回合结束之后再发一次
    // 模型调用去提取记忆，而那次的 token **不在 `TurnOutcome.usage` 里**——
    // `TSK-005` 的预算按它记账，所以那一次会让预算悄悄超。
    // `EmbeddedEngine::settings` 把它关掉了，这条测试守着那个开关。
    //
    // 这不是推理出来的：脚本里排了两个回合，跑一次就少了一个。
    let (engine, mock) = engine(vec![one_line_turn("一"), one_line_turn("二")]);
    engine.run(&worksheet("跑一次"), &Cancel::new()).unwrap();
    assert_eq!(mock.calls(), 1, "一次执行只该发一次模型调用");
    assert_eq!(mock.turns_remaining(), 1, "第二个脚本不该被谁偷走");
}

#[test]
fn 已经置起的取消信号让这次执行归为超时() {
    // `EXE-019`：超时强制终止。取消信号要真的递到引擎那一侧，
    // **不然会留下一个还在烧模型额度的会话。**
    let (engine, _) = engine(vec![one_line_turn("不该跑完")]);
    let cancel = Cancel::new();
    cancel.request();
    let outcome = engine.run(&worksheet("会被取消"), &cancel);
    match outcome {
        Err((FailureKind::Timeout, _)) => {}
        Err((kind, detail)) => panic!("该归为超时，实际是 {kind:?}：{detail}"),
        Ok(done) => panic!("取消之后不该跑完：{}", done.output),
    }
}

/// **AttaCore 的类型只允许出现在 `embedded.rs` 里。**
///
/// `EXE-014` 被 `D61` 改掉的只有"两个分立的进程"那半句;
/// **"引擎的概念不得泄漏进契约"那半句一个字没改。**
/// 最容易破坏它的正是图省事让引擎的数据结构直接流进契约。
#[test]
fn 引擎的类型不出现在契约里() {
    let guarded = [
        ("contract.rs", include_str!("../src/contract.rs")),
        ("worksheet.rs", include_str!("../src/worksheet.rs")),
        ("engine.rs", include_str!("../src/engine.rs")),
        ("failure.rs", include_str!("../src/failure.rs")),
        ("runtime.rs", include_str!("../src/runtime.rs")),
        ("provider.rs", include_str!("../src/provider.rs")),
        ("stub.rs", include_str!("../src/stub.rs")),
    ];
    for (name, source) in guarded {
        let body = source.split("#[cfg(test)]").next().unwrap();
        for leak in [
            "attacore_",
            "AgentEvent",
            "TurnOutcome",
            "TurnError",
            "AgentScene",
        ] {
            assert!(
                !body.contains(leak),
                "{name} 里出现了 {leak}——引擎的概念泄漏进契约了（EXE-014）"
            );
        }
    }
}
