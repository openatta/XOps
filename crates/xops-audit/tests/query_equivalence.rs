//! **把筛选翻成 `WHERE` 是最容易漏一条、错一个边界的地方。**
//!
//! 所以这里不只测"查得对不对"，而是拿一份**独立的、显然正确的实现**去对答案:
//! [`Query::matches`] 是参考语义，`AuditLog::query` 是真正走的那条路（SQL 里筛）。
//! 两者对同一批记录必须给出同一个集合。
//!
//! ⚠️ 早先这两件事是同一段代码：索引只按 scope 前缀扫，`kind` / `actor` / `target`
//! 全靠 `matches` 在内存里筛。现在筛选下沉到 SQL 了，**参考语义因此才有了对照的价值**。

use std::sync::Arc;

use xops_audit::envelope::EventKind;
use xops_audit::{AuditEnvelope, AuditLog, AuditRecord, Query, kinds};
use xops_core::{Actor, Id, SystemClock, TableName, Timestamp};
use xops_store::{MemoryRelations, MemoryStore, Relations, SqliteStore, Store, WriteEngine};

struct Harness {
    label: &'static str,
    audit: Arc<AuditLog>,
    engine: Arc<WriteEngine>,
}

fn harnesses() -> Vec<Harness> {
    let sqlite = Arc::new(SqliteStore::in_memory().unwrap());
    let sqlite_relations = sqlite.relations();
    [
        (
            "memory",
            Arc::new(MemoryStore::new()) as Arc<dyn Store>,
            Arc::new(MemoryRelations::new()) as Arc<dyn Relations>,
        ),
        ("sqlite", sqlite as Arc<dyn Store>, sqlite_relations),
    ]
    .into_iter()
    .map(|(label, store, relations)| {
        let clock = Arc::new(SystemClock);
        let engine = Arc::new(WriteEngine::new(Arc::clone(&store), clock));
        let audit = Arc::new(AuditLog::new(Arc::clone(&engine), store, relations).unwrap());
        Harness {
            label,
            audit,
            engine,
        }
    })
    .collect()
}

/// 造一批**故意各不相同**的留痕：两个项目 + 平台级，三种事件类型，两个 actor。
fn seed(audit: &AuditLog) -> (Id, Id, Id, Id) {
    let alpha = Id::generate();
    let beta = Id::generate();
    let me = Id::generate();
    let someone = Id::generate();

    let user = |name: &str| Actor::User {
        user: name.to_owned(),
    };
    let kinds_used = [
        kinds::PROJECT_CREATED,
        kinds::MEMBER_ADDED,
        kinds::TOKEN_ISSUED,
    ];

    for (index, kind) in kinds_used.into_iter().enumerate() {
        for project in [Some(alpha), Some(beta)] {
            let envelope = AuditEnvelope::project_scoped(
                kind,
                project.unwrap(),
                Id::generate(),
                serde_json::json!({"i": index}),
            )
            .unwrap();
            audit
                .append(&user(if index % 2 == 0 { "a" } else { "b" }), &envelope)
                .unwrap();
        }
    }
    // 平台级的两条：一条是我的，一条是别人的（`AUD-003`）。
    for subject in [me, someone] {
        let envelope = AuditEnvelope::platform(
            kinds::TOKEN_ISSUED,
            subject,
            Id::generate(),
            serde_json::json!({}),
        )
        .unwrap();
        audit.append(&user("a"), &envelope).unwrap();
    }
    (alpha, beta, me, someone)
}

/// 直接从事件流把全部留痕读出来——**参考路径不碰索引**，那是它的全部意义。
fn all_records(engine: &WriteEngine) -> Vec<AuditRecord> {
    let table = TableName::new(xops_audit::AUDIT_TABLE).unwrap();
    let mut out = Vec::new();
    let mut after = 0;
    loop {
        let events = engine.events(&table, after, 256).unwrap();
        if events.is_empty() {
            return out;
        }
        after = events.last().map_or(after, |event| event.seq);
        out.extend(events.into_iter().filter_map(AuditRecord::from_event));
    }
}

/// 用参考语义把同一批记录筛一遍。
fn by_reference(harness: &Harness, query: &Query) -> Vec<Id> {
    let mut hit: Vec<AuditRecord> = all_records(&harness.engine)
        .into_iter()
        .filter(|record| query.matches(record))
        .collect();
    hit.sort_by_key(|record| (record.at.as_millis(), record.seq));
    hit.truncate(query.limit);
    hit.into_iter().map(|record| record.id).collect()
}

