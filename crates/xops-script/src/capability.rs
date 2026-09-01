//! 能力声明（`PLG-012`）。
//!
//! > **能力默认为零，未声明即没有**——不是"调用时被拒绝"，是**那个函数不存在**（`I-Z`）。
//!
//! **流转插件没有可声明项**（`PLG-002`）：没有文件、没有网络、没有表、没有时钟
//! （除 `Date`）、没有任何宿主绑定。
//!
//! **输出插件只能声明三样，仅此三样**：出网白名单 · 读自己的配置 · 读声明的那几张表。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Result};
use xops_table::TableId;

/// 插件装在哪个位置调用（`PLG-001`）。**不存在第三个调用位置。**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    /// 流程节点求值时。**能力为零，没有可声明项。**
    Transition,
    /// 任务 onComplete 时。
    Output,
}

/// 一份能力声明。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// ① 出网主机白名单。**未声明的主机连不上**；
    /// **重定向每一跳重新判定**——否则白名单只对第一跳成立。
    pub network: Vec<String>,
    /// ② 读自己的配置。**只能读到它自己那一份**，读不到别的插件的、读不到平台的。
    pub own_config: bool,
    /// ③ 读表：声明的那几张、**本项目的**。
    pub tables: Vec<TableId>,
}

impl Capabilities {
    /// 一样都不声明。
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.network.is_empty() && !self.own_config && self.tables.is_empty()
    }

    /// 逐条披露。**安装时必须给人看这个，披露不可跳过**（`PLG-007`）。
    #[must_use]
    pub fn disclose(&self) -> Vec<String> {
        let mut out = Vec::new();
        for host in &self.network {
            out.push(format!("出网：{host}"));
        }
        if self.own_config {
            out.push("读它自己的配置（读不到别的插件的、读不到平台的）".to_owned());
        }
        for table in &self.tables {
            out.push(format!("读表：{table}"));
        }
        if out.is_empty() {
            out.push("什么都不声明——能力为零".to_owned());
        }
        out
    }

    /// 校验这份声明配不配这个位置。
    ///
    /// # Errors
    /// 流转插件声明了任何东西 · 声明读 `_notices` · 白名单里有空串。
    pub fn check(&self, position: Position) -> Result<()> {
        if position == Position::Transition && !self.is_empty() {
            return Err(Error::invalid(
                "流转插件没有可声明项（PLG-002）：没有文件、没有网络、没有表、\
                 没有时钟（除 Date）、没有任何宿主绑定",
            ));
        }
        if self.network.iter().any(|host| host.trim().is_empty()) {
            return Err(Error::invalid("出网白名单里不能有空串"));
        }
        for table in &self.tables {
            if table.as_str() == xops_table::system::NOTICES {
                return Err(Error::invalid(
                    "_notices 不在可声明之列（NTF-012）——它只有两个平台专属 tool",
                ));
            }
        }
        Ok(())
    }

    /// 允不允许连这个主机。**重定向的每一跳都要再问一次这个函数。**
    #[must_use]
    pub fn allows_host(&self, host: &str) -> bool {
        self.network.iter().any(|allowed| allowed == host)
    }

    /// 允不允许读这张表。
    #[must_use]
    pub fn allows_table(&self, table: &TableId) -> bool {
        table.as_str() != xops_table::system::NOTICES && self.tables.contains(table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 流转插件一样都不能声明() {
        let none = Capabilities::none();
        assert!(none.check(Position::Transition).is_ok());
        let some = Capabilities {
            own_config: true,
            ..Capabilities::none()
        };
        let error = some.check(Position::Transition).unwrap_err();
        assert!(error.message().contains("没有可声明项"));
        assert!(some.check(Position::Output).is_ok(), "输出插件可以");
    }

    #[test]
    fn 通知表不在可声明之列() {
        let capabilities = Capabilities {
            tables: vec![TableId::system("_notices").unwrap()],
            ..Capabilities::none()
        };
        assert!(capabilities.check(Position::Output).is_err(), "NTF-012");
        assert!(!capabilities.allows_table(&TableId::system("_notices").unwrap()));
    }

    #[test]
    fn 未声明的主机连不上() {
        let capabilities = Capabilities {
            network: vec!["api.example.com".into()],
            ..Capabilities::none()
        };
        assert!(capabilities.allows_host("api.example.com"));
        assert!(!capabilities.allows_host("evil.example"));
    }

    #[test]
    fn 披露逐条且不可为空() {
        assert_eq!(
            Capabilities::none().disclose(),
            vec!["什么都不声明——能力为零"]
        );
        let capabilities = Capabilities {
            network: vec!["a.example".into()],
            own_config: true,
            tables: vec![TableId::user("bugs").unwrap()],
        };
        assert_eq!(capabilities.disclose().len(), 3, "PLG-007：逐条披露");
    }
}
