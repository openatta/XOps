//! 技能把产出行交回来的那个入口（`EXE-031`）。
//!
//! # 为什么是一个工具，不是在正文里约定一段围栏
//!
//! 另一条路是让技能把行写进产出正文里的一个围栏块，平台再从自由文本里抠出来。
//! 那条路**是在赌模型会不会写对**:围栏漏了、JSON 写坏了、把例子当真数据写了下去，
//! 平台只能整批拒绝（`EXE-024`），而**模型没有任何机会知道自己写坏了**。
//!
//! 做成工具，schema 就替我们说话:参数形状是**按目标表的列生成的**，
//! 模型看见的是真的列名和类型，不是"希望它猜对"；写坏了当场收到一句错，
//! **在同一个回合里还能改**。
//!
//! 这与 MCP 那一面的纪律是同一条（`MCP-004`:逐字段声明，不收一整份 JSON）——
//! 向内没有理由降一档。
//!
//! # 它不是一条到 XOps 的路
//!
//! ⚠️ `EXE-004` / `I-F`:执行方**没有任何写表的路径**，也拿不到凭据。
//! 这个工具不联网、不认识 XOps、不碰数据库——**它只是把行攒在这次执行的内存里**，
//! 跑完随执行结果一起交回去。接容器后端那天，攒的位置从进程内存换成容器里的一处，
//! 由容器契约在收尾时带回宿主（`TSK-006` ②"收敛并移交容器里已产生的行"）。
//!
//! # 校验分两层，权威那一层不在这里
//!
//! 这里只做**形状**检查:是不是对象、列在不在声明里、带没带 `_instance`。
//! **那是给模型改的机会，不是判定。** 判定留在执行之外（`EXE-023`），
//! 走 `EXE-024` 的两层拒绝——把权威移进来等于让技能自己判自己。

use std::sync::Mutex;

use attacore_core::error::ToolError;
use attacore_core::tool::{ProgressSender, Tool, ToolContext, ToolResult};
use serde_json::{Value, json};

use crate::worksheet::RowTarget;

/// tool 名。**一个具名常量**——场景白名单要放行它，两处对不上是静默失效。
pub const NAME: &str = "EmitRow";

/// 一次执行最多交回多少行（`EXE-025`:产物有体量上限）。
pub const MAX_ROWS: usize = 500;

/// 攒着这次执行交回来的行。
pub struct EmitRow {
    target: RowTarget,
    rows: Mutex<Vec<Value>>,
}

impl std::fmt::Debug for EmitRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmitRow")
            .field("table", &self.target.table)
            .finish_non_exhaustive()
    }
}

impl EmitRow {
    #[must_use]
    pub fn new(target: RowTarget) -> Self {
        Self {
            target,
            rows: Mutex::new(Vec::new()),
        }
    }