fn by_sql(audit: &AuditLog, query: &Query) -> Vec<Id> {
    audit
        .query(query)
        .unwrap()
        .into_iter()
        .map(|record| record.id)
        .collect()
}

#[test]
fn 每一种筛选下两条路给同一个答案() {
    for harness in harnesses() {
        let label = harness.label;
        let audit = &harness.audit;
        let (alpha, beta, me, someone) = seed(audit);

        let mut queries = vec![
            ("某个项目", Query::in_project(alpha, me)),
            ("另一个项目", Query::in_project(beta, me)),
            ("平台级：我的", Query::platform(me)),
            ("平台级：别人的", Query::platform(someone)),
        ];
        // 加上各维度的组合。
        queries.push((
            "项目 + 类型",
            Query::in_project(alpha, me).of_kind(EventKind::new(kinds::PROJECT_CREATED).unwrap()),
        ));
        queries.push((
            "项目 + actor",
            Query::in_project(alpha, me).by(Actor::User {
                user: "a".to_owned(),
            }),
        ));
        queries.push((
            "项目 + 时间区间（全开）",
            Query::in_project(alpha, me)
                .between(Timestamp::from_millis(0), Timestamp::from_millis(i64::MAX)),
        ));
        queries.push((
            "项目 + 时间区间（早已过去，应当为空）",
            Query::in_project(alpha, me)
                .between(Timestamp::from_millis(0), Timestamp::from_millis(1)),
        ));
        queries.push((
            "项目 + 具体对象",
            Query::in_project(alpha, me).about(Id::generate()),
        ));

        // ⚠️ 一个两边都返回空的比较是**恒真的**——先确认这批查询真的查到了东西，
        // 否则这条测试只是在证明"两个空集合相等"。
        let mut hits = 0;
        for (what, query) in queries {
            let sql = by_sql(audit, &query);
            hits += sql.len();
            assert_eq!(
                sql,
                by_reference(&harness, &query),
                "{label} / {what}：SQL 那条路与参考语义对不上"
            );
        }
        assert!(hits > 10, "{label}：这批查询要真的查到东西，实际 {hits}");
    }
}

#[test]
fn 平台级事件只有主体本人读得到() {
    for harness in harnesses() {
        let label = harness.label;
        let audit = &harness.audit;
        let (_, _, me, someone) = seed(audit);

        let mine = by_sql(audit, &Query::platform(me));
        let theirs = by_sql(audit, &Query::platform(someone));
        assert_eq!(mine.len(), 1, "{label}：AUD-003");
        assert_eq!(theirs.len(), 1, "{label}");
        assert_ne!(mine, theirs, "{label}：两个人看到的不是同一条");
    }
}

#[test]
fn 项目的事件流不串到另一个项目() {
    for harness in harnesses() {
        let label = harness.label;
        let audit = &harness.audit;
        let (alpha, beta, me, _) = seed(audit);
        let a = by_sql(audit, &Query::in_project(alpha, me));
        let b = by_sql(audit, &Query::in_project(beta, me));
        assert_eq!(a.len(), 3, "{label}");
        assert_eq!(b.len(), 3, "{label}");
        assert!(
            a.iter().all(|id| !b.contains(id)),
            "{label}：一条都不该串过去"
        );
    }
}

#[test]
fn 索引清掉之后重建得回来() {
    for harness in harnesses() {
        let label = harness.label;
        let audit = &harness.audit;
        let (alpha, _, me, _) = seed(audit);
        let before = by_sql(audit, &Query::in_project(alpha, me));

        // 索引是缓存，事件流才是权威。
        let rebuilt = audit.rebuild_index().unwrap();
        assert!(rebuilt >= 8, "{label}：八条留痕都在事件流里");
        assert_eq!(
            by_sql(audit, &Query::in_project(alpha, me)),
            before,
            "{label}：重建之后一模一样"
        );
    }
}

#[test]
fn 同一毫秒内的先后是确定的() {
    // ⚠️ 光按 `at` 排，同刻的顺序就交给引擎决定了——而那在两个实现之间会漂。
    // 排序键把 `(时刻, 序号)` 拼成定宽十六进制，字典序即那个序。
    for harness in harnesses() {
        let label = harness.label;
        let audit = &harness.audit;
        let (alpha, _, me, _) = seed(audit);
        let once = by_sql(audit, &Query::in_project(alpha, me));
        for _ in 0..5 {
            assert_eq!(
                by_sql(audit, &Query::in_project(alpha, me)),
                once,
                "{label}：同一个查询每次都该给同一个顺序"
            );
        }
    }
}
