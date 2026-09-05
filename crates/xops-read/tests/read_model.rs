//! 读模型自己的验收。
//!
//! ⚠️ **这个 crate 以前一条集成测试都没有**（`model.rs` 里连一个 `#[test]` 都没有，
//! 只有 `board.rs` 那三条）。它全部的覆盖来自 `xops-web` 的 27 条——
//! 而那些走的是 `WebServer::handle`，验的是**路由、授权与形状**。
//!
//! 读模型是"**前端唯一能看见的东西**"，它的完备性是 RP-06 能并行开工的全部前提。
//! 这一份验的是隔着 HTTP 看不见的那些：**投影 · 排序与切片的先后 · 软删**。
//!
//! 归属：RP-05。

use std::sync::Arc;

use serde_json::json;
use xops_audit::AuditLog;
use xops_core::{Clock, Role, SystemClock, TableName};
use xops_identity::{Directory, ProjectId, Slug, UserId};
use xops_read::{BoardSpec, Direction, Filter, ReadModel};
use xops_store::{MemoryStore, Store, WriteEngine};
use xops_table::{Catalog, Column, ColumnType, Protection, TableId, Tables, writtenby::WrittenBy};

struct 一套 {
    model: Arc<ReadModel>,
    tables: Arc<Tables>,
    directory: Arc<Directory>,
}

fn 备好() -> 一套 {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let catalog = Arc::new(Catalog::open(Arc::clone(&store), Arc::clone(&clock)).unwrap());
    let engine = Arc::new(WriteEngine::new(Arc::clone(&store), Arc::clone(&clock)));
    let relations: Arc<dyn xops_store::Relations> = Arc::new(xops_store::MemoryRelations::new());
    let mut audit = AuditLog::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&relations),
    )
    .unwrap();
    for table in xops_identity::directory::platform_tables().unwrap() {
        audit = audit.watching(table);
    }
    let audit = Arc::new(audit.watching(TableName::new(xops_table::CATALOG_TABLE).unwrap()));
    let directory = Arc::new(Directory::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&audit),
        Arc::clone(&clock),
    ));
    let tables = Arc::new(Tables::new(
        Arc::clone(&engine),
        catalog,
        Arc::clone(&audit),
        Arc::clone(&directory),
        Arc::clone(&clock),
        Arc::clone(&store),
    ));
    let notices = Arc::new(
        xops_notice::Notices::new(
            Arc::clone(&tables),
            Arc::clone(&relations),
            Arc::clone(&directory),
            Arc::clone(&clock),
        )
        .unwrap(),
    );
    let model = Arc::new(ReadModel::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&audit),
        Arc::clone(&directory),
        Arc::clone(&tables),
        notices,
        clock,
    ));
    一套 {
        model,
        tables,
        directory,
    }
}

fn 建人(套: &一套, account: &str, display: &str) -> UserId {
    套.directory
        .provision(
            xops_identity::user::ExternalAccount {
                provider: xops_identity::user::ProviderId::new("builtin").unwrap(),
                account: account.to_owned(),
            },
            display,
            None,
        )
        .unwrap()
        .id
}

/// 一个项目、一张 `bugs` 表、`n` 行，`seq` 从 0 数上去。
fn 装满(套: &一套, n: i64) -> (UserId, ProjectId, TableId) {
    let alice = 建人(套, "alice", "Alice");
    let project = 套
        .directory
        .create_project(alice, Slug::new("acme").unwrap(), "Acme")
        .unwrap()
        .id;
    let bugs = TableId::user("bugs").unwrap();
    套.tables
        .create(
            alice,
            project,
            bugs.clone(),
            Protection::Normal,
            vec![
                Column::new("title", ColumnType::Text { max_len: 64 }, true).unwrap(),
                Column::new("seq", ColumnType::Integer, false).unwrap(),
                Column::new("note", ColumnType::Text { max_len: 64 }, false).unwrap(),
            ],
        )
        .unwrap();
    let written = WrittenBy::Person { user: alice };
    for index in 0..n {
        套.tables
            .insert(
                &written,
                Some(project),
                &bugs,
                json!({"title": format!("第{index}条"), "seq": index, "note": "旁注"}),
            )
            .unwrap();
    }
    (alice, project, bugs)
}

fn 定看板(
    套: &一套, alice: UserId, project: ProjectId, spec: BoardSpec
) -> xops_read::BoardId {
    套.model.define_board(alice, project, spec).unwrap().id
}

#[test]
fn 排序在切片之前() {
    // ⚠️ **顺序反了不报错**：先切再排会稳定地显示**最老的那一批**，
    // 而页面看着完全正常。排序要拿到全部命中才答得出来。
    let 套 = 备好();
    let (alice, project, bugs) = 装满(&套, 10);
    let board = 定看板(
        &套,
        alice,
        project,
        BoardSpec {
            name: "倒序".into(),
            table: bugs,
            filters: Vec::new(),
            sort: Some("seq".into()),
            direction: Direction::Desc,
            columns: Vec::new(),
        },
    );

    let 头一页 = 套.model.board(alice, board, 0, 3).unwrap();
    assert_eq!(头一页.rows.len(), 3);
    assert_eq!(
        头一页.rows[0].values["seq"],
        json!(9),
        "倒序的第一页该是最大的那几个 —— 先切再排会得到 0"
    );
    assert!(头一页.has_more);

    let 末页 = 套.model.board(alice, board, 9, 3).unwrap();
    assert_eq!(末页.rows.len(), 1, "只剩一行");
    assert_eq!(末页.rows[0].values["seq"], json!(0));
    assert!(!末页.has_more, "到头了");
    assert_eq!(末页.offset, 9);
}

