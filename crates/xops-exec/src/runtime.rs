//! 执行运行时：把同步的引擎变成异步的契约。
//!
//! 它守着四条，每条都有一个具体的失败形态要挡：
//!
//! ```text
//! EXE-021  提交即返回        —— 不阻塞调用方
//! EXE-030  引擎不可用        —— 如实归入引擎错误类，**绝不就地跑**
//! EXE-019  超时强制终止      —— 不留孤儿会话继续消耗模型额度
//! EXE-017  引擎崩了 / 卡死   —— 在有限时间内归入明确的失败分类，**不得无限期挂起**
//! ```

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use xops_core::{Clock, Error, Result};

use crate::contract::{ExecContract, Outcome, Status};
use crate::engine::{Cancel, Engine};
use crate::failure::FailureKind;
use crate::provider::IsolationLevel;
use crate::worksheet::{RunId, Worksheet};

/// 超时之后再宽限多久，然后无论如何都收摊。
///
/// `EXE-017` 的"有限时间"就是它：引擎不认取消信号也好、线程卡死也好，
/// **到这里一律归入超时**，不会有一次执行永远停在 running。
pub const GRACE_MILLIS: u64 = 2_000;

struct Slot {
    status: Status,
    cancel: Cancel,
    /// 跑完之前是 `None`。**它一旦有值就不再被覆盖**——看门狗与工作线程可能同时到终点。
    outcome: Option<Outcome>,
}

/// 运行时。
pub struct Runtime {
    engine: Arc<dyn Engine>,
    clock: Arc<dyn Clock>,
    isolation: IsolationLevel,
    runs: Arc<Mutex<HashMap<RunId, Slot>>>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("isolation", &self.isolation)
            .finish_non_exhaustive()
    }
}

