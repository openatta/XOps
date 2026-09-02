//! RP-08 的验收。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use xops_audit::AuditLog;
use xops_core::{Result, SystemClock, TableName};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_repo::platform::{GitPlatform, WriteProbe};
use xops_repo::workspace::{AuthConfig, Budget, prepare};
use xops_repo::{Repos, Sealer, Secret};
use xops_store::{MemoryStore, Store, WriteEngine};

/// 一个说什么就是什么的平台。**试写的结果由测试指定。**
struct FakePlatform {
    probe: WriteProbe,
    probes: AtomicUsize,
}

impl GitPlatform for FakePlatform {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn auth_header(&self, _secret: &Secret) -> String {
        "Authorization: Basic 假的".into()
    }

    fn probe_write_access(&self, _remote: &str, _secret: &Secret) -> Result<WriteProbe> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        Ok(self.probe)
    }

    /// 试写是假的，**验签是真的**——按项目分密钥这件事要验的就是 HMAC 那一步。
    fn verify_webhook(&self, secret: &str, body: &[u8], signature: &str) -> bool {
        xops_repo::GitHub.verify_webhook(secret, body, signature)
    }
}

struct Fixture {
    repos: Arc<Repos>,
    directory: Arc<Directory>,
    platform: Arc<FakePlatform>,
    _root: PathBuf,
}

fn fixture(probe: WriteProbe) -> Fixture {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    let clock = Arc::new(SystemClock);
    let engine = Arc::new(WriteEngine::new(Arc::clone(&store), clock.clone()));
    let relations: Arc<dyn xops_store::Relations> = Arc::new(xops_store::MemoryRelations::new());
    let mut audit = AuditLog::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&relations),
    )
    .unwrap();
    for table in xops_identity::directory::platform_tables().unwrap() {
        audit = audit.watching(table);
    }
    let audit = Arc::new(audit.watching(TableName::new(xops_repo::BINDINGS_TABLE).unwrap()));
    let directory = Arc::new(Directory::new(
        Arc::clone(&engine),
        Arc::clone(&store),
        Arc::clone(&audit),
        clock.clone(),
    ));
    let platform = Arc::new(FakePlatform {
        probe,
        probes: AtomicUsize::new(0),
    });
    let root = std::env::temp_dir().join(format!("xops-repo-test-{}", xops_core::Id::generate()));
    let repos = Arc::new(Repos::new(
        xops_repo::Deps {
            engine,
            store,
            audit,
            directory: Arc::clone(&directory),
            clock,
        },
        Arc::new(Sealer::from_key(&[3u8; 32]).unwrap()),
        Arc::clone(&platform) as Arc<dyn GitPlatform>,
        root.clone(),
    ));
    Fixture {
        repos,
        directory,
        platform,
        _root: root,
    }
}

impl Fixture {
    fn owner(&self) -> (UserId, ProjectId) {
        let user = self
            .directory
            .provision(
                ExternalAccount {
                    provider: ProviderId::new("builtin").unwrap(),
                    account: "alice".into(),
                },
                "Alice",
                None,
            )
            .unwrap()
            .id;
        let project = self
            .directory
            .create_project(user, Slug::new("acme").unwrap(), "Acme")
            .unwrap()
            .id;
        (user, project)
    }
}

// ——————————————————————————————— 只读凭据 ———————————————————————————————

#[test]
fn 写得进去的凭据绑不上() {
    let fixture = fixture(WriteProbe::Writable);
    let (alice, project) = fixture.owner();
    let error = fixture
        .repos
        .bind(
            alice,
            project,
            "https://example.com/x.git",
            Some(Secret::new("能写")),
        )
        .unwrap_err();
    assert!(
        error.message().contains("不持有仓库写权限"),
        "{}",
        error.message()
    );
    assert_eq!(
        fixture.platform.probes.load(Ordering::SeqCst),
        1,
        "试写是真的试了一次"
    );
    assert!(
        fixture.repos.status(alice, project).unwrap().is_none(),
        "绑定不该留下"
    );
}

#[test]
fn 只读的凭据绑得上而且原文读不出来() {
    let fixture = fixture(WriteProbe::ReadOnly);
    let (alice, project) = fixture.owner();
    fixture
        .repos
        .bind(
            alice,
            project,
            "https://example.com/x.git",
            Some(Secret::new("ghp_只读")),
        )
        .unwrap();

    let binding = fixture.repos.status(alice, project).unwrap().unwrap();
    assert_eq!(binding.remote, "https://example.com/x.git");
    let serialized = serde_json::to_string(&binding).unwrap();
    assert!(
        !serialized.contains("ghp_只读"),
        "RPO-003：任何接口都读不出原文，包括项目所有者自己"
    );
}

