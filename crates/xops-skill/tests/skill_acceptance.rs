//! RP-09 的验收。

use std::fs;
use std::path::Path;
use std::sync::Arc;

use xops_audit::AuditLog;
use xops_core::{Id, Role, SystemClock, TableName};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId, Slug, UserId};
use xops_skill::declaration::{Declaration, Input, InputType, OutputShape};
use xops_skill::{Ownership, SkillId, Skills, State};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};

struct Fixture {
    label: &'static str,
    skills: Arc<Skills>,
    directory: Arc<Directory>,
}

fn fixtures() -> Vec<Fixture> {
    [
        ("memory", Arc::new(MemoryStore::new()) as Arc<dyn Store>),
        ("sqlite", Arc::new(SqliteStore::in_memory().unwrap())),
    ]
    .into_iter()
    .map(|(label, store)| {
        let clock = Arc::new(SystemClock);
        let engine = Arc::new(WriteEngine::new(Arc::clone(&store), clock.clone()));
        let mut audit = AuditLog::new(Arc::clone(&engine), Arc::clone(&store)).unwrap();
        for table in xops_identity::directory::platform_tables().unwrap() {
            audit = audit.watching(table);
        }
        for table in [xops_skill::SKILLS_TABLE, xops_skill::VERSIONS_TABLE] {
            audit = audit.watching(TableName::new(table).unwrap());
        }
        let audit = Arc::new(audit);
        let directory = Arc::new(Directory::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            clock.clone(),
        ));
        let skills = Arc::new(Skills::new(
            engine,
            store,
            audit,
            Arc::clone(&directory),
            clock,
        ));
        Fixture {
            label,
            skills,
            directory,
        }
    })
    .collect()
}

impl Fixture {
    fn user(&self, account: &str) -> UserId {
        self.directory
            .provision(
                ExternalAccount {
                    provider: ProviderId::new("builtin").unwrap(),
                    account: account.into(),
                },
                account,
                None,
            )
            .unwrap()
            .id
    }

    fn project(&self, owner: UserId) -> ProjectId {
        self.directory
            .create_project(owner, Slug::new("acme").unwrap(), "Acme")
            .unwrap()
            .id
    }
}

fn declaration() -> Declaration {
    Declaration {
        inputs: vec![Input {
            name: "target".into(),
            ty: InputType::Text,
            required: true,
            description: "看哪儿".into(),
        }],
        output: OutputShape::Report,
        needs_repository: true,
        network: vec![],
        max_duration_millis: 60_000,
    }
}

// ——————————————————————————————— 上传不执行 ———————————————————————————————

#[test]
fn 建技能这条路径上没有任何提交执行的调用() {
    // 枚举，不是靠读。**这条防的不主要是别人，是作者自己**（SKL-004）。
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service.rs")).unwrap();
    let code: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in ["submit(", "ExecContract", "Worksheet", "xops_exec"] {
        assert!(
            !code.contains(forbidden),
            "技能资产这一层出现了 {forbidden} —— 上传不执行（SKL-004）"
        );
    }
}

#[test]
fn 新建的技能是草稿而且跑不了() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice);
        let resolved = fixture
            .skills
            .create(
                alice,
                project,
                "查缺陷",
                Ownership::Public,
                "看看",
                declaration(),
            )
            .unwrap();
        assert_eq!(resolved.version.state, State::Draft, "{label}");
        assert!(
            !fixture.skills.runnable_for(resolved.skill.id, 1).unwrap(),
            "{label}：草稿不会被任何自动触发路径执行"
        );
    }
}

// ——————————————————————————————— 发布与版本 ———————————————————————————————

#[test]
fn 没测过的版本发布不了() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice);
        let resolved = fixture
            .skills
            .create(
                alice,
                project,
                "查缺陷",
                Ownership::Public,
                "看看",
                declaration(),
            )
            .unwrap();

        let error = fixture
            .skills
            .publish(alice, resolved.skill.id, 1)
            .unwrap_err();
        assert!(error.message().contains("成功的测试执行"), "{label}");

        // 这条不需要 RP-11 完成 —— 伪造一条成功记录即可验收。
        fixture
            .skills
            .record_successful_test(alice, resolved.skill.id, 1, Id::generate())
            .unwrap();
        let published = fixture.skills.publish(alice, resolved.skill.id, 1).unwrap();
        assert_eq!(published.state, State::Published, "{label}");
        assert!(
            fixture.skills.runnable_for(resolved.skill.id, 1).unwrap(),
            "{label}"
        );
    }
}

