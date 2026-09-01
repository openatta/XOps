//! 关系投影：**一张真表，有真列、真索引**。
//!
//! # 它为什么存在
//!
//! [`Store`](crate::Store) 那四个方法只认"点查一个键"和"扫一段前缀"。
//! 需要**按别的列找**的时候，剩下的只有扫全表再过滤——那在数据量上去之后
//! 要么慢，要么(更糟)被一个写死的上限截断成一个错误答案。
//!
//! 两条出路。**这里选的是第二条：**
//!
//! ```text
//! ① 在键值里手写一条二级索引   —— 自己维护、自己修、自己保证一致
//! ② 把那张表独立成一张真表      —— 索引交给数据库，它本来就干这个
//! ```
//!
//! ① 是在重新实现数据库已经做好的事，**引入复杂性而没有收益**。
//!
//! # 它是缓存，不是账
//!
//! ⚠️ **这一层可以整个删掉重建。** 账在事件流里（`space::EVENT`），
//! 关系投影只是"当前视图"的另一种存法——与键值投影平级。
//! 所以：
//!
//! - `I-N` 不受影响：写照样先追加事件，关系投影是事件之后的第二次落地。
//! - 漂了就重建，不需要修补。**能重建这件事本身就是它敢做缓存的理由。**
//!
//! # 它有没有违反 `CON-012`
//!
//! 没有。那条禁用的是**触发器、存储过程、行锁、事务隔离级别、MVCC、外键、级联、
//! JSON 列**——`CREATE TABLE` 与 `CREATE INDEX` 不在其中，而且它们在
//! SQLite / MySQL / PostgreSQL 上是同一个东西。**换库照旧。**

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xops_core::{Error, Result, RowId};

/// 一个投影列的值类型。**只有三种**——够表达"能被索引的东西"了，
/// 这不是在做类型系统。业务上的列类型归 `xops-table`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueKind {
    Text,
    Integer,
    Bool,
}

/// 一个投影列。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub kind: ValueKind,
    /// 给它建一条索引。**该建的建，不该建的别建**——
    /// 每一条索引都要在写入那一侧还回去。
    pub indexed: bool,
}

impl Column {
    #[must_use]
    pub fn text(name: &str, indexed: bool) -> Self {
        Self {
            name: name.to_owned(),
            kind: ValueKind::Text,
            indexed,
        }
    }

    #[must_use]
    pub fn integer(name: &str, indexed: bool) -> Self {
        Self {
            name: name.to_owned(),
            kind: ValueKind::Integer,
            indexed,
        }
    }
}

/// 一张关系投影的声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub name: String,
    pub columns: Vec<Column>,
}

impl Relation {
    /// # Errors
    /// 名字不合法（它会进 SQL 标识符的位置）· 没有列 · 列名重名。
    pub fn check(&self) -> Result<()> {
        check_identifier(&self.name)?;
        if self.columns.is_empty() {
            return Err(Error::invalid("关系投影至少要有一列"));
        }
        // ⚠️ **重名按大小写不敏感判**：目标库是 MySQL，那里的列名不区分大小写。
        // 在 SQLite 上 `readAt` 与 `readat` 是两列，换到 MySQL 就是同一列——
        // 这种差异要在声明这一刻就挡住，不要留到迁移那天。
        let mut seen = std::collections::BTreeSet::new();
        for column in &self.columns {
            check_identifier(&column.name)?;
            if !seen.insert(column.name.to_ascii_lowercase()) {
                return Err(Error::invalid(format!(
                    "列名重了（大小写不敏感，因为 MySQL 那边就是）：{}",
                    column.name
                )));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| column.name == name)
    }
}

/// SQL 的保留字，**不能拿来当列名或表名**。
///
/// ⚠️ SQLite 对这些很宽容，**MySQL 不是**——`CREATE TABLE ... (table TEXT)` 在那边直接语法错。
/// 目标库是 MySQL，所以这条也在声明这一刻挡住，不留到迁移那天。
///
/// 这不是一份完整的保留字表（那有几百个），是**最容易被当成业务列名的那些**。
const RESERVED: [&str; 24] = [
    "table", "index", "key", "order", "group", "select", "insert", "update", "delete", "from",
    "where", "join", "union", "column", "primary", "foreign", "default", "check", "unique",
    "values", "into", "and", "or", "not",
];

/// 名字要能安全地拼进 SQL 标识符的位置。
///
/// ⚠️ **这不是"清洗输入"，是"只认一个白名单形状"。** 关系名与列名都来自代码里的
/// 常量，不来自调用方——真有一天它们来自调用方了，这条也还在。
///
/// # Errors
/// 空、太长、或者含小写字母数字下划线之外的东西。
pub fn check_identifier(name: &str) -> Result<()> {
    let shaped = !name.is_empty()
        && name.len() <= 48
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !shaped {
        return Err(Error::invalid(format!(
            "关系投影的名字要是小写字母开头、只含字母数字下划线：{name}"
        )));
    }
    if RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
        return Err(Error::invalid(format!(
            "{name} 是 SQL 保留字，当不了列名。\
             SQLite 容得下，**MySQL 那边是语法错**——所以在这里就挡住"
        )));
    }
    Ok(())
}

