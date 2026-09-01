//! RP-02 的验收。
//!
//! 每一条都对两个存储实现各跑一遍——`CON-012` 的换实现验收不是 RP-01 一个包的事，
//! 它是**上面每一层都不该知道自己跑在谁上面**。

use std::sync::Arc;

use xops_audit::{AuditLog, Query, kinds};
use xops_core::{Actor, Clock, FixedClock, Role, TableName, Timestamp};
use xops_identity::{
    Action, BuiltinProvider, Directory, ExternalAccount, ProviderId, Slug, UserId,
};
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};

struct Fixture {
    label: &'static str,
    directory: Directory,
    audit: Arc<AuditLog>,
    engine: Arc<WriteEngine>,
    clock: Arc<FixedClock>,
}

impl Fixture {
    fn build(label: &'static str, store: Arc<dyn Store>) -> Self {
        let clock = Arc::new(FixedClock::new(1_700_000_000_000));
        let engine = Arc::new(WriteEngine::new(Arc::clone(&store), clock.clone()));
        let relations: Arc<dyn xops_store::Relations> =
            Arc::new(xops_store::MemoryRelations::new());
        let mut audit = AuditLog::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&relations),
        )
        .unwrap();
        for table in xops_identity::directory::platform_tables().unwrap() {
            audit = audit.watching(table);
        }
        let audit = Arc::new(audit);
        let directory = Directory::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            clock.clone(),
        )
        .with_provider(Box::new(
            BuiltinProvider::new()
                .with_account("root", "s3cret", "管理员")
                .with_account("alice", "hunter2", "Alice"),
        ));
        Self {
            label,
            directory,
            audit,
            engine,
            clock,
        }
    }

    /// 预置一个用户（`IDN-003` 关着自注册，所以要走 provision 这条路）。
    fn user(&self, account: &str, name: &str) -> UserId {
        self.directory
            .provision(
                ExternalAccount {
                    provider: ProviderId::new("builtin").unwrap(),
                    account: account.into(),
                },
                name,
                None,
            )
            .unwrap()
            .id
    }
}

fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture::build("memory", Arc::new(MemoryStore::new())),
        Fixture::build("sqlite", Arc::new(SqliteStore::in_memory().unwrap())),
    ]
}

// ——————————————————————————————— 身份与令牌 ———————————————————————————————

#[test]
fn 自注册默认关闭且不留下任何用户记录() {
    for fixture in fixtures() {
        let label = fixture.label;
        // root 被预置在提供方里，但 XOps 里还没有他的用户记录。
        let error = fixture
            .directory
            .login("builtin", "root", "s3cret")
            .unwrap_err();
        assert_eq!(error.message(), "凭证不对", "{label}");
        assert!(
            fixture
                .directory
                .user_by_account(&ExternalAccount {
                    provider: ProviderId::new("builtin").unwrap(),
                    account: "root".into(),
                })
                .unwrap()
                .is_none(),
            "{label}：IDN-003 —— 被拒绝时不创建任何用户记录"
        );
        let table = TableName::new(xops_identity::USERS).unwrap();
        assert_eq!(
            fixture.engine.last_seq(&table).unwrap(),
            0,
            "{label}：连事件都不该有一条"
        );
    }
}

#[test]
fn 预置过的账号登得进来() {
    for fixture in fixtures() {
        let label = fixture.label;
        let id = fixture.user("root", "管理员");
        let user = fixture
            .directory
            .login("builtin", "root", "s3cret")
            .unwrap();
        assert_eq!(user.id, id, "{label}：登录不该再造一个人");
        assert!(
            fixture.directory.login("builtin", "root", "wrong").is_err(),
            "{label}"
        );
    }
}

#[test]
fn 自注册打开之后陌生账号才建得出用户() {
    for (label, store) in [
        ("memory", Arc::new(MemoryStore::new()) as Arc<dyn Store>),
        ("sqlite", Arc::new(SqliteStore::in_memory().unwrap())),
    ] {
        let fixture = Fixture::build(label, store);
        let directory = fixture.directory.with_self_registration(true);
        let user = directory.login("builtin", "alice", "hunter2").unwrap();
        assert_eq!(user.display_name, "Alice", "{label}");
        // 第二次登录复用同一个人。
        assert_eq!(
            directory.login("builtin", "alice", "hunter2").unwrap().id,
            user.id,
            "{label}"
        );
    }
}

#[test]
fn 令牌解析出来的身份就是签发它的那个人() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let (_, secret) = fixture
            .directory
            .issue_token(alice, "笔记本", None)
            .unwrap();
        let identity = fixture.directory.resolve(secret.expose()).unwrap();
        assert_eq!(identity.user.id, alice, "{label}");
        assert_eq!(
            identity.actor(),
            Actor::User {
                user: alice.to_string()
            },
            "{label}"
        );
    }
}

