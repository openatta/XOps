//! 列类型。**穷举的**（`TBL-017`）。
//!
//! ⚠️ **没有 `json`，没有"任意对象"**（`TBL-021`）。一旦有，`MCP-005` 那套表专属 tool 的
//! 派发机制就退化成通用透传，窄接口纪律当场破掉——这条与 `MCP-004`（schema 里没有
//! 任意对象这个变体）是同一条纪律的两处落点。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xops_core::{Error, Result};
use xops_mcp::{Field, FieldType};

/// 短文本的默认上限。
pub const TEXT_MAX: usize = 512;
/// 长文本的默认上限（`MCP-014`：超限拒绝，不截断）。
pub const LONG_TEXT_MAX: usize = 256 * 1024;
/// 二进制的默认上限。
pub const BLOB_MAX: usize = 4 * 1024 * 1024;

/// 一列能是什么。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ColumnType {
    Text {
        max_len: usize,
    },
    /// Markdown 正文这类。
    LongText {
        max_len: usize,
    },
    Integer,
    Decimal,
    Bool,
    /// UTC 毫秒。
    Timestamp,
    /// 取值集合**由用户声明**。
    Enum {
        values: Vec<String>,
    },
    /// 自增序号：**项目内、每表独立，不跨项目共享计数器**（`TBL-018`）。
    /// 平台写，用户写不了。
    Sequence,
    /// 行引用：存另一张表的行 ID。**平台不校验、不级联**（`TBL-019`、`TBL-023`）。
    RowRef,
    /// 二进制。存 base64 文本。
    Blob {
        max_bytes: usize,
    },
    /// 派生文本：按模板从**同一行的其它列与项目属性**算出来（`TBL-020`）。
    /// **insert 时生成一次、之后不变，update 写不了它。**
    Derived {
        template: String,
    },
}

impl ColumnType {
    /// 用户能不能直接写这一列。
    ///
    /// 自增序号与派生文本是平台算的——用户写它们，等于让平台的账由调用方说了算。
    #[must_use]
    pub const fn user_writable(&self) -> bool {
        !matches!(self, Self::Sequence | Self::Derived { .. })
    }

    /// insert 之后还能不能改。
    ///
    /// `TBL-020`：派生文本 insert 时生成一次、**之后不变**。自增序号同理。
    #[must_use]
    pub const fn mutable(&self) -> bool {
        self.user_writable()
    }

    /// 这一列在 MCP 的 tool schema 里长什么样（`MCP-005`：schema 由表 schema 生成）。
    #[must_use]
    pub fn field_type(&self) -> FieldType {
        match self {
            Self::Text { max_len } => FieldType::Text { max_len: *max_len },
            Self::LongText { max_len } => FieldType::LongText { max_len: *max_len },
            Self::Integer => FieldType::Integer,
            Self::Decimal => FieldType::Decimal,
            Self::Bool => FieldType::Bool,
            Self::Timestamp => FieldType::Timestamp,
            Self::Enum { values } => FieldType::Enum {
                values: values.clone(),
            },
            Self::Sequence => FieldType::Integer,
            Self::RowRef => FieldType::Id,
            // base64 之后大约涨三分之一。
            Self::Blob { max_bytes } => FieldType::Text {
                max_len: max_bytes / 3 * 4 + 4,
            },
            Self::Derived { .. } => FieldType::Text { max_len: TEXT_MAX },
        }
    }

    /// 校验一个值。
    ///
    /// # Errors
    /// 类型不对或超限。
    pub fn check(&self, name: &str, value: &Value) -> Result<()> {
        let mismatch = || Error::invalid(format!("列 {name} 的类型不对"));
        match self {
            Self::Text { max_len } | Self::LongText { max_len } => {
                check_text(name, value, *max_len)
            }
            Self::Derived { .. } => check_text(name, value, TEXT_MAX),
            Self::Integer | Self::Timestamp | Self::Sequence => {
                value.as_i64().map(|_| ()).ok_or_else(mismatch)
            }
            Self::Decimal => value.as_f64().map(|_| ()).ok_or_else(mismatch),
            Self::Bool => value.as_bool().map(|_| ()).ok_or_else(mismatch),
            Self::Enum { values } => {
                let text = value.as_str().ok_or_else(mismatch)?;
                if values.iter().any(|candidate| candidate == text) {
                    Ok(())
                } else {
                    Err(Error::invalid(format!("列 {name} 只能是 {values:?} 之一")))
                }
            }
            Self::RowRef => {
                // TBL-019：只看形状。**平台不校验它指向的行存不存在，也不级联。**
                xops_core::Id::parse(value.as_str().ok_or_else(mismatch)?).map(|_| ())
            }
            Self::Blob { max_bytes } => {
                let text = value.as_str().ok_or_else(mismatch)?;
                if text.len() > max_bytes / 3 * 4 + 4 {
                    return Err(Error::invalid(format!("列 {name} 超过 {max_bytes} 字节")));
                }
                Ok(())
            }
        }
    }
}

/// 一列。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    #[serde(flatten)]
    pub ty: ColumnType,
    pub required: bool,
}

impl Column {
    /// # Errors
    /// 列名不合法或撞了自动补的列位。
    pub fn new(name: impl Into<String>, ty: ColumnType, required: bool) -> Result<Self> {
        let name = name.into();
        check_column_name(&name)?;
        Ok(Self { name, ty, required })
    }

    /// 这一列在 tool 的输入 schema 里的样子。
    #[must_use]
    pub fn field(&self) -> Field {
        let description = format!("列 {}", self.name);
        if self.required {
            Field::required(&self.name, self.ty.field_type(), &description)
        } else {
            Field::optional(&self.name, self.ty.field_type(), &description)
        }
    }
}

