//! **一条全程经 MCP 的路**：建项目 → 建表 → 写行 → 查行 → 查单行历史 → 查审计。
//!
//! 这是 M1 的那句话——"一个 agent 友好的、可审计的记录系统：经 MCP 建表、写行、查行、
//! 查单行历史"——**用一个客户端能看得见的方式跑一遍**。除了发令牌，全程没有内部接口。

use std::sync::Arc;

use serde_json::{Value, json};
use xops_audit::AuditLog;
use xops_core::{SystemClock, TableName};
use xops_identity::{Directory, ExternalAccount, ProjectId, ProviderId};
use xops_mcp::McpServer;
use xops_mcp::tools::project::ProjectHook;
use xops_store::{MemoryStore, SqliteStore, Store, WriteEngine};
use xops_table::Tables;
use xops_table::engine::Catalog;

/// 项目建好之后把那四张系统表建起来（`TBL-005`）。
struct SystemTables {
    tables: Arc<Tables>,
}

impl ProjectHook for SystemTables {
    fn after_create(&self, project: ProjectId, slug: &str) -> xops_core::Result<()> {
        self.tables.ensure_system_tables(project, slug)
    }
}

struct Client {
    label: &'static str,
    server: Arc<McpServer>,
    token: String,
}

impl Client {
    fn call(&self, name: &str, arguments: Value) -> Value {
        let response = self
            .server
            .handle(
                Some(&self.token),
                &json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": {"name": name, "arguments": arguments},
                }),
            )
            .expect("有 id 就有响应");
        assert!(
            response.get("error").is_none(),
            "{}：调 {name} 失败了 —— {response}",
            self.label
        );
        response["result"]["structuredContent"].clone()
    }
}

fn clients() -> Vec<Client> {
    [
        ("memory", Arc::new(MemoryStore::new()) as Arc<dyn Store>),
        ("sqlite", Arc::new(SqliteStore::in_memory().unwrap())),
    ]
    .into_iter()
    .map(|(label, store)| {
        let clock = Arc::new(SystemClock);
        let catalog = Arc::new(Catalog::open(Arc::clone(&store), clock.clone()).unwrap());
        let engine = Arc::new(
            WriteEngine::new(Arc::clone(&store), clock.clone())
                .with_pre_write(Arc::clone(&catalog) as Arc<dyn xops_store::PreWrite>)
                .with_schema_check(Arc::clone(&catalog) as Arc<dyn xops_store::SchemaCheck>),
        );
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
        let audit = Arc::new(audit.watching(TableName::new(xops_table::CATALOG_TABLE).unwrap()));
        let directory = Arc::new(Directory::new(
            Arc::clone(&engine),
            Arc::clone(&store),
            Arc::clone(&audit),
            clock.clone(),
        ));
        let tables = Arc::new(Tables::new(
            Arc::clone(&engine),
            catalog,
            Arc::clone(&audit),
            Arc::clone(&directory),
            clock,
            Arc::clone(&store),
        ));

        let mut server = McpServer::new(
            Arc::clone(&directory),
            Arc::clone(&audit),
            Arc::clone(&store),
        );
        {
            let registry = server.registry_mut();
            xops_mcp::tools::identity::register_identity(registry, &directory).unwrap();
            xops_mcp::tools::project::register_with_hook(
                registry,
                &directory,
                Arc::new(SystemTables {
                    tables: Arc::clone(&tables),
                }),
            )
            .unwrap();
            xops_mcp::tools::token::register(registry, &directory).unwrap();
            xops_mcp::tools::audit::register(registry, &directory, &audit).unwrap();
            xops_table::tools::register(registry, &tables).unwrap();
        }

        // 唯一一处不经 MCP 的动作：把人预置进来、发第一个令牌。
        // 它们正是 MCP-013 认下的两个凭据类例外。
        let alice = directory
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
        let token = directory
            .issue_token(alice, "笔记本", None)
            .unwrap()
            .1
            .into_string();
        Client {
            label,
            server: Arc::new(server),
            token,
        }
    })
    .collect()
}