#[test]
fn 四种解析失败给的是同一句话() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let (revoked, revoked_secret) = fixture.directory.issue_token(alice, "撤", None).unwrap();
        fixture.directory.revoke_token(alice, revoked.id).unwrap();
        let expires = Timestamp::from_millis(fixture.clock.now().as_millis() + 10);
        let (_, expired_secret) = fixture
            .directory
            .issue_token(alice, "过", Some(expires))
            .unwrap();
        fixture.clock.advance(1_000);

        let errors: Vec<_> = [
            fixture
                .directory
                .resolve("xops_ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
            fixture.directory.resolve(revoked_secret.expose()),
            fixture.directory.resolve(expired_secret.expose()),
            fixture.directory.resolve("这不是一个令牌"),
        ]
        .into_iter()
        .map(|result| result.unwrap_err())
        .collect();

        for error in &errors {
            assert_eq!(error.kind(), errors[0].kind(), "{label}");
            assert_eq!(
                error.message(),
                errors[0].message(),
                "{label}：TOK-005 —— 四种情形必须逐字一致，否则错误本身就泄露了令牌存不存在"
            );
        }
    }
}

#[test]
fn 撤销立即生效() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let (token, secret) = fixture
            .directory
            .issue_token(alice, "笔记本", None)
            .unwrap();
        assert!(
            fixture.directory.resolve(secret.expose()).is_ok(),
            "{label}"
        );
        fixture.directory.revoke_token(alice, token.id).unwrap();
        assert!(
            fixture.directory.resolve(secret.expose()).is_err(),
            "{label}：没有延迟窗口"
        );
    }
}

#[test]
fn 别人的令牌撤不掉且错误与不存在一致() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        let (token, _) = fixture
            .directory
            .issue_token(alice, "笔记本", None)
            .unwrap();

        let stolen = fixture.directory.revoke_token(bob, token.id).unwrap_err();
        let missing = fixture
            .directory
            .revoke_token(bob, xops_identity::TokenId::generate())
            .unwrap_err();
        assert_eq!(stolen.message(), missing.message(), "{label}");
    }
}

#[test]
fn 最后使用时间按分钟节流() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let (token, secret) = fixture
            .directory
            .issue_token(alice, "笔记本", None)
            .unwrap();
        let tokens_table = TableName::new(xops_identity::TOKENS).unwrap();

        fixture.directory.resolve(secret.expose()).unwrap();
        let after_first = fixture.engine.last_seq(&tokens_table).unwrap();
        for _ in 0..20 {
            fixture.directory.resolve(secret.expose()).unwrap();
        }
        assert_eq!(
            fixture.engine.last_seq(&tokens_table).unwrap(),
            after_first,
            "{label}：认证在每次调用的路径上，每次都写会让 _tokens 串行掉全系统"
        );

        fixture
            .clock
            .advance(xops_identity::token::LAST_USED_RESOLUTION_MILLIS);
        fixture.directory.resolve(secret.expose()).unwrap();
        assert!(
            fixture.engine.last_seq(&tokens_table).unwrap() > after_first,
            "{label}"
        );

        let stored = fixture.directory.tokens_of(alice).unwrap();
        assert_eq!(stored.len(), 1, "{label}");
        assert_eq!(stored[0].id, token.id, "{label}");
        assert!(stored[0].last_used_at.is_some(), "{label}");
    }
}

// ——————————————————————————————— 项目与成员 ———————————————————————————————

#[test]
fn 建项目的人自动是所有者() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let project = fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();
        assert_eq!(
            fixture.directory.role_of(project.id, alice).unwrap(),
            Some(Role::Owner),
            "{label}"
        );
        assert_eq!(
            fixture.directory.my_projects(alice).unwrap().len(),
            1,
            "{label}"
        );
    }
}

#[test]
fn 短名全平台唯一() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();
        assert!(
            fixture
                .directory
                .create_project(bob, Slug::new("acme").unwrap(), "别人的")
                .is_err(),
            "{label}"
        );
    }
}

#[test]
fn 非成员看到的与不存在完全一致() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        let project = fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();

        let outsider = fixture.directory.project(bob, project.id).unwrap_err();
        let missing = fixture
            .directory
            .project(bob, xops_identity::ProjectId::generate())
            .unwrap_err();
        assert_eq!(outsider.kind(), missing.kind(), "{label}");
        assert_eq!(
            outsider.message(),
            missing.message(),
            "{label}：PRJ-008 —— 否则错误码本身就是探测他人项目的工具"
        );
        assert!(
            fixture.directory.my_projects(bob).unwrap().is_empty(),
            "{label}"
        );
    }
}