/// 排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Asc,
    Desc,
}

/// 一次查询。
///
/// **四个算子，每一个都有一处真实调用把它带出来**——不是先设计一套语言再找用处：
///
/// ```text
/// equals       "我的通知"            user = 令牌持有人
/// is_null      "未读"                readAt 还没填
/// is_not_null  "已经决定过的"        对称地留着，不留会立刻有人用 equals 凑
/// at_most      "到期的那批"          retainUntil ≤ 现在（RET-005 整批按时间）
/// at_least     "这个时刻之后的"      审计的时间区间查（AUD-008）
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Select {
    pub equals: Vec<(String, Value)>,
    pub is_null: Vec<String>,
    pub is_not_null: Vec<String>,
    pub at_most: Vec<(String, i64)>,
    pub at_least: Vec<(String, i64)>,
    pub order: Option<(String, Direction)>,
    /// 0 表示不限。
    pub limit: usize,
}

impl Select {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn equal(mut self, column: &str, value: impl Into<Value>) -> Self {
        self.equals.push((column.to_owned(), value.into()));
        self
    }

    #[must_use]
    pub fn null(mut self, column: &str) -> Self {
        self.is_null.push(column.to_owned());
        self
    }

    #[must_use]
    pub fn not_null(mut self, column: &str) -> Self {
        self.is_not_null.push(column.to_owned());
        self
    }

    #[must_use]
    pub fn no_later_than(mut self, column: &str, value: i64) -> Self {
        self.at_most.push((column.to_owned(), value));
        self
    }

    #[must_use]
    pub fn no_earlier_than(mut self, column: &str, value: i64) -> Self {
        self.at_least.push((column.to_owned(), value));
        self
    }

    #[must_use]
    pub fn oldest_first(mut self, column: &str) -> Self {
        self.order = Some((column.to_owned(), Direction::Asc));
        self
    }

    #[must_use]
    pub fn newest_first(mut self, column: &str) -> Self {
        self.order = Some((column.to_owned(), Direction::Desc));
        self
    }

    #[must_use]
    pub const fn take(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// 它引用到的每一列都得在声明里。
    ///
    /// # Errors
    /// 引用了一个没声明过的列——**当场失败，不是当作不匹配**。
    /// 后者会让一个拼错的列名表现成"没有数据"。
    pub fn check(&self, relation: &Relation) -> Result<()> {
        let mut referenced: Vec<&str> = Vec::new();
        referenced.extend(self.equals.iter().map(|(column, _)| column.as_str()));
        referenced.extend(self.is_null.iter().map(String::as_str));
        referenced.extend(self.is_not_null.iter().map(String::as_str));
        referenced.extend(self.at_most.iter().map(|(column, _)| column.as_str()));
        referenced.extend(self.at_least.iter().map(|(column, _)| column.as_str()));
        if let Some((column, _)) = &self.order {
            referenced.push(column);
        }
        for column in referenced {
            if relation.column(column).is_none() {
                return Err(Error::invalid(format!(
                    "{} 上没有 {column} 这一列——拼错的列名会表现成「没有数据」，所以这里当场失败",
                    relation.name
                )));
            }
        }
        Ok(())
    }
}

/// 关系投影的存取。
///
/// **它与 [`Store`](crate::Store) 平级，不在它下面**：一个管事件与键值投影，
/// 一个管带索引的当前视图。两个都有内存实现，
/// 所以"换一个实现进去不改上层"这条验收对两条缝都跑得起来（`G12`）。
pub trait Relations: Send + Sync + 'static {
    /// 声明一张关系投影。**幂等**：已经在了就什么也不做。
    ///
    /// # Errors
    /// 声明不合法或底层不可用。
    fn declare(&self, relation: &Relation) -> Result<()>;

