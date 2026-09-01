//! 关系投影的契约一致性:**两个实现跑同一组测试。**
//!
//! 与 `store_conformance.rs` 是同一种验收放在第二条缝上（`G12`）:
//! 只写一个实现的契约会不自觉地长成那个实现的形状,所以内存实现不是桩,
//! **它是契约正确性的证据**。
//!
//! ⚠️ 排序、NULL 的位置、越界值这几样最容易在两个实现之间漂,所以它们各有一条。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_core::{Id, RowId};
use xops_store::relation::Direction;
use xops_store::{Column, MemoryRelations, Relation, Relations, Select, SqliteStore};

fn notices() -> Relation {
    Relation {
        name: "notices".into(),
        columns: vec![
            Column::text("user", true),
            Column::integer("created_at", true),
            Column::integer("read_at", false),
            Column::integer("retain_until", true),
        ],
    }
}

fn implementations() -> Vec<(&'static str, Arc<dyn Relations>)> {
    let sqlite = Arc::new(SqliteStore::in_memory().unwrap());
    vec![
        (
            "memory",
            Arc::new(MemoryRelations::new()) as Arc<dyn Relations>,
        ),
        ("sqlite", sqlite.relations()),
    ]
}

fn row(seed: u64) -> RowId {
    RowId::from_id(Id::from_parts(seed, u128::from(seed)))
}

fn notice(user: &str, created_at: i64, read_at: Option<i64>) -> Value {
    json!({
        "user": user,
        "created_at": created_at,
        "read_at": read_at,
        "retain_until": created_at + 1_000,
        "text": "正文原样带着",
    })
}

/// 每个实现都装同一批数据。
fn seeded() -> Vec<(&'static str, Arc<dyn Relations>)> {
    let mut out = Vec::new();
    for (label, relations) in implementations() {
        relations.declare(&notices()).unwrap();
        relations
            .upsert(
                "notices",
                row(1),
                &notice("alice", 100, None),
                &notice("alice", 100, None),
            )
            .unwrap();
        relations
            .upsert(
                "notices",
                row(2),
                &notice("bob", 200, None),
                &notice("bob", 200, None),
            )
            .unwrap();
        relations
            .upsert(
                "notices",
                row(3),
                &notice("bob", 300, Some(350)),
                &notice("bob", 300, Some(350)),
            )
            .unwrap();
        relations
            .upsert(
                "notices",
                row(4),
                &notice("bob", 400, None),
                &notice("bob", 400, None),
            )
            .unwrap();
        out.push((label, relations));
    }
    out
}

#[test]
fn 声明是幂等的() {
    for (label, relations) in implementations() {
        relations.declare(&notices()).unwrap();
        relations.declare(&notices()).unwrap();
        assert!(
            relations
                .select("notices", &Select::new())
                .unwrap()
                .is_empty(),
            "{label}：再声明一次不该把数据冲掉"
        );
    }
}

#[test]
fn 没声明过的投影用不了() {
    for (label, relations) in implementations() {
        assert!(relations.select("nope", &Select::new()).is_err(), "{label}");
        assert!(
            relations
                .upsert("nope", row(1), &json!({}), &json!({}))
                .is_err(),
            "{label}"
        );
    }
}

#[test]
fn 等值加为空就是我的未读() {
    for (label, relations) in seeded() {
        let mine = relations
            .select(
                "notices",
                &Select::new().equal("user", "bob").null("read_at"),
            )
            .unwrap();
        assert_eq!(mine.len(), 2, "{label}：bob 的三条里有一条读过了");
        assert!(
            mine.iter()
                .all(|(_, values)| values["user"] == json!("bob")),
            "{label}"
        );
    }
}

#[test]
fn 排序两个实现给同一个答案() {
    for (label, relations) in seeded() {
        let newest = relations
            .select(
                "notices",
                &Select::new()
                    .equal("user", "bob")
                    .newest_first("created_at")
                    .take(2),
            )
            .unwrap();
        let times: Vec<i64> = newest
            .iter()
            .map(|(_, values)| values["created_at"].as_i64().unwrap())
            .collect();
        assert_eq!(
            times,
            vec![400, 300],
            "{label}：新的在前，而且 limit 是后切的"
        );
    }
}

#[test]
fn null排在最前这件事两个实现一致() {
    for (label, relations) in seeded() {
        let mut select = Select::new().equal("user", "bob");
        select.order = Some(("read_at".into(), Direction::Asc));
        let ordered = relations.select("notices", &select).unwrap();
        assert_eq!(
            ordered[0].1["read_at"],
            json!(null),
            "{label}：null 最小 —— 这条最容易在两个实现之间漂"
        );
    }
}