#[test]
fn 权限不足与不存在也一致() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        let project = fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();
        fixture
            .directory
            .set_member(alice, project.id, bob, Role::Member)
            .unwrap();

        // bob 是成员，看得见项目，但改不了成员。
        assert!(
            fixture.directory.project(bob, project.id).is_ok(),
            "{label}"
        );
        let denied = fixture
            .directory
            .set_member(bob, project.id, alice, Role::Member)
            .unwrap_err();
        let missing = fixture
            .directory
            .set_member(
                bob,
                xops_identity::ProjectId::generate(),
                alice,
                Role::Member,
            )
            .unwrap_err();
        assert_eq!(denied.message(), missing.message(), "{label}：MCP-008");
    }
}

#[test]
fn 同一个人在不同项目里的角色互相独立() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        let first = fixture
            .directory
            .create_project(alice, Slug::new("first").unwrap(), "一")
            .unwrap();
        let second = fixture
            .directory
            .create_project(bob, Slug::new("second").unwrap(), "二")
            .unwrap();
        fixture
            .directory
            .set_member(alice, first.id, bob, Role::Member)
            .unwrap();

        assert_eq!(
            fixture.directory.role_of(first.id, bob).unwrap(),
            Some(Role::Member),
            "{label}"
        );
        assert_eq!(
            fixture.directory.role_of(second.id, bob).unwrap(),
            Some(Role::Owner),
            "{label}"
        );
    }
}

#[test]
fn 最后一个所有者移不走也降不了() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        let project = fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();
        fixture
            .directory
            .set_member(alice, project.id, bob, Role::Member)
            .unwrap();

        assert!(
            fixture
                .directory
                .remove_member(alice, project.id, alice)
                .is_err(),
            "{label}"
        );
        assert!(
            fixture
                .directory
                .set_member(alice, project.id, alice, Role::Maintainer)
                .is_err(),
            "{label}：降级也是移走最后一个所有者"
        );

        // 有了第二个所有者，第一个就走得了。
        fixture
            .directory
            .set_member(alice, project.id, bob, Role::Owner)
            .unwrap();
        fixture
            .directory
            .remove_member(alice, project.id, alice)
            .unwrap();
        assert_eq!(
            fixture.directory.role_of(project.id, alice).unwrap(),
            None,
            "{label}"
        );
    }
}

#[test]
fn 归档之后只读() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        let project = fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();
        fixture
            .directory
            .archive_project(alice, project.id)
            .unwrap();

        assert!(
            fixture
                .directory
                .project(alice, project.id)
                .unwrap()
                .is_archived(),
            "{label}"
        );
        assert!(
            fixture
                .directory
                .set_member(alice, project.id, bob, Role::Member)
                .is_err(),
            "{label}：归档后不再接受任何写操作，所有者也不行"
        );
        assert!(
            fixture
                .directory
                .authorize(alice, project.id, Action::ReadProject)
                .is_ok(),
            "{label}：历史内容完整保留、可查询"
        );
    }
}

// ——————————————————————————————— 审计 ———————————————————————————————

#[test]
fn 业务写失败时审计事件也不存在() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();
        let table = TableName::new(xops_identity::PROJECTS).unwrap();
        let before = fixture.engine.last_seq(&table).unwrap();

        assert!(
            fixture
                .directory
                .create_project(bob, Slug::new("acme").unwrap(), "撞名")
                .is_err()
        );
        assert_eq!(
            fixture.engine.last_seq(&table).unwrap(),
            before,
            "{label}：AUD-005 —— 没有'留了痕但业务没生效'的中间态"
        );
    }
}

#[test]
fn 项目事件流查得到成员变更() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        let project = fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();
        fixture
            .directory
            .set_member(alice, project.id, bob, Role::Member)
            .unwrap();

        let records = fixture
            .audit
            .query(&Query::in_project(project.id.as_id(), alice.as_id()))
            .unwrap();
        let kinds_seen: Vec<&str> = records
            .iter()
            .map(|record| record.envelope.kind.as_str())
            .collect();
        assert!(
            kinds_seen.contains(&kinds::PROJECT_CREATED),
            "{label}：{kinds_seen:?}"
        );
        assert!(
            kinds_seen.contains(&kinds::MEMBER_ADDED),
            "{label}：{kinds_seen:?}"
        );
    }
}