impl Runtime {
    #[must_use]
    pub fn new(engine: Arc<dyn Engine>, clock: Arc<dyn Clock>, isolation: IsolationLevel) -> Self {
        Self {
            engine,
            clock,
            isolation,
            runs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 这个运行时隔离到什么程度。**调用方问得出来**——
    /// 见 [`IsolationLevel::unsatisfied`]。
    #[must_use]
    pub fn isolation(&self) -> IsolationLevel {
        self.isolation
    }

    /// 还在跑的那些。
    ///
    /// # Errors
    /// 登记表的锁中毒。
    pub fn running(&self) -> Result<Vec<RunId>> {
        Ok(self
            .locked()?
            .iter()
            .filter(|(_, slot)| slot.status == Status::Running)
            .map(|(run, _)| *run)
            .collect())
    }

    fn locked(&self) -> Result<std::sync::MutexGuard<'_, HashMap<RunId, Slot>>> {
        self.runs
            .lock()
            .map_err(|_| Error::internal("执行登记表的锁中毒了"))
    }

    fn finish(runs: &Arc<Mutex<HashMap<RunId, Slot>>>, run: RunId, outcome: Outcome) {
        let mut guard = runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(slot) = guard.get_mut(&run) {
            // 已经有结果就不再覆盖 —— 看门狗与工作线程可能同时到达终点。
            if slot.outcome.is_none() {
                slot.status = outcome.status;
                slot.outcome = Some(outcome);
            }
        }
    }
}

impl ExecContract for Runtime {
    fn submit(&self, worksheet: Worksheet) -> Result<RunId> {
        worksheet.check()?;
        let run = worksheet.run;
        let cancel = Cancel::new();
        let started_at = self.clock.now();
        self.locked()?.insert(
            run,
            Slot {
                status: Status::Running,
                cancel: cancel.clone(),
                outcome: None,
            },
        );

        let engine = Arc::clone(&self.engine);
        let clock = Arc::clone(&self.clock);
        let runs = Arc::clone(&self.runs);
        let timeout = worksheet.limits.timeout_millis;
        // 派工单在下面被搬进线程，先把要比的那个数留出来（`TSK-005`）。
        let budget = worksheet.limits.token_budget;

        // 看门狗：到点先请求取消，再宽限一段，然后**无论如何**收摊（EXE-017 / EXE-019）。
        {
            let cancel = cancel.clone();
            let runs = Arc::clone(&runs);
            let clock = Arc::clone(&clock);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(timeout));
                if runs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&run)
                    .is_some_and(|slot| slot.outcome.is_none())
                {
                    cancel.request();
                    thread::sleep(Duration::from_millis(GRACE_MILLIS));
                    Runtime::finish(
                        &runs,
                        run,
                        Outcome::failed(
                            run,
                            FailureKind::Timeout,
                            "超时：先请求取消，宽限之后仍未收尾",
                            started_at,
                            clock.now(),
                        ),
                    );
                }
            });
        }

        thread::spawn(move || {
            // EXE-030：引擎不可用就如实归类，**绝不在这里就地跑一遍**。
            if !engine.healthy() {
                Runtime::finish(
                    &runs,
                    run,
                    Outcome::failed(
                        run,
                        FailureKind::Engine,
                        "引擎不可用。**没有就地跑**——那会让隔离与凭据边界一起失效",
                        started_at,
                        clock.now(),
                    ),
                );
                return;
            }

            // 引擎自己崩了也不能让这次执行永远停在 running（EXE-017）。
            let result =
                std::panic::catch_unwind(AssertUnwindSafe(|| engine.run(&worksheet, &cancel)));
            let finished_at = clock.now();
            let outcome = match result {
                // ⚠️ **预算在这里比，不在引擎里比**（`TSK-005`）。
                //
                // 它以前只在那条走 socket 的路上比过一次——而那条路
                // **从来没接进任何部署**，`D63` 把它删了。也就是说
                // **今天真正跑的那条路上，单次 token 上限一次都没有被比过**：
                // 派工单带着 `token_budget`、`_runs` 那一行同时记着 `tokensUsed`
                // 与 `tokenBudget`，两个数并排躺着，**没有人拿它们比一下**。
                //
                // 放在这里是因为它不是引擎的性质，是执行契约的性质：
                // 换成 `StubEngine` 也该照样成立。
                //
                // ⚠️ **这一道是事后归类，"撞上即终止"在引擎那一侧**
                // （`EmbeddedEngine::settings_for` 把 `max_budget_tokens` 按派工单设，
                // 引擎每次模型调用之后比一次，撞上就结束回合）。两处都要有：
                // **引擎那一道管住烧，这一道管住算**——桩引擎、以及任何不自己收手的
                // 引擎，都靠这一道兜住。
                //
                // ⚠️ **比的是 `>=`，不是 `>`。** `TSK-005` 的原话是"消耗的模型 token
                // **撞上**这个数"，引擎那一侧也是 `>=`。写成 `>` 的话，
                // 一次**正好被引擎按预算掐掉**的执行在这里会被判成成功——
                // 引擎停在 `total == budget`，而 `>` 不成立。**那个缝很窄，
                // 而且只有在缓存用量为零时才露出来**，所以它不会在测试里自己出现。
                Ok(Ok(completed)) if completed.tokens_used >= budget => Outcome {
                    tokens_used: completed.tokens_used,
                    trace: completed.trace,
                    ..Outcome::failed(
                        run,
                        FailureKind::TokenBudget,
                        format!(
                            "这次执行用了 {} 个 token，撞上单次上限 {budget}（TSK-005）",
                            completed.tokens_used
                        ),
                        started_at,
                        finished_at,
                    )
                },
                Ok(Ok(completed)) => Outcome {
                    run,
                    status: Status::Succeeded,
                    failure: None,
                    output: completed.output,
                    trace: completed.trace,
                    tokens_used: completed.tokens_used,
                    rows: completed.rows,
                    started_at,
                    finished_at: Some(finished_at),
                },
                Ok(Err((kind, trace))) => {
                    let status = if cancel.requested() && kind == FailureKind::Timeout {
                        Status::Cancelled
                    } else {
                        Status::Failed
                    };
                    Outcome {
                        status,
                        ..Outcome::failed(run, kind, trace, started_at, finished_at)
                    }
                }
                Err(_) => Outcome::failed(
                    run,
                    FailureKind::Engine,
                    "引擎崩了。归入引擎错误类，可重跑——**不会无限期挂在 running 上**",
                    started_at,
                    finished_at,
                ),
            };
            Runtime::finish(&runs, run, outcome);
        });

        Ok(run)
    }

    fn status(&self, run: RunId) -> Result<Status> {
        self.locked()?
            .get(&run)
            .map(|slot| slot.status)
            .ok_or_else(|| Error::not_found("不存在"))
    }

    fn cancel(&self, run: RunId) -> Result<()> {
        let guard = self.locked()?;
        let slot = guard.get(&run).ok_or_else(|| Error::not_found("不存在"))?;
        // 已经结束的取消是无操作，不是错误。
        if slot.status == Status::Running {
            slot.cancel.request();
        }
        Ok(())
    }

    fn collect(&self, run: RunId) -> Result<Option<Outcome>> {
        let guard = self.locked()?;
        let slot = guard.get(&run).ok_or_else(|| Error::not_found("不存在"))?;
        Ok(slot.outcome.clone())
    }
}