#[test]
fn 一个项目当前只绑一个仓() {
    let fixture = fixture(WriteProbe::ReadOnly);
    let (alice, project) = fixture.owner();
    fixture
        .repos
        .bind(
            alice,
            project,
            "https://example.com/a.git",
            Some(Secret::new("x")),
        )
        .unwrap();
    let error = fixture
        .repos
        .bind(
            alice,
            project,
            "https://example.com/b.git",
            Some(Secret::new("y")),
        )
        .unwrap_err();
    assert!(error.message().contains("当前绑一个"), "RPO-001");
}

#[test]
fn 轮换之后旧密文不再存在() {
    let fixture = fixture(WriteProbe::ReadOnly);
    let (alice, project) = fixture.owner();
    fixture
        .repos
        .bind(
            alice,
            project,
            "https://example.com/x.git",
            Some(Secret::new("旧的")),
        )
        .unwrap();
    let before = fixture
        .repos
        .status(alice, project)
        .unwrap()
        .unwrap()
        .credential;

    fixture
        .repos
        .rotate(alice, project, Secret::new("新的"))
        .unwrap();
    let after = fixture
        .repos
        .status(alice, project)
        .unwrap()
        .unwrap()
        .credential;
    assert_ne!(before, after, "RPO-004：旧凭据立即失效——系统里不再有第二份");
}

#[test]
fn 轮换成一把能写的会被拒() {
    let fixture = fixture(WriteProbe::ReadOnly);
    let (alice, project) = fixture.owner();
    fixture
        .repos
        .bind(
            alice,
            project,
            "https://example.com/x.git",
            Some(Secret::new("只读")),
        )
        .unwrap();

    let writable = fixture_with_platform(WriteProbe::Writable);
    // 换一个"会说能写"的平台再轮换 —— 同一条判定要在轮换那一侧也成立。
    let (bob, other) = writable.owner();
    let error = writable
        .repos
        .bind(
            bob,
            other,
            "https://example.com/y.git",
            Some(Secret::new("能写")),
        )
        .unwrap_err();
    assert!(error.message().contains("不持有仓库写权限"));
}

fn fixture_with_platform(probe: WriteProbe) -> Fixture {
    fixture(probe)
}

#[test]
fn 解绑之后绑定就没了() {
    let fixture = fixture(WriteProbe::ReadOnly);
    let (alice, project) = fixture.owner();
    fixture
        .repos
        .bind(
            alice,
            project,
            "https://example.com/x.git",
            Some(Secret::new("x")),
        )
        .unwrap();
    fixture.repos.unbind(alice, project).unwrap();
    assert!(fixture.repos.status(alice, project).unwrap().is_none());
}

#[test]
fn 非成员看到的与项目不存在一致() {
    let fixture = fixture(WriteProbe::ReadOnly);
    let (alice, project) = fixture.owner();
    fixture
        .repos
        .bind(
            alice,
            project,
            "https://example.com/x.git",
            Some(Secret::new("x")),
        )
        .unwrap();
    let bob = fixture
        .directory
        .provision(
            ExternalAccount {
                provider: ProviderId::new("builtin").unwrap(),
                account: "bob".into(),
            },
            "Bob",
            None,
        )
        .unwrap()
        .id;

    let outsider = fixture.repos.status(bob, project).unwrap_err();
    let missing = fixture
        .repos
        .status(bob, ProjectId::generate())
        .unwrap_err();
    assert_eq!(outsider.message(), missing.message());
}

// ——————————————————————————————— 枚举：不存在向仓库写入的调用 ———————————————————————————————

