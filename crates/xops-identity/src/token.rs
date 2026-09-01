//! 访问令牌。
//!
//! 这个文件守的是全系统最下面那条线：**行为人一律由令牌解析得出**（`TOK-007`、`I-B`、G5）。
//! MCP 协议本身不认证调用者——管道那头的 agent 说自己是谁没有任何约束力。这里松了，
//! 后面所有东西都是装饰。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use xops_core::{Error, Id, Result, Timestamp};

use crate::user::UserId;

/// 令牌标识（不是令牌原文）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TokenId(Id);

impl TokenId {
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

impl std::fmt::Display for TokenId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// 令牌原文。**只在签发那一刻存在一次**（`TOK-002`、`I-A`）。
///
/// 它故意没有 `Clone`、没有 `Debug` 里的内容、也不实现 `Serialize`——
/// 想把它存下来的每一条路都得先绕过这个类型，而那种绕法在评审里是看得见的。
pub struct TokenSecret(String);

impl TokenSecret {
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// 拿去交给用户，之后这个值就不存在了。
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for TokenSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenSecret(<只呈现一次，不打印>)")
    }
}

/// 存下来的那一份：**只有摘要，没有原文**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub id: TokenId,
    /// `TOK-001`：**一律按人签发**。没有按服务、按项目、按团队共享的令牌。
    pub user: UserId,
    /// SHA-256(原文)。不可逆（`TOK-002`）。
    pub digest: [u8; 32],
    /// 给人看的名字，用来分辨"哪台机器上的哪个工具"。
    pub label: String,
    pub issued_at: Timestamp,
    /// `TOK-004`：可设过期；已过期与已撤销**行为完全一致**。
    pub expires_at: Option<Timestamp>,
    pub revoked_at: Option<Timestamp>,
    /// `TOK-006`：最后一次**成功**使用的时间。
    pub last_used_at: Option<Timestamp>,
}

impl Token {
    /// 现在还能用吗。
    #[must_use]
    pub fn usable_at(&self, now: Timestamp) -> bool {
        if self.revoked_at.is_some() {
            return false;
        }
        !self.expires_at.is_some_and(|expires| now >= expires)
    }
}

/// 签一个新令牌，交回「要存的那一份」与「只呈现一次的原文」。
///
/// # Errors
/// 取不到系统熵。
pub fn issue(
    user: UserId,
    label: impl Into<String>,
    now: Timestamp,
    expires_at: Option<Timestamp>,
) -> Result<(Token, TokenSecret)> {
    let secret = generate_secret()?;
    let token = Token {
        id: TokenId::generate(),
        user,
        digest: digest(secret.expose()),
        label: label.into(),
        issued_at: now,
        expires_at,
        revoked_at: None,
        last_used_at: None,
    };
    Ok((token, secret))
}

/// 令牌原文的前缀。让它在日志与代码里一眼能被认出来是个凭据。
pub const SECRET_PREFIX: &str = "xops_";
/// 原文的熵，字节。
const SECRET_BYTES: usize = 32;

fn generate_secret() -> Result<TokenSecret> {
    let mut bytes = [0u8; SECRET_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|error| Error::internal(format!("取不到系统熵：{error}")))?;
    let mut text = String::with_capacity(SECRET_PREFIX.len() + SECRET_BYTES * 2);
    text.push_str(SECRET_PREFIX);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
    }
    Ok(TokenSecret(text))
}

/// SHA-256。令牌与预置口令都用它。
#[must_use]
pub fn digest(secret: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.finalize().into()
}

/// 等时比较。**不要用 `==`**：比较提前返回的时刻本身就是一条信道。
#[must_use]
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    // 长度不同直接判否，但仍然把两边都走一遍 —— 长度是公开信息，内容不是。
    let mut difference = u8::from(left.len() != right.len());
    for index in 0..left.len().max(right.len()) {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        difference |= a ^ b;
    }
    difference == 0
}

/// 解析失败时**唯一**的那句话。
///
/// `TOK-005`：不存在、已撤销、已过期、格式非法——四种情形的错误必须一模一样，
/// 而且不得泄露这个令牌是否曾经存在。给它一个常量，是为了让"顺手加一句更友好的提示"
/// 这件事必须先改到这里。
#[must_use]
pub fn rejection() -> Error {
    Error::denied("令牌无效")
}

/// 最后使用时间的写回节流：短于这个间隔就不写。
///
/// `TOK-006` 要记最后使用时间，而认证在每一次调用的路径上——每次都写一遍，
/// 等于让 `_tokens` 这一张表串行掉全系统的调用（`CON-001` 是表级锁）。
/// 所以它的精度是**分钟级**，这是刻意的，不是偷懒。
pub const LAST_USED_RESOLUTION_MILLIS: i64 = 60_000;

/// 这次成功使用要不要写回 `last_used_at`。
#[must_use]
pub fn should_touch(token: &Token, now: Timestamp) -> bool {
    token.last_used_at.is_none_or(|last| {
        now.as_millis().saturating_sub(last.as_millis()) >= LAST_USED_RESOLUTION_MILLIS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis)
    }

    #[test]
    fn 签出来的原文带前缀且每次不同() {
        let user = UserId::generate();
        let (_, first) = issue(user, "笔记本", at(0), None).unwrap();
        let (_, second) = issue(user, "台式机", at(0), None).unwrap();
        assert!(first.expose().starts_with(SECRET_PREFIX));
        assert_eq!(first.expose().len(), SECRET_PREFIX.len() + SECRET_BYTES * 2);
        assert_ne!(first.expose(), second.expose());
    }

    #[test]
    fn 存的是摘要不是原文() {
        let (token, secret) = issue(UserId::generate(), "l", at(0), None).unwrap();
        let stored = serde_json::to_string(&token).unwrap();
        assert!(
            !stored.contains(secret.expose()),
            "序列化出来的令牌记录里不该有原文"
        );
        assert_eq!(token.digest, digest(secret.expose()));
    }

    #[test]
    fn 原文不打印() {
        let (_, secret) = issue(UserId::generate(), "l", at(0), None).unwrap();
        assert!(!format!("{secret:?}").contains(secret.expose()));
    }

    #[test]
    fn 撤销与过期表现一致() {
        let (mut revoked, _) = issue(UserId::generate(), "l", at(0), None).unwrap();
        revoked.revoked_at = Some(at(10));
        let (expired, _) = issue(UserId::generate(), "l", at(0), Some(at(10))).unwrap();

        assert!(!revoked.usable_at(at(20)));
        assert!(!expired.usable_at(at(20)));
        assert!(!expired.usable_at(at(10)), "到点即失效，不留窗口");
        assert!(expired.usable_at(at(9)));
    }

    #[test]
    fn 撤销立即生效没有延迟窗口() {
        let (mut token, _) = issue(UserId::generate(), "l", at(0), None).unwrap();
        token.revoked_at = Some(at(5));
        assert!(!token.usable_at(at(5)), "撤销那一刻就不能用了");
    }

    #[test]
    fn 等时比较认得出相同与不同() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn 最后使用时间按分钟节流() {
        let (mut token, _) = issue(UserId::generate(), "l", at(0), None).unwrap();
        assert!(should_touch(&token, at(0)), "从没用过就该写一次");
        token.last_used_at = Some(at(1_000));
        assert!(!should_touch(&token, at(1_500)));
        assert!(should_touch(
            &token,
            at(1_000 + LAST_USED_RESOLUTION_MILLIS)
        ));
    }

    #[test]
    fn 拒绝的话只有一句() {
        assert_eq!(rejection().message(), "令牌无效");
    }
}