    /// 写一行。已经在了就覆盖。
    ///
    /// `columns` 是**用来找的那几样**（按声明的列名取，只取一层）；
    /// `payload` 是**要原样带回来的东西**。
    ///
    /// ⚠️ **它们是两个参数,不是一个。** 一开始这里只有一个值——两者同形时那很省事,
    /// 但它会让人以为"被索引的字段一定在载荷的第一层"。
    /// 载荷是嵌套结构（比如流程实例的 `subject`）时,那个假设当场就不成立了。
    ///
    /// # Errors
    /// 没声明过这张投影，或者底层不可用。
    fn upsert(&self, relation: &str, row: RowId, columns: &Value, payload: &Value) -> Result<()>;

    /// 删一行。**这里是真删**——投影是缓存，缓存里不需要墓碑。
    ///
    /// # Errors
    /// 底层不可用。
    fn remove(&self, relation: &str, row: RowId) -> Result<()>;

    /// 查。
    ///
    /// # Errors
    /// 没声明过这张投影 · 引用了没声明的列 · 底层不可用。
    fn select(&self, relation: &str, select: &Select) -> Result<Vec<(RowId, Value)>>;

    /// 清空。**重建的第一步。**
    ///
    /// # Errors
    /// 底层不可用。
    fn clear(&self, relation: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notices() -> Relation {
        Relation {
            name: "notices".into(),
            columns: vec![
                Column::text("user", true),
                Column::integer("created_at", true),
                Column::integer("read_at", false),
            ],
        }
    }

    #[test]
    fn 名字只认一个白名单形状() {
        assert!(check_identifier("notices").is_ok());
        assert!(check_identifier("read_at").is_ok());
        // 这些如果拼进 SQL 标识符的位置就麻烦了。
        assert!(check_identifier("createdAt").is_ok(), "驼峰的列名是合法的");
        // 保留字：SQLite 容得下，MySQL 那边是语法错。
        for reserved in ["table", "order", "key", "TABLE", "Index"] {
            assert!(check_identifier(reserved).is_err(), "{reserved}");
        }
        // 这些如果拼进 SQL 标识符的位置就麻烦了。
        for bad in [
            "",
            "_notices",
            "Notices",
            "notices; drop table",
            "notices-1",
            "a b",
        ] {
            assert!(check_identifier(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn 声明要有列而且列名不能重() {
        assert!(notices().check().is_ok());
        let mut empty = notices();
        empty.columns.clear();
        assert!(empty.check().is_err());
        let mut dup = notices();
        dup.columns.push(Column::text("user", false));
        assert!(dup.check().is_err());
        // **大小写不敏感** —— MySQL 那边 readAt 与 readat 是同一列。
        let mut cased = notices();
        cased.columns.push(Column::text("useR", false));
        assert!(cased.check().is_err(), "换到 MySQL 就撞了，现在就该挡");
    }

    #[test]
    fn 拼错的列名当场失败而不是当作没有数据() {
        let relation = notices();
        assert!(Select::new().equal("user", "u1").check(&relation).is_ok());
        let error = Select::new()
            .equal("usr", "u1")
            .check(&relation)
            .unwrap_err();
        assert!(error.message().contains("没有 usr 这一列"));
        // 排序列也要查。
        assert!(Select::new().newest_first("nope").check(&relation).is_err());
    }

    #[test]
    fn 五个算子都在() {
        let select = Select::new()
            .equal("user", "u1")
            .null("read_at")
            .not_null("created_at")
            .no_later_than("created_at", 100)
            .no_earlier_than("created_at", 1)
            .newest_first("created_at")
            .take(50);
        assert!(select.check(&notices()).is_ok());
        assert_eq!(select.limit, 50);
    }
}
