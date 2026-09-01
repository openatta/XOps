//! 出网这一样能力（`PLG-012` ①）。
//!
//! > 未声明的主机连不上；**重定向每一跳重新判定**——否则白名单只对第一跳成立。
//!
//! 平台**不提供"发消息"这种能力，也不定义"通道"这个概念**（`PLG-004`）。这里只有
//! 一条最朴素的请求-应答，以及一道按声明放行的闸。真正的发包由 [`Net`] 的实现方做。

use xops_core::{Error, Result};

use crate::capability::Capabilities;

/// 最多跟几跳重定向。
pub const MAX_HOPS: usize = 5;

/// 一次请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub method: String,
    pub body: Option<String>,
}

/// 一次应答。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// 重定向时的下一跳。**它要重新过一次白名单。**
    pub location: Option<String>,
    pub body: String,
}

/// 谁去真的发包。**XOps 不实现它**——这是一个接缝，不是一个 HTTP 客户端。
pub trait Net: Send + Sync + 'static {
    /// 发一次。
    ///
    /// # Errors
    /// 网络本身的失败。
    fn send(&self, request: &Request) -> Result<Response>;
}

/// 没有后端时的实现：**一律不通**。
#[derive(Debug, Clone, Copy)]
pub struct Denied;

impl Net for Denied {
    fn send(&self, _request: &Request) -> Result<Response> {
        Err(Error::invalid("这个部署没有接出网后端"))
    }
}

/// 从 URL 里取主机名。
///
/// # Errors
/// 不是一个带 scheme 与主机的 URL。
pub fn host_of(url: &str) -> Result<String> {
    let rest = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(|| Error::invalid("URL 要带 scheme"))?;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    let host = authority.split(':').next().unwrap_or_default();
    if host.is_empty() {
        return Err(Error::invalid("URL 里没有主机"));
    }
    Ok(host.to_ascii_lowercase())
}

/// 按声明放行地发一次请求，**每一跳都重新判定**。
///
/// # Errors
/// 主机不在白名单 · 跳太多 · 后端失败。
pub fn fetch(net: &dyn Net, capabilities: &Capabilities, request: Request) -> Result<Response> {
    let mut current = request;
    for _ in 0..=MAX_HOPS {
        let host = host_of(&current.url)?;
        // ⚠️ 这一句在循环**里面**。挪到循环外面，白名单就只对第一跳成立了。
        if !capabilities.allows_host(&host) {
            return Err(Error::invalid(format!(
                "{host} 不在这个插件声明过的出网白名单里"
            )));
        }
        let response = net.send(&current)?;
        let Some(next) = response
            .location
            .clone()
            .filter(|_| is_redirect(response.status))
        else {
            return Ok(response);
        };
        current = Request {
            url: next,
            method: current.method.clone(),
            body: current.body.clone(),
        };
    }
    Err(Error::invalid("重定向跳太多"))
}

const fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Recording {
        hops: Mutex<Vec<String>>,
        redirect_to: Option<String>,
    }

    impl Net for Recording {
        fn send(&self, request: &Request) -> Result<Response> {
            let mut hops = self.hops.lock().unwrap();
            hops.push(request.url.clone());
            let first = hops.len() == 1;
            Ok(Response {
                status: if first && self.redirect_to.is_some() {
                    302
                } else {
                    200
                },
                location: if first {
                    self.redirect_to.clone()
                } else {
                    None
                },
                body: "ok".into(),
            })
        }
    }

    fn allowing(hosts: &[&str]) -> Capabilities {
        Capabilities {
            network: hosts.iter().map(|host| (*host).to_owned()).collect(),
            ..Capabilities::none()
        }
    }

    fn request(url: &str) -> Request {
        Request {
            url: url.to_owned(),
            method: "GET".into(),
            body: None,
        }
    }

    #[test]
    fn 未声明的主机连不上() {
        let net = Recording {
            hops: Mutex::new(vec![]),
            redirect_to: None,
        };
        let error = fetch(
            &net,
            &allowing(&["ok.example"]),
            request("https://evil.example/x"),
        )
        .unwrap_err();
        assert!(error.message().contains("白名单"));
        assert!(net.hops.lock().unwrap().is_empty(), "连发都没发出去");
    }

    #[test]
    fn 重定向的每一跳重新判定() {
        let net = Recording {
            hops: Mutex::new(vec![]),
            redirect_to: Some("https://evil.example/x".into()),
        };
        let error = fetch(
            &net,
            &allowing(&["ok.example"]),
            request("https://ok.example/x"),
        )
        .unwrap_err();
        assert!(
            error.message().contains("evil.example"),
            "白名单只对第一跳成立就是这条挡的"
        );
        assert_eq!(net.hops.lock().unwrap().len(), 1, "第二跳没有发出去");
    }

    #[test]
    fn 声明过的两跳都放行() {
        let net = Recording {
            hops: Mutex::new(vec![]),
            redirect_to: Some("https://cdn.example/x".into()),
        };
        let response = fetch(
            &net,
            &allowing(&["ok.example", "cdn.example"]),
            request("https://ok.example/x"),
        )
        .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(net.hops.lock().unwrap().len(), 2);
    }

    #[test]
    fn 取主机名() {
        assert_eq!(host_of("https://A.example:8443/x?y").unwrap(), "a.example");
        assert_eq!(host_of("http://user:pw@a.example/x").unwrap(), "a.example");
        assert!(host_of("a.example/x").is_err());
        assert!(host_of("https:///x").is_err());
    }
}
