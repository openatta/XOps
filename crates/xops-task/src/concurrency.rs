//! 并发上限（`EXE-027`）。
//!
//! > 并发执行数有上限，按平台与项目两级控制，**防止单个项目耗尽全部算力**。
//!
//! 两级都要有：只有平台级，一个项目就能把名额吃光；只有项目级，项目一多平台还是会垮。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use xops_identity::ProjectId;

/// 默认的平台级上限。
pub const DEFAULT_PLATFORM_LIMIT: usize = 16;
/// 默认的项目级上限。**明显小于平台级**——这条限制存在的意义就在这个差值里。
pub const DEFAULT_PROJECT_LIMIT: usize = 4;

#[derive(Debug, Default)]
struct Counts {
    total: usize,
    per_project: HashMap<ProjectId, usize>,
}

/// 并发名额。
#[derive(Debug)]
pub struct Concurrency {
    platform_limit: usize,
    project_limit: usize,
    counts: Mutex<Counts>,
}

impl Default for Concurrency {
    fn default() -> Self {
        Self::new(DEFAULT_PLATFORM_LIMIT, DEFAULT_PROJECT_LIMIT)
    }
}

impl Concurrency {
    #[must_use]
    pub fn new(platform_limit: usize, project_limit: usize) -> Self {
        Self {
            platform_limit,
            project_limit,
            counts: Mutex::new(Counts::default()),
        }
    }

    /// 要一个名额。**要不到就返回 `None`**——调用方据此拒绝或排队，不阻塞。
    #[must_use]
    pub fn acquire(self: &Arc<Self>, project: ProjectId) -> Option<Permit> {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if counts.total >= self.platform_limit {
            return None;
        }
        let used = counts.per_project.entry(project).or_insert(0);
        if *used >= self.project_limit {
            return None;
        }
        *used += 1;
        counts.total += 1;
        Some(Permit {
            concurrency: Arc::clone(self),
            project,
        })
    }

    /// 此刻在跑几个。
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .total
    }

    fn release(&self, project: ProjectId) {
        let mut counts = self
            .counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        counts.total = counts.total.saturating_sub(1);
        if let Some(used) = counts.per_project.get_mut(&project) {
            *used = used.saturating_sub(1);
        }
    }
}

/// 一个名额。**析构即归还**——忘了归还是这类代码最常见的漏。
#[derive(Debug)]
pub struct Permit {
    concurrency: Arc<Concurrency>,
    project: ProjectId,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.concurrency.release(self.project);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 单个项目吃不掉全部算力() {
        let concurrency = Arc::new(Concurrency::new(10, 2));
        let hungry = ProjectId::generate();
        let other = ProjectId::generate();

        let _first = concurrency.acquire(hungry).expect("第一个");
        let _second = concurrency.acquire(hungry).expect("第二个");
        assert!(concurrency.acquire(hungry).is_none(), "EXE-027：项目级上限");
        // 别的项目照样要得到 —— 这就是两级控制的意义。
        assert!(concurrency.acquire(other).is_some());
    }

    #[test]
    fn 平台级上限也在() {
        let concurrency = Arc::new(Concurrency::new(2, 10));
        let _first = concurrency.acquire(ProjectId::generate()).unwrap();
        let _second = concurrency.acquire(ProjectId::generate()).unwrap();
        assert!(concurrency.acquire(ProjectId::generate()).is_none());
    }

    #[test]
    fn 名额析构即归还() {
        let concurrency = Arc::new(Concurrency::new(1, 1));
        let project = ProjectId::generate();
        {
            let _permit = concurrency.acquire(project).unwrap();
            assert_eq!(concurrency.in_flight(), 1);
            assert!(concurrency.acquire(project).is_none());
        }
        assert_eq!(concurrency.in_flight(), 0);
        assert!(concurrency.acquire(project).is_some(), "放开之后要得到");
    }

    #[test]
    fn 项目级默认明显小于平台级() {
        // 这条限制存在的意义就在这个差值里 —— 两者相等的话，一个项目就能把名额吃光。
        let (project, platform) = (DEFAULT_PROJECT_LIMIT, DEFAULT_PLATFORM_LIMIT);
        assert!(
            project * 2 <= platform,
            "项目级 {project} 对平台级 {platform}"
        );
    }
}