#[test]
fn 别的项目的事件不会串进来() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        let mine = fixture
            .directory
            .create_project(alice, Slug::new("mine").unwrap(), "我的")
            .unwrap();
        let theirs = fixture
            .directory
            .create_project(bob, Slug::new("theirs").unwrap(), "他的")
            .unwrap();

        let records = fixture
            .audit
            .query(&Query::in_project(mine.id.as_id(), alice.as_id()))
            .unwrap();
        assert!(!records.is_empty(), "{label}");
        assert!(
            records
                .iter()
                .all(|record| record.envelope.project == Some(mine.id.as_id())),
            "{label}：AUD-003 —— 按项目分区，越权的行根本不进结果集"
        );
        assert!(
            fixture
                .audit
                .query(&Query::in_project(theirs.id.as_id(), alice.as_id()))
                .unwrap()
                .iter()
                .all(|record| record.envelope.project == Some(theirs.id.as_id())),
            "{label}"
        );
    }
}

#[test]
fn 平台级事件只有本人读得到() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        fixture
            .directory
            .issue_token(alice, "笔记本", None)
            .unwrap();

        let hers = fixture
            .audit
            .query(&Query::platform(alice.as_id()))
            .unwrap();
        assert!(
            hers.iter()
                .any(|record| record.envelope.kind.as_str() == kinds::TOKEN_ISSUED),
            "{label}"
        );
        let his = fixture.audit.query(&Query::platform(bob.as_id())).unwrap();
        assert!(
            !his.iter().any(
                |record| record.envelope.kind.as_str() == kinds::TOKEN_ISSUED
                    && record.envelope.subject == Some(alice.as_id())
            ),
            "{label}：AUD-003 —— 平台级事件只有主体本人可读"
        );
    }
}

#[test]
fn 按类型与目标查得到() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        let project = fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();
        fixture
            .directory
            .set_member(alice, project.id, bob, Role::Member)
            .unwrap();

        let base = Query::in_project(project.id.as_id(), alice.as_id());
        let added = fixture
            .audit
            .query(
                &base
                    .clone()
                    .of_kind(xops_audit::EventKind::new(kinds::MEMBER_ADDED).unwrap()),
            )
            .unwrap();
        assert_eq!(added.len(), 2, "{label}：创建者自己那条 + bob 那条");

        let about_bob = fixture.audit.history(bob.as_id(), &base).unwrap();
        assert_eq!(about_bob.len(), 1, "{label}");
        assert_eq!(
            about_bob[0].envelope.kind.as_str(),
            kinds::MEMBER_ADDED,
            "{label}"
        );
    }
}

#[test]
fn 索引重建之后结果一样() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let project = fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();
        let query = Query::in_project(project.id.as_id(), alice.as_id());
        let before = fixture.audit.query(&query).unwrap();

        let rebuilt = fixture.audit.rebuild_index().unwrap();
        assert!(rebuilt > 0, "{label}");
        assert_eq!(
            fixture.audit.query(&query).unwrap(),
            before,
            "{label}：索引是缓存不是权威"
        );
    }
}

#[test]
fn 仅凭事件流重建出当时的状态() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let bob = fixture.user("bob", "Bob");
        let project = fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();

        let before_bob = fixture.clock.now();
        fixture.clock.advance(1_000);
        fixture
            .directory
            .set_member(alice, project.id, bob, Role::Member)
            .unwrap();
        let after_bob = fixture.clock.now();

        let early = fixture.directory.rebuild_at(before_bob).unwrap();
        assert_eq!(early.projects.len(), 1, "{label}");
        assert_eq!(early.members.len(), 1, "{label}：那时候只有创建者一个人");

        let late = fixture.directory.rebuild_at(after_bob).unwrap();
        assert_eq!(late.members.len(), 2, "{label}");
        assert_eq!(late.users.len(), 2, "{label}");
        assert_eq!(late.projects[0].slug, project.slug, "{label}");
    }
}

#[test]
fn 保留期只吃没有业务行的留痕() {
    for fixture in fixtures() {
        let label = fixture.label;
        let alice = fixture.user("alice", "Alice");
        let project = fixture
            .directory
            .create_project(alice, Slug::new("acme").unwrap(), "Acme")
            .unwrap();

        let envelope = xops_audit::AuditEnvelope::project_scoped(
            kinds::CALL_REJECTED,
            project.id.as_id(),
            project.id.as_id(),
            serde_json::json!({"tool": "table.create"}),
        )
        .unwrap()
        .rejected();
        fixture.audit.append(&Actor::Platform, &envelope).unwrap();
        fixture.clock.advance(10_000);
        let cutoff = fixture.clock.now();

        let pruned = fixture.audit.prune(cutoff).unwrap();
        assert_eq!(pruned, 1, "{label}：只有 _audit 上那一条被清掉");

        // 项目还在 —— 它的事件是业务状态本身，删了 AUD-004 就落空了。
        assert!(
            fixture.directory.project(alice, project.id).is_ok(),
            "{label}"
        );
        assert_eq!(
            fixture.directory.rebuild_at(cutoff).unwrap().projects.len(),
            1,
            "{label}"
        );
    }
}