#[test]
fn 不大于用来捞到期的那一批() {
    for (label, relations) in seeded() {
        let due = relations
            .select(
                "notices",
                &Select::new().no_later_than("retain_until", 1_300),
            )
            .unwrap();
        // retain_until 是 1100 / 1200 / 1300 / 1400，**边界算在内**。
        assert_eq!(due.len(), 3, "{label}：≤ 是闭区间，1300 也在里面");
    }
}

#[test]
fn 不大于遇到null是不匹配() {
    // ⚠️ 两个实现最容易在这里漂：SQL 里 `NULL <= 5` 是 NULL（不匹配），
    // 而"手写过滤"很容易把缺列当成 0 或者当成匹配。
    // 到期清理靠这条：**没设过期时刻的实例不该被扫进来。**
    for (label, relations) in implementations() {
        relations.declare(&notices()).unwrap();
        relations
            .upsert(
                "notices",
                row(1),
                &json!({"user": "a", "created_at": 1, "read_at": null, "retain_until": null}),
                &json!({}),
            )
            .unwrap();
        assert!(
            relations
                .select(
                    "notices",
                    &Select::new().no_later_than("retain_until", 9_999)
                )
                .unwrap()
                .is_empty(),
            "{label}：null 不参与 ≤ 的比较"
        );
    }
}

#[test]
fn 非空是为空的镜像() {
    for (label, relations) in seeded() {
        let read = relations
            .select("notices", &Select::new().not_null("read_at"))
            .unwrap();
        assert_eq!(read.len(), 1, "{label}");
        assert_eq!(read[0].1["read_at"], json!(350));
    }
}

#[test]
fn 覆盖写与删除() {
    for (label, relations) in seeded() {
        relations
            .upsert(
                "notices",
                row(2),
                &notice("bob", 200, Some(999)),
                &notice("bob", 200, Some(999)),
            )
            .unwrap();
        assert_eq!(
            relations
                .select(
                    "notices",
                    &Select::new().equal("user", "bob").null("read_at")
                )
                .unwrap()
                .len(),
            1,
            "{label}：覆盖之后 bob 只剩一条未读"
        );
        relations.remove("notices", row(4)).unwrap();
        assert_eq!(
            relations
                .select("notices", &Select::new().equal("user", "bob"))
                .unwrap()
                .len(),
            2,
            "{label}：删了就是真没了 —— 投影是缓存，缓存里不要墓碑"
        );
    }
}

#[test]
fn 清空之后一行不剩() {
    for (label, relations) in seeded() {
        relations.clear("notices").unwrap();
        assert!(
            relations
                .select("notices", &Select::new())
                .unwrap()
                .is_empty(),
            "{label}：这是重建的第一步"
        );
        // 清空不是删表 —— 还能接着写。
        relations
            .upsert(
                "notices",
                row(9),
                &notice("carol", 1, None),
                &notice("carol", 1, None),
            )
            .unwrap();
        assert_eq!(
            relations.select("notices", &Select::new()).unwrap().len(),
            1
        );
    }
}

#[test]
fn 用来找的那几样与原样带回来的可以是两回事() {
    // 这是 `upsert` 分成两个参数的理由：**被索引的字段不一定在载荷的第一层。**
    // 流程实例的 `subject` 就是嵌套的。
    for (label, relations) in implementations() {
        relations.declare(&notices()).unwrap();
        relations
            .upsert(
                "notices",
                row(7),
                &json!({"user": "dave", "created_at": 5, "read_at": null, "retain_until": 9}),
                &json!({"很深": {"的": {"东西": "在这儿"}}}),
            )
            .unwrap();
        let hit = relations
            .select("notices", &Select::new().equal("user", "dave"))
            .unwrap();
        assert_eq!(hit.len(), 1, "{label}：按扁平的那份找");
        assert_eq!(
            hit[0].1["很深"]["的"]["东西"],
            json!("在这儿"),
            "{label}：带回来的是嵌套的那份"
        );
    }
}

#[test]
fn 载荷原样带回来() {
    for (label, relations) in seeded() {
        let one = relations
            .select("notices", &Select::new().equal("user", "alice"))
            .unwrap();
        assert_eq!(
            one[0].1["text"],
            json!("正文原样带着"),
            "{label}：没进列的字段也要带回来"
        );
    }
}

#[test]
fn 拼错的列名当场失败() {
    for (label, relations) in seeded() {
        assert!(
            relations
                .select("notices", &Select::new().equal("usr", "bob"))
                .is_err(),
            "{label}：拼错的列名会表现成「没有数据」，所以要当场失败"
        );
    }
}
