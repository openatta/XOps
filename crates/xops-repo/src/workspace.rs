//! 按确切修订准备一份只读工作区。
//!
//! `RPO-008`：**工作区在容器外准备**，定位到确切修订，交给执行方；
//! `RPO-010`：**每次读取记下确切修订**——否则"这份报告针对哪版代码"没法回答。
//!
//! ⚠️ **修订不存在时明确失败，不静默用 HEAD 顶替**。这条是 `XFG` 那句
//! "gitHead 必须已推送"的落点：顶替一次，整条追溯链就断了，而且断得看不出来。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use xops_core::{Error, Result};

/// 拉取的容量与时间上限（`RPO-011`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub max_bytes: u64,
    pub max_millis: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_bytes: 2 * 1024 * 1024 * 1024,
            max_millis: 10 * 60 * 1_000,
        }
    }
}

/// 一个临时的 git 配置文件，**凭据只活在它里面**。
///
/// 为什么不是环境变量、不是命令行参数（`RPO-005`）：
///
/// ```text
/// 命令行参数   ps 看得见，别的进程也看得见
/// 环境变量     子进程整个继承过去，过程记录里也常常带上
/// 配置文件     0600、用完即删、路径本身不是秘密
/// ```
///
/// 残留风险认下来：**拉取期间它在磁盘上**。容器碰不到它（它在容器外），
/// 它不进 argv、不进环境、不进过程记录，但它确实在那儿几秒钟。
pub struct AuthConfig {
    path: PathBuf,
}

impl AuthConfig {
    /// 写一份带认证头的配置。
    ///
    /// # Errors
    /// 写不出临时文件。
    pub fn write(header: String) -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "xops-git-{}-{}.config",
            std::process::id(),
            xops_core::Id::generate()
        ));
        fs::write(&path, format!("[http]\n\textraHeader = {header}\n"))
            .map_err(|error| Error::internal(format!("写不出 git 配置：{error}")))?;
        harden(&path)?;
        Ok(Self { path })
    }

    /// 一份不带凭据的空配置。公开仓用它。
    ///
    /// # Errors
    /// 写不出临时文件。
    pub fn anonymous() -> Result<Self> {
        Self::write_raw("")
    }

    fn write_raw(body: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "xops-git-{}-{}.config",
            std::process::id(),
            xops_core::Id::generate()
        ));
        fs::write(&path, body)
            .map_err(|error| Error::internal(format!("写不出 git 配置：{error}")))?;
        harden(&path)?;
        Ok(Self { path })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for AuthConfig {
    fn drop(&mut self) {
        // 用完即删。**这一步不能等 GC、不能等重启。**
        let _ = fs::remove_file(&self.path);
    }
}

fn harden(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| Error::internal(format!("改不了权限：{error}")))
}

/// 一份备好的只读工作区。
///
/// **析构即销毁**：`RPO-008` 说容器内的任何写入不回流仓库，且随容器一并销毁——
/// 我们这一侧的对应动作就是这个 `Drop`。
#[derive(Debug)]
pub struct Workspace {
    root: PathBuf,
    revision: String,
}

impl Workspace {
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 这份工作区**确切**是哪个修订（`RPO-010`）。
    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        // 只读目录删起来要先放开权限。
        let _ = relax(&self.root);
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// 这个远端此刻的默认分支指向哪个 sha。
///
/// # Errors
/// 连不上、仓是空的、解不出。**空仓明确失败**——
/// 拿不到修订就备不了工作区，而"备了一个空的"比失败更难查。
pub fn head_of(remote: &str, auth: &AuthConfig) -> Result<String> {
    let output = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", auth.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["ls-remote", "--symref", remote, "HEAD"])
        .output()
        .map_err(|error| Error::unavailable(format!("跑不起来 git：{error}")))?;
    if !output.status.success() {
        return Err(Error::unavailable(format!(
            "问不到这个仓的当前修订：{}",
            scrub(&String::from_utf8_lossy(&output.stderr))
        )));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            let (sha, name) = line.split_once(char::is_whitespace)?;
            (name.trim() == "HEAD" && sha.len() == 40).then(|| sha.to_owned())
        })
        .ok_or_else(|| Error::not_found("这个仓上解不出 HEAD —— 它可能是空的"))
}