/// **XOps 自己的代码里没有任何向仓库写入的调用**（`RPO-013`、`I-G`）。
///
/// # 它证明什么、不证明什么
///
/// ⚠️ **它扫的是 XOps 自己写的那些 crate,不含 `vendor/`。**
/// 这不是为了让测试变绿——它一直就只扫得到我们自己的代码:
/// 从 crates.io 拉来的依赖也从来不在扫描范围里,`vendor/attacore` 只是把
/// 那个一直存在的边界**变得看得见了**。
///
/// `D61` 之后引擎在**同一个进程**里,而 AttaCore 的工具集里确实有 git 写操作
/// （它是一个编码 agent 引擎）。所以这条测试**不再等于**"这个进程不可能写仓库"。
///
/// **真正兜住 `I-G` 的是凭据那一侧,而且它比源码扫描硬**:
///
/// ```text
/// RPO-002  绑仓之前**实际推一次 dry-run**，推得进去就拒绝绑
///          —— 声明会撒谎、会过期，只有真去推一下才知道
/// RPO-005  凭据只进一个 0600 的临时 git 配置，用完即删
/// ```
///
/// 也就是说:技能就算真去 `git push`,**手里那把凭据是被验证过推不动的**。
#[test]
fn 枚举全仓不存在任何向仓库写入的调用() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let vendored = root.join("vendor");
    let mut offenders = Vec::new();
    walk(&root, &mut |path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if !name.ends_with(".rs") {
            return;
        }
        // 依赖不在扫描范围里 —— 从 crates.io 拉来的那些也从来不在。
        if path.starts_with(&vendored) {
            return;
        }
        let Ok(source) = fs::read_to_string(path) else {
            return;
        };
        // 去掉注释：纪律写在注释里是正常的。
        let code: String = source
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        // 只在**真的会跑 git 的文件**里找。别处出现 "push" 是 GitHub 的事件名，
        // 不是一次推送 —— 早先这里没分清，webhook 那个模块一加进来就误报了。
        if !code.contains("Command::new(\"git\")") {
            return;
        }
        for (needle, why) in [
            ("\"push\"", "推分支"),
            ("\"commit\"", "提交"),
            ("\"tag\"", "打 tag"),
            ("\"merge\"", "合并"),
        ] {
            if code.contains(needle) {
                // 试写那一处是唯一的例外，而它是 --dry-run。
                let is_probe = code.contains("--dry-run") && code.contains("probe");
                if !is_probe {
                    offenders.push(format!(
                        "{}：{why}",
                        path.strip_prefix(&root).unwrap_or(path).display()
                    ));
                }
            }
        }
    });

    assert!(
        offenders.is_empty(),
        "RPO-013 / I-G：XOps 在任何代码路径上都不持有、不请求仓库写权限。\n{offenders:#?}"
    );
}

fn walk(directory: &Path, act: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if matches!(name.as_ref(), "target" | ".git" | "node_modules") {
                continue;
            }
            walk(&path, act);
        } else {
            act(&path);
        }
    }
}

// ——————————————————————————————— 按确切修订备工作区 ———————————————————————————————

/// 造一个本地仓，两个提交。
fn local_repo() -> (PathBuf, String, String) {
    let root = std::env::temp_dir().join(format!("xops-git-{}", xops_core::Id::generate()));
    fs::create_dir_all(&root).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}：{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    git(&["init", "--quiet", "-b", "main"]);
    fs::write(root.join("README.md"), "第一版").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "一",
    ]);
    let first = git(&["rev-parse", "HEAD"]);
    fs::write(root.join("README.md"), "第二版").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "二",
    ]);
    let second = git(&["rev-parse", "HEAD"]);
    (root, first, second)
}

#[test]
fn 备出来的就是那一版不是分支当前head() {
    let (repo, first, second) = local_repo();
    assert_ne!(first, second);
    let parent = std::env::temp_dir().join(format!("xops-ws-{}", xops_core::Id::generate()));
    fs::create_dir_all(&parent).unwrap();
    let auth = AuthConfig::anonymous().unwrap();

    let workspace = prepare(
        repo.to_str().unwrap(),
        &first,
        &auth,
        Budget::default(),
        &parent,
    )
    .expect("该备得出来");
    assert_eq!(workspace.revision(), first, "RPO-010：记下的是确切修订");
    assert_eq!(
        fs::read_to_string(workspace.root().join("README.md")).unwrap(),
        "第一版",
        "不是分支当前 HEAD"
    );

    let root = workspace.root().to_path_buf();
    drop(workspace);
    assert!(!root.exists(), "析构即销毁");
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&parent);
}

