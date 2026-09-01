//! 通知的写入与读取。
//!
//! 这个文件里最要紧的一条：
//!
//! > **通知行在业务写的串行区间之外追加**（`CON-006`、`NTF-008`）：
//! > 写失败**只留痕，绝不回滚业务写**。
//!
//! 落法是**签名上的**：[`Notices::notify`] 的返回类型里没有 `Result`——
//! 它交回一组 [`Failure`]，**调用方拿不到一个能用 `?` 往上抛的东西**。
//! 这让"通知的失败绝不影响产生该事件的业务操作"成为**结构保证**，
//! 而不是"几乎不会失败"这种概率话术。

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use xops_core::{Clock, Error, Id, Result, RowId, Timestamp};
use xops_identity::{Directory, ProjectId, UserId};
use xops_table::{Filter, MAX_SCAN, TableId, Tables, WrittenBy, system};

use crate::derive::{Derived, SourceEvent, from_event, materialize};
use crate::notice::{Kind, Notice, NoticeId};
use crate::retention::Retention;

/// 通知落在这张平台**全局**表上（`TBL-010`、`NTF-014`）。
pub const NOTICES_TABLE: &str = system::NOTICES;

/// 一次没写进去的通知。**它是痕迹，不是异常。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub user: UserId,
    pub kind: Kind,
    pub subject: String,
    pub why: String,
}

/// 通知。
pub struct Notices {
    tables: Arc<Tables>,
    directory: Arc<Directory>,
    clock: Arc<dyn Clock>,
    retention: Retention,
    /// 写失败的痕迹。**留痕的落点就是它**——不进 `Result`，进这里。
    failures: Mutex<Vec<Failure>>,
}

impl std::fmt::Debug for Notices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notices")
            .field("retention", &self.retention)
            .finish_non_exhaustive()
    }
}

impl Notices {
    #[must_use]
    pub fn new(tables: Arc<Tables>, directory: Arc<Directory>, clock: Arc<dyn Clock>) -> Self {
        Self {
            tables,
            directory,
            clock,
            retention: Retention::default(),
            failures: Mutex::new(Vec::new()),
        }
    }

    #[must_use]
    pub const fn with_retention(mut self, retention: Retention) -> Self {
        self.retention = retention;
        self
    }

    /// 派生并追加通知。
    ///
    /// ⚠️ **它不返回 `Result`。** 这是 `NTF-008` 的落法：调用方在业务写完成之后调它，
    /// 拿到的是一组"没写进去的"，**没有一个能用 `?` 把业务写带崩的东西**。
    pub fn notify(&self, event: &SourceEvent) -> Vec<Failure> {
        let mut failures = Vec::new();
        let now = self.clock.now();
        for derived in from_event(event) {
            for user in self.audience(event.project(), &derived) {
                let notice = materialize(&derived, user, Some(event.project()), now);
                if let Err(error) = self.append(&notice) {
                    failures.push(Failure {
                        user,
                        kind: notice.kind,
                        subject: notice.subject.clone(),
                        why: format!("{error}"),
                    });
                }
            }
        }
        if !failures.is_empty()
            && let Ok(mut recorded) = self.failures.lock()
        {
            recorded.extend(failures.iter().cloned());
        }
        failures
    }

    /// 至今为止没写进去的那些。**"失败有痕迹"就是它。**
    #[must_use]
    pub fn failures(&self) -> Vec<Failure> {
        self.failures
            .lock()
            .map(|recorded| recorded.clone())
            .unwrap_or_default()
    }

    /// 我的未读（`NTF-009` 的第一个 tool）。
    ///
    /// **硬限定为 `user = 令牌持有人`**（`NTF-010`）：这个方法没有"看别人的"那个参数，
    /// 所以调用方**表达不出**那个请求。
    ///
    /// # Errors
    /// 底层不可用。
    pub fn unread(&self, viewer: UserId, limit: usize) -> Result<Vec<Notice>> {
        // ⚠️ **`user` 是一个谓词，不是「前一万行里凑巧是我的那些」。**
        //
        // 旧写法扫前一万行再过滤：`_notices` 是全局表、留三个月，二十个人用两个月
        // 就能过一万——过了之后**新通知反而看不见**，因为截断留下的是最老的一批。
        //
        // 这里的代价是一次全表扫描。**`_notices.user` 是全系统最该先加索引的那一列**，
        // 加上之后这个调用就变成一次索引点查，而调用方一行不用改。
        let mut mine: Vec<Notice> = self
            .matching(&[Filter::equals("user", viewer.to_string())])?
            .into_iter()
            .map(|(_, values)| Self::from_row(&values))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .filter(Notice::unread)
            .collect();
        // 新的在前。**跨项目一起排**——这是 `NTF-014` 的那句"在一个地方看得到"。
        mine.sort_by_key(|notice| std::cmp::Reverse(notice.created_at.as_millis()));
        mine.truncate(limit);
        Ok(mine)
    }

