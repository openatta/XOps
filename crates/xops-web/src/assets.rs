//! 静态资源托管。**前端的构建产物随二进制发行**（D55），部署方不需要 Node。

use std::path::{Path, PathBuf};

use crate::server::Response;

/// 从哪个目录取前端产物。
#[derive(Debug, Clone)]
pub struct Assets {
    root: Option<PathBuf>,
}

impl Assets {
    /// 没有前端产物（只跑 API）。
    #[must_use]
    pub fn none() -> Self {
        Self { root: None }
    }

    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    /// 取一个文件。**SPA 的深链回落到 `index.html`**——路由在前端那一侧。
    #[must_use]
    pub fn serve(&self, method: &str, path: &str) -> Response {
        if method != "GET" {
            return Response {
                status: 404,
                content_type: "application/json; charset=utf-8",
                body: r#"{"error":"没有这个路径"}"#.as_bytes().to_vec(),
                set_session: None,
            };
        }
        let Some(root) = self.root.as_ref() else {
            return not_found();
        };
        // 路径穿越：只允许一段一段的普通名字。
        let relative = crate::routes::segments(path);
        if relative
            .iter()
            .any(|segment| *segment == ".." || segment.contains('\\'))
        {
            return not_found();
        }
        let mut candidate = root.clone();
        for segment in &relative {
            candidate.push(segment);
        }
        if candidate.is_file()
            && let Ok(body) = std::fs::read(&candidate)
        {
            return Response {
                status: 200,
                content_type: content_type(&candidate),
                body,
                set_session: None,
            };
        }
        // 深链回落。
        let index = root.join("index.html");
        if let Ok(body) = std::fs::read(&index) {
            return Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                body,
                set_session: None,
            };
        }
        not_found()
    }
}

fn not_found() -> Response {
    Response {
        status: 404,
        content_type: "application/json; charset=utf-8",
        body: r#"{"error":"没有这个路径"}"#.as_bytes().to_vec(),
        set_session: None,
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
