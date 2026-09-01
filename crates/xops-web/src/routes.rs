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
    /// **凭据类**：建立或销毁会话。`MCP-013` 认下的四个例外之一，
    /// **不写任何项目内的业务对象**。
    Credential,
    /// 静态资源。
    Asset,
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
pub const ROUTES: [Route; 9] = [
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
    fn 非get的路由只有凭据面那两条() {
        let non_read: Vec<&str> = ROUTES
            .iter()
            .filter(|route| route.method != "GET")
            .map(|route| route.path)
            .collect();
        assert_eq!(non_read, vec!["/session", "/session"]);
        assert!(
            ROUTES
                .iter()
                .filter(|route| route.method != "GET")
                .all(|route| route.kind == Kind::Credential)
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
