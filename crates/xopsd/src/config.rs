//! 从环境变量读配置。**没有配置文件格式**——单实例部署，环境变量够了。

use std::path::PathBuf;

use xops_core::{Error, Result};

/// 数据库路径。给 `:memory:` 就跑内存实现。
pub const DB: &str = "XOPS_DB";
/// MCP 写入面监听哪里。
pub const MCP_ADDR: &str = "XOPS_MCP_ADDR";
/// 只读 Web 面监听哪里。
pub const WEB_ADDR: &str = "XOPS_WEB_ADDR";
/// 前端产物目录。不给就用编译期嵌进来的那一份（`D55`）。
pub const ASSETS: &str = "XOPS_ASSETS";
/// 工作区根目录。
pub const WORKSPACES: &str = "XOPS_WORKSPACES";
/// AttaCore 的 Unix socket。
///
/// ⚠️ **不给就跑桩引擎**——这件事会在启动横幅上说出来，
/// 因为"以为接了真引擎、其实跑的是桩"是一种查起来很慢的错。
pub const ATTACORE_SOCKET: &str = "XOPS_ATTACORE_SOCKET";

/// 一份启动配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// 只读仓凭据与插件配置的加密密钥（32 字节十六进制）。
    ///
    /// ⚠️ **它没有默认值。** 空的时候装配会拒绝起来——
    /// **一个写死的默认密钥看起来是加密的，实际不是**，那比没有密钥更糟。
    pub secret_key: String,
    pub db: String,
    pub mcp_addr: String,
    pub web_addr: String,
    pub assets: Option<PathBuf>,
    pub workspaces: PathBuf,
    pub attacore_socket: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            secret_key: String::new(),
            db: ":memory:".to_owned(),
            mcp_addr: "127.0.0.1:8765".to_owned(),
            web_addr: "127.0.0.1:8766".to_owned(),
            assets: None,
            workspaces: std::env::temp_dir().join("xops-workspaces"),
            attacore_socket: None,
        }
    }
}

impl Config {
    /// 从环境读。
    ///
    /// # Errors
    /// 缺 [`xops_repo::KEY_ENV`]——**这一条不给默认值**：
    /// 没有它，只读仓凭据与插件配置都没法加密存放，而**给一个默认密钥比没有密钥更糟**
    /// （它看起来是加密的）。
    pub fn from_env() -> Result<Self> {
        let Some(secret_key) = var(xops_repo::KEY_ENV) else {
            return Err(Error::invalid(format!(
                "没有设 {}。它是只读仓凭据与插件配置的加密密钥——\
                 **这里不给默认值**：一个写死的默认密钥看起来是加密的，实际不是。\
                 生成一个：`xopsd --generate-key`",
                xops_repo::KEY_ENV
            )));
        };
        let default = Self::default();
        Ok(Self {
            secret_key,
            db: var(DB).unwrap_or(default.db),
            mcp_addr: var(MCP_ADDR).unwrap_or(default.mcp_addr),
            web_addr: var(WEB_ADDR).unwrap_or(default.web_addr),
            assets: var(ASSETS).map(PathBuf::from),
            workspaces: var(WORKSPACES).map_or(default.workspaces, PathBuf::from),
            attacore_socket: var(ATTACORE_SOCKET).map(PathBuf::from),
        })
    }

    /// 数据落在内存里吗。
    #[must_use]
    pub fn in_memory(&self) -> bool {
        self.db == ":memory:"
    }
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 密钥没有默认值() {
        assert!(
            Config::default().secret_key.is_empty(),
            "**一个写死的默认密钥看起来是加密的，实际不是**"
        );
    }

    #[test]
    fn 默认值是本机两个端口() {
        let config = Config::default();
        assert!(config.mcp_addr.starts_with("127.0.0.1"), "默认只听本机");
        assert!(config.web_addr.starts_with("127.0.0.1"));
        assert_ne!(config.mcp_addr, config.web_addr, "两个服务面分开");
        assert!(config.in_memory());
        assert!(config.attacore_socket.is_none(), "不给就是桩引擎");
    }
}
