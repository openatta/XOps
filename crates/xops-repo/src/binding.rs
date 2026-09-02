//! 仓绑定这个对象。

use serde::{Deserialize, Serialize};
use xops_core::{Error, Result, Timestamp};
use xops_identity::{ProjectId, UserId};

use crate::credential::SealedCredential;

/// 一个项目绑的那个仓。
///
/// `RPO-001`：**当前绑一个**。多仓是 Q11 / M6。
/// `RPO-014`：**XForge 的登记挂在它上面，不另开一套对象**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub project: ProjectId,
    /// 远端地址。
    pub remote: String,
    /// 哪个平台（`RPO-007`）。
    pub platform: String,
    /// **密文**。原文任何接口都读不出来（`RPO-003`）。
    ///
    /// ⚠️ **本地仓（`file://`）没有凭据,这里是 `None`。** 它不是"忘了填":
    /// 本地仓的取用不经过任何认证,也就没有可轮换、可泄漏、可过期的东西。
    /// 让它必填、由调用方塞一个占位串进来更糟——**那是往一个专放密钥的字段里放垃圾**,
    /// 而 `repo.rotate` 会把那串垃圾当成一把真凭据去换。
    #[serde(default)]
    pub credential: Option<SealedCredential>,
    pub bound_by: UserId,
    pub bound_at: Timestamp,
    /// 上次拉取的时刻与修订（`RPO-012`）。
    pub last_fetch_at: Option<Timestamp>,
    pub last_revision: Option<String>,
    /// Git webhook 的验签密钥，**密文**（`TRG-012`）。
    ///
    /// ⚠️ **它按项目存，不是平台一把。** 一把平台级的密钥意味着任何一个能拿到它的人
    /// 都能给**每一个**项目投递事件——而 webhook 端点是无凭据的公网入口，
    /// 那把密钥就是它唯一的门。密钥的作用面必须和它守的东西一样大，不能更大。
    ///
    /// 没设就是**这个项目收不到 webhook**，与没绑仓一样回"不存在"（`TRG-012`）。
    #[serde(default)]
    pub webhook_secret: Option<SealedCredential>,
    /// XForge 登记：`policyId → 流程 + 结果列映射` 挂在这里（`RPO-014` / `XFG-002`）。
    /// 本包只留位，内容归 RP-19。
    pub xforge: Option<serde_json::Value>,
}

impl Binding {
    /// # Errors
    /// 远端地址不合法。
    pub fn new(
        project: ProjectId,
        remote: impl Into<String>,
        platform: impl Into<String>,
        credential: Option<SealedCredential>,
        bound_by: UserId,
        bound_at: Timestamp,
    ) -> Result<Self> {
        let remote = remote.into();
        check_remote(&remote)?;
        Ok(Self {
            project,
            remote,
            platform: platform.into(),
            credential,
            bound_by,
            bound_at,
            last_fetch_at: None,
            last_revision: None,
            webhook_secret: None,
            xforge: None,
        })
    }
}

/// # Errors
/// 空、太长、不是 https/ssh、或者**把凭据写进了 URL**。
pub fn check_remote(remote: &str) -> Result<()> {
    if remote.is_empty() || remote.len() > 512 {
        return Err(Error::invalid("远端地址长度不对"));
    }
    if remote.contains('@') && remote.starts_with("http") {
        // `https://token@host/x.git` 这种写法会让凭据跟着 URL 到处跑 ——
        // 进日志、进错误消息、进 `git remote -v`。**凭据走认证头，不走 URL。**
        return Err(Error::invalid("凭据不要写进远端地址，它会跟着 URL 到处跑"));
    }
    if !(remote.starts_with("https://")
        || remote.starts_with("ssh://")
        || remote.starts_with("git@")
        || remote.starts_with(crate::local::SCHEME))
    {
        return Err(Error::invalid(
            "远端地址只认 https:// · ssh:// · git@ · file://",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 远端地址挑得住() {
        assert!(check_remote("https://github.com/openatta/XOps.git").is_ok());
        assert!(check_remote("git@github.com:openatta/XOps.git").is_ok());
        assert!(check_remote("file:///srv/repos/x.git").is_ok());
        assert!(check_remote("").is_err());
        assert!(check_remote("ftp://x").is_err());
        // 裸路径不认:它与 scp 式的 `host:path` 分不开。
        assert!(check_remote("/srv/repos/x.git").is_err());
    }

    #[test]
    fn 凭据写进url会被拒() {
        let error = check_remote("https://ghp_token@github.com/x/y.git").unwrap_err();
        assert!(error.message().contains("跟着 URL 到处跑"));
    }
}
