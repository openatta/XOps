//! RP-07 的验收。
//!
//! ⚠️ **一半的验收在这里做不了，而那不是遗漏。** 本实现是裸跑，不是 D51 的一次性容器，
//! 所以"主动攻击隔离"那一组没有被攻击的对象。它没有被悄悄跳过——
//! [`IsolationLevel::unsatisfied`] 把没兑现的逐条列了出来，下面第一个测试盯着那张表。
//! 这正是 `EXE-029` 要的"**沙箱不静默降级：兑现不了的逐条如实上报，绝不当作已兑现**"。

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use xops_core::SystemClock;
use xops_exec::attacore::AttaCoreEngine;
use xops_exec::worksheet::{Capabilities, Limits, RunId};
use xops_exec::{
    Behaviour, Cancel, Completed, Engine, ExecContract, FailureKind, IsolationLevel, Runtime,
    Status, StubEngine, Worksheet,
};

fn worksheet(timeout_millis: u64) -> Worksheet {
    Worksheet {
        run: RunId::generate(),
        instruction: "看看有没有崩的地方".into(),
        skill: "查缺陷".into(),
        skill_version: "v1".into(),
        inputs: String::new(),
        revision: None,
        capabilities: Capabilities::default(),
        rows_to: None,
        limits: Limits {
            timeout_millis,
            ..Limits::default()
        },
    }
}

fn runtime(engine: Arc<dyn Engine>) -> Runtime {
    Runtime::new(engine, Arc::new(SystemClock), IsolationLevel::Bare)
}

fn wait_for(runtime: &Runtime, run: RunId, limit: Duration) -> Status {
    let deadline = Instant::now() + limit;
    loop {
        let status = runtime.status(run).expect("这次执行该在登记表里");
        if status.finished() {
            return status;
        }
        assert!(Instant::now() < deadline, "EXE-017：不得无限期挂起");
        thread::sleep(Duration::from_millis(10));
    }
}

// ——————————————————————————————— 沙箱不静默降级 ———————————————————————————————

#[test]
fn 裸跑没兑现的那几条是逐条上报的() {
    let missing = IsolationLevel::Bare.unsatisfied();
    assert!(
        !missing.is_empty(),
        "裸跑当然有没兑现的。这张表是空的，说明它在骗人"
    );
    // 最要紧的四条：容器没有、网络没强制、资源没限、攻击测试无从谈起。
    for id in ["EXE-002", "EXE-007", "EXE-008", "EXE-028"] {
        assert!(
            missing.iter().any(|(missing, _)| *missing == id),
            "少报了 {id}"
        );
    }
    // ⚠️ **`EXE-029` 不在这张表里，是对的**：`D62` 把它关闭了——
    // 不做容器后端，隔离归部署侧。**"决定不做"和"还没做"要分开**，
    // 混在一张表里会让人一直等着它缩短，而它不会。
    assert!(
        !missing.iter().any(|(missing, _)| *missing == "EXE-029"),
        "EXE-029 是决定不做（D62），不该报成没兑现"
    );
    // 每一条都要说清是什么没兑现，不能只有一个编号。
    assert!(missing.iter().all(|(_, why)| why.len() > 8));
}

#[test]
fn 不靠容器也成立的那几条没有被顺手划掉() {
    let held = IsolationLevel::Bare.still_held();
    for id in [
        "EXE-004", "EXE-010", "EXE-013", "EXE-014", "EXE-015", "EXE-030",
    ] {
        assert!(
            held.iter().any(|(held, _)| *held == id),
            "{id} 本来就不靠容器，不该被划掉"
        );
    }
    // 两张表不能有交集 —— 一条要么兑现了要么没兑现。
    for (missing, _) in IsolationLevel::Bare.unsatisfied() {
        assert!(
            !held.iter().any(|(held, _)| held == missing),
            "{missing} 同时出现在两张表上"
        );
    }
}

// ——————————————————————————————— EXE-014 换引擎硬验收 ———————————————————————————————

/// 第二个引擎实现。**它与 `StubEngine` 之间没有任何共同代码**——
/// 换它进去，上面的一切不改一行，这就是 `EXE-014` 那句话的证明。
struct EchoEngine {
    calls: AtomicUsize,
}

impl Engine for EchoEngine {
    fn healthy(&self) -> bool {
        true
    }

    fn run(
        &self,
        worksheet: &Worksheet,
        _cancel: &Cancel,
    ) -> Result<Completed, (FailureKind, String)> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Completed {
            output: worksheet.instruction.clone(),
            trace: "echo".into(),
            tokens_used: 1,
            rows: Vec::new(),
        })
    }
}

