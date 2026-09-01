//! 表：三种、它们的 schema、以及它到物理表名的映射。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Result, TableName, Timestamp};
use xops_identity::ProjectId;

use crate::column::{AUTO_COLUMNS, Column, check_column_name};

/// 业务表名。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TableId(String);

impl TableId {
    pub const MAX_LEN: usize = 32;
    /// 系统表名的前缀。用户表不能用。
    pub const SYSTEM_PREFIX: char = '_';
    /// 系统表在 tool 名里的前缀（`_` 不是合法的 tool 名字符）。
    /// 用户表**不能**以它开头，否则两者会撞。
    pub const SYSTEM_SLUG: &'static str = "sys-";

    /// 用户建的表。
    ///
    /// # Errors
    /// 空、超长、不是小写字母开头、含允许字符之外的东西、以 `_` 或 `sys-` 开头。
    pub fn user(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        let shaped = (1..=Self::MAX_LEN).contains(&name.len())
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && !name.ends_with('-')
            && !name.contains("--")
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !shaped {
            return Err(Error::invalid(format!(
                "表名要 1–{} 个字符，小写字母开头，只含小写字母、数字与单个连字符：{name}",
                Self::MAX_LEN
            )));
        }
        if name.starts_with(Self::SYSTEM_SLUG) {
            return Err(Error::invalid(format!(
                "{} 是系统表在 tool 名里的前缀，用户表不能用它开头",
                Self::SYSTEM_SLUG
            )));
        }
        Ok(Self(name))
    }

    /// 平台建的系统表。
    ///
    /// # Errors
    /// 不以 `_` 开头。
    pub fn system(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if !name.starts_with(Self::SYSTEM_PREFIX) {
            return Err(Error::invalid(format!(
                "系统表名要以 {} 开头：{name}",
                Self::SYSTEM_PREFIX
            )));
        }
        Ok(Self(name))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_system(&self) -> bool {
        self.0.starts_with(Self::SYSTEM_PREFIX)
    }

    /// 它在 tool 名里的那一段。`_runs` → `sys-runs`。
    #[must_use]
    pub fn slug(&self) -> String {
        match self.0.strip_prefix(Self::SYSTEM_PREFIX) {
            Some(rest) => format!("{}{rest}", Self::SYSTEM_SLUG).replace('_', "-"),
            None => self.0.clone(),
        }
    }
}

impl std::fmt::Display for TableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 保护级别。**建表时声明，之后不可降级**（`TBL-004`、`I-Q`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protection {
    /// 按项目角色写。
    Normal,
    /// **只有项目所有者能写**（名单表，`TBL-025`、`I-Q`）。
    Protected,
}

/// 三种表（`TBL-003`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// 平台自动建，schema 固定且**不受用户列类型集合限制**，**只有平台能写**。
    System,
    /// 用户建。
    User,
}

/// 一张表的 schema。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableSchema {
    /// 归哪个项目。`_notices` 是平台全局表，它是 `None`（`TBL-010`）。
    pub project: Option<ProjectId>,
    /// 项目短名的副本。
    ///
    /// 派生文本要用 `{project.slug}`（`TBL-020`），而那一步发生在写入区间里——
    /// 在锁内去问身份目录要一次短名，等于把一次查询接进这张表的写吞吐。
    /// 短名创建后不可变（`PRJ-003`），所以抄一份是安全的。
    pub project_slug: String,
    pub name: TableId,
    pub kind: Kind,
    pub protection: Protection,
    pub columns: Vec<Column>,
    pub created_at: Timestamp,
    /// 软删（`TBL-026`）：从列出结果中消失、专属 tool 停止派发，
    /// **行与事件一律保留、单行历史仍可查**。
    pub dropped_at: Option<Timestamp>,
}

impl TableSchema {
    /// # Errors
    /// 列名重复、列名撞了自动补的列位、或者一列都没有。
    pub fn new(
        project: Option<ProjectId>,
        project_slug: impl Into<String>,
        name: TableId,
        kind: Kind,
        protection: Protection,
        columns: Vec<Column>,
        created_at: Timestamp,
    ) -> Result<Self> {
        let schema = Self {
            project,
            project_slug: project_slug.into(),
            name,
            kind,
            protection,
            columns,
            created_at,
            dropped_at: None,
        };
        schema.check_columns()?;
        Ok(schema)
    }

