//! RP-17 的验收。
//!
//! 逐条对着包文档的验收清单来，其中三条是**枚举源码**而不是跑一次：
//! "只从事件派生""不经模型""不派发通用 tool"——它们要证明的是
//! **某条路径不存在**，跑一次证不了这个。

use std::sync::Arc;

use serde_json::json;
use xops_audit::AuditLog;
use xops_core::{Clock, Error, Result, Role, SystemClock, TableName, Timestamp};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_notice::derive::SourceEvent;
use xops_notice::notice::Kind;
use xops_notice::{Notices, Retention};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};
use xops_table::engine::Catalog;
use xops_table::table::{Protection, TableId};
use xops_table::{Column, ColumnType, Tables};

/// 一个只对 `_notices` 写失败的存储。**用来构造"通知写不进去"这一种情形。**
struct NoticesUnwritable {
    inner: Arc<dyn Store>,
}

impl Store for NoticesUnwritable {
    fn get(&self, space: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.inner.get(space, key)
    }

    fn put(&self, space: &str, key: &[u8], value: &[u8]) -> Result<()> {
        if key.starts_with(b"_notices\0") {
            return Err(Error::internal("这次就是不让通知写进去"));
        }
        self.inner.put(space, key, value)
    }

    fn delete(&self, space: &str, key: &[u8]) -> Result<()> {
        self.inner.delete(space, key)
    }

    fn scan(
        &self,
        space: &str,
        prefix: &[u8],
        after: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        self.inner.scan(space, prefix, after, limit)
    }
}

struct Fixture {
    label: &'static str,
    notices: Arc<Notices>,
    tables: Arc<Tables>,
    directory: Arc<Directory>,
    clock: Arc<dyn Clock>,
}

fn build(label: &'static str, store: Arc<dyn Store>) -> Fixture {
    let clock = Arc::new(SystemClock);
    let catalog = Arc::new(Catalog::open(Arc::clone(&store), clock.clone()).unwrap());
    let engine = Arc::new(
        WriteEngine::new(Arc::clone(&store), clock.clone())
            .with_pre_write(Arc::clone(&catalog) as Arc<dyn xops_store::PreWrite>)
            .with_schema_check(Arc::clone(&catalog) as Arc<dyn xops_store::SchemaCheck>),
    );
    let mut audit = AuditLog::new(Arc::clone(&engine), Arc::clone(&store)).unwrap();
    for table in xops_identity::directory::platform_tables().unwrap() {
        audit = audit.watching(table);
    }
    audit = audit.watching(TableName::new(xops_table::CATALOG_TABLE).unwrap());
    let audit = Arc::new(audit);
    let directory = Arc::new(Directory::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&audit),
        clock.clone(),
    ));
    let tables = Arc::new(Tables::new(
        engine,
        catalog,
        audit,
        Arc::clone(&directory),
        clock.clone(),
        store,
    ));
    tables.ensure_global_tables().unwrap();
    let notices = Arc::new(Notices::new(
        Arc::clone(&tables),
        Arc::clone(&directory),
        clock.clone(),
    ));
    Fixture {
        label,
        notices,
        tables,
        directory,
        clock,
    }
}

fn fixtures() -> Vec<Fixture> {
    vec![
        build("memory", Arc::new(MemoryStore::new())),
        build("sqlite", Arc::new(SqliteStore::in_memory().unwrap())),
    ]
}

struct Scene {
    alice: UserId,
    bob: UserId,
    outsider: UserId,
    project: ProjectId,
}

fn scene(fixture: &Fixture) -> Scene {
    let user = |account: &str| {
        fixture
            .directory
            .provision(
                ExternalAccount {
                    provider: ProviderId::new("builtin").unwrap(),
                    account: account.into(),
                },
                account,
                None,
            )
            .unwrap()
            .id
    };
    let alice = user("alice");
    let bob = user("bob");
    let outsider = user("outsider");
    let project = fixture
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap()
        .id;
    fixture
        .directory
        .set_member(alice, project, bob, Role::Member)
        .unwrap();
    fixture
        .tables
        .ensure_system_tables(project, "acme")
        .unwrap();
    Scene {
        alice,
        bob,
        outsider,
        project,
    }
}

