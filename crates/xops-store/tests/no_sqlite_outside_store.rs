//! 枚举全仓，证明 `CON-012` 的分界线还在。
//!
//! RP-01 的验收标准里有一条是"枚举全仓"：`xops-store` 之外不存在任何一处直接引用 SQLite
//! 的代码，`xops-store` 内部不使用触发器、存储过程、行锁、事务隔离级别、MVCC、外键、
//! 级联、JSON 列。**它是可执行的，不是评审时靠眼睛看的。**
//!
//! 为什么值得写这个测试：破坏这条线最常见的形态不是有人故意违规，是"就用一下 SQLite 的
//! 这个特性会快很多"。那一刻它总是划算的，而代价要到真换库的那天才结算。

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("找不到仓根")
}

/// 收集全仓的 Rust 源码与清单，跳过构建产物与 git 目录。
fn sources(root: &Path) -> Vec<PathBuf> {
    fn walk(directory: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if matches!(name.as_ref(), "target" | ".git" | "node_modules") {
                    continue;
                }
                walk(&path, out);
            } else if name.ends_with(".rs") || name == "Cargo.toml" {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

/// 去掉行注释再看代码。**契约的纪律写在注释里，注释里出现这些词是正常的。**
///
/// naive 的切法（遇到第一个 `//` 就截断）在这个仓里够用：没有任何字符串字面量含 `//`。
/// 哪天有了，这个函数要跟着改——那时它会以误报的形式让人知道。
fn code_only(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn 只有一个文件碰得到sqlite() {
    let root = repo_root();
    let allowed: Vec<PathBuf> = [
        "crates/xops-store/src/sqlite.rs",
        "crates/xops-store/Cargo.toml",
        "Cargo.toml",
        "crates/xops-store/tests/no_sqlite_outside_store.rs",
    ]
    .iter()
    .map(|path| root.join(path))
    .collect();

    let mut offenders = Vec::new();
    for path in sources(&root) {
        if allowed.contains(&path) {
            continue;
        }
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        if code_only(&source).contains("rusqlite") {
            offenders.push(
                path.strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
        }
    }
    assert!(
        offenders.is_empty(),
        "这些文件直接碰了 SQLite，D46「换库不改写入路径」当场落空：{offenders:#?}\n\
         要用存储就经 xops_store::Store，那是它存在的全部理由。"
    );
}

#[test]
fn sqlite实现不依赖任何数据库特有能力() {
    let path = repo_root().join("crates/xops-store/src/sqlite.rs");
    let code = code_only(&fs::read_to_string(&path).expect("读不到 sqlite.rs")).to_uppercase();

    // 每一条都逐字对应 CON-012 点名的那一样。
    let forbidden = [
        ("TRIGGER", "触发器"),
        ("PROCEDURE", "存储过程"),
        ("FOR UPDATE", "行锁"),
        ("ISOLATION", "事务隔离级别"),
        ("READ_UNCOMMITTED", "事务隔离级别"),
        ("BEGIN", "显式事务"),
        ("COMMIT", "显式事务"),
        ("ROLLBACK", "显式事务"),
        ("SAVEPOINT", "显式事务"),
        ("FOREIGN KEY", "外键"),
        ("REFERENCES", "外键"),
        ("CASCADE", "级联"),
        ("JSON_", "JSON 列"),
        ("->>", "JSON 列"),
    ];
    let hits: Vec<&str> = forbidden
        .iter()
        .filter(|(needle, _)| code.contains(needle))
        .map(|(_, what)| *what)
        .collect();
    assert!(
        hits.is_empty(),
        "sqlite.rs 用上了 CON-012 明确排除的能力：{hits:?}\n\
         换一个数据库就要改写入路径了，而这正是这一条要挡住的事。"
    );
}

#[test]
fn 存储契约就只有四个方法() {
    // "get / put / delete / scan —— 仅此，不多"。多出来的每一个方法，
    // 都是下一个实现要额外兑现的承诺，也是契约往某个具体数据库形状上长的第一步。
    let path = repo_root().join("crates/xops-store/src/store.rs");
    let code = code_only(&fs::read_to_string(&path).expect("读不到 store.rs"));
    let trait_body = code
        .split_once("pub trait Store")
        .expect("找不到 Store trait")
        .1
        .split_once("\n}")
        .expect("Store trait 没有收尾")
        .0;
    let methods: Vec<&str> = trait_body
        .lines()
        .filter_map(|line| line.trim().strip_prefix("fn "))
        .filter_map(|line| line.split(['(', '<']).next())
        .collect();
    assert_eq!(
        methods,
        vec!["get", "put", "delete", "scan"],
        "存储契约的方法集合变了"
    );
}

#[test]
fn 关系投影的契约就那五个方法() {
    // 第二条缝也要有这条纪律:多出来的每一个方法,都是下一个实现要额外兑现的承诺。
    //
    // ⚠️ `sqlite.rs` 里那部分的"不依赖数据库特有能力"已经由上面那条覆盖了——
    // 它读的是整个文件。`CREATE TABLE` 与 `CREATE INDEX` **不在 `CON-012` 的
    // 排除清单里**,而且它们在 SQLite / MySQL / PostgreSQL 上是同一个东西。
    let path = repo_root().join("crates/xops-store/src/relation.rs");
    let code = code_only(&fs::read_to_string(&path).expect("读不到 relation.rs"));
    let trait_body = code
        .split_once("pub trait Relations")
        .expect("找不到 Relations trait")
        .1
        .split_once("\n}")
        .expect("Relations trait 没有收尾")
        .0;
    let methods: Vec<&str> = trait_body
        .lines()
        .filter_map(|line| line.trim().strip_prefix("fn "))
        .filter_map(|line| line.split(['(', '<']).next())
        .collect();
    assert_eq!(
        methods,
        vec!["declare", "upsert", "remove", "select", "clear"],
        "关系投影契约的方法集合变了"
    );
}
