//! 保留期那一环：**谁定期去清**。
//!
//! ⚠️ 它**不在 `cleanup.rs` 里**是有意的：那个文件的公开面被一条验收数着
//! （`RET-005`:只接受一个时刻，**没有"删掉某一行"的入口**）。
//! 把这个循环塞进去会把那条验收变成一句空话。

use std::sync::Arc;

use xops_core::{Clock, Result, Timestamp};
use xops_table::Tables;

use crate::cleanup::Cleanup;

/// 把三条保留期一次做完（`RET-003` · `AUD-010` · `RET-008`）。
///
/// # 它补的是哪个口子
///
/// `Cleanup::sweep`、`AuditLog::prune`、`Notices::prune` 三条都实现了、都有验收——
/// **而没有任何东西调用它们**。后果不是"慢",是**保留期从不生效**:库只涨不减，
/// 而 `RET-001` 那句"输出留一个月、过程记录留七天"在成品里一个字也没兑现。
///
/// ⚠️ **整批按时间进行，不得选择性删除个别行**（`RET-005`）——
/// 这里的每一条都只吃一个时刻，没有"删掉某一行"的入口。
pub struct Keeper {
    cleanup: Arc<Cleanup>,
    tables: Arc<Tables>,
    directory: Arc<xops_identity::Directory>,
    clock: Arc<dyn Clock>,
    others: Vec<Arc<dyn Prunable>>,
}

/// 「按一个时刻整批清一次」的注入位。审计与通知各填一个。
pub trait Prunable: Send + Sync + 'static {
    /// 这一条清的是什么，用在留痕里。
    fn what(&self) -> &'static str;

    /// # Errors
    /// 底层不可用。
    fn prune(&self, now: Timestamp) -> Result<usize>;
}

impl std::fmt::Debug for Keeper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keeper")
            .field("others", &self.others.len())
            .finish_non_exhaustive()
    }
}

impl Keeper {
    #[must_use]
    pub fn new(
        cleanup: Arc<Cleanup>,
        tables: Arc<Tables>,
        directory: Arc<xops_identity::Directory>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            cleanup,
            tables,
            directory,
            clock,
            others: Vec::new(),
        }
    }

    #[must_use]
    pub fn with(mut self, prunable: Arc<dyn Prunable>) -> Self {
        self.others.push(prunable);
        self
    }

    /// 清一遍。返回一共动了多少行。
    ///
    /// # Errors
    /// 底层不可用。**单张表失败不中断整轮**——一张表清不动不该让别的也留着。
    pub fn prune(&self) -> Result<usize> {
        let now = self.clock.now();
        let mut total = 0;
        for project in self.directory.all_projects()? {
            let Ok(schemas) = self.tables.list_internal(project.id) else {
                continue;
            };
            for schema in schemas {
                match self.cleanup.sweep(project.id, &schema.name, now) {
                    Ok(swept) => total += swept.rows + swept.traces,
                    Err(error) => xops_core::log::warn(
                        "keeper.sweep",
                        &[
                            ("table", schema.name.as_str()),
                            ("error", &format!("{error}")),
                        ],
                    ),
                }
            }
        }
        for other in &self.others {
            match other.prune(now) {
                Ok(count) => total += count,
                Err(error) => xops_core::log::warn(
                    "keeper.prune",
                    &[("what", other.what()), ("error", &format!("{error}"))],
                ),
            }
        }
        Ok(total)
    }
}