fn awaiting(project: ProjectId, who: &[UserId]) -> SourceEvent {
    SourceEvent::NodeActivated {
        project,
        instance: "I1".into(),
        node: "复核".into(),
        awaiting: who.to_vec(),
    }
}

// ——————————————————————————————— 锁外追加 ———————————————————————————————

#[test]
fn 通知写失败业务写不回滚而且有痕迹() {
    let inner: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let fixture = build(
        "notices-unwritable",
        Arc::new(NoticesUnwritable {
            inner: Arc::clone(&inner),
        }),
    );
    let scene = scene(&fixture);
    fixture
        .tables
        .create(
            scene.alice,
            scene.project,
            TableId::user("bugs").unwrap(),
            Protection::Normal,
            vec![Column::new("status", ColumnType::Text { max_len: 16 }, false).unwrap()],
        )
        .unwrap();

    // 业务写：照常成功。
    let row = fixture
        .tables
        .insert(
            &xops_table::WrittenBy::Person { user: scene.alice },
            Some(scene.project),
            &TableId::user("bugs").unwrap(),
            json!({"status": "open"}),
        )
        .unwrap();

    // 通知：写不进去。**但它交回的是一组痕迹，不是一个能往上抛的错误。**
    let failures = fixture
        .notices
        .notify(&awaiting(scene.project, &[scene.bob]));
    assert_eq!(failures.len(), 1, "NTF-008：写失败");
    assert!(failures[0].why.contains("不让通知写进去"));
    assert_eq!(fixture.notices.failures().len(), 1, "失败有痕迹");

    // 业务行还在 —— **绝不回滚业务写**。
    assert!(
        fixture
            .tables
            .get(Some(scene.project), &TableId::user("bugs").unwrap(), row)
            .unwrap()
            .is_some()
    );
}

/// `notify` 的返回类型里**没有 `Result`**——这是"绝不回滚业务写"的落法。
///
/// 它不是一句注释：调用方**拿不到一个能用 `?` 把业务写带崩的东西**。
#[test]
fn 追加通知的签名里没有可以往上抛的错误() {
    let source = include_str!("../src/service.rs");
    let body = source.split("#[cfg(test)]").next().unwrap();
    let line = body
        .lines()
        .find(|line| line.contains("pub fn notify("))
        .expect("找不到 notify");
    let signature: String = body[body.find(line).unwrap()..]
        .lines()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        signature.contains("Vec<Failure>") && !signature.contains("Result"),
        "NTF-008 的落法在签名上：{signature}"
    );
}

// ——————————————————————————————— 只从事件派生 ———————————————————————————————

#[test]
fn 造通知的路径只有事件派生这一条() {
    // 枚举本 crate 的源码：`Notice::new` 只能在 derive.rs 里被调到。
    let files = [
        ("notice.rs", include_str!("../src/notice.rs")),
        ("derive.rs", include_str!("../src/derive.rs")),
        ("service.rs", include_str!("../src/service.rs")),
        ("tools.rs", include_str!("../src/tools.rs")),
        ("retention.rs", include_str!("../src/retention.rs")),
    ];
    let needle = format!("Notice::{}(", "new");
    let mut callers: Vec<&str> = Vec::new();
    for (name, source) in files {
        let body = source.split("#[cfg(test)]").next().unwrap();
        if body.contains(&needle) {
            callers.push(name);
        }
    }
    // service.rs 的 from_row 是把已经写下的行读回来，不是产生一条新的。
    assert_eq!(
        callers,
        vec!["derive.rs", "service.rs"],
        "NTF-002：没有第三处造得出通知"
    );
    // 而且它不是 pub —— 跨 crate 造不出来。
    let notice = include_str!("../src/notice.rs");
    assert!(
        notice.contains(&format!("pub(crate) const fn {}(", "new")),
        "构造函数不对外"
    );
}

