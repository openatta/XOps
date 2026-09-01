//! 用户与身份提供方。
//!
//! `IDN-001`：**全部登录经同一个提供方接口完成**——新增一个提供方只实现这个 trait，
//! 不改用户模型、不改令牌模型、不改权限判定。这个约束就是这个文件存在的理由。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Id, Result};

/// 用户标识。稳定、创建后不可变（`IDN-005`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UserId(Id);

impl UserId {
    #[must_use]
    pub fn generate() -> Self {
        Self(Id::generate())
    }

    #[must_use]
    pub const fn from_id(id: Id) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_id(self) -> Id {
        self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 身份提供方的标识，如 `builtin` / `github`。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProviderId(String);

impl ProviderId {
    /// # Errors
    /// 空、超长或含小写字母数字与 `-` 之外的字符。
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        let shaped = !id.is_empty()
            && id.len() <= 32
            && id.starts_with(|c: char| c.is_ascii_lowercase())
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !shaped {
            return Err(Error::invalid(format!("身份提供方标识不合法：{id}")));
        }
        Ok(Self(id))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 「XOps 里的这个人」与「代码仓里提交的那个人」怎么对上（`IDN-006`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalAccount {
    pub provider: ProviderId,
    /// 在那个平台上的账号标识。
    pub account: String,
}

impl ExternalAccount {
    /// 查重用的键。
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}\u{0}{}", self.provider, self.account)
    }
}

/// 提供方交回来的一份资料。**提供方不创建用户**——它只回答"这份凭证是谁"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalProfile {
    pub account: String,
    pub display_name: String,
    pub email: Option<String>,
}

/// 一个用户。
///
/// `IDN-004`：用户是顶层主体，彼此**地位平等**，没有上下级、没有分组。
/// `IDN-005`：`id` 不可变；`display_name` 与 `email` 可变，**不得作为关联键使用**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    pub account: ExternalAccount,
    pub display_name: String,
    pub email: Option<String>,
}

/// 登录的唯一入口形状（`IDN-001`）。
pub trait IdentityProvider: Send + Sync + 'static {
    fn id(&self) -> ProviderId;

    /// 校验一份凭证。
    ///
    /// # Errors
    /// 凭证不对。**错误不区分"账号不存在"与"密码不对"**——那个区分是给探测者的。
    fn authenticate(&self, account: &str, secret: &str) -> Result<ExternalProfile>;
}

/// 预置账号（`IDN-002` 的前一半，部署自测用）。
///
/// 口令存的是 SHA-256 摘要。**这不是给终端用户用的口令体系**——它只是让一个新部署
/// 能在没有 OAuth 的情况下登录进去，所以没有加盐、没有慢哈希，也不打算有。
/// 真正的登录路径是 OAuth。
pub struct BuiltinProvider {
    accounts: Vec<(String, [u8; 32], String)>,
}

impl std::fmt::Debug for BuiltinProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltinProvider")
            .field("accounts", &self.accounts.len())
            .finish()
    }
}

impl BuiltinProvider {
    #[must_use]
    pub fn new() -> Self {
        Self {
            accounts: Vec::new(),
        }
    }

    /// 预置一个账号。
    #[must_use]
    pub fn with_account(
        mut self,
        account: impl Into<String>,
        secret: &str,
        display_name: impl Into<String>,
    ) -> Self {
        self.accounts.push((
            account.into(),
            crate::token::digest(secret),
            display_name.into(),
        ));
        self
    }
}

impl Default for BuiltinProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl IdentityProvider for BuiltinProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("builtin").expect("builtin 是合法的提供方标识")
    }

    fn authenticate(&self, account: &str, secret: &str) -> Result<ExternalProfile> {
        let offered = crate::token::digest(secret);
        let found = self.accounts.iter().find(|(name, expected, _)| {
            // 逐字节等时比较，且账号对不上也照样比一次 —— 免得比较耗时本身成了探测手段。
            let name_ok = crate::token::constant_time_eq(name.as_bytes(), account.as_bytes());
            let secret_ok = crate::token::constant_time_eq(expected, &offered);
            name_ok & secret_ok
        });
        let (name, _, display_name) = found.ok_or_else(|| Error::denied("凭证不对"))?;
        Ok(ExternalProfile {
            account: name.clone(),
            display_name: display_name.clone(),
            email: None,
        })
    }
}