#[test]
fn 全程经mcp建项目建表写行查历史() {
    for client in clients() {
        let label = client.label;

        // ① 建项目。任何用户都可以建，创建者自动成为所有者。
        let project = client.call(
            "project.create",
            json!({"slug": "acme", "displayName": "Acme 的项目"}),
        );
        let project_id = project["project"].as_str().unwrap().to_owned();
        assert_eq!(project["role"], json!("owner"), "{label}");

        // ② 建表。
        let created = client.call(
            "table.create",
            json!({
                "project": project_id,
                "table": "bugs",
                "columns": [
                    {"name": "title", "type": "text", "required": true},
                    {"name": "seq", "type": "sequence"},
                    {"name": "code", "type": "derived", "template": "{project.slug}-{seq}"},
                    {"name": "state", "type": "enum", "enumValues": ["新建", "已修"]},
                ],
            }),
        );
        assert_eq!(created["table"], json!("bugs"), "{label}");

        // ③ 写一行 —— 用的是**为这张表派发出来的**那个 tool。
        let inserted = client.call(
            "row.bugs.insert",
            json!({"project": project_id, "title": "登录页崩了", "state": "新建"}),
        );
        let row = inserted["row"].as_str().unwrap().to_owned();

        // ④ 查回来。派生列与序号是平台补的。
        let rows = client.call("row.bugs.select", json!({"project": project_id}));
        let first = &rows["rows"][0]["values"];
        assert_eq!(first["title"], json!("登录页崩了"), "{label}");
        assert_eq!(first["seq"], json!(1), "{label}");
        assert_eq!(first["code"], json!("acme-1"), "{label}");
        assert_eq!(first["writtenBy"]["kind"], json!("person"), "{label}");

        // ⑤ 改一行，再查单行历史。
        client.call(
            "row.bugs.update",
            json!({"project": project_id, "row": row, "state": "已修"}),
        );
        let history = client.call(
            "table.history",
            json!({"project": project_id, "table": "bugs", "row": row}),
        );
        let versions = history["versions"].as_array().unwrap();
        assert_eq!(versions.len(), 2, "{label}：插入 + 修改");
        assert_eq!(versions[0]["values"]["state"], json!("新建"), "{label}");
        assert_eq!(versions[1]["values"]["state"], json!("已修"), "{label}");
        assert!(
            versions
                .iter()
                .all(|version| version["writtenBy"].is_object()),
            "{label}"
        );

        // ⑥ 审计里看得到这一路。
        let events = client.call("audit.query", json!({"project": project_id, "limit": 100}));
        let kinds: Vec<&str> = events["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["kind"].as_str().unwrap())
            .collect();
        for expected in ["project.created", "member.added", "table.created"] {
            assert!(
                kinds.contains(&expected),
                "{label}：审计里少了 {expected} —— {kinds:?}"
            );
        }

        // ⑦ 建表这件事本身也能按对象查历史。
        let listed = client.call("table.list", json!({"project": project_id}));
        let names: Vec<&str> = listed["tables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|table| table["table"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"bugs"), "{label}：{names:?}");
        assert!(
            names.contains(&"_runs"),
            "{label}：系统表也建起来了 —— {names:?}"
        );
    }
}

#[test]
fn 令牌管理面不写任何业务对象() {
    for client in clients() {
        let label = client.label;
        let issued = client.call("token.issue", json!({"label": "第二个"}));
        assert!(
            issued["secret"].as_str().unwrap().starts_with("xops_"),
            "{label}"
        );

        let listed = client.call("token.mine", json!({}));
        let tokens = listed["tokens"].as_array().unwrap();
        assert_eq!(tokens.len(), 2, "{label}");
        assert!(
            tokens.iter().all(|token| token.get("secret").is_none()),
            "{label}：列出来的时候没有原文（TOK-002）"
        );

        client.call("token.revoke", json!({"token": issued["token"]}));
    }
}