#[test]
fn 改内容产生新版本旧版本原样可查() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice);
        let resolved = fixture
            .skills
            .create(
                alice,
                project,
                "查缺陷",
                Ownership::Public,
                "第一版",
                declaration(),
            )
            .unwrap();
        fixture
            .skills
            .record_successful_test(alice, resolved.skill.id, 1, Id::generate())
            .unwrap();
        fixture.skills.publish(alice, resolved.skill.id, 1).unwrap();

        let second = fixture
            .skills
            .update(alice, resolved.skill.id, "第二版", declaration())
            .unwrap();
        assert_eq!(second.version, 2, "{label}");
        assert_eq!(second.state, State::Draft, "{label}：新版本从草稿开始");

        let versions = fixture.skills.versions(resolved.skill.id).unwrap();
        let first = versions
            .iter()
            .find(|version| version.version == 1)
            .unwrap();
        assert_eq!(
            first.content, "第一版",
            "{label}：已发布的版本不可变（SKL-002）"
        );
        assert_eq!(first.state, State::Published, "{label}");
    }
}

#[test]
fn 停用之后不再能被触发() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice);
        let resolved = fixture
            .skills
            .create(
                alice,
                project,
                "查缺陷",
                Ownership::Public,
                "看看",
                declaration(),
            )
            .unwrap();
        fixture
            .skills
            .record_successful_test(alice, resolved.skill.id, 1, Id::generate())
            .unwrap();
        fixture.skills.publish(alice, resolved.skill.id, 1).unwrap();
        fixture.skills.disable(alice, resolved.skill.id, 1).unwrap();
        assert!(
            !fixture.skills.runnable_for(resolved.skill.id, 1).unwrap(),
            "{label}"
        );
        // 历史完整保留。
        assert_eq!(
            fixture.skills.versions(resolved.skill.id).unwrap().len(),
            1,
            "{label}"
        );
    }
}

// ——————————————————————————————— 私有与可见性 ———————————————————————————————

#[test]
fn 同项目其他成员看不到也用不了私有技能() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let bob = fixture.user("bob");
        let project = fixture.project(alice);
        fixture
            .directory
            .set_member(alice, project, bob, Role::Member)
            .unwrap();

        let private = fixture
            .skills
            .create(
                alice,
                project,
                "我的",
                Ownership::Private { owner: alice },
                "私货",
                declaration(),
            )
            .unwrap();

        assert!(
            fixture.skills.read(alice, private.skill.id).is_ok(),
            "{label}：本人看得见"
        );
        let error = fixture.skills.read(bob, private.skill.id).unwrap_err();
        let missing = fixture.skills.read(bob, SkillId::generate()).unwrap_err();
        assert_eq!(error.message(), missing.message(), "{label}：与不存在一致");
        assert!(
            !fixture
                .skills
                .list(bob, project)
                .unwrap()
                .iter()
                .any(|r| r.skill.id == private.skill.id),
            "{label}：列不出来"
        );
    }
}

#[test]
fn 退出项目之后他的私有技能立刻不能再执行() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let bob = fixture.user("bob");
        let project = fixture.project(alice);
        fixture
            .directory
            .set_member(alice, project, bob, Role::Member)
            .unwrap();

        let private = fixture
            .skills
            .create(
                bob,
                project,
                "bob 的",
                Ownership::Private { owner: bob },
                "私货",
                declaration(),
            )
            .unwrap();
        fixture
            .skills
            .record_successful_test(bob, private.skill.id, 1, Id::generate())
            .unwrap();
        fixture.skills.publish(bob, private.skill.id, 1).unwrap();
        assert!(
            fixture.skills.runnable_for(private.skill.id, 1).unwrap(),
            "{label}"
        );

        fixture
            .directory
            .remove_member(alice, project, bob)
            .unwrap();
        assert!(
            !fixture.skills.runnable_for(private.skill.id, 1).unwrap(),
            "{label}：SKL-009 —— 权限来自人，不来自技能"
        );
    }
}