/// OAuth 提供方（`IDN-002` 的后一半，先做 GitHub）。
///
/// ⚠️ **这里没有 HTTP。** 用授权码换令牌、拿令牌换资料，是两次出网调用，它们属于
/// 那个持有回调端点的包（RP-05 的只读 Web）。本 crate 只定义"换回来的资料长什么样"
/// 与"它怎么变成一个 XOps 用户"，注入方式就是下面这个 trait。
///
/// `IDN-007`：回调**只能做一件事**——完成身份验证、建立会话，不能创建或修改任何业务对象。
pub trait ProfileExchange: Send + Sync + 'static {
    /// 拿授权码换一份资料。
    ///
    /// # Errors
    /// 授权码无效或对方不可用。
    fn exchange(&self, code: &str) -> Result<ExternalProfile>;
}

/// 任何一个走"授权码 → 资料"的提供方。GitHub 是它的第一个实例。
pub struct OAuthProvider {
    id: ProviderId,
    exchange: Box<dyn ProfileExchange>,
}

impl std::fmt::Debug for OAuthProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthProvider")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl OAuthProvider {
    /// # Errors
    /// 提供方标识不合法。
    pub fn new(id: &str, exchange: Box<dyn ProfileExchange>) -> Result<Self> {
        Ok(Self {
            id: ProviderId::new(id)?,
            exchange,
        })
    }

    /// GitHub。
    ///
    /// # Errors
    /// 不会——`github` 是合法标识；签名保持一致是为了调用处不用分叉。
    pub fn github(exchange: Box<dyn ProfileExchange>) -> Result<Self> {
        Self::new("github", exchange)
    }
}

impl IdentityProvider for OAuthProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    /// `account` 在 OAuth 这条路上没有意义（身份由授权码决定），忽略它。
    fn authenticate(&self, _account: &str, code: &str) -> Result<ExternalProfile> {
        self.exchange.exchange(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 预置账号认口令() {
        let provider = BuiltinProvider::new().with_account("root", "s3cret", "管理员");
        let profile = provider.authenticate("root", "s3cret").unwrap();
        assert_eq!(profile.account, "root");
        assert_eq!(profile.display_name, "管理员");
    }

    #[test]
    fn 账号不存在与口令不对是同一个错() {
        let provider = BuiltinProvider::new().with_account("root", "s3cret", "管理员");
        let missing = provider.authenticate("nobody", "s3cret").unwrap_err();
        let wrong = provider.authenticate("root", "nope").unwrap_err();
        assert_eq!(missing.kind(), wrong.kind());
        assert_eq!(
            missing.message(),
            wrong.message(),
            "两者必须逐字一致，否则错误本身就是探测器"
        );
    }

    #[test]
    fn 提供方标识挑得住() {
        assert!(ProviderId::new("github").is_ok());
        assert!(ProviderId::new("").is_err());
        assert!(ProviderId::new("GitHub").is_err());
        assert!(ProviderId::new("1st").is_err());
    }

    #[test]
    fn 外部账号的查重键把提供方也算上() {
        let github = ExternalAccount {
            provider: ProviderId::new("github").unwrap(),
            account: "alice".into(),
        };
        let builtin = ExternalAccount {
            provider: ProviderId::new("builtin").unwrap(),
            account: "alice".into(),
        };
        assert_ne!(
            github.key(),
            builtin.key(),
            "两个提供方上的同名账号不是同一个人"
        );
    }

    struct FixedExchange;

    impl ProfileExchange for FixedExchange {
        fn exchange(&self, code: &str) -> Result<ExternalProfile> {
            if code == "good" {
                Ok(ExternalProfile {
                    account: "alice".into(),
                    display_name: "Alice".into(),
                    email: Some("alice@example.com".into()),
                })
            } else {
                Err(Error::denied("凭证不对"))
            }
        }
    }

    #[test]
    fn oauth走注入的兑换() {
        let provider = OAuthProvider::github(Box::new(FixedExchange)).unwrap();
        assert_eq!(provider.id().as_str(), "github");
        assert_eq!(provider.authenticate("", "good").unwrap().account, "alice");
        assert!(provider.authenticate("", "bad").is_err());
    }
}