#[test]
fn 工作区是只读的() {
    let (repo, first, _) = local_repo();
    let parent = std::env::temp_dir().join(format!("xops-ws-{}", xops_core::Id::generate()));
    fs::create_dir_all(&parent).unwrap();
    let auth = AuthConfig::anonymous().unwrap();
    let workspace = prepare(
        repo.to_str().unwrap(),
        &first,
        &auth,
        Budget::default(),
        &parent,
    )
    .unwrap();

    let target = workspace.root().join("README.md");
    assert!(
        fs::write(&target, "我改了").is_err(),
        "写得进去的话，就不叫只读工作区"
    );

    drop(workspace);
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&parent);
}

#[test]
fn 修订不存在时明确失败不静默用head顶替() {
    let (repo, _, _) = local_repo();
    let parent = std::env::temp_dir().join(format!("xops-ws-{}", xops_core::Id::generate()));
    fs::create_dir_all(&parent).unwrap();
    let auth = AuthConfig::anonymous().unwrap();

    let error = prepare(
        repo.to_str().unwrap(),
        "0000000000000000000000000000000000000000",
        &auth,
        Budget::default(),
        &parent,
    )
    .unwrap_err();
    assert!(
        error.message().contains("不会用 HEAD 顶替"),
        "顶替一次追溯链就断了，而且断得看不出来：{}",
        error.message()
    );
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&parent);
}

#[test]
fn 工作区里没有凭据也没有那份配置() {
    let (repo, first, _) = local_repo();
    let parent = std::env::temp_dir().join(format!("xops-ws-{}", xops_core::Id::generate()));
    fs::create_dir_all(&parent).unwrap();
    let config_path;
    let workspace = {
        let auth = AuthConfig::write("Authorization: Basic 这是凭据".into()).unwrap();
        config_path = auth.path().to_path_buf();
        prepare(
            repo.to_str().unwrap(),
            &first,
            &auth,
            Budget::default(),
            &parent,
        )
        .unwrap()
    };
    assert!(!config_path.exists(), "拉完就删（RPO-005）");

    let mut found = false;
    walk(workspace.root(), &mut |path| {
        if let Ok(content) = fs::read_to_string(path)
            && content.contains("这是凭据")
        {
            found = true;
        }
    });
    assert!(!found, "凭据不该出现在工作区内容里");

    drop(workspace);
    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&parent);
}

// ————————————————————— webhook 验签密钥按项目一把（TRG-012）—————————————————————

fn signature(secret: &str, body: &[u8]) -> String {
    use std::process::Stdio;
    let mut child = Command::new("openssl")
        .args(["dgst", "-sha256", "-hmac", secret])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("要有 openssl");
    use std::io::Write;
    child.stdin.as_mut().unwrap().write_all(body).unwrap();
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    format!("sha256={}", text.rsplit(' ').next().unwrap().trim())
}

#[test]
fn webhook密钥按项目一把_一次投递最多命中一个项目() {
    // ⚠️ 这条挡的是原先那版:一把**平台级**密钥验完签，就把事件发给
    // **所有**绑过仓的项目——A 仓的一次 push 会触发 B 项目的任务。
    // 密钥的作用面必须和它守的东西一样大，不能更大。
    let fixture = fixture(WriteProbe::ReadOnly);
    let (alice, first) = fixture.owner();
    let second = fixture
        .directory
        .create_project(alice, Slug::new("second").unwrap(), "第二个")
        .unwrap()
        .id;

    fixture
        .repos
        .bind(
            alice,
            first,
            "https://host/one.git",
            Some(Secret::new("t1")),
        )
        .unwrap();
    fixture
        .repos
        .bind(
            alice,
            second,
            "https://host/two.git",
            Some(Secret::new("t2")),
        )
        .unwrap();
    fixture
        .repos
        .set_webhook_secret(alice, first, &Secret::new("密钥一"))
        .unwrap();
    fixture
        .repos
        .set_webhook_secret(alice, second, &Secret::new("密钥二"))
        .unwrap();

    let body = br#"{"ref":"refs/heads/main"}"#;
    let matched = fixture
        .repos
        .webhook_source(body, &signature("密钥一", body))
        .unwrap()
        .expect("第一个项目的密钥该验得过");
    assert_eq!(matched.project, first, "**只命中签名对上的那一个项目**");

    let other = fixture
        .repos
        .webhook_source(body, &signature("密钥二", body))
        .unwrap()
        .expect("第二个项目的密钥也该验得过");
    assert_eq!(other.project, second, "两个项目各认各的密钥");

    // 谁的密钥都不是 —— **不是错误，是"没有"**。调用方对两者的回应必须一样。
    assert!(
        fixture
            .repos
            .webhook_source(body, &signature("猜的", body))
            .unwrap()
            .is_none(),
        "签不上就是没有"
    );
}