    fn check_columns(&self) -> Result<()> {
        if self.columns.is_empty() {
            return Err(Error::invalid("一张表至少要有一列"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for column in &self.columns {
            check_column_name(&column.name)?;
            if !seen.insert(column.name.as_str()) {
                return Err(Error::invalid(format!("列 {} 声明了两次", column.name)));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn is_dropped(&self) -> bool {
        self.dropped_at.is_some()
    }

    #[must_use]
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| column.name == name)
    }

    /// 物理表名——**给 RP-01 的写入路径用的那个名字**。
    ///
    /// 业务上的"表"是 `(项目, 名字)`，物理上它是键的一段前缀。两个项目各建一张 `bugs`
    /// 是完全正常的事，所以名字必须带上项目。
    ///
    /// # Errors
    /// 拼出来的名字超过 [`TableName`] 的上限——项目标识 26 字符 + 表名 32 字符 + 2，
    /// 在 64 以内，所以这只可能是常量被改坏了。
    pub fn physical(&self) -> Result<TableName> {
        physical_name(self.project, &self.name)
    }

    /// 加一列（`TBL-022`）。**新列对历史行为空。**
    ///
    /// # Errors
    /// 列名重复或不合法。**改列类型、删列、改列名不做**——那三件在 API 上根本不存在。
    pub fn add_column(&mut self, column: Column) -> Result<()> {
        if self.column(&column.name).is_some() {
            return Err(Error::invalid(format!(
                "列 {} 已经有了。改列类型、删列、改列名都不做——需要就新建一张表，自己把数据搬过去（TBL-022）",
                column.name
            )));
        }
        check_column_name(&column.name)?;
        self.columns.push(column);
        Ok(())
    }

    /// 用户能写的列。序号与派生列不在其中。
    pub fn writable_columns(&self) -> impl Iterator<Item = &Column> {
        self.columns
            .iter()
            .filter(|column| column.ty.user_writable())
    }
}

/// `(项目, 表名)` → 物理表名。
///
/// # Errors
/// 拼出来超过上限。
pub fn physical_name(project: Option<ProjectId>, name: &TableId) -> Result<TableName> {
    match project {
        Some(project) => TableName::new(format!("p{project}.{name}")),
        None => TableName::new(name.as_str()),
    }
}

/// 自动补的列位，给外面枚举用。
#[must_use]
pub fn auto_columns() -> &'static [&'static str] {
    &AUTO_COLUMNS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::column::ColumnType;

    fn columns() -> Vec<Column> {
        vec![Column::new("title", ColumnType::Text { max_len: 64 }, true).unwrap()]
    }

    fn schema() -> TableSchema {
        TableSchema::new(
            Some(ProjectId::generate()),
            "acme",
            TableId::user("bugs").unwrap(),
            Kind::User,
            Protection::Normal,
            columns(),
            Timestamp::from_millis(0),
        )
        .unwrap()
    }

    #[test]
    fn 表名认得出好坏() {
        assert!(TableId::user("bugs").is_ok());
        assert!(TableId::user("my-table-2").is_ok());
        assert!(TableId::user("_runs").is_err(), "下划线开头是系统表");
        assert!(TableId::user("sys-runs").is_err(), "会与系统表的 slug 撞");
        assert!(TableId::user("Bugs").is_err());
        assert!(TableId::user("").is_err());
        assert!(TableId::user("a".repeat(TableId::MAX_LEN + 1)).is_err());
    }

    #[test]
    fn 系统表的slug拿掉下划线() {
        assert_eq!(TableId::system("_runs").unwrap().slug(), "sys-runs");
        assert_eq!(
            TableId::system("_flow_nodes").unwrap().slug(),
            "sys-flow-nodes"
        );
        assert_eq!(TableId::user("bugs").unwrap().slug(), "bugs");
    }

    #[test]
    fn 物理表名带项目() {
        let schema = schema();
        let physical = schema.physical().unwrap();
        assert!(physical.as_str().ends_with(".bugs"));
        assert!(physical.as_str().len() <= 64, "{}", physical.as_str().len());
    }

    #[test]
    fn 两个项目各建一张同名表互不相干() {
        let name = TableId::user("bugs").unwrap();
        let first = physical_name(Some(ProjectId::generate()), &name).unwrap();
        let second = physical_name(Some(ProjectId::generate()), &name).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn 加列可以改列不做() {
        let mut schema = schema();
        assert!(
            schema
                .add_column(Column::new("state", ColumnType::Bool, false).unwrap())
                .is_ok()
        );
        let error = schema
            .add_column(Column::new("state", ColumnType::Integer, false).unwrap())
            .unwrap_err();
        assert!(
            error.message().contains("新建一张表"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn 重复列名建不出来() {
        let duplicated = vec![
            Column::new("title", ColumnType::Bool, false).unwrap(),
            Column::new("title", ColumnType::Integer, false).unwrap(),
        ];
        assert!(
            TableSchema::new(
                Some(ProjectId::generate()),
                "acme",
                TableId::user("bugs").unwrap(),
                Kind::User,
                Protection::Normal,
                duplicated,
                Timestamp::from_millis(0),
            )
            .is_err()
        );
    }

    #[test]
    fn 保护级别有序因而降级判得出来() {
        assert!(Protection::Protected > Protection::Normal);
    }
}
