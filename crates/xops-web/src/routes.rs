//! 路由表。**它是一份可枚举的常量，而不是散在代码里的一堆 match 分支。**
//!
//! `BRD-005` 的第 ① 道是这么说的：
//!
//! > **后端不存在写路由**——不是"有但不给 Web 用"，是不存在。这道是**结构性**的：
//! > 前端就算想写也没有地方可发。
//!
//! 而 RP-05 的验收标准接着说：**这条要用路由表枚举来证明，不是靠代码审查。**
//! 所以路由先是数据，再是行为——`tests/no_write_routes.rs` 枚举的就是这张表。

/// 一条路由是干什么的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 只读。
    Read,
    /// **存活探针**：不认证、不读任何数据、什么也不泄露。
    ///
    /// ⚠️ 它**不是 `MCP-013` 的第五个例外**——例外说的是"能写点什么的非 MCP 入口"，
    /// 而这条连读都不读：它只回答"这个进程还在不在"。
    Health,
    /// **凭据类**：建立或销毁会话。`MCP-013` 认下的四个例外之一，
    /// **不写任何项目内的业务对象**。
    Credential,
    /// 静态资源。
    Asset,
    /// **Git webhook 端点**：`MCP-013` 认下的四个例外之一（`TRG-011`）。
    /// 它只能做一件事——产生一个 git 事件，**不能创建或修改任何对象**。
    Webhook,
}

/// 一条路由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub method: &'static str,
    /// 路径模板。`{}` 是一段参数。
    pub path: &'static str,
    pub kind: Kind,
    /// 它会不会写项目内的业务对象。**全部是 `false`，这是这张表的全部意义。**
    pub writes_business_objects: bool,
    pub summary: &'static str,
}

/// 全部路由。**多一条写路由都不行。**
pub const ROUTES: [Route; 14] = [
    Route {
        method: "GET",
        path: "/healthz",
        kind: Kind::Health,
        writes_business_objects: false,
        summary: "存活探针。**不认证、不查库、回话里没有任何信息**——\
                  版本、项目数、连接数一律不给：探针是给编排器看的，不是给人探的",
    },
    Route {
        method: "GET",
        path: "/api/me",
        kind: Kind::Read,
        writes_business_objects: false,
        summary: "我是谁（BRD-011：明确展示当前用户身份）",
    },
    Route {
        method: "GET",
        path: "/api/projects",
        kind: Kind::Read,
        writes_business_objects: false,
        summary: "我参与的项目",
    },
    Route {
        method: "GET",
        path: "/api/me/notices",
        kind: Kind::Read,
        writes_business_objects: false,
        // ⚠️ **路径上没有 user 参数，这是刻意的。** NTF-010 说读写被硬限定为
        // `user = 令牌持有人`——落到这里就是调用方**表达不出**"看别人的"这个请求，
        // 不是"表达得出但被拒绝"。挂在 /api/me/ 下面，是为了让这条性质
        // **在路由表上就看得见**。
        summary: "个人看板：我的未读通知，跨项目一起排（NTF-001 / NTF-014）",
    },
    Route {
        method: "GET",
        path: "/api/projects/{}/members",
        kind: Kind::Read,
        writes_business_objects: false,
        summary: "项目成员与各自的角色（PRJ-007：角色是（项目，用户）上的记录）",
    },
    Route {
        method: "GET",
        path: "/api/projects/{}/tables",
        kind: Kind::Read,
        writes_business_objects: false,
        // ⚠️ 它回答的是"有哪些表"，**不是"表里有什么"**。一行数据都不回——
        // 要看行就去看板那条路（BRD-001）。
        summary: "项目里有哪些表、各自有哪些列。软删掉的不在里面（TBL-026）",
    },
    Route {
        method: "GET",
        path: "/api/projects/{}/boards",
        kind: Kind::Read,
        writes_business_objects: false,
        summary: "项目里的看板",
    },
    Route {
        method: "GET",
        path: "/api/projects/{}/boards/{}",
        kind: Kind::Read,
        writes_business_objects: false,
        summary: "一个看板的视图",
    },
    Route {
        method: "GET",
        path: "/api/projects/{}/tables/{}/rows/{}/history",
        kind: Kind::Read,
        writes_business_objects: false,
        summary: "单行历史（BRD-006 的前一半）",
    },
    Route {
        method: "GET",
        path: "/api/projects/{}/tables/{}/instances/{}/settlements",
        kind: Kind::Read,
        writes_business_objects: false,
        summary: "同实例的结算行（BRD-006 的后一半）。**与上一条分开查，后端不做 join**",
    },
    Route {
        method: "GET",
        path: "/api/projects/{}/tables/{}/rows/{}/columns/{}/raw",
        kind: Kind::Read,
        writes_business_objects: false,
        summary: "长文本原文（BRD-010：供不信任渲染的人自行查看）",
    },
    Route {
        method: "POST",
        path: "/session",
        kind: Kind::Credential,
        writes_business_objects: false,
        summary: "登录。**凭据类例外**：只建立会话，不创建或修改任何业务对象",
    },
    Route {
        method: "DELETE",
        path: "/session",
        kind: Kind::Credential,
        writes_business_objects: false,
        summary: "注销。同上",
    },
    Route {
        method: "POST",
        path: "/webhooks/git",
        kind: Kind::Webhook,
        writes_business_objects: false,
        summary: "Git webhook。**只能产生一个 git 事件**——验签、按投递标识幂等、                  立刻返回，端点内不做任何拉取或执行（TRG-011～TRG-014）",
    },
];

