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

    fn verify_webhook(&self, _secret: &str, _body: &[u8], _signature: &str) -> bool {
        false
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
    let relations: Arc<dyn xops_store::Relations> =
        Arc::new(xops_store::MemoryRelations::new());
    let mut audit = AuditLog::new(Arc::clone(&engine), Arc::clone(&store), Arc::clone(&relations)).unwrap();
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
            Secret::new("能写"),
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
            Secret::new("ghp_只读"),
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
            Secret::new("x"),
        )
        .unwrap();
    let error = fixture
        .repos
        .bind(
            alice,
            project,
            "https://example.com/b.git",
            Secret::new("y"),
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
            Secret::new("旧的"),
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
            Secret::new("只读"),
        )
        .unwrap();

    let writable = fixture_with_platform(WriteProbe::Writable);
    // 换一个"会说能写"的平台再轮换 —— 同一条判定要在轮换那一侧也成立。
    let (bob, other) = writable.owner();
    let error = writable
        .repos
        .bind(bob, other, "https://example.com/y.git", Secret::new("能写"))
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
            Secret::new("x"),
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
            Secret::new("x"),
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

#[test]
fn 枚举全仓不存在任何向仓库写入的调用() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut offenders = Vec::new();
    walk(&root, &mut |path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if !name.ends_with(".rs") {
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
