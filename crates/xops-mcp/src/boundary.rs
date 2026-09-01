//! 四个非 MCP 入口的清单与边界（`MCP-013`）。
//!
//! **不存在绕过 MCP 的业务写入路径。** 已声明的例外只有四个，且每一个都被削得
//! 只剩一件事可做。把它们写成一份可枚举的常量，是为了让"再加一个入口"这件事
//! 必须先改这里——而改这里在评审时是看得见的。

/// 一个被允许存在的非 MCP 入口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exception {
    /// 谁提供它。
    pub owner: &'static str,
    /// 它是什么。
    pub entrypoint: &'static str,
    /// 它**只能做**的那一件事。
    pub only: &'static str,
    /// 它能不能写项目内的业务对象。**四个全是 `false`，这是这份清单的全部意义。**
    pub writes_business_objects: bool,
    /// 出处。
    pub requirement: &'static str,
}

/// 全部四个。**多一个都不行。**
pub const NON_MCP_ENTRYPOINTS: [Exception; 4] = [
    Exception {
        owner: "RP-05 xops-web",
        entrypoint: "OAuth 登录回调",
        only: "完成身份验证、建立会话",
        writes_business_objects: false,
        requirement: "IDN-007",
    },
    Exception {
        owner: "RP-13 xops-web",
        entrypoint: "Git webhook 端点",
        only: "产生一个 git 事件，让订阅了它的任务被触发",
        writes_business_objects: false,
        requirement: "TRG-011",
    },
    Exception {
        owner: "RP-05 xops-web",
        entrypoint: "会话面（登录与注销）",
        only: "凭据类：建立与销毁 Web 会话",
        writes_business_objects: false,
        requirement: "MCP-013",
    },
    Exception {
        owner: "RP-02 xops-identity",
        entrypoint: "令牌管理面",
        only: "凭据类：签发与撤销令牌",
        writes_business_objects: false,
        requirement: "MCP-013",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 恰好四个() {
        assert_eq!(NON_MCP_ENTRYPOINTS.len(), 4);
    }

    #[test]
    fn 没有一个能写业务对象() {
        for exception in NON_MCP_ENTRYPOINTS {
            assert!(
                !exception.writes_business_objects,
                "{} 能写业务对象的话，G1 就没了",
                exception.entrypoint
            );
        }
    }

    #[test]
    fn 每一个都说得出出处与它只能做的那件事() {
        for exception in NON_MCP_ENTRYPOINTS {
            assert!(!exception.only.is_empty(), "{}", exception.entrypoint);
            assert!(
                !exception.requirement.is_empty(),
                "{}",
                exception.entrypoint
            );
        }
    }
}