    /// 标记已读（`NTF-009` 的第二个 tool、`NTF-011`）。
    ///
    /// > `readAt` 是**全系统唯一一个用户可改的系统表列**：只能改自己那一行、
    /// > 只能改这一列，由平台专属 tool 代写，**照样追加事件**（`I-N`）。
    ///
    /// 三条各自的落法：
    ///
    /// ```text
    /// 只能改自己那一行   这里比一次 notice.user == actor，不一致就当作不存在
    /// 只能改这一列       写下去的 patch 里**只有 readAt**，没有别的键
    /// 照样追加事件       走 Tables::update，它本来就只有"写事件"这一条路（I-N）
    /// ```
    ///
    /// # Errors
    /// 不是自己的那一行——**与"不存在"完全一致**，不告诉调用方它存在。
    pub fn mark_read(&self, actor: UserId, notice: NoticeId) -> Result<Notice> {
        let (row, mut record) = self
            .find(notice)?
            .filter(|(_, record)| record.user == actor)
            .ok_or_else(|| Error::not_found("不存在"))?;
        let at = self.clock.now();
        record.read_at = Some(at);
        self.tables.update(
            &WrittenBy::Platform,
            None,
            &Self::table()?,
            row,
            // **只有这一列。** 多写一个键，"只能改这一列"就没了。
            json!({"readAt": at.as_millis()}),
        )?;
        Ok(record)
    }

    /// 到期清理（`RET-008`、`RET-005`）。**整批按时间，不挑行。**
    ///
    /// # Errors
    /// 底层不可用。
    pub fn prune(&self, now: Timestamp) -> Result<usize> {
        let table = Self::table()?;
        let mut swept = 0;
        for (row, values) in self.matching(&[])? {
            let expired = values
                .get("retainUntil")
                .and_then(Value::as_i64)
                .is_some_and(|until| until <= now.as_millis());
            if expired {
                self.tables
                    .delete(&WrittenBy::Platform, None, &table, row)?;
                swept += 1;
            }
        }
        Ok(swept)
    }

    // ——————————————————————————————— 内部 ———————————————————————————————

    /// 谁收得到（`NTF-005`）。**非项目成员一条都收不到。**
    fn audience(&self, project: ProjectId, derived: &Derived) -> Vec<UserId> {
        derived
            .recipients
            .0
            .clone()
            .into_iter()
            .filter(|user| {
                // 每次现查 —— 一个人退出项目之后就不该再收到这个项目的通知。
                self.directory
                    .role_of(project, *user)
                    .is_ok_and(|role| role.is_some())
            })
            .collect()
    }

    fn append(&self, notice: &Notice) -> Result<RowId> {
        self.tables.insert(
            &WrittenBy::Platform,
            None,
            &Self::table()?,
            json!({
                "notice": notice.id.to_string(),
                "user": notice.user.to_string(),
                "project": notice.project.map(|project| project.to_string()),
                "kind": notice.kind.as_str(),
                "subject": notice.subject,
                "text": notice.text,
                "createdAt": notice.created_at.as_millis(),
                "retainUntil": self.retention.retain_until(notice.created_at).as_millis(),
            }),
        )
    }

    fn table() -> Result<TableId> {
        TableId::system(NOTICES_TABLE)
    }

    /// 全部命中。**扫不动时明确失败，不截断**（`xops_table::MAX_SCAN`）。
    fn matching(&self, filters: &[Filter]) -> Result<Vec<(RowId, Value)>> {
        self.tables
            .query_all(None, &Self::table()?, filters, MAX_SCAN)
    }

    fn find(&self, notice: NoticeId) -> Result<Option<(RowId, Notice)>> {
        for (row, values) in self.matching(&[Filter::equals("notice", notice.to_string())])? {
            let record = Self::from_row(&values)?;
            if record.id == notice {
                return Ok(Some((row, record)));
            }
        }
        Ok(None)
    }

    fn from_row(values: &Value) -> Result<Notice> {
        let text = |key: &str| values.get(key).and_then(Value::as_str).unwrap_or_default();
        let id = Id::parse(text("notice")).map(NoticeId::from_id)?;
        let user = Id::parse(text("user")).map(UserId::from_id)?;
        let mut notice = Notice::new(
            id,
            user,
            Id::parse(text("project")).ok().map(ProjectId::from_id),
            match text("kind") {
                "node-awaiting-me" => Kind::NodeAwaitingMe,
                "instance-decided" => Kind::InstanceDecided,
                "row-not-settled" => Kind::RowNotSettled,
                "run-finished" => Kind::RunFinished,
                _ => Kind::RowAssignedToMe,
            },
            text("subject").to_owned(),
            text("text").to_owned(),
            values
                .get("createdAt")
                .and_then(Value::as_i64)
                .map_or_else(|| Timestamp::from_millis(0), Timestamp::from_millis),
        );
        notice.read_at = values
            .get("readAt")
            .and_then(Value::as_i64)
            .map(Timestamp::from_millis);
        Ok(notice)
    }
}
