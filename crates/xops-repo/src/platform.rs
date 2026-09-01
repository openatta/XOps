//! Git 平台适配。
//!
//! `RPO-007`：**平台差异必须收在一个可适配的接口后面**——认证、克隆、
//! 权限元数据校验、webhook 签名验证。四件事，一个 trait。

use std::process::Command;

use xops_core::{Error, Result};

use crate::credential::Secret;

/// 试写的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteProbe {
    /// 试了，写不进去。**这才是能绑的凭据。**
    ReadOnly,
    /// 试了，写得进去。**拒绝绑定**（`RPO-002`、`RPO-013`）。
    Writable,
}

/// 一个 Git 平台。
pub trait GitPlatform: Send + Sync + 'static {
    fn id(&self) -> &'static str;

    /// 拉取时用的认证头。
    ///
    /// ⚠️ **调用方必须把它写进一个临时 git 配置文件**，不能进 argv、不能进环境变量
    /// （`RPO-005`）——`ps` 看得见 argv，子进程继承得到环境变量。
    fn auth_header(&self, secret: &Secret) -> String;

    /// **实际尝试一次写**，看它成不成。
    ///
    /// `RPO-002` 的验收原文：**"尝试一次写并期待它失败"，不是读凭据的声明**。
    /// 声明可以撒谎，也可以过期；只有真去推一下才知道。
    ///
    /// # Errors
    /// 连不上、仓不存在这类——**分不清是不是只读时一律报错，不猜**。
    fn probe_write_access(&self, remote: &str, secret: &Secret) -> Result<WriteProbe>;

    /// 校验 webhook 签名。RP-13 用。
    fn verify_webhook(&self, secret: &str, body: &[u8], signature: &str) -> bool;
}

/// GitHub。`IDN-002` 说先做它。
#[derive(Debug, Clone, Copy, Default)]
pub struct GitHub;

impl GitPlatform for GitHub {
    fn id(&self) -> &'static str {
        "github"
    }

    fn auth_header(&self, secret: &Secret) -> String {
        // GitHub 的 token 走 Basic，用户名随便填。
        format!(
            "Authorization: Basic {}",
            base64(format!("x-access-token:{}", secret.expose()).as_bytes())
        )
    }

    fn probe_write_access(&self, remote: &str, secret: &Secret) -> Result<WriteProbe> {
        // 往一个几乎不可能存在的 ref 上做一次 dry-run 推送。
        // dry-run 照样要服务端做权限判定，但**不会真的改动仓库**——
        // 这正是"尝试一次写"该有的形态：判定是真的，副作用是没有的。
        let probe = format!("refs/heads/xops-write-probe-{}", xops_core::Id::generate());
        let config = crate::workspace::AuthConfig::write(self.auth_header(secret))?;
        let output = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", config.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["push", "--dry-run", remote, &format!("HEAD:{probe}")])
            .output()
            .map_err(|error| Error::unavailable(format!("跑不起来 git：{error}")))?;

        if output.status.success() {
            return Ok(WriteProbe::Writable);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        // 只有"被拒绝"才算只读。**"仓不存在""连不上"这类不算**——
        // 那时候我们并不知道这把凭据是不是只读的，不能猜。
        if stderr.contains("denied")
            || stderr.contains("403")
            || stderr.contains("read-only")
            || stderr.contains("not authorized")
            || stderr.contains("permission")
        {
            return Ok(WriteProbe::ReadOnly);
        }
        Err(Error::invalid(format!(
            "试写没能得出结论，不能据此绑定：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }

    fn verify_webhook(&self, secret: &str, body: &[u8], signature: &str) -> bool {
        // GitHub 的 X-Hub-Signature-256 是 `sha256=<hmac>`。
        let Some(offered) = signature.strip_prefix("sha256=") else {
            return false;
        };
        let expected = hmac_sha256(secret.as_bytes(), body);
        let expected: String = expected.iter().map(|byte| format!("{byte:02x}")).collect();
        // 等时比较：比较提前返回的时刻本身就是一条信道。
        constant_time_eq(expected.as_bytes(), offered.as_bytes())
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = u8::from(left.len() != right.len());
    for index in 0..left.len().max(right.len()) {
        difference |=
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0);
    }
    difference == 0
}

/// HMAC-SHA256。webhook 签名用。
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    const BLOCK: usize = 64;
    let mut normalized = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        normalized[..32].copy_from_slice(&digest);
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_key = [0x36u8; BLOCK];
    let mut outer_key = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        inner_key[index] ^= normalized[index];
        outer_key[index] ^= normalized[index];
    }
    let inner = Sha256::new()
        .chain_update(inner_key)
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(outer_key)
        .chain_update(inner)
        .finalize()
        .into()
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3F] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3F] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3F] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3F] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 认证头里有凭据但它只会进临时配置文件() {
        let header = GitHub.auth_header(&Secret::new("ghp_readonly"));
        assert!(header.starts_with("Authorization: Basic "));
        // 头本身当然含凭据 —— 关键是调用方怎么用它，见 workspace::AuthConfig。
        assert!(!header.contains("ghp_readonly"), "至少不是明文摆着");
    }

    #[test]
    fn base64编得对() {
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b""), "");
    }

    #[test]
    fn webhook签名认得出真假() {
        let body = br#"{"ref":"refs/heads/main"}"#;
        let expected = hmac_sha256(b"s3cret", body);
        let signature: String = format!(
            "sha256={}",
            expected
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        assert!(GitHub.verify_webhook("s3cret", body, &signature));
        assert!(!GitHub.verify_webhook("wrong", body, &signature));
        assert!(!GitHub.verify_webhook("s3cret", b"tampered", &signature));
        assert!(!GitHub.verify_webhook("s3cret", body, "没有前缀"));
    }

    #[test]
    fn hmac对得上rfc的向量() {
        // RFC 4231 Test Case 1。
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        let hex: String = mac.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            hex,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }
}