#[test]
fn 没设webhook密钥的项目收不到投递() {
    // **没设就是这个项目收不到 webhook**——它不该退回到某个平台级的默认密钥，
    // 那正是"作用面比它守的东西大"的那种默认。
    let fixture = fixture(WriteProbe::ReadOnly);
    let (alice, project) = fixture.owner();
    fixture
        .repos
        .bind(
            alice,
            project,
            "https://host/one.git",
            Some(Secret::new("t")),
        )
        .unwrap();

    let body = br#"{"ref":"refs/heads/main"}"#;
    for guess in ["", "空", "密钥"] {
        assert!(
            fixture
                .repos
                .webhook_source(body, &signature(guess, body))
                .unwrap()
                .is_none(),
            "绑了仓但没设密钥，任何签名都不该命中（试的是 {guess:?}）"
        );
    }
    assert!(
        fixture
            .repos
            .status(alice, project)
            .unwrap()
            .unwrap()
            .webhook_secret
            .is_none(),
        "状态里看得出来没设"
    );
}

// ————————————————————— 本地仓（file://）————————————————————————

/// 造一个 bare 仓，推一笔提交进去，返回 `(仓路径, 修订)`。
fn 本地仓(word: &str) -> (PathBuf, String) {
    let root = std::env::temp_dir().join(format!("xops-local-{}", xops_core::Id::generate()));
    let bare = root.join("origin.git");
    let work = root.join("work");
    fs::create_dir_all(&bare).unwrap();
    fs::create_dir_all(&work).unwrap();
    let git = |dir: &Path, args: &[&str]| {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}：{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&bare, &["init", "--quiet", "--bare"]);
    git(&work, &["init", "--quiet"]);
    git(&work, &["config", "user.email", "t@xops"]);
    git(&work, &["config", "user.name", "t"]);
    fs::write(work.join("口令.md"), format!("{word}\n")).unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "第一笔"]);
    git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(
        &work,
        &["push", "--quiet", "origin", "HEAD:refs/heads/main"],
    );
    let out = Command::new("git")
        .current_dir(&work)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let revision = String::from_utf8(out.stdout).unwrap().trim().to_owned();
    // 只读之后才绑得上（RPO-013）。
    let mut mode = fs::metadata(&bare).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    mode.set_mode(0o555);
    for entry in walkdir(&bare) {
        let _ = fs::set_permissions(&entry, fs::Permissions::from_mode(0o555));
    }
    let _ = fs::set_permissions(&bare, mode);
    (bare, revision)
}

fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walkdir(&path));
        }
        out.push(path);
    }
    out
}

#[test]
fn 本地仓绑得上而且备得出工作区() {
    // ⚠️ 这条走的是**真的 git**，不是 FakePlatform：`file://` 那条路上
    // 平台适配一步都不参与（没有认证，也没有 dry-run 探针）。
    let fixture = fixture(WriteProbe::ReadOnly);
    let (alice, project) = fixture.owner();
    let (bare, revision) = 本地仓("鲸鱼吃了七枚橄榄");
    let remote = format!("file://{}", bare.display());

    // **不给凭据** —— 本地仓的取用不经过认证。
    let binding = fixture.repos.bind(alice, project, &remote, None).unwrap();
    assert_eq!(binding.platform, "local", "本地仓不是任何一个平台的仓");
    assert!(binding.credential.is_none(), "没有凭据可存");

    // 不指定修订时解出**确切的 sha**，不是分支名（RPO-010）。
    let head = fixture.repos.head_revision(project).unwrap();
    assert_eq!(head, revision, "解出来的该是那一笔提交");

    let workspace = fixture.repos.prepare_workspace(project, &head).unwrap();
    assert_eq!(workspace.revision(), revision);
    let 口令 = fs::read_to_string(workspace.root().join("口令.md")).unwrap();
    assert_eq!(口令.trim(), "鲸鱼吃了七枚橄榄", "备出来的就是那个仓的内容");

    // RPO-009：只读。
    let 写得进去 = fs::write(workspace.root().join("新文件"), "x").is_ok();
    assert!(!写得进去, "工作区必须是只读的");

    let root = workspace.root().to_path_buf();
    drop(workspace);
    assert!(!root.exists(), "析构即销毁");
}