/// 备一份工作区。
///
/// # Errors
/// 修订不存在 · 拉取超时或超量 · git 跑不起来 · 凭据不对。
/// **每一种都归到工作区错误那一类**，由调用方翻译成 `FailureKind::Workspace`。
pub fn prepare(
    remote: &str,
    revision: &str,
    auth: &AuthConfig,
    budget: Budget,
    parent: &Path,
) -> Result<Workspace> {
    if revision.trim().is_empty() {
        return Err(Error::invalid("要备工作区就得给一个确切修订"));
    }
    let root = parent.join(format!("ws-{}", xops_core::Id::generate()));
    fs::create_dir_all(&root)
        .map_err(|error| Error::internal(format!("建不了工作区目录：{error}")))?;
    let started = Instant::now();

    let mut guard = Cleanup {
        root: Some(root.clone()),
    };
    run_git(auth, &root, &["init", "--quiet"], budget, started)?;
    run_git(
        auth,
        &root,
        &["remote", "add", "origin", remote],
        budget,
        started,
    )?;
    // 只取那一个修订。**不是 fetch 整个分支再 checkout**——
    // 那样在"分支往前动了"的时候会拿到别的东西。
    let fetched = run_git(
        auth,
        &root,
        &["fetch", "--quiet", "--depth", "1", "origin", revision],
        budget,
        started,
    );
    if fetched.is_err() {
        // RPO-008 那条验收：**修订不存在就明确失败，不静默用 HEAD 顶替。**
        return Err(Error::not_found(format!(
            "取不到修订 {revision}：它可能还没被推上去。**不会用 HEAD 顶替**——顶替一次，追溯链就断了"
        )));
    }
    run_git(
        auth,
        &root,
        &["checkout", "--quiet", "FETCH_HEAD"],
        budget,
        started,
    )?;

    let resolved = capture(auth, &root, &["rev-parse", "HEAD"])?;
    let size = directory_size(&root);
    if size > budget.max_bytes {
        return Err(Error::invalid(format!(
            "工作区 {size} 字节，超过上限 {}（RPO-011）",
            budget.max_bytes
        )));
    }
    // 只读：容器那一侧写不回来，我们这一侧也不指望它自觉。
    make_read_only(&root)?;
    guard.root = None;
    Ok(Workspace {
        root,
        revision: resolved.trim().to_owned(),
    })
}

struct Cleanup {
    root: Option<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(root) = self.root.take() {
            let _ = relax(&root);
            let _ = fs::remove_dir_all(root);
        }
    }
}

fn run_git(
    auth: &AuthConfig,
    root: &Path,
    args: &[&str],
    budget: Budget,
    started: Instant,
) -> Result<()> {
    if started.elapsed() > Duration::from_millis(budget.max_millis) {
        return Err(Error::timeout("拉取超时（RPO-011）"));
    }
    let output = command(auth, root)
        .args(args)
        .output()
        .map_err(|error| Error::unavailable(format!("跑不起来 git：{error}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "git {} 失败：{}",
        args.first().copied().unwrap_or("?"),
        scrub(&String::from_utf8_lossy(&output.stderr))
    )))
}

fn capture(auth: &AuthConfig, root: &Path, args: &[&str]) -> Result<String> {
    let output = command(auth, root)
        .args(args)
        .output()
        .map_err(|error| Error::unavailable(format!("跑不起来 git：{error}")))?;
    if !output.status.success() {
        return Err(Error::invalid(scrub(&String::from_utf8_lossy(
            &output.stderr,
        ))));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn command(auth: &AuthConfig, root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        // 凭据在这个文件里，不在 argv、不在环境（`RPO-005`）。
        .env("GIT_CONFIG_GLOBAL", auth.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // 绝不弹交互提示：那会让一次拉取永远挂着。
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

/// 错误消息里不许出现凭据。**`RPO-005`：不进过程记录、不进日志、不进错误消息。**
fn scrub(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.to_ascii_lowercase().contains("authorization")
                || line.contains("://")
                    && (line.contains('@') || line.to_ascii_lowercase().contains("token"))
            {
                "<含凭据的一行已抹掉>"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

fn make_read_only(root: &Path) -> Result<()> {
    walk(root, &mut |path| {
        use std::os::unix::fs::PermissionsExt;
        let Ok(metadata) = fs::metadata(path) else {
            return;
        };
        let mode = metadata.permissions().mode();
        // 去掉全部写位。目录保留 x，否则进不去。
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o555));
    });
    Ok(())
}

fn relax(root: &Path) -> Result<()> {
    walk(root, &mut |path| {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
    });
    Ok(())
}

fn walk(root: &Path, act: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(root) else {
        act(root);
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, act);
        } else {
            act(&path);
        }
    }
    act(root);
}

fn directory_size(root: &Path) -> u64 {
    let mut total = 0;
    walk(root, &mut |path| {
        if let Ok(metadata) = fs::metadata(path)
            && metadata.is_file()
        {
            total += metadata.len();
        }
    });
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 配置文件用完即删且是私有的() {
        use std::os::unix::fs::PermissionsExt;
        let path = {
            let config = AuthConfig::write("Authorization: Basic 秘密".into()).unwrap();
            let path = config.path().to_path_buf();
            assert!(path.exists());
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "别人读得到就白搭了");
            assert!(fs::read_to_string(&path).unwrap().contains("秘密"));
            path
        };
        assert!(!path.exists(), "析构即删除");
    }

    #[test]
    fn 错误消息里的凭据被抹掉() {
        let dirty = "fatal: unable to access\nAuthorization: Basic eGFiYzo=\nremote: 403";
        let clean = scrub(dirty);
        assert!(!clean.contains("eGFiYzo="), "RPO-005：不进错误消息");
        assert!(clean.contains("403"), "有用的那部分要留着");
    }

    #[test]
    fn 空修订不给备() {
        let auth = AuthConfig::anonymous().unwrap();
        let error = prepare(
            "https://example.invalid/x.git",
            "  ",
            &auth,
            Budget::default(),
            Path::new("/tmp"),
        )
        .unwrap_err();
        assert!(error.message().contains("确切修订"));
    }
}