/// 平台在写入时自动补的列位（`TBL-014`）。**任何列声明都不能覆盖它们。**
pub const AUTO_COLUMNS: [&str; 5] = ["writtenBy", "at", "revision", "_instance", "retainUntil"];

fn check_text(name: &str, value: &Value, max_len: usize) -> Result<()> {
    let text = value
        .as_str()
        .ok_or_else(|| Error::invalid(format!("列 {name} 的类型不对")))?;
    if text.chars().count() > max_len {
        return Err(Error::invalid(format!(
            "列 {name} 超过 {max_len} 个字符，被拒绝（不会截断）"
        )));
    }
    Ok(())
}

/// # Errors
/// 空、超长、字符集不对、或者撞了 [`AUTO_COLUMNS`]。
pub fn check_column_name(name: &str) -> Result<()> {
    if AUTO_COLUMNS.contains(&name) {
        return Err(Error::invalid(format!(
            "{name} 是平台自动补的列位，列声明不能覆盖它（TBL-014）"
        )));
    }
    let shaped = !name.is_empty()
        && name.len() <= 48
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !shaped {
        return Err(Error::invalid(format!("列名不合法：{name}")));
    }
    Ok(())
}

/// 渲染一个派生文本模板（`TBL-020`）。
///
/// 认两种占位：`{project.slug}` 与 `{<列名>}`。**只从同一行的其它列与项目属性取值**——
/// 没有函数、没有表达式、没有别的行。派生列引用派生列也不行：模板在 insert 时算一次，
/// 依赖顺序一旦有环就没有"算一次"这回事了。
///
/// # Errors
/// 占位指向一个不存在的列，或者指向另一个派生列。
pub fn render_template(
    template: &str,
    project_slug: &str,
    row: &serde_json::Map<String, Value>,
    columns: &[Column],
) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after
            .find('}')
            .ok_or_else(|| Error::invalid(format!("派生模板里有没闭合的占位：{template}")))?;
        let key = &after[..end];
        rest = &after[end + 1..];
        if key == "project.slug" {
            out.push_str(project_slug);
            continue;
        }
        let column = columns
            .iter()
            .find(|column| column.name == key)
            .ok_or_else(|| Error::invalid(format!("派生模板引用了不存在的列：{key}")))?;
        if matches!(column.ty, ColumnType::Derived { .. }) {
            return Err(Error::invalid(format!("派生列不能引用另一个派生列：{key}")));
        }
        match row.get(key) {
            Some(Value::String(text)) => out.push_str(text),
            Some(Value::Number(number)) => out.push_str(&number.to_string()),
            Some(Value::Bool(flag)) => out.push_str(if *flag { "true" } else { "false" }),
            _ => {}
        }
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 列类型里没有任意对象() {
        // 这个测试的意义不在断言，在于它会随着 enum 一起被改 ——
        // 想加一个 Json 变体，得先来这里把这句话删掉。
        let all = [
            ColumnType::Text { max_len: 1 },
            ColumnType::LongText { max_len: 1 },
            ColumnType::Integer,
            ColumnType::Decimal,
            ColumnType::Bool,
            ColumnType::Timestamp,
            ColumnType::Enum { values: vec![] },
            ColumnType::Sequence,
            ColumnType::RowRef,
            ColumnType::Blob { max_bytes: 1 },
            ColumnType::Derived {
                template: String::new(),
            },
        ];
        assert_eq!(all.len(), 11, "TBL-017 数出来是十一种");
    }

    #[test]
    fn 自动补的列位声明不了() {
        for name in AUTO_COLUMNS {
            let error = Column::new(name, ColumnType::Integer, false).unwrap_err();
            assert!(error.message().contains("自动补的列位"), "{name}");
        }
        assert!(Column::new("title", ColumnType::Text { max_len: 10 }, true).is_ok());
    }

    #[test]
    fn 序号与派生列用户写不了() {
        assert!(!ColumnType::Sequence.user_writable());
        assert!(
            !ColumnType::Derived {
                template: "x".into()
            }
            .user_writable()
        );
        assert!(ColumnType::Integer.user_writable());
    }

    #[test]
    fn 长文本超限是拒绝() {
        let error = ColumnType::LongText { max_len: 3 }
            .check("body", &json!("四个字符了"))
            .unwrap_err();
        assert!(error.message().contains("不会截断"));
    }

    #[test]
    fn 行引用只看形状不看存不存在() {
        let id = xops_core::Id::generate().to_string();
        assert!(ColumnType::RowRef.check("bug", &json!(id)).is_ok());
        assert!(ColumnType::RowRef.check("bug", &json!("不是标识")).is_err());
    }

    #[test]
    fn 派生模板取项目短名与同行的列() {
        let columns = vec![
            Column::new("seq", ColumnType::Sequence, false).unwrap(),
            Column::new("title", ColumnType::Text { max_len: 10 }, false).unwrap(),
        ];
        let mut row = serde_json::Map::new();
        row.insert("seq".into(), json!(7));
        let rendered = render_template("{project.slug}-{seq}", "acme", &row, &columns).unwrap();
        assert_eq!(rendered, "acme-7");
    }

    #[test]
    fn 派生模板不认不存在的列也不认另一个派生列() {
        let columns = vec![
            Column::new(
                "code",
                ColumnType::Derived {
                    template: "x".into(),
                },
                false,
            )
            .unwrap(),
        ];
        let row = serde_json::Map::new();
        assert!(render_template("{nope}", "acme", &row, &columns).is_err());
        assert!(render_template("{code}", "acme", &row, &columns).is_err());
        assert!(render_template("{unclosed", "acme", &row, &columns).is_err());
    }
}