#[test]
fn 换一个引擎进去上面的一切不改一行() {
    // 同一段调用代码，跑两个毫不相干的引擎。
    for engine in [
        Arc::new(StubEngine::new()) as Arc<dyn Engine>,
        Arc::new(EchoEngine {
            calls: AtomicUsize::new(0),
        }),
    ] {
        let runtime = runtime(engine);
        let sheet = worksheet(5_000);
        let run = runtime.submit(sheet).unwrap();
        assert_eq!(
            wait_for(&runtime, run, Duration::from_secs(5)),
            Status::Succeeded
        );
        let outcome = runtime.collect(run).unwrap().unwrap();
        assert!(!outcome.output.is_empty());
        assert!(outcome.finished_at.is_some());
    }
}

// ——————————————————————————————— 异步、超时、崩溃 ———————————————————————————————

#[test]
fn 提交立即返回不阻塞() {
    let stub = Arc::new(StubEngine::new());
    stub.behaves(Behaviour::Hang);
    let runtime = runtime(stub);

    let started = Instant::now();
    let run = runtime.submit(worksheet(60_000)).unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "EXE-021：提交后立即返回"
    );
    assert_eq!(runtime.status(run).unwrap(), Status::Running);
    runtime.cancel(run).unwrap();
    wait_for(&runtime, run, Duration::from_secs(5));
}

#[test]
fn 超时被强制终止且不留孤儿() {
    let stub = Arc::new(StubEngine::new());
    stub.behaves(Behaviour::Hang);
    let runtime = runtime(Arc::clone(&stub) as Arc<dyn Engine>);

    let run = runtime.submit(worksheet(100)).unwrap();
    let status = wait_for(&runtime, run, Duration::from_secs(10));
    assert!(status.finished(), "EXE-019");
    let outcome = runtime.collect(run).unwrap().unwrap();
    assert_eq!(outcome.failure, Some(FailureKind::Timeout));
    // 引擎那一侧确实收到了取消 —— 孤儿会话不会继续烧额度。
    assert_eq!(stub.seen().len(), 1);
}

#[test]
fn 引擎崩了不会让执行挂在running上() {
    let stub = Arc::new(StubEngine::new());
    stub.behaves(Behaviour::Panic);
    let runtime = runtime(stub);

    let run = runtime.submit(worksheet(30_000)).unwrap();
    let status = wait_for(&runtime, run, Duration::from_secs(5));
    assert_eq!(
        status,
        Status::Failed,
        "EXE-017：有限时间内归入明确的失败分类"
    );
    let outcome = runtime.collect(run).unwrap().unwrap();
    assert_eq!(outcome.failure, Some(FailureKind::Engine));
    assert!(outcome.failure.unwrap().worth_retrying(), "可重跑");
}

#[test]
fn 三个在途执行都能在引擎垮掉时收摊() {
    let stub = Arc::new(StubEngine::new());
    stub.behaves(Behaviour::Hang);
    let runtime = runtime(Arc::clone(&stub) as Arc<dyn Engine>);

    let runs: Vec<RunId> = (0..3)
        .map(|_| runtime.submit(worksheet(150)).unwrap())
        .collect();
    assert_eq!(runtime.running().unwrap().len(), 3);
    for run in runs {
        let status = wait_for(&runtime, run, Duration::from_secs(10));
        assert!(status.finished(), "三个都要收摊，一个都不许挂着");
        assert_eq!(
            runtime.collect(run).unwrap().unwrap().failure,
            Some(FailureKind::Timeout)
        );
    }
}

#[test]
fn 引擎不可用时绝不就地跑() {
    let stub = Arc::new(StubEngine::new());
    stub.set_healthy(false);
    let runtime = runtime(Arc::clone(&stub) as Arc<dyn Engine>);

    let run = runtime.submit(worksheet(5_000)).unwrap();
    assert_eq!(
        wait_for(&runtime, run, Duration::from_secs(5)),
        Status::Failed
    );
    let outcome = runtime.collect(run).unwrap().unwrap();
    assert_eq!(outcome.failure, Some(FailureKind::Engine), "EXE-030");
    assert!(
        stub.seen().is_empty(),
        "引擎不可用时它一次都不该被调用 —— 就地跑会让隔离与凭据边界一起失效"
    );
}

#[test]
fn 取消已经结束的执行是无操作() {
    let runtime = runtime(Arc::new(StubEngine::new()));
    let run = runtime.submit(worksheet(5_000)).unwrap();
    wait_for(&runtime, run, Duration::from_secs(5));
    assert!(runtime.cancel(run).is_ok(), "不是错误");
    assert!(
        runtime.cancel(RunId::generate()).is_err(),
        "不存在的才是错误"
    );
}

