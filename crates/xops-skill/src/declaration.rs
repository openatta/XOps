//! 技能的声明（`SKL-007`）。
//!
//! **四样，穷举的**：输入契约 · 产出形态 · 是否需要读代码仓 + 出网白名单 · 预计时长上限。
//!
//! ⚠️ `I-I`：**未声明的一律不提供。** 这个类型里没有"其它能力"这一栏，
//! 所以"声明之外还有第五条获取能力的途径"这件事，得先改这个结构才做得到。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Result};

/// 输入参数的类型。**可机读**（`SKL-007`）——不是一段说明文字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputType {
    Text,
    Integer,
    Bool,
    /// 一个 XOps 标识（行、项目、执行这些）。
    Id,
}

/// 一个输入参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: InputType,
    pub required: bool,
    pub description: String,
}

/// 产出长什么样。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputShape {
    /// 一份 Markdown 报告。
    Report,
    /// 往声明的那张表写行。
    Rows,
    /// 一段 JS 源码（生成插件的那种技能，`PLG-005`）。
    PluginSource,
}

/// 一份声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    /// 输入契约。
    pub inputs: Vec<Input>,
    /// 产出形态。
    pub output: OutputShape,
    /// 要不要读代码仓。**不需要的技能连工作区都不会被备**（`RPO-008` 那一侧）。
    pub needs_repository: bool,
    /// 出网白名单。**空表示不出网**——默认拒绝（`EXE-007`）。
    pub network: Vec<String>,
    /// 预计时长上限，毫秒。
    pub max_duration_millis: u64,
}

impl Declaration {
    /// # Errors
    /// 参数名重复或不合法 · 时长上限为 0 · 白名单里有空串。
    pub fn check(&self) -> Result<()> {
        let mut seen = std::collections::BTreeSet::new();
        for input in &self.inputs {
            if input.name.is_empty() || input.name.len() > 48 {
                return Err(Error::invalid(format!("参数名不合法：{}", input.name)));
            }
            if !seen.insert(input.name.as_str()) {
                return Err(Error::invalid(format!("参数 {} 声明了两次", input.name)));
            }
        }
        if self.max_duration_millis == 0 {
            return Err(Error::invalid("时长上限不能是 0"));
        }
        if self.network.iter().any(|host| host.trim().is_empty()) {
            return Err(Error::invalid("出网白名单里不能有空串"));
        }
        Ok(())
    }

    /// 校验一次调用给的输入。
    ///
    /// # Errors
    /// 少了必填的，或者多了没声明的。**多的那一条与 `MCP-003` 是同一条纪律。**
    pub fn check_arguments(&self, arguments: &serde_json::Value) -> Result<()> {
        let object = arguments
            .as_object()
            .ok_or_else(|| Error::invalid("技能输入必须是一个对象"))?;
        for name in object.keys() {
            if !self.inputs.iter().any(|input| input.name == *name) {
                return Err(Error::invalid(format!("技能没有声明参数 {name}")));
            }
        }
        for input in &self.inputs {
            let value = object.get(&input.name);
            match value {
                None | Some(serde_json::Value::Null) if input.required => {
                    return Err(Error::invalid(format!("缺少必填参数 {}", input.name)));
                }
                None | Some(serde_json::Value::Null) => {}
                Some(value) => {
                    let ok = match input.ty {
                        InputType::Text => value.is_string(),
                        InputType::Integer => value.is_i64(),
                        InputType::Bool => value.is_boolean(),
                        InputType::Id => value
                            .as_str()
                            .is_some_and(|text| xops_core::Id::parse(text).is_ok()),
                    };
                    if !ok {
                        return Err(Error::invalid(format!("参数 {} 类型不对", input.name)));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn declaration() -> Declaration {
        Declaration {
            inputs: vec![
                Input {
                    name: "target".into(),
                    ty: InputType::Text,
                    required: true,
                    description: "看哪儿".into(),
                },
                Input {
                    name: "深度".into(),
                    ty: InputType::Integer,
                    required: false,
                    description: "看多深".into(),
                },
            ],
            output: OutputShape::Report,
            needs_repository: true,
            network: vec![],
            max_duration_millis: 60_000,
        }
    }

    #[test]
    fn 声明就这四样() {
        let value = serde_json::to_value(declaration()).unwrap();
        let keys: std::collections::BTreeSet<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "inputs",
                "max_duration_millis",
                "needs_repository",
                "network",
                "output"
            ]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
            "多一栏就是多一条获取能力的途径（I-I）"
        );
    }

    #[test]
    fn 默认不出网() {
        assert!(declaration().network.is_empty());
    }

    #[test]
    fn 重复参数名与零时长都不收() {
        let mut broken = declaration();
        broken.inputs.push(broken.inputs[0].clone());
        assert!(broken.check().is_err());
        let mut broken = declaration();
        broken.max_duration_millis = 0;
        assert!(broken.check().is_err());
    }

    #[test]
    fn 输入契约是可机读的所以校验得了() {
        let declaration = declaration();
        assert!(
            declaration
                .check_arguments(&json!({"target": "src/"}))
                .is_ok()
        );
        assert!(
            declaration
                .check_arguments(&json!({"target": "src/", "深度": 3}))
                .is_ok()
        );
        assert!(
            declaration.check_arguments(&json!({})).is_err(),
            "少了必填的"
        );
        assert!(
            declaration.check_arguments(&json!({"target": 1})).is_err(),
            "类型不对"
        );
        assert!(
            declaration
                .check_arguments(&json!({"target": "x", "别的": 1}))
                .is_err(),
            "没声明的参数不收 —— 与 MCP-003 是同一条纪律"
        );
    }
}