/// 把请求路径按 `/` 切成段。
#[must_use]
pub fn segments(path: &str) -> Vec<&str> {
    path.trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// 这条请求命中哪条路由，以及路径里的那几段参数。
#[must_use]
pub fn match_route(method: &str, path: &str) -> Option<(&'static Route, Vec<String>)> {
    let actual = segments(path);
    ROUTES.iter().find_map(|route| {
        if route.method != method {
            return None;
        }
        let template = segments(route.path);
        if template.len() != actual.len() {
            return None;
        }
        let mut captured = Vec::new();
        for (expected, found) in template.iter().zip(actual.iter()) {
            if *expected == "{}" {
                captured.push((*found).to_owned());
            } else if expected != found {
                return None;
            }
        }
        Some((route, captured))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 没有任何一条路由会写业务对象() {
        for route in ROUTES {
            assert!(
                !route.writes_business_objects,
                "{} {} 会写业务对象的话，G2 的第 ① 道就没了",
                route.method, route.path
            );
        }
    }

    #[test]
    fn 非get的路由只有凭据面与webhook() {
        let non_read: Vec<&str> = ROUTES
            .iter()
            .filter(|route| route.method != "GET")
            .map(|route| route.path)
            .collect();
        assert_eq!(non_read, vec!["/session", "/session", "/webhooks/git"]);
        // 三条都是 MCP-013 认下的例外，**没有一条写业务对象**。
        assert!(
            ROUTES
                .iter()
                .filter(|route| route.method != "GET")
                .all(
                    |route| matches!(route.kind, Kind::Credential | Kind::Webhook)
                        && !route.writes_business_objects
                )
        );
    }

    #[test]
    fn 路由匹配认得出参数() {
        let (route, captured) = match_route("GET", "/api/projects/P1/boards/B2").expect("该命中");
        assert_eq!(route.path, "/api/projects/{}/boards/{}");
        assert_eq!(captured, vec!["P1", "B2"]);
    }

    #[test]
    fn 不认识的路径与方法都不命中() {
        assert!(match_route("GET", "/api/nope").is_none());
        assert!(
            match_route("POST", "/api/me").is_none(),
            "只读面上没有 POST"
        );
        assert!(match_route("PUT", "/session").is_none());
    }
}
