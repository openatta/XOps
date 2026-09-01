//! 受保护的列（`FLW-022`、`FLW-023`、`FLW-036`、`I-P`）。
//!
//! 两组列，两个理由：
//!
//! ```text
//! _instance   技能与用户都不能自己写。没有它，两个并发的实例会在同一张表上
//!             产生两条同样满足筛选的行，节点判定无从区分 ——
//!             **这是整个流程模型的地基**（FLW-023）
//! 状态列       只有平台与流转插件能写。不这么做，任何成员都能直接
//!             `update bugs.status = closed` 绕过整条流程 ——
//!             七条判定只管"这行算不算结算"，**从不阻止写入**（FLW-036）
//! ```

use serde_json::Value;
use xops_core::{Error, Result};
use xops_table::WrittenBy;

/// `_instance` 这一列的名字。
pub const INSTANCE_COLUMN: &str = "_instance";

/// 谁在写。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// 人经 MCP 写的普通行。
    User,
    /// 一次执行写的行。
    Execution,
    /// 平台自己（含流转插件写回，由平台代写）。
    Platform,
}

impl Origin {
    #[must_use]
    pub fn of(written_by: &WrittenBy) -> Self {
        match written_by {
            WrittenBy::Person { .. } => Self::User,
            WrittenBy::Execution { .. } => Self::Execution,
            WrittenBy::Plugin { .. } | WrittenBy::Platform => Self::Platform,
        }
    }

    /// 能不能写状态列。**只有平台与流转插件能。**
    #[must_use]
    pub const fn may_write_status(self) -> bool {
        matches!(self, Self::Platform)
    }

    /// 能不能自己填 `_instance`。**技能与用户都不能。**
    #[must_use]
    pub const fn may_write_instance(self) -> bool {
        matches!(self, Self::Platform)
    }
}

/// 校验一次写有没有碰这条流程声明的受保护列。
///
/// **状态列的名单从流程定义来**（`FLW-036`）——它是流程声明的，不是表声明的：
/// 同一张表在不同流程里可以有不同的状态列。
///
/// # Errors
/// 碰了。
pub fn check_for(
    definition: &xops_flow::Definition,
    origin: Origin,
    values: &Value,
    platform_filled_instance: bool,
) -> Result<()> {
    check(
        origin,
        &definition.status_columns,
        values,
        platform_filled_instance,
    )
}

/// 校验一次写有没有碰受保护的列。
///
/// # Errors
/// 碰了。错误消息会说清它挡的是什么——那句话比"权限不足"有用得多。
pub fn check(
    origin: Origin,
    status_columns: &[String],
    values: &Value,
    platform_filled_instance: bool,
) -> Result<()> {
    let Some(object) = values.as_object() else {
        return Ok(());
    };

    if object.contains_key(INSTANCE_COLUMN)
        && !origin.may_write_instance()
        && !platform_filled_instance
    {
        return Err(Error::invalid(
            "_instance 是受保护列，技能与用户都不能自己写（I-P）。\
             没有它，两个并发的流程实例会在同一张表上产生两条同样满足筛选的行，\
             节点判定无从区分——**这是整个流程模型的地基**",
        ));
    }

    if !origin.may_write_status() {
        for column in status_columns {
            if object.contains_key(column) {
                return Err(Error::invalid(format!(
                    "{column} 是这条流程声明的状态列，只有平台与流转插件能写（FLW-036）。\
                     否则任何成员都能直接把它改成完成态，绕过整条流程——\
                     七条判定只管「这行算不算结算」，**从不阻止写入**"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 用户与执行都写不了instance() {
        let row = json!({"decision": "同意", "_instance": "某个实例"});
        for origin in [Origin::User, Origin::Execution] {
            let error = check(origin, &[], &row, false).unwrap_err();
            assert!(error.message().contains("整个流程模型的地基"), "{origin:?}");
        }
        assert!(check(Origin::Platform, &[], &row, false).is_ok());
        assert!(
            check(Origin::User, &[], &row, true).is_ok(),
            "平台代填的那次不算用户写"
        );
    }

    #[test]
    fn 状态列只有平台与流转插件能写() {
        let status = vec!["status".to_owned()];
        let row = json!({"status": "closed"});
        let error = check(Origin::User, &status, &row, false).unwrap_err();
        assert!(error.message().contains("绕过整条流程"));
        assert!(check(Origin::Platform, &status, &row, false).is_ok());
        // 别的列照写不误。
        assert!(check(Origin::User, &status, &json!({"title": "崩了"}), false).is_ok());
    }

    #[test]
    fn 插件写回算平台() {
        let plugin = WrittenBy::Plugin {
            plugin: "gate".into(),
            version: "1".into(),
            installed_by: xops_identity::UserId::generate(),
            instance: xops_core::Id::generate(),
        };
        assert_eq!(
            Origin::of(&plugin),
            Origin::Platform,
            "插件不写表，是平台代写"
        );
        assert!(Origin::of(&plugin).may_write_status());
    }

    #[test]
    fn 状态列的名单从流程定义来() {
        let mut definition = xops_flow::Definition {
            flow: xops_flow::FlowId::generate(),
            project: xops_identity::ProjectId::generate(),
            version: 1,
            name: "缺陷流转".into(),
            settlement_table: xops_table::table::TableId::user("bug-events").unwrap(),
            subject_table: Some(xops_table::table::TableId::user("bugs").unwrap()),
            start: xops_flow::definition::Start::Automatic,
            status_columns: vec!["status".into()],
            steps: vec![],
            state: xops_flow::definition::State::Published,
            created_by: xops_identity::UserId::generate(),
            created_at: xops_core::Timestamp::from_millis(0),
        };
        let write = json!({"status": "已关闭"});
        assert!(
            check_for(&definition, Origin::User, &write, false).is_err(),
            "FLW-036：任何成员都能直接改状态的话，整条流程就白搭了"
        );
        assert!(check_for(&definition, Origin::Platform, &write, false).is_ok());

        // **同一张表在别的流程里可以不是状态列** —— 名单是流程声明的。
        definition.status_columns.clear();
        assert!(check_for(&definition, Origin::User, &write, false).is_ok());
    }
}