    /// 把攒下的行取走。
    #[must_use]
    pub fn take(&self) -> Vec<Value> {
        std::mem::take(
            &mut *self
                .rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// 形状检查。**不是判定**——见模块注释。
    fn shape(&self, row: &Value) -> Result<(), String> {
        let Some(object) = row.as_object() else {
            return Err("一行要是一个对象".into());
        };
        if object.is_empty() {
            return Err("空行没有意义".into());
        }
        for name in object.keys() {
            // `I-P`：**技能与用户都不能自己写 `_instance`**，它由平台按流程上下文填。
            if name == "_instance" {
                return Err(
                    "不要自己带 _instance —— 它是受保护列，由平台按流程上下文填（I-P）".into(),
                );
            }
            if !self.target.columns.iter().any(|(column, _)| column == name) {
                let known: Vec<&str> = self
                    .target
                    .columns
                    .iter()
                    .map(|(column, _)| column.as_str())
                    .collect();
                return Err(format!(
                    "{} 上没有 {name} 这一列。有的是：{}",
                    self.target.table,
                    known.join(" · ")
                ));
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Tool for EmitRow {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        "交回一行产出。写进任务声明的那张表；一次一行，可以调多次。\
         这是把结果变成数据的唯一途径——写在正文里的表格不会进表。"
    }

    fn input_schema(&self) -> Value {
        // ⚠️ **按目标表的列生成**，不是一个自由对象。模型看见的是真的列名。
        let mut properties = serde_json::Map::new();
        for (name, kind) in &self.target.columns {
            properties.insert(
                name.clone(),
                json!({"type": "string", "description": format!("{kind}（按这一列的类型写）")}),
            );
        }
        json!({
            "type": "object",
            "properties": {
                "values": {
                    "type": "object",
                    "description": format!("这一行的内容，写进 {}", self.target.table),
                    "properties": properties,
                    "additionalProperties": false,
                },
            },
            "required": ["values"],
            "additionalProperties": false,
        })
    }

    fn is_read_only(&self, _: &Value) -> bool {
        false
    }

    async fn call(
        &self,
        input: Value,
        _context: ToolContext,
        _progress: ProgressSender,
    ) -> Result<ToolResult, ToolError> {
        let Some(values) = input.get("values") else {
            return Ok(ToolResult::error_text("缺少 values"));
        };
        if let Err(why) = self.shape(values) {
            // **回一句错，不是失败**：模型在同一个回合里还能改。
            return Ok(ToolResult::error_text(why));
        }
        let mut rows = self
            .rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if rows.len() >= MAX_ROWS {
            return Ok(ToolResult::error_text(format!(
                "一次执行最多交回 {MAX_ROWS} 行，已经到上限了（EXE-025）"
            )));
        }
        rows.push(values.clone());
        Ok(ToolResult::text(format!(
            "收下了，这是第 {} 行。**还没写进表**——落表在执行之外，schema 不过整批都不入表",
            rows.len()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> RowTarget {
        RowTarget {
            table: "findings".into(),
            columns: vec![
                ("severity".into(), "枚举：high / low".into()),
                ("note".into(), "文本".into()),
            ],
        }
    }

    #[test]
    fn 参数形状是按目标表的列生成的() {
        // 模型看见的该是真的列名 —— 不是一个自由对象。
        // 一批行里错一列，`EXE-024` 是**整批不入表**，代价太大了。
        let schema = EmitRow::new(target()).input_schema();
        let properties = &schema["properties"]["values"]["properties"];
        assert!(properties.get("severity").is_some());
        assert!(properties.get("note").is_some());
        assert_eq!(
            schema["properties"]["values"]["additionalProperties"],
            json!(false),
            "没声明的列要被拒，不能静默丢弃"
        );
    }

    #[test]
    fn 不在声明里的列当场被拒() {
        let tool = EmitRow::new(target());
        let error = tool
            .shape(&json!({"severity": "high", "谁加的": "x"}))
            .unwrap_err();
        assert!(error.contains("谁加的"), "要指出是哪一列：{error}");
        assert!(error.contains("severity"), "还要说有的是哪些：{error}");
    }

    #[test]
    fn 自己带instance一律拒() {
        // `I-P`：没有它，两个并发实例会在同一张表上产生两条同样满足筛选的行，
        // 节点判定无从区分 —— **这是整个流程模型的地基**。
        let tool = EmitRow::new(target());
        let error = tool
            .shape(&json!({"severity": "high", "_instance": "01ABC"}))
            .unwrap_err();
        assert!(error.contains("_instance"), "{error}");
    }

    #[tokio::test]
    async fn 收下的行取得回来而且取一次就空() {
        let tool = EmitRow::new(target());
        for note in ["一", "二"] {
            let result = tool
                .call(
                    json!({"values": {"severity": "low", "note": note}}),
                    ToolContext::for_test("/tmp".into()),
                    ProgressSender::noop("t"),
                )
                .await
                .unwrap();
            assert!(!result.is_error, "{result:?}");
        }
        assert_eq!(tool.take().len(), 2);
        assert!(tool.take().is_empty(), "取走就该空 —— 不然会重复落表");
    }
}
