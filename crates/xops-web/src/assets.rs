//! 静态资源托管。**前端的构建产物随二进制发行**（D55），部署方不需要 Node。

use std::path::{Path, PathBuf};

use crate::server::Response;

/// 编译期嵌进来的那一份，由 `build.rs` 生成。
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
}

/// 前端产物从哪来。
#[derive(Debug, Clone)]
pub enum Assets {
    /// 不带页面（只跑 API）。
    None,
    /// **编译期嵌进二进制**（D55）。发行形态是这个。
    Embedded,
    /// 运行时从一个目录读。开发时用它——改一行页面不用重编 Rust。
    Directory(PathBuf),
}

impl Assets {
    /// 没有前端产物（只跑 API）。
    #[must_use]
    pub fn none() -> Self {
        Self::None
    }

    /// 用嵌进二进制的那一份。
    #[must_use]
    pub fn embedded() -> Self {
        Self::Embedded
    }

    /// 有多少个文件嵌进来了。**0 表示这次 `cargo build` 时 `web/dist` 不在。**
    #[must_use]
    pub fn embedded_count() -> usize {
        embedded::EMBEDDED.len()
    }

    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self::Directory(root.into())
    }

    /// 取一个文件。**SPA 的深链回落到 `index.html`**——路由在前端那一侧。
    #[must_use]
    pub fn serve(&self, method: &str, path: &str) -> Response {
        if method != "GET" {
            return not_found();
        }
        // 路径穿越：只允许一段一段的普通名字。
        let relative = crate::routes::segments(path);
        if relative
            .iter()
            .any(|segment| *segment == ".." || segment.contains('\\'))
        {
            return not_found();
        }
        let joined = relative.join("/");

        match self {
            Self::None => not_found(),
            Self::Embedded => {
                if let Some((name, body)) =
                    embedded::EMBEDDED.iter().find(|(name, _)| *name == joined)
                {
                    return Response {
                        status: 200,
                        content_type: content_type(Path::new(name)),
                        body: (*body).to_vec(),
                        set_session: None,
                    };
                }
                embedded::EMBEDDED
                    .iter()
                    .find(|(name, _)| *name == "index.html")
                    .map_or_else(not_found, |(_, body)| Response {
                        status: 200,
                        content_type: "text/html; charset=utf-8",
                        body: (*body).to_vec(),
                        set_session: None,
                    })
            }
            Self::Directory(root) => {
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
                std::fs::read(root.join("index.html")).map_or_else(
                    |_| not_found(),
                    |body| Response {
                        status: 200,
                        content_type: "text/html; charset=utf-8",
                        body,
                        set_session: None,
                    },
                )
            }
        }
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