#[test]
fn 八类失败每一类都到得了() {
    for kind in FailureKind::all() {
        if kind == FailureKind::Timeout {
            continue; // 上面单独验过。
        }
        let stub = Arc::new(StubEngine::new());
        stub.behaves(Behaviour::Fail(kind));
        let runtime = runtime(stub);
        let run = runtime.submit(worksheet(5_000)).unwrap();
        wait_for(&runtime, run, Duration::from_secs(5));
        assert_eq!(
            runtime.collect(run).unwrap().unwrap().failure,
            Some(kind),
            "{kind}"
        );
    }
}

#[test]
fn 派工单不合法时提交就失败() {
    let runtime = runtime(Arc::new(StubEngine::new()));
    let mut sheet = worksheet(5_000);
    sheet.instruction = String::new();
    assert!(
        runtime.submit(sheet).is_err(),
        "这是一次失败的提交，不是一次失败的执行"
    );
}

// ——————————————————————————————— attacored 客户端 ———————————————————————————————

/// 一个只会说 NDJSON 的假 attacored。**用来实测会话隔离**（`EXE-016`）。
fn fake_attacored(path: &std::path::Path, sessions: Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = UnixListener::bind(path).expect("绑不上");
    thread::spawn(move || {
        let mut counter = 0;
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let sessions = Arc::clone(&sessions);
            counter += 1;
            let index = counter;
            thread::spawn(move || serve(stream, index, &sessions));
        }
    });
}

fn serve(stream: UnixStream, index: usize, sessions: &Arc<std::sync::Mutex<Vec<String>>>) {
    let mut writer = stream.try_clone().expect("复制不了");
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { return };
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = frame.get("id").and_then(Value::as_u64).unwrap_or_default();
        let response = match frame.get("method").and_then(Value::as_str) {
            Some("daemon.ping") => json!({"jsonrpc": "2.0", "id": id, "result": {"ok": true}}),
            Some("session.create") => {
                // **每次都是一个新会话** —— 这正是 EXE-016 要被实测的那件事。
                let session = format!("session-{index}");
                sessions.lock().unwrap().push(session.clone());
                json!({"jsonrpc": "2.0", "id": id, "result": {"session_id": session}})
            }
            Some("session.run_turn") => json!({
                "jsonrpc": "2.0", "id": id,
                "result": {"text": "跑完了", "usage": {"total_tokens": 42}},
            }),
            _ => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        };
        if writeln!(writer, "{response}").is_err() {
            return;
        }
    }
}

#[test]
fn 两次执行拿到的是两个会话() {
    let dir = std::env::temp_dir().join(format!("xops-exec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("attacored.sock");
    let _ = std::fs::remove_file(&socket);
    let sessions = Arc::new(std::sync::Mutex::new(Vec::new()));
    fake_attacored(&socket, Arc::clone(&sessions));
    thread::sleep(Duration::from_millis(50));

    let engine = Arc::new(AttaCoreEngine::at(&socket));
    assert!(engine.healthy(), "假 daemon 该答得上 ping");
    let runtime = runtime(engine);

    let mut traces = Vec::new();
    for _ in 0..2 {
        let run = runtime.submit(worksheet(10_000)).unwrap();
        assert_eq!(
            wait_for(&runtime, run, Duration::from_secs(10)),
            Status::Succeeded
        );
        let outcome = runtime.collect(run).unwrap().unwrap();
        assert_eq!(outcome.output, "跑完了");
        assert_eq!(outcome.tokens_used, 42);
        traces.push(outcome.trace);
    }

    let recorded = sessions.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2, "一次执行一个会话");
    assert_ne!(recorded[0], recorded[1], "EXE-016：两次执行不共用会话");
    // 会话 id 进了 trace —— 这条因此是**实测得到的**，不是看代码看出来的。
    assert!(traces[0].contains(&recorded[0]));
    assert!(traces[1].contains(&recorded[1]));
    assert!(
        !traces[1].contains(&recorded[0]),
        "第二次读不到第一次的痕迹"
    );

    let _ = std::fs::remove_file(&socket);
}

#[test]
fn 超过token预算算预算超支不算别的() {
    let dir = std::env::temp_dir().join(format!("xops-exec-budget-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("attacored.sock");
    let _ = std::fs::remove_file(&socket);
    fake_attacored(&socket, Arc::new(std::sync::Mutex::new(Vec::new())));
    thread::sleep(Duration::from_millis(50));

    let runtime = runtime(Arc::new(AttaCoreEngine::at(&socket)));
    let mut sheet = worksheet(10_000);
    sheet.limits.token_budget = 10; // 假 daemon 回的是 42。
    let run = runtime.submit(sheet).unwrap();
    wait_for(&runtime, run, Duration::from_secs(10));
    assert_eq!(
        runtime.collect(run).unwrap().unwrap().failure,
        Some(FailureKind::TokenBudget)
    );

    let _ = std::fs::remove_file(&socket);
}