#[test]
fn 内容生成路径不经模型() {
    // 这个 crate 连执行域都不依赖 —— **没有一条能调到模型的边**。
    let manifest = include_str!("../Cargo.toml");
    for forbidden in ["xops-exec", "xops-script", "xops-task"] {
        assert!(!manifest.contains(forbidden), "NTF-003 / G8：{forbidden}");
    }
}

// ——————————————————————————————— 五类与可见权限 ———————————————————————————————

#[test]
fn 五类都实际触发一遍() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let events = [
            awaiting(scene.project, &[scene.bob]),
            SourceEvent::InstanceDecided {
                project: scene.project,
                instance: "I1".into(),
                state: "approved".into(),
                interested: vec![scene.bob],
            },
            SourceEvent::RowNotSettled {
                project: scene.project,
                instance: "I1".into(),
                table: "approvals".into(),
                row: "R1".into(),
                writer: scene.bob,
                reason: "写入者不在允许名单里".into(),
            },
            SourceEvent::RunFinished {
                project: scene.project,
                run: "RUN1".into(),
                task: "T1".into(),
                status: "failed".into(),
                owner: scene.bob,
                after_failure: None,
            },
            SourceEvent::RowAssigned {
                project: scene.project,
                table: "bugs".into(),
                row: "R2".into(),
                assignee: scene.bob,
            },
        ];
        for event in &events {
            assert!(
                fixture.notices.notify(event).is_empty(),
                "{}",
                fixture.label
            );
        }
        let unread = fixture.notices.unread(scene.bob, 100).unwrap();
        let kinds: std::collections::BTreeSet<&str> =
            unread.iter().map(|notice| notice.kind.as_str()).collect();
        assert_eq!(kinds.len(), 5, "{}：五类都到了", fixture.label);

        // "我写的行未被采纳"那一条 —— **自动化失灵时唯一的信号**，理由要在正文里。
        let not_settled = unread
            .iter()
            .find(|notice| notice.kind == Kind::RowNotSettled)
            .unwrap();
        assert!(not_settled.text.contains("写入者不在允许名单里"));
    }
}

/// 别的包里每一处"必须通知"，在本包都有落点。
///
/// 包文档的风险一节点名了这件事：**本包是别人声明的兑现方**，
/// 验收时要逐条对照那几个包的声明，而不是只测本包自己列的五类。
#[test]
fn 别的包声明的每一处必须通知都有落点() {
    let project = ProjectId::generate();
    let who = UserId::generate();
    let obligations: [(&str, SourceEvent, Kind); 5] = [
        (
            "TSK-012 后处理失败只留痕并通知任务所有者",
            SourceEvent::RunFinished {
                project,
                run: "RUN1".into(),
                task: "T1".into(),
                status: "succeeded".into(),
                owner: who,
                after_failure: Some("输出插件抛异常".into()),
            },
            Kind::RunFinished,
        ),
        (
            "EXE-024① schema 不过 → 整批行不入表，归为技能错误类失败",
            SourceEvent::RunFinished {
                project,
                run: "RUN2".into(),
                task: "T1".into(),
                status: "failed".into(),
                owner: who,
                after_failure: None,
            },
            Kind::RunFinished,
        ),
        (
            "EXE-024② schema 过但节点判定不过 → 行入表，只是不结算",
            SourceEvent::RowNotSettled {
                project,
                instance: "I1".into(),
                table: "approvals".into(),
                row: "R1".into(),
                writer: who,
                reason: "节点判定不过".into(),
            },
            Kind::RowNotSettled,
        ),
        (
            "FLW-027 / FLW-034 不结算时通知写入者",
            SourceEvent::RowNotSettled {
                project,
                instance: "I1".into(),
                table: "approvals".into(),
                row: "R2".into(),
                writer: who,
                reason: "写入者不在允许名单里".into(),
            },
            Kind::RowNotSettled,
        ),
        (
            "XFG-006 gitHead 没推上去 → 归为工作区错误、通知写入者",
            SourceEvent::RowNotSettled {
                project,
                instance: "I1".into(),
                table: "approvals".into(),
                row: "R3".into(),
                writer: who,
                reason: "工作区准备失败：gitHead 还没推上去".into(),
            },
            Kind::RowNotSettled,
        ),
    ];
    for (声明, event, expected) in obligations {
        let derived = xops_notice::from_event(&event);
        assert_eq!(derived.len(), 1, "{声明}");
        assert_eq!(derived[0].kind, expected, "{声明}");
        assert_eq!(derived[0].recipients.0, vec![who], "{声明}：发给该发的人");
    }
}

