//! 只读凭据的存储与保护。
//!
//! `RPO-003`：**加密存储，任何接口都不能读出原文，包括项目所有者自己。**
//!
//! ⚠️ 它与访问令牌（`TOK-002`）**不是一回事**：令牌存的是单向摘要，因为系统只需要
//! "对不对得上"；仓凭据拉取时要用原文，所以必须是**可逆**加密。两者共用一套做法
//! 是这一处最容易犯的错。
//!
//! **密钥从部署侧来**（`XOPS_SECRET_KEY`，32 字节十六进制）。这条边界要说清楚：
//! 拿到库文件**加上**密钥的人能解出凭据——加密防的是"库被拷走"，不是"部署被攻陷"。

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use xops_core::{Error, Result};

/// 密钥从哪个环境变量来。
pub const KEY_ENV: &str = "XOPS_SECRET_KEY";

/// 存下来的那一份：**密文 + 随机数，没有原文**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedCredential {
    /// 12 字节随机数，十六进制。
    nonce: String,
    /// 密文，十六进制。
    ciphertext: String,
}

/// 凭据原文。**它故意不实现 `Serialize`，也不在 `Debug` 里打印内容**——
/// 想把它存下来或记进日志的每一条路都得先绕过这个类型。
pub struct Secret(String);

impl Secret {
    #[must_use]
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<不打印>)")
    }
}

/// 一把密封凭据的钥匙。
pub struct Sealer {
    cipher: ChaCha20Poly1305,
}

impl std::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sealer(<不打印>)")
    }
}

impl Sealer {
    /// 从 32 字节密钥建一把。
    ///
    /// # Errors
    /// 密钥长度不对。
    pub fn from_key(key: &[u8]) -> Result<Self> {
        if key.len() != 32 {
            return Err(Error::invalid("密钥必须是 32 字节"));
        }
        Ok(Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(key)),
        })
    }

    /// 从环境变量取密钥。
    ///
    /// # Errors
    /// 没设、不是十六进制、或者长度不对。
    pub fn from_env() -> Result<Self> {
        let hex = std::env::var(KEY_ENV)
            .map_err(|_| Error::invalid(format!("没有设 {KEY_ENV}（32 字节十六进制）")))?;
        Self::from_hex(&hex)
    }

    /// 从十六进制文本取密钥。
    ///
    /// 装配层要它：**密钥从哪来是装配的事，不是这个类型的事**——
    /// 读环境变量那一步在进程边界上做，测试才好构造。
    ///
    /// # Errors
    /// 不是十六进制，或者长度不对。
    pub fn from_hex(hex: &str) -> Result<Self> {
        Self::from_key(&decode_hex(hex)?)
    }

    /// 生成一把新钥匙的十六进制形态。部署时用它。
    ///
    /// # Errors
    /// 取不到系统熵。
    pub fn generate_key() -> Result<String> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key)
            .map_err(|error| Error::internal(format!("取不到系统熵：{error}")))?;
        Ok(encode_hex(&key))
    }

    /// 封起来。
    ///
    /// # Errors
    /// 取不到系统熵或加密失败。
    pub fn seal(&self, secret: &Secret) -> Result<SealedCredential> {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce)
            .map_err(|error| Error::internal(format!("取不到系统熵：{error}")))?;
        let ciphertext = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), secret.expose().as_bytes())
            .map_err(|_| Error::internal("凭据封不上"))?;
        Ok(SealedCredential {
            nonce: encode_hex(&nonce),
            ciphertext: encode_hex(&ciphertext),
        })
    }

    /// 解开。
    ///
    /// ⚠️ **只有一处该调它**：`workspace` 里那一次拉取（`RPO-005`）。
    /// 别的地方需要"凭据"时要的其实是"绑定存不存在"，那是另一个问题。
    ///
    /// # Errors
    /// 密文损坏或密钥不对。
    pub fn open(&self, sealed: &SealedCredential) -> Result<Secret> {
        let nonce = decode_hex(&sealed.nonce)?;
        let ciphertext = decode_hex(&sealed.ciphertext)?;
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext.as_slice())
            .map_err(|_| Error::invalid("凭据解不开：密钥不对，或者密文被动过"))?;
        String::from_utf8(plaintext)
            .map(Secret::new)
            .map_err(|_| Error::internal("凭据不是合法文本"))
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn decode_hex(text: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return Err(Error::invalid("十六进制长度必须是偶数"));
    }
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16)
                .map_err(|_| Error::invalid("不是十六进制"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealer() -> Sealer {
        Sealer::from_key(&[7u8; 32]).unwrap()
    }

    #[test]
    fn 封了能解开() {
        let sealer = sealer();
        let sealed = sealer.seal(&Secret::new("ghp_readonly")).unwrap();
        assert_eq!(sealer.open(&sealed).unwrap().expose(), "ghp_readonly");
    }

    #[test]
    fn 存下来的那一份里没有原文() {
        let sealed = sealer().seal(&Secret::new("ghp_readonly")).unwrap();
        let stored = serde_json::to_string(&sealed).unwrap();
        assert!(
            !stored.contains("ghp_readonly"),
            "RPO-003：任何接口都不能读出原文"
        );
    }

    #[test]
    fn 原文不打印() {
        let secret = Secret::new("ghp_readonly");
        assert!(!format!("{secret:?}").contains("ghp_readonly"));
        assert!(!format!("{:?}", sealer()).contains("7"));
    }

    #[test]
    fn 每次封出来的都不一样() {
        let sealer = sealer();
        let first = sealer.seal(&Secret::new("same")).unwrap();
        let second = sealer.seal(&Secret::new("same")).unwrap();
        assert_ne!(
            first.ciphertext, second.ciphertext,
            "随机数不同，密文就该不同"
        );
    }

    #[test]
    fn 换把钥匙就解不开() {
        let sealed = sealer().seal(&Secret::new("ghp_readonly")).unwrap();
        let other = Sealer::from_key(&[9u8; 32]).unwrap();
        assert!(other.open(&sealed).is_err());
    }

    #[test]
    fn 密文被动过就解不开() {
        let sealer = sealer();
        let mut sealed = sealer.seal(&Secret::new("ghp_readonly")).unwrap();
        sealed.ciphertext.replace_range(0..2, "ff");
        assert!(sealer.open(&sealed).is_err(), "AEAD 认得出篡改");
    }

    #[test]
    fn 钥匙长度不对就建不出来() {
        assert!(Sealer::from_key(&[0u8; 16]).is_err());
        let generated = Sealer::generate_key().unwrap();
        assert_eq!(generated.len(), 64);
        assert!(Sealer::from_key(&decode_hex(&generated).unwrap()).is_ok());
    }
}