#[test]
fn 越过末尾拿到的是空页不是报错() {
    // 翻页的人手快按过头是常事，那不该是一个错。
    let 套 = 备好();
    let (alice, project, bugs) = 装满(&套, 3);
    let board = 定看板(
        &套,
        alice,
        project,
        BoardSpec {
            name: "全部".into(),
            table: bugs,
            filters: Vec::new(),
            sort: None,
            direction: Direction::Asc,
            columns: Vec::new(),
        },
    );
    let 空页 = 套.model.board(alice, board, 99, 10).unwrap();
    assert!(空页.rows.is_empty());
    assert!(!空页.has_more);
}

#[test]
fn 筛选之后才分页而不是分页之后再筛() {
    // ⚠️ 反过来的话，第一页可能一行都没有而 `has_more` 却是 true——
    // 看的人以为"这个筛选没结果"，其实是**筛选被用在了错的时机**。
    let 套 = 备好();
    let (alice, project, bugs) = 装满(&套, 10);
    let board = 定看板(
        &套,
        alice,
        project,
        BoardSpec {
            name: "只要第7条".into(),
            table: bugs,
            filters: vec![Filter::Equals {
                column: "title".into(),
                value: json!("第7条"),
            }],
            sort: None,
            direction: Direction::Asc,
            columns: Vec::new(),
        },
    );
    let view = 套.model.board(alice, board, 0, 3).unwrap();
    assert_eq!(view.rows.len(), 1, "命中的只有一行");
    assert!(!view.has_more);
}

#[test]
fn 只给看板声明的那几列而且来源标识总是留着() {
    // `TBL-016`：看板上的来源标识读的就是 `writtenBy`——**它不受选列影响**。
    // 否则一个只选了两列的看板会把"这行是谁写的"一起选没了，
    // 而"模型产出，内容不可信"这句话正是从它来的。
    let 套 = 备好();
    let (alice, project, bugs) = 装满(&套, 2);
    let board = 定看板(
        &套,
        alice,
        project,
        BoardSpec {
            name: "只看标题".into(),
            table: bugs,
            filters: Vec::new(),
            sort: None,
            direction: Direction::Asc,
            columns: vec!["title".into()],
        },
    );
    let view = 套.model.board(alice, board, 0, 10).unwrap();
    assert_eq!(view.columns, vec!["title".to_owned()]);
    let values = &view.rows[0].values;
    assert!(values.get("title").is_some());
    assert!(values.get("note").is_none(), "没声明的列不给");
    assert!(
        values.get("writtenBy").is_some(),
        "来源标识总是留着（TBL-016）"
    );
}

#[test]
fn 表清单不含软删掉的那些() {
    // `TBL-026`：软删之后**从列出结果里消失**，而行与事件一律保留、单行历史仍可查。
    let 套 = 备好();
    let (alice, project, bugs) = 装满(&套, 1);
    assert!(
        套.model
            .tables(alice, project)
            .unwrap()
            .iter()
            .any(|table| table.table == "bugs")
    );
    套.tables.drop_table(alice, project, &bugs).unwrap();
    assert!(
        !套.model
            .tables(alice, project)
            .unwrap()
            .iter()
            .any(|table| table.table == "bugs"),
        "软删之后不该再列出来"
    );
}

#[test]
fn 表清单一行数据都不给() {
    // ⚠️ 这条界线要守住：一个顺手"再回十行"的版本，
    // 就是**绕过看板定义的第二条读数据通路**（`BRD-001`）。
    let 套 = 备好();
    let (alice, project, _) = 装满(&套, 3);
    let 印出来 = serde_json::to_string(&套.model.tables(alice, project).unwrap()).unwrap();
    assert!(印出来.contains("bugs"));
    assert!(!印出来.contains("第0条"), "表清单里出现了行：{印出来}");
}

#[test]
fn 成员清单带得出显示名与角色() {
    // 前端**没有第二条数据通路**去按 id 换名字——给 id 不给名字的视图
    // 等于逼它去开一条。
    let 套 = 备好();
    let (alice, project, _) = 装满(&套, 0);
    let bob = 建人(&套, "bob", "Bob");
    套.directory
        .set_member(alice, project, bob, Role::Member)
        .unwrap();

    let members = 套.model.members(alice, project).unwrap();
    assert_eq!(members.len(), 2);
    let 乙 = members
        .iter()
        .find(|member| member.user == bob.to_string())
        .unwrap();
    assert_eq!(乙.display_name, "Bob");
    assert_eq!(乙.role, "member");
}

#[test]
fn 非成员读什么都与项目不存在一致() {
    // `PRJ-008`：分开说等于把"这个项目存在"告诉一个不该知道的人。
    let 套 = 备好();
    let (_, project, _) = 装满(&套, 1);
    let mallory = 建人(&套, "mallory", "Mallory");
    assert!(套.model.members(mallory, project).is_err());
    assert!(套.model.tables(mallory, project).is_err());
}

#[test]
fn 个人看板的签名里没有看谁的那个参数() {
    // `NTF-010` 的硬限定靠**调用方表达不出那个请求**兑现，不靠一次检查。
    // 这一条断言的是**形状**：`my_notices(viewer, limit)` 只有两个参数——
    // 加一个"看谁的"进去，这个文件当场编译不过。
    let 套 = 备好();
    let (alice, _, _) = 装满(&套, 0);
    let (notices, truncated) = 套.model.my_notices(alice, 10).unwrap();
    assert!(notices.is_empty());
    assert!(!truncated, "一条都没有的时候不该说截断");
}