#[test]
fn 非项目成员收不到() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        // 指名道姓地发给一个不在项目里的人 —— **照样收不到**（NTF-005）。
        let failures = fixture
            .notices
            .notify(&awaiting(scene.project, &[scene.outsider, scene.bob]));
        assert!(failures.is_empty());
        assert!(
            fixture
                .notices
                .unread(scene.outsider, 100)
                .unwrap()
                .is_empty(),
            "{}",
            fixture.label
        );
        assert_eq!(fixture.notices.unread(scene.bob, 100).unwrap().len(), 1);
    }
}

// ——————————————————————————————— 行级限定 ———————————————————————————————

/// **通知表很大的时候,我的新通知照样看得见。**
///
/// 这一条对着一个真实的缺陷:早先 `unread` 是"扫前一万行再按 user 过滤"。
/// `_notices` 是**平台全局表**、留三个月——二十个人用两个月就能过一万。
/// 过了之后**新通知反而看不见**,因为行 ID 时间有序,截断留下的是最老的一批。
///
/// 而且它是静默的:没有报错,只有"怎么没收到通知"。
#[test]
fn 通知表过了旧上限之后新通知照样看得见() {
    let fixture = build("memory", Arc::new(MemoryStore::new()));
    let scene = scene(&fixture);
    let notices_table = TableId::system(xops_table::system::NOTICES).unwrap();

    // 先塞过旧的那个上限。**这些都不是给 bob 的**——它们只负责把他挤到截断线外。
    for index in 0..10_100 {
        fixture
            .tables
            .insert(
                &xops_table::WrittenBy::Platform,
                None,
                &notices_table,
                json!({
                    "notice": xops_core::Id::generate().to_string(),
                    "user": scene.alice.to_string(),
                    "kind": "run-finished",
                    "subject": format!("噪声 {index}"),
                    "text": "别人的通知",
                    "createdAt": 1,
                }),
            )
            .unwrap();
    }

    // 现在给 bob 发一条 —— 它排在第 10101 位。
    assert!(
        fixture
            .notices
            .notify(&awaiting(scene.project, &[scene.bob]))
            .is_empty()
    );

    let mine = fixture.notices.unread(scene.bob, 100).unwrap();
    assert_eq!(
        mine.len(),
        1,
        "旧写法在这里返回空:截断留下的是最老的一万条,而我的那条排在后面"
    );
    assert_eq!(mine[0].kind, Kind::NodeAwaitingMe);
    // 而且**别人的一条都没串进来**（NTF-010）。
    assert!(mine.iter().all(|notice| notice.user == scene.bob));
}

#[test]
fn 查不到别人的也标记不了别人的() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        fixture
            .notices
            .notify(&awaiting(scene.project, &[scene.bob]));
        // a 的令牌看不到 b 的。
        assert!(fixture.notices.unread(scene.alice, 100).unwrap().is_empty());
        let bobs = fixture.notices.unread(scene.bob, 100).unwrap();
        assert_eq!(bobs.len(), 1);
        // a 拿着 b 那条的标识去标记已读 —— **与"不存在"完全一致**。
        let error = fixture
            .notices
            .mark_read(scene.alice, bobs[0].id)
            .unwrap_err();
        assert!(
            format!("{error}").contains("不存在"),
            "{}：不告诉他它存在",
            fixture.label
        );
        assert!(fixture.notices.unread(scene.bob, 100).unwrap()[0].unread());
    }
}

