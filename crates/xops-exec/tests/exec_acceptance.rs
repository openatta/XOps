//! RP-07 的验收。
//!
//! ⚠️ **一半的验收在这里做不了，而那不是遗漏。** 本实现是裸跑，不是 D51 的一次性容器，
//! 所以"主动攻击隔离"那一组没有被攻击的对象。它没有被悄悄跳过——
//! [`IsolationLevel::unsatisfied`] 把没兑现的逐条列了出来，下面第一个测试盯着那张表。
//! 这正是 `EXE-029` 要的"**沙箱不静默降级：兑现不了的逐条如实上报，绝不当作已兑现**"。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use xops_core::SystemClock;
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

// ——————————————————————————————— TSK-005 单次 token 上限 ———————————————————————————————

#[test]
fn 超过token预算算预算超支不算别的() {
    // ⚠️ **这条断言以前跑在一条死路上。** 它用的是一个假 attacored，
    // 而那个假 daemon 的回话形状是**照着我们的假设写的**——真实守护进程
    // 既不回 `text` 也不回 `usage`。那条路 `D63` 删了，这条断言搬到
    // `Runtime` 上重新做：**换成桩引擎也该成立**，因为预算不是引擎的性质，
    // 是执行契约的性质。
    //
    // ⚠️ 搬的过程里撞出一件事：**预算以前只在那条死路上比过一次**。
    // 今天真正跑的嵌入式那条路上，`token_budget` 一次都没有被比过——
    // 派工单带着它、`_runs` 那一行同时记着 `tokensUsed` 与 `tokenBudget`，
    // **两个数并排躺着，没有人拿它们比一下**。
    let engine = Arc::new(StubEngine::new());
    engine.behaves(Behaviour::Succeed {
        output: "跑完了".into(),
        tokens: 500,
    });
    let runtime = runtime(engine);
    let mut sheet = worksheet(10_000);
    sheet.limits.token_budget = 100;
    let run = runtime.submit(sheet).unwrap();
    wait_for(&runtime, run, Duration::from_secs(10));

    let outcome = runtime.collect(run).unwrap().unwrap();
    assert_eq!(outcome.failure, Some(FailureKind::TokenBudget));
    assert_eq!(outcome.status, Status::Failed);
    assert_eq!(outcome.tokens_used, 500, "用了多少照实记，不是记成上限");
    assert!(
        !FailureKind::TokenBudget.worth_retrying(),
        "同一份派工单重跑一次会撞上同一个上限"
    );
}

#[test]
fn 没撞上预算的执行照常成功() {
    // 反过来的那一半：**一个一律判超支的实现也"挡住了超支"，但它没用。**
    let engine = Arc::new(StubEngine::new());
    engine.behaves(Behaviour::Succeed {
        output: "跑完了".into(),
        tokens: 99,
    });
    let runtime = runtime(engine);
    let mut sheet = worksheet(10_000);
    sheet.limits.token_budget = 100;
    let run = runtime.submit(sheet).unwrap();
    wait_for(&runtime, run, Duration::from_secs(10));

    let outcome = runtime.collect(run).unwrap().unwrap();
    assert_eq!(outcome.status, Status::Succeeded);
    assert_eq!(outcome.output, "跑完了");
}

#[test]
fn 正好用满就算撞上() {
    // ⚠️ **这条盯的是 `>=` 与 `>` 的那一格之差。** `TSK-005` 说的是"**撞上**这个数"，
    // 而引擎那一侧的 `EngineBudget` 也是 `>=`——它会**停在正好等于**的位置。
    // 这里写成 `>` 的话，一次正好被引擎按预算掐掉的执行会被判成成功，
    // **而那个缝只有在缓存用量为零时才露出来**，不会自己在测试里出现。
    let engine = Arc::new(StubEngine::new());
    engine.behaves(Behaviour::Succeed {
        output: "刚好".into(),
        tokens: 100,
    });
    let runtime = runtime(engine);
    let mut sheet = worksheet(10_000);
    sheet.limits.token_budget = 100;
    let run = runtime.submit(sheet).unwrap();
    wait_for(&runtime, run, Duration::from_secs(10));
    assert_eq!(
        runtime.collect(run).unwrap().unwrap().failure,
        Some(FailureKind::TokenBudget)
    );
}

#[test]
fn 预算这条判定与引擎无关() {
    // ⚠️ 这一条盯的是**它现在长在哪**：`Runtime` 上，不是某个引擎里。
    // 长在引擎里的后果已经见过一次了——**换一条路就没有了，而且不报错**。
    // 桩引擎手上没有任何预算逻辑，照样判得出来。
    let engine = Arc::new(StubEngine::new());
    engine.behaves(Behaviour::Succeed {
        output: String::new(),
        tokens: u64::MAX,
    });
    let runtime = runtime(engine);
    let run = runtime.submit(worksheet(10_000)).unwrap();
    wait_for(&runtime, run, Duration::from_secs(10));
    assert_eq!(
        runtime.collect(run).unwrap().unwrap().failure,
        Some(FailureKind::TokenBudget)
    );
}
