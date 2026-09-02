//! 本地仓（`file://`）。
//!
//! # 为什么它要单独一段
//!
//! `RPO-002` 的验收原文是**"尝试一次写并期待它失败"**。远端上这句话落成
//! 一次 `git push --dry-run`：服务端做权限判定，dry-run 不留副作用。
//!
//! **这一招在本地仓上是失效的,而且是静默失效**——实测：
//!
//! ```text
//! git push --dry-run file://<bare 仓> HEAD:refs/heads/probe   → exit 0   （仓可写）
//! chmod -R a-w <bare 仓>；同一条命令                          → exit 0   （仓只读）
//! ```
//!
//! 两种情形一模一样。照远端那条路判，本地仓**永远**被判成"写得进去"，
//! 于是 `RPO-013` 一律拒绝——而如果哪天有人为了让它过而放宽判定，
//! 放宽掉的正是这条守则本身。
//!
//! # 那本地的"写不进去"是什么
//!
//! **是操作系统说 xopsd 这个进程写不了那个目录。** 它是"服务端拒绝了我"在本地的
//! 对应物：同样是一次真的判定，同样不靠任何声明。`RPO-013` 的实质——
//! **XOps 在任何代码路径上都不持有仓库写权限**——照样成立，而且更硬：
//! 远端那条靠的是服务端此刻的授权，这条靠的是文件系统权限位。
//!
//! ⚠️ **判的是"能不能写",不是"是不是 root"。** 以 root 跑的 xopsd 写得了任何目录，
//! 所以这条判定在 root 下会一律说"写得进去"→ 绑不上。**那是对的**：
//! 以 root 跑的进程确实持有那个仓的写权限，这条守则不该为它开一个例外。

use std::path::{Path, PathBuf};

use xops_core::{Error, Result};

use crate::platform::WriteProbe;

/// 本地仓的 URL 前缀。**只认 `file://`,不认裸路径**——
/// 裸路径与 scp 式的 `host:path` 分不开（`git@host:x/y.git` 也是"有冒号的路径"）。
pub const SCHEME: &str = "file://";

/// 这个远端是不是本地仓；是的话给出那个路径。
#[must_use]
pub fn path_of(remote: &str) -> Option<PathBuf> {
    remote.strip_prefix(SCHEME).map(PathBuf::from)
}

/// 本地仓的只读判定。
///
/// # Errors
/// 路径不存在、不是目录、看不到权限位——**分不清的时候一律报错，不猜**
/// （与远端那条同一口径）。
pub fn probe(path: &Path) -> Result<WriteProbe> {
    if !path.is_absolute() {
        return Err(Error::invalid("本地仓要给绝对路径"));
    }
    let metadata = std::fs::metadata(path).map_err(|error| {
        Error::invalid(format!(
            "看不到 {}：{error}。**分不清是不是只读时不绑**",
            path.display()
        ))
    })?;
    if !metadata.is_dir() {
        return Err(Error::invalid("本地仓要指向一个目录"));
    }
    // 是不是一个仓。`RPO-002` 之前先确认它确实是个仓——
    // 指错目录被判成"只读"然后绑上去，是一个要到取工作区时才炸的错。
    if !(path.join("HEAD").exists() || path.join(".git").is_dir()) {
        return Err(Error::invalid(format!(
            "{} 看着不像一个 Git 仓（既没有 HEAD 也没有 .git/）",
            path.display()
        )));
    }
    Ok(if writable(path) {
        WriteProbe::Writable
    } else {
        WriteProbe::ReadOnly
    })
}

/// 这个进程写不写得了这个目录。
///
/// ⚠️ **真去建一个文件,不是读权限位算一遍。** 权限位算出来的"应该能写"会在
/// 只读挂载、ACL、SELinux、不可变属性这些地方对不上,而**每一次对不上都是
/// 往错的方向对**:算出来不能写、实际能写。真建一次不会。
fn writable(path: &Path) -> bool {
    let probe = path.join(format!(".xops-write-probe-{}", xops_core::Id::generate()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 造一个仓(read_only: bool) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("xops-local-test-{}", xops_core::Id::generate()));
        std::fs::create_dir_all(&root).unwrap();
        std::process::Command::new("git")
            .current_dir(&root)
            .args(["init", "--quiet", "--bare"])
            .output()
            .unwrap();
        if read_only {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        root
    }

    #[test]
    fn 只读的本地仓绑得上() {
        let root = 造一个仓(true);
        assert_eq!(probe(&root).unwrap(), WriteProbe::ReadOnly);
    }

    #[test]
    fn 可写的本地仓绑不上() {
        // `RPO-013`：XOps 在任何代码路径上都不持有仓库写权限。
        // 本地仓不是这条的例外 —— **它只是换了一种证明方式**。
        let root = 造一个仓(false);
        assert_eq!(probe(&root).unwrap(), WriteProbe::Writable);
    }

    #[test]
    fn 分不清的时候报错不猜() {
        let missing = std::env::temp_dir().join(format!("没有这个-{}", xops_core::Id::generate()));
        assert!(probe(&missing).is_err(), "路径不存在");

        let 不是仓 =
            std::env::temp_dir().join(format!("xops-notrepo-{}", xops_core::Id::generate()));
        std::fs::create_dir_all(&不是仓).unwrap();
        let error = probe(&不是仓).unwrap_err();
        assert!(
            error.message().contains("不像一个 Git 仓"),
            "指错目录要当场说，不要绑上去等到取工作区时才炸：{}",
            error.message()
        );

        assert!(probe(Path::new("相对/路径")).is_err(), "相对路径");
    }

    #[test]
    fn 只认file前缀() {
        assert_eq!(
            path_of("file:///srv/x.git"),
            Some(PathBuf::from("/srv/x.git"))
        );
        assert_eq!(path_of("https://host/x.git"), None);
        // scp 式的写法与裸路径分不开，所以裸路径不认。
        assert_eq!(path_of("git@host:x/y.git"), None);
        assert_eq!(path_of("/srv/x.git"), None);
    }
}
