//! tool 的输入 schema。
//!
//! `MCP-004` 是这个文件的全部理由：**输入 schema 必须是固定形状的窄接口，
//! 不存在接受任意结构、任意上下文或自由指令的通用 tool。** 它不是靠评审守住的——
//! 这里根本没有"任意对象"这个变体，想写一个通用透传 tool，得先改这个 enum。
//!
//! `MCP-003`：未在 schema 中声明的字段**一律拒绝，不静默丢弃**。静默丢弃会让调用方
//! 以为自己传的东西生效了。

use serde_json::{Map, Value, json};
use xops_core::{Error, Result};

/// 一个字段能是什么。
///
/// ⚠️ **没有 `Object`，没有 `Any`，没有 `Json`。** 需要嵌套结构就把它拆成几个字段，
/// 或者拆成两个 tool。这条与 `TBL-021`（不提供 json 列类型）是同一条纪律的两处落点。
#[derive(Debug, Clone, PartialEq)]
pub enum FieldType {
    /// 短文本。有长度上限。
    Text {
        max_len: usize,
    },
    /// 长文本（Markdown 正文这类）。`MCP-014`：**超限拒绝，不截断**。
    LongText {
        max_len: usize,
    },
    Integer,
    Decimal,
    Bool,
    /// UTC 毫秒。
    Timestamp,
    /// 取值集合由声明方给死。
    Enum {
        values: Vec<String>,
    },
    /// 一个 26 字符的 XOps 标识。
    Id,
    /// 同一种标量的列表，**元素类型也必须是标量**。
    List {
        of: Box<FieldType>,
        max_len: usize,
    },
}

impl FieldType {
    fn json_type(&self) -> Value {
        match self {
            Self::Text { max_len } => json!({"type": "string", "maxLength": max_len}),
            Self::LongText { max_len } => json!({"type": "string", "maxLength": max_len}),
            Self::Integer => json!({"type": "integer"}),
            Self::Decimal => json!({"type": "number"}),
            Self::Bool => json!({"type": "boolean"}),
            Self::Timestamp => json!({"type": "integer", "description": "UTC 毫秒"}),
            Self::Enum { values } => json!({"type": "string", "enum": values}),
            Self::Id => json!({"type": "string", "minLength": 26, "maxLength": 26}),
            Self::List { of, max_len } => {
                json!({"type": "array", "items": of.json_type(), "maxItems": max_len})
            }
        }
    }

    fn check(&self, name: &str, value: &Value) -> Result<()> {
        let mismatch = || Error::invalid(format!("字段 {name} 的类型不对"));
        match self {
            Self::Text { max_len } | Self::LongText { max_len } => {
                let text = value.as_str().ok_or_else(mismatch)?;
                if text.chars().count() > *max_len {
                    // MCP-014：超限拒绝而不是截断 —— 截断会让调用方以为写进去的是完整内容。
                    return Err(Error::invalid(format!(
                        "字段 {name} 超过 {max_len} 个字符，被拒绝（不会截断）"
                    )));
                }
                Ok(())
            }
            Self::Integer | Self::Timestamp => value.as_i64().map(|_| ()).ok_or_else(mismatch),
            Self::Decimal => value.as_f64().map(|_| ()).ok_or_else(mismatch),
            Self::Bool => value.as_bool().map(|_| ()).ok_or_else(mismatch),
            Self::Enum { values } => {
                let text = value.as_str().ok_or_else(mismatch)?;
                if values.iter().any(|candidate| candidate == text) {
                    Ok(())
                } else {
                    Err(Error::invalid(format!(
                        "字段 {name} 只能是 {values:?} 之一"
                    )))
                }
            }
            Self::Id => {
                let text = value.as_str().ok_or_else(mismatch)?;
                xops_core::Id::parse(text).map(|_| ())
            }
            Self::List { of, max_len } => {
                let items = value.as_array().ok_or_else(mismatch)?;
                if items.len() > *max_len {
                    return Err(Error::invalid(format!("字段 {name} 最多 {max_len} 项")));
                }
                for (index, item) in items.iter().enumerate() {
                    of.check(&format!("{name}[{index}]"), item)?;
                }
                Ok(())
            }
        }
    }
}

/// 一个字段。
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: FieldType,
    pub required: bool,
    pub description: String,
}

impl Field {
    #[must_use]
    pub fn required(name: &str, ty: FieldType, description: &str) -> Self {
        Self {
            name: name.to_owned(),
            ty,
            required: true,
            description: description.to_owned(),
        }
    }

    #[must_use]
    pub fn optional(name: &str, ty: FieldType, description: &str) -> Self {
        Self {
            required: false,
            ..Self::required(name, ty, description)
        }
    }
}

/// 一个 tool 的输入形状。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Schema {
    fields: Vec<Field>,
}

impl Schema {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn field(mut self, field: Field) -> Self {
        self.fields.push(field);
        self
    }

