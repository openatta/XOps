//! 桩引擎。**它不是玩具，是 `EXE-014` 的硬验收载体。**
//!
//! > XOps 与执行引擎是两个分立的进程，之间只有一条执行契约这一个接缝。
//!
//! 这句话唯一能被证明的方式，就是把引擎整个换掉、上面的一切不改一行。
//! 与 `CON-012` 的内存存储是同一种验收放在两个接缝上（G12）。

use std::sync::Mutex;

use crate::engine::{Cancel, Completed, Engine};
use crate::failure::FailureKind;
use crate::worksheet::Worksheet;

/// 一个想让它怎样就怎样的引擎。
pub struct StubEngine {
    healthy: Mutex<bool>,
    behaviour: Mutex<Behaviour>,
    /// 跑过的每一次都记下来，测试用。
    seen: Mutex<Vec<String>>,
}

/// 这个桩这次要表现成什么样。
#[derive(Debug, Clone)]
pub enum Behaviour {
    /// 正常跑完。
    Succeed { output: String, tokens: u64 },
    /// 按某一类失败。
    Fail(FailureKind),
    /// 一直等到被取消。**用来验超时与取消。**
    Hang,
    /// 直接 panic。**用来验"编排进程崩了不能让执行无限期挂起"**（`EXE-017`）。
    Panic,
}

impl Default for StubEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl StubEngine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            healthy: Mutex::new(true),
            behaviour: Mutex::new(Behaviour::Succeed {
                output: "跑完了".into(),
                tokens: 100,
            }),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// 换一种表现。
    pub fn behaves(&self, behaviour: Behaviour) {
        *self
            .behaviour
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = behaviour;
    }

    /// 让引擎"不可用"。
    pub fn set_healthy(&self, healthy: bool) {
        *self
            .healthy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = healthy;
    }

    /// 它跑过哪些。
    #[must_use]
    pub fn seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Engine for StubEngine {
    fn healthy(&self) -> bool {
        *self
            .healthy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn run(
        &self,
        worksheet: &Worksheet,
        cancel: &Cancel,
    ) -> std::result::Result<Completed, (FailureKind, String)> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(worksheet.run.to_string());
        let behaviour = self
            .behaviour
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        match behaviour {
            // 桩引擎不产出行 —— `G12` 那条硬验收要的是"契约与其余部分无需修改"，
            // 不是"桩也要能产出行"。
            Behaviour::Succeed { output, tokens } => Ok(Completed {
                output,
                trace: format!("桩引擎跑了 {}", worksheet.skill),
                tokens_used: tokens,
                rows: Vec::new(),
            }),
            Behaviour::Fail(kind) => Err((kind, format!("桩引擎按 {kind} 失败"))),
            Behaviour::Hang => {
                // 盯着取消信号 —— 真引擎也必须这么做（EXE-019）。
                while !cancel.requested() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err((FailureKind::Timeout, "桩引擎被取消了".into()))
            }
            Behaviour::Panic => panic!("桩引擎故意崩了"),
        }
    }
}