#[test]
fn 标记已读只改自己那一行的那一列而且照样追加事件() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        fixture
            .notices
            .notify(&awaiting(scene.project, &[scene.bob]));
        let before = fixture.notices.unread(scene.bob, 100).unwrap();
        let notice = before[0].id;

        let table = TableId::system(xops_table::system::NOTICES).unwrap();
        let rows = fixture.tables.rows(None, &table, 16).unwrap();
        let (row, values) = rows.first().cloned().unwrap();
        let events_before = fixture.tables.history(None, &table, row).unwrap().len();
        let subject_before = values["subject"].clone();

        let updated = fixture.notices.mark_read(scene.bob, notice).unwrap();
        assert!(updated.read_at.is_some());
        assert!(fixture.notices.unread(scene.bob, 100).unwrap().is_empty());

        let after = fixture.tables.get(None, &table, row).unwrap().unwrap();
        assert_eq!(after["subject"], subject_before, "别的列一个字没动");
        assert!(after["readAt"].is_i64());
        assert_eq!(
            fixture.tables.history(None, &table, row).unwrap().len(),
            events_before + 1,
            "{}：I-N —— 改动照样追加事件",
            fixture.label
        );
    }
}

// ——————————————————————————————— 不派发 ———————————————————————————————

#[test]
fn 通知域恰好两个tool而且没有第三个() {
    assert_eq!(xops_notice::tools::NOTICE_TOOLS.len(), 2, "NTF-009");
    let source = include_str!("../src/tools.rs");
    let body = source.split("#[cfg(test)]").next().unwrap();
    assert_eq!(body.matches("registry.register(").count(), 2);
}

#[test]
fn 通知表建不了自由看板() {
    // BRD-004 由 RP-05 那一侧拒掉，这里把它当成本包的邻接验收再确认一次。
    let notices = TableId::system(xops_table::system::NOTICES).unwrap();
    assert!(
        xops_read::board::check_boardable(&notices).is_err(),
        "个人看板是平台内建的固定视图，不是用户能配的那种"
    );
    assert!(xops_read::board::check_boardable(&TableId::user("bugs").unwrap()).is_ok());
}

// ——————————————————————————————— 聚合与保留期 ———————————————————————————————

#[test]
fn 跨项目的待办在一个地方看得到() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let second = fixture
            .directory
            .create_project(scene.alice, Slug::new("beta").unwrap(), "Beta")
            .unwrap()
            .id;
        fixture
            .directory
            .set_member(scene.alice, second, scene.bob, Role::Member)
            .unwrap();
        fixture.tables.ensure_system_tables(second, "beta").unwrap();

        fixture
            .notices
            .notify(&awaiting(scene.project, &[scene.bob]));
        fixture.notices.notify(&awaiting(second, &[scene.bob]));

        let unread = fixture.notices.unread(scene.bob, 100).unwrap();
        assert_eq!(unread.len(), 2, "{}", fixture.label);
        let projects: std::collections::BTreeSet<_> =
            unread.iter().filter_map(|notice| notice.project).collect();
        assert_eq!(projects.len(), 2, "NTF-014：它是平台全局表");
    }
}

#[test]
fn 按自己的保留期整批清理() {
    for fixture in fixtures() {
        let scene = scene(&fixture);
        let short = Arc::new(
            Notices::new(
                Arc::clone(&fixture.tables),
                Arc::clone(&fixture.directory),
                Arc::clone(&fixture.clock),
            )
            .with_retention(Retention { keep_days: 1 }),
        );
        short.notify(&awaiting(scene.project, &[scene.bob]));
        assert_eq!(short.unread(scene.bob, 100).unwrap().len(), 1);

        // 一天之内不清。
        let now = fixture.clock.now();
        assert_eq!(short.prune(now).unwrap(), 0, "{}", fixture.label);

        // 过了保留期就整批清掉 —— **与任务保留期无关**。
        let later = Timestamp::from_millis(now.as_millis() + 2 * 24 * 60 * 60 * 1_000);
        assert_eq!(short.prune(later).unwrap(), 1);
        assert!(short.unread(scene.bob, 100).unwrap().is_empty());
    }
}

#[test]
fn 默认保留期是三个月() {
    assert_eq!(Retention::default().keep_days, 90, "RET-008");
}
