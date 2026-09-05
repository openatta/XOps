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
/// 模型 API key。**给了就用真引擎，不给就跑桩**。
///
/// ⚠️ 这件事会在启动横幅上说出来，因为"以为接了真引擎、其实跑的是桩"
/// 是一种查起来很慢的错。
pub const MODEL_KEY: &str = "XOPS_MODEL_KEY";
/// 默认模型。
pub const MODEL: &str = "XOPS_MODEL";
/// 模型服务地址（兼容 Anthropic Messages 的任何一个）。
pub const MODEL_BASE_URL: &str = "XOPS_MODEL_BASE_URL";
/// 预置账号（`IDN-002` 的前一半，**部署自测用**）。
///
/// 形如 `账号:口令[:显示名]`，多个用逗号隔开。
///
/// ⚠️ **不给它，Web 上一个人都登不进来。** `Directory` 默认一个身份提供方都没有，
/// 于是 `POST /session` 一律回"凭证不对"——**页面在，路由在，就是进不去**，
/// 而且从错误上看不出是"没配"还是"打错了"（那个不区分是给探测者的，`IDN-001`）。
/// 这件事会在启动横幅上说出来。
///
/// ⚠️ **这不是给终端用户用的口令体系**：摘要没加盐、没有慢哈希，也不打算有
/// （见 `BuiltinProvider` 的注释）。**真正的登录路径是 OAuth。**
pub const LOGIN: &str = "XOPS_LOGIN";

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
    /// 模型凭据。**没有它就跑桩引擎。**
    pub model_key: Option<String>,
    pub model: String,
    pub model_base_url: Option<String>,
    /// 预置账号：`(账号, 口令, 显示名)`。**空的时候没有人登得进 Web。**
    pub logins: Vec<(String, String, String)>,
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
            model_key: None,
            model: "claude-sonnet-4-6".to_owned(),
            model_base_url: None,
            // ⚠️ **没有默认账号。** 一个写死的默认口令与一个写死的默认密钥是同一种东西。
            logins: Vec::new(),
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
            model_key: var(MODEL_KEY),
            model: var(MODEL).unwrap_or(default.model),
            model_base_url: var(MODEL_BASE_URL),
            logins: var(LOGIN).map(|raw| parse_logins(&raw)).unwrap_or_default(),
        })
    }

    /// 数据落在内存里吗。
    #[must_use]
    pub fn in_memory(&self) -> bool {
        self.db == ":memory:"
    }
}

/// `账号:口令[:显示名]`，逗号分隔。
///
/// ⚠️ **缺了口令的那一条直接丢掉，不当成"口令是空串"。** 空口令能登进去，
/// 而写的人以为自己配的是"没配"。
fn parse_logins(raw: &str) -> Vec<(String, String, String)> {
    raw.split(',')
        .filter_map(|entry| {
            let mut parts = entry.trim().splitn(3, ':');
            let account = parts.next()?.trim();
            let secret = parts.next()?.trim();
            if account.is_empty() || secret.is_empty() {
                return None;
            }
            let display = parts.next().map(str::trim).filter(|name| !name.is_empty());
            Some((
                account.to_owned(),
                secret.to_owned(),
                display.unwrap_or(account).to_owned(),
            ))
        })
        .collect()
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
        assert!(config.model_key.is_none(), "不给模型凭据就是桩引擎");
        assert!(config.logins.is_empty(), "没有默认账号");
    }

    #[test]
    fn 预置账号解析得出来而且缺口令的那条被丢掉() {
        assert_eq!(
            parse_logins("alice:pw1:Alice,bob:pw2"),
            vec![
                ("alice".to_owned(), "pw1".to_owned(), "Alice".to_owned()),
                ("bob".to_owned(), "pw2".to_owned(), "bob".to_owned()),
            ],
            "不给显示名就用账号"
        );
        // ⚠️ **空口令登得进去，而写的人以为自己配的是「没配」。**
        assert!(parse_logins("alice").is_empty(), "没有口令");
        assert!(parse_logins("alice:").is_empty(), "口令是空的");
        assert!(parse_logins(":pw").is_empty(), "账号是空的");
    }
}