    #[must_use]
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// 校验一份参数。
    ///
    /// # Errors
    /// 多了没声明的字段 · 少了必填字段 · 某个字段类型不对或超限。
    pub fn validate(&self, args: &Value) -> Result<()> {
        let object = args
            .as_object()
            .ok_or_else(|| Error::invalid("参数必须是一个对象"))?;

        // MCP-003：未声明的字段一律拒绝。这一条要在别的检查之前 ——
        // 先告诉调用方"你传的这个东西根本没生效"，比先告诉他别的更有用。
        for name in object.keys() {
            if !self.fields.iter().any(|field| field.name == *name) {
                return Err(Error::invalid(format!(
                    "字段 {name} 不在这个 tool 的 schema 里，被拒绝（不会静默丢弃）"
                )));
            }
        }
        for field in &self.fields {
            match object.get(&field.name) {
                None | Some(Value::Null) if field.required => {
                    return Err(Error::invalid(format!("缺少必填字段 {}", field.name)));
                }
                None | Some(Value::Null) => {}
                Some(value) => field.ty.check(&field.name, value)?,
            }
        }
        Ok(())
    }

    /// 渲染成 JSON Schema 2020-12——MCP 的 `tools/list` 就用它，
    /// 契约基线的方言文件也是它（`api:mcp.tool.*`）。
    #[must_use]
    pub fn to_json_schema(&self) -> Value {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for field in &self.fields {
            let mut entry = field.ty.json_type();
            if let Some(object) = entry.as_object_mut() {
                object.insert("description".into(), json!(field.description));
            }
            properties.insert(field.name.clone(), entry);
            if field.required {
                required.push(json!(field.name));
            }
        }
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": Value::Object(properties),
            "required": required,
            // MCP-003 在协议层的兑现：多一个字段就是不合 schema，客户端自己就能看出来。
            "additionalProperties": false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Schema {
        Schema::new()
            .field(Field::required(
                "slug",
                FieldType::Text { max_len: 24 },
                "短名",
            ))
            .field(Field::optional(
                "note",
                FieldType::LongText { max_len: 8 },
                "备注",
            ))
    }

    #[test]
    fn 未声明的字段被拒绝而不是静默丢弃() {
        let error = schema()
            .validate(&json!({"slug": "acme", "actor": "root"}))
            .unwrap_err();
        assert!(error.message().contains("actor"), "{}", error.message());
        assert!(error.message().contains("不会静默丢弃"));
    }

    #[test]
    fn 必填字段少不得() {
        assert!(schema().validate(&json!({})).is_err());
        assert!(
            schema().validate(&json!({"slug": null})).is_err(),
            "显式 null 也算没给"
        );
        assert!(schema().validate(&json!({"slug": "acme"})).is_ok());
    }

    #[test]
    fn 长文本超限是拒绝不是截断() {
        let error = schema()
            .validate(&json!({"slug": "acme", "note": "九个字符整整好"}))
            .err();
        assert!(error.is_none(), "七个字符在上限内");
        let error = schema()
            .validate(&json!({"slug": "acme", "note": "这一句显然超过八个字符了吧"}))
            .unwrap_err();
        assert!(error.message().contains("不会截断"), "{}", error.message());
    }

    #[test]
    fn 类型对不上就拒() {
        assert!(schema().validate(&json!({"slug": 1})).is_err());
        assert!(schema().validate(&json!("不是对象")).is_err());
    }

    #[test]
    fn 枚举只认声明过的值() {
        let schema = Schema::new().field(Field::required(
            "role",
            FieldType::Enum {
                values: vec!["owner".into(), "member".into()],
            },
            "角色",
        ));
        assert!(schema.validate(&json!({"role": "owner"})).is_ok());
        assert!(schema.validate(&json!({"role": "admin"})).is_err());
    }

    #[test]
    fn 标识要真的是标识() {
        let schema = Schema::new().field(Field::required("project", FieldType::Id, "项目"));
        assert!(
            schema
                .validate(&json!({"project": xops_core::Id::generate().to_string()}))
                .is_ok()
        );
        assert!(schema.validate(&json!({"project": "不是标识"})).is_err());
    }

    #[test]
    fn 列表的元素也要合形状() {
        let schema = Schema::new().field(Field::required(
            "tags",
            FieldType::List {
                of: Box::new(FieldType::Text { max_len: 4 }),
                max_len: 2,
            },
            "标签",
        ));
        assert!(schema.validate(&json!({"tags": ["a", "bb"]})).is_ok());
        assert!(
            schema.validate(&json!({"tags": ["a", "b", "c"]})).is_err(),
            "超个数"
        );
        assert!(
            schema.validate(&json!({"tags": ["太长了一点点"]})).is_err(),
            "元素超长"
        );
        assert!(
            schema.validate(&json!({"tags": [1]})).is_err(),
            "元素类型不对"
        );
    }

    #[test]
    fn 渲染出来的jsonschema关着任意字段() {
        let rendered = schema().to_json_schema();
        assert_eq!(
            rendered["additionalProperties"],
            json!(false),
            "MCP-003 在协议层的兑现"
        );
        assert_eq!(rendered["required"], json!(["slug"]));
        assert_eq!(rendered["properties"]["note"]["maxLength"], json!(8));
    }
}