#[test]
fn 用于满足过流程节点之后转为可读() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let bob = fixture.user("bob");
        let project = fixture.project(alice);
        fixture
            .directory
            .set_member(alice, project, bob, Role::Member)
            .unwrap();
        let private = fixture
            .skills
            .create(
                alice,
                project,
                "我的",
                Ownership::Private { owner: alice },
                "私货",
                declaration(),
            )
            .unwrap();

        assert!(
            fixture.skills.read(bob, private.skill.id).is_err(),
            "{label}：一开始看不到"
        );
        // 标记由 RP-15 打，本包按它判可见性。
        fixture
            .skills
            .mark_used_for_settlement(private.skill.id, 1)
            .unwrap();
        let seen = fixture.skills.read(bob, private.skill.id).unwrap();
        assert_eq!(
            seen.version.content, "私货",
            "{label}：SKL-011 —— 私有是为了不打扰别人，不是为了让自动决策不可审查"
        );
    }
}

#[test]
fn 派生是拷贝不是引用() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice);
        let public = fixture
            .skills
            .create(
                alice,
                project,
                "公共的",
                Ownership::Public,
                "原样",
                declaration(),
            )
            .unwrap();

        let copy = fixture
            .skills
            .derive_private(alice, public.skill.id)
            .unwrap();
        assert_ne!(copy.skill.id, public.skill.id, "{label}");
        assert_eq!(copy.version.content, "原样", "{label}");
        assert!(
            matches!(copy.skill.ownership, Ownership::Private { .. }),
            "{label}"
        );

        // 改私有副本，公共的不受影响。
        fixture
            .skills
            .update(alice, copy.skill.id, "改过了", declaration())
            .unwrap();
        let original = fixture.skills.read(alice, public.skill.id).unwrap();
        assert_eq!(original.version.content, "原样", "{label}：是拷贝不是引用");
    }
}

#[test]
fn 别人的私有技能改不了() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let bob = fixture.user("bob");
        let project = fixture.project(alice);
        fixture
            .directory
            .set_member(alice, project, bob, Role::Member)
            .unwrap();
        let private = fixture
            .skills
            .create(
                alice,
                project,
                "我的",
                Ownership::Private { owner: alice },
                "私货",
                declaration(),
            )
            .unwrap();
        assert!(
            fixture
                .skills
                .update(bob, private.skill.id, "我改", declaration())
                .is_err(),
            "{label}"
        );
    }
}

#[test]
fn 只能给自己建私有技能() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let bob = fixture.user("bob");
        let project = fixture.project(alice);
        assert!(
            fixture
                .skills
                .create(
                    alice,
                    project,
                    "替 bob 建的",
                    Ownership::Private { owner: bob },
                    "x",
                    declaration()
                )
                .is_err(),
            "{label}"
        );
    }
}

// ——————————————————————————————— 声明 ———————————————————————————————

#[test]
fn 声明之外没有第五条获取能力的途径() {
    // 四样：输入契约 · 产出形态 · 是否读仓 + 出网白名单 · 时长上限。
    let value = serde_json::to_value(declaration()).unwrap();
    let keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys.len(),
        5,
        "四样声明摊成五个字段（读仓与出网各一个）：{keys:?}"
    );
    for expected in [
        "inputs",
        "output",
        "needs_repository",
        "network",
        "max_duration_millis",
    ] {
        assert!(keys.contains(&expected), "少了 {expected}");
    }
}

#[test]
fn 声明不合法的技能建不出来() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice");
        let project = fixture.project(alice);
        let mut broken = declaration();
        broken.max_duration_millis = 0;
        assert!(
            fixture
                .skills
                .create(alice, project, "坏的", Ownership::Public, "x", broken)
                .is_err(),
            "{label}"
        );
    }
}
