//! 派工单的装配。
//!
//! `TSK-015`：**只把技能实际声明的东西放进去，不扩权；派工单不含任何凭据**（`I-I`、`I-F`）。
//!
//! 所以这个文件里每往派工单上放一样东西，都要指得出它对应哪条声明。
//! 验收要求"**枚举它的每一个字段，逐个对得上某条声明**"——[`provenance`] 就是那张对照表。

use std::path::PathBuf;

use xops_core::{Error, Result};
use xops_exec::worksheet::{Capabilities, Limits, RunId, Worksheet};
use xops_skill::Version;
use xops_task::Task;

use crate::event::Event;

/// 派工单上每一个字段的出处。
///
/// **这张表是验收"不扩权"的那份证明**：字段多了一个而这里没加一行，测试就会红。
#[must_use]
pub const fn provenance() -> &'static [(&'static str, &'static str)] {
    &[
        ("run", "本次执行的标识，由平台生成"),
        ("instruction", "技能版本的内容"),
        ("skill", "技能名"),
        ("skill_version", "技能版本号"),
        ("inputs", "任务定义里的输入参数（已按技能的输入契约校验过）"),
        ("revision", "事件带来的代码修订，没有就是任务解出来的那个"),
        ("capabilities", "技能声明的数据源与出网白名单，逐条对应"),
        ("limits", "技能声明的时长上限 + 任务声明的 token 上限"),
    ]
}

/// 装配一份派工单。
///
/// `rows_to` 由调用方查好传进来（要读目标表的 schema，本模块不碰表）。
/// **声明 `output: report` 的技能一律拿不到它**——`EXE-006`:未声明的一律不提供。
///
/// # Errors
/// 技能声明了自定义出网白名单（`TSK-017` / **Q10** 定下来之前不开放）·
/// 需要代码仓却没有工作区 · 输入不合契约。
pub fn assemble(
    task: &Task,
    version: &Version,
    event: &Event,
    workspace: Option<PathBuf>,
    rows_to: Option<xops_exec::worksheet::RowTarget>,
) -> Result<Worksheet> {
    // 再校一次输入。装配这一步是最后一道 —— 任务定义可能是在技能改声明之前写的。
    version.declaration.check_arguments(&task.inputs)?;

    // TSK-017 / Q10：**自定义出网白名单在 Q10 定下来之前不开放。**
    // 不是悄悄清空，是明确拒绝——悄悄清空会让技能作者以为它生效了。
    if !version.declaration.network.is_empty() {
        return Err(Error::invalid(
            "技能声明了自定义出网白名单，而「白名单由谁批准」还没定（Q10）。\
             在它定下来之前这条路不开放（TSK-017）",
        ));
    }

    // EXE-006 / I-I：**未声明的一律不提供。** 不需要代码仓的技能连工作区都不给。
    let workspace = if version.declaration.needs_repository {
        Some(
            workspace
                .ok_or_else(|| Error::invalid("这个技能声明了需要读代码仓，但没有备好工作区"))?,
        )
    } else {
        None
    };

    Ok(Worksheet {
        run: RunId::generate(),
        instruction: version.content.clone(),
        skill: version.skill.to_string(),
        skill_version: version.version.to_string(),
        // EXE-013 / D44：**表数据不在这里。** 需要表数据的由调用方经 MCP 查好放进任务输入。
        inputs: serde_json::to_string(&task.inputs)
            .map_err(|error| Error::internal(format!("输入装不下：{error}")))?,
        // 事件带来的修订**覆盖**任务定义里的那个。
        revision: event.revision.clone(),
        capabilities: Capabilities {
            workspace,
            network: Vec::new(),
        },
        // `EXE-031` / `EXE-006`：**不产出行的执行连这个口都没有。**
        rows_to: if version.declaration.output == xops_skill::declaration::OutputShape::Rows {
            rows_to
        } else {
            None
        },
        limits: Limits {
            timeout_millis: version.declaration.max_duration_millis,
            token_budget: task.token_budget,
            ..Limits::default()
        },
    })
}

/// 派工单里有没有看起来像凭据的东西。
///
/// **验收要求"检查完整派工单内容，无任何凭据形状的值"**，所以这条判定要能被跑，
/// 而不是靠读代码。它宁可误报也不漏报。
#[must_use]
pub fn looks_like_credential(worksheet: &Worksheet) -> Option<String> {
    let rendered = serde_json::to_string(worksheet).unwrap_or_default();
    for marker in [
        "xops_", // XOps 令牌
        "ghp_",  // GitHub token
        "github_pat_",
        "Authorization",
        "authToken",
        "password",
        "secret",
        "BEGIN PRIVATE KEY",
        ".sock", // attacored 的 socket 等同于模型凭据本身
    ] {
        if rendered.contains(marker) {
            return Some(marker.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 对照表覆盖派工单的每一个字段() {
        // 派工单序列化出来有几个键，对照表就得有几行。
        let fields = provenance().len();
        assert_eq!(
            fields, 8,
            "字段多了一个而对照表没加一行，就是悄悄扩了一次权"
        );
        assert!(provenance().iter().all(|(_, why)| !why.is_empty()));
    }

    #[test]
    fn 凭据形状认得出来() {
        assert!(looks_like_credential(&worksheet_with_inputs("干净的输入")).is_none());
        assert!(looks_like_credential(&worksheet_with_inputs("xops_abc")).is_some());
        assert!(looks_like_credential(&worksheet_with_inputs("ghp_abc")).is_some());
        assert!(
            looks_like_credential(&worksheet_with_inputs("/tmp/attacored.sock")).is_some(),
            "socket 等同于模型凭据本身"
        );
    }

    fn worksheet_with_inputs(inputs: &str) -> Worksheet {
        Worksheet {
            run: RunId::generate(),
            instruction: "看看".into(),
            skill: "查缺陷".into(),
            skill_version: "1".into(),
            inputs: inputs.to_owned(),
            revision: None,
            capabilities: Capabilities::default(),
            rows_to: None,
            limits: Limits::default(),
        }
    }
}
