//! XForge 侧的配套四样，以及**检出 ③④ 缺失**的检查（`XFG-021`、`XFG-022`）。
//!
//! ```text
//! ① 一份 `McpServer` 资源（transport: http · url · authTokenEnv · timeoutSeconds）
//! ② 它在 manifest.yaml 的 scaffold.mcpServers 里的登记
//! ③ 一条 approvals.providers[] 条目（roles 与 XOps 实际返回的角色对齐）
//! ④ 某条 Flow 的 approvalPolicies[].providers 里**引用这个 provider id**
//! ```
//!
//! > **①② 缺了会加载失败，③④ 缺了会静默失效**——后者更危险：`xforge doctor`
//! > 对未被引用的扩展资源**只警告、从不阻塞**，于是 provider 装好了、连得上、
//! > 却没有任何一条 Flow 引用它，**这道审批门等于不存在，而一切看起来都正常**。
//!
//! ⚠️ **这里的检查是"这几个名字在不在文本里"，不是一次 YAML 解析。**
//! 写清楚它的口径比让它看起来更聪明重要：它证明得了**缺失**（`XFG-022` 要的就是这个），
//! 证明不了"结构完全正确"——那件事由 `xforge doctor` 加上真实的
//! `xforge approve --provider xops` 一起回答（`XFG-024`）。

use serde::{Deserialize, Serialize};

/// 四样里的哪一样。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Piece {
    /// ① `McpServer` 资源。
    McpServerResource,
    /// ② `manifest.yaml` 的 `scaffold.mcpServers` 登记。
    ManifestEntry,
    /// ③ `approvals.providers[]` 条目。
    ProviderEntry,
    /// ④ 某条 Flow 引用了这个 provider id。
    FlowReference,
}

impl Piece {
    /// 四样，一样不少。
    pub const ALL: [Self; 4] = [
        Self::McpServerResource,
        Self::ManifestEntry,
        Self::ProviderEntry,
        Self::FlowReference,
    ];

    /// 缺了它会怎样。**③④ 那两句是这个模块存在的理由。**
    #[must_use]
    pub const fn consequence(self) -> &'static str {
        match self {
            Self::McpServerResource | Self::ManifestEntry => "缺了会**加载失败**——看得见",
            Self::ProviderEntry | Self::FlowReference => {
                "缺了会**静默失效**：provider 装好了、连得上、却没有任何一条 Flow 引用它，\
                 **这道审批门等于不存在，而一切看起来都正常**"
            }
        }
    }
}

/// 一次检查看到的那几份文本。
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// `McpServer` 资源文件的内容。
    pub mcp_server: String,
    /// `manifest.yaml` 的内容。
    pub manifest: String,
    /// approvals 配置的内容。
    pub approvals: String,
    /// 全部 Flow 文件的内容（拼在一起也行）。
    pub flows: String,
}

/// 缺了哪几样。**空表示四样齐全。**
#[must_use]
pub fn missing(provider_id: &str, server_id: &str, sources: &Sources) -> Vec<Piece> {
    let mut out = Vec::new();
    // ① 资源本体要像个 http 的 McpServer，而且带着取令牌的环境变量名。
    let looks_like_server = sources.mcp_server.contains(server_id)
        && sources.mcp_server.contains("http")
        && sources.mcp_server.contains("authTokenEnv");
    if !looks_like_server {
        out.push(Piece::McpServerResource);
    }
    // ② 它在 manifest 的 scaffold.mcpServers 里登记过。
    if !(sources.manifest.contains("mcpServers") && sources.manifest.contains(server_id)) {
        out.push(Piece::ManifestEntry);
    }
    // ③ approvals.providers[] 里有这个 provider。
    if !(sources.approvals.contains("providers") && sources.approvals.contains(provider_id)) {
        out.push(Piece::ProviderEntry);
    }
    // ④ **某条 Flow 真的引用了它。** 这一条是 doctor 不管的那条。
    if !(sources.flows.contains("approvalPolicies") && sources.flows.contains(provider_id)) {
        out.push(Piece::FlowReference);
    }
    out
}

/// ① 的模板。
#[must_use]
pub fn mcp_server_resource(server_id: &str, url: &str, token_env: &str) -> String {
    format!(
        "kind: McpServer\n\
         metadata:\n  \
           name: {server_id}\n\
         spec:\n  \
           transport: http\n  \
           url: {url}\n  \
           authTokenEnv: {token_env}\n  \
           timeoutSeconds: 30\n"
    )
}

/// ③ 的模板。**`roles` 与 XOps 实际返回的角色对齐**（`XFG-019`）。
#[must_use]
pub fn provider_entry(provider_id: &str, server_id: &str) -> String {
    format!(
        "approvals:\n  \
           providers:\n    \
             - id: {provider_id}\n      \
               mcpServer: {server_id}\n      \
               submitTool: submit_approval_request\n      \
               pollTool: poll_approval\n      \
               roles: [{}]\n",
        crate::registration::XOPS_ROLES.join(", ")
    )
}

/// ④ 的模板。
#[must_use]
pub fn flow_reference(provider_id: &str) -> String {
    format!(
        "approvalPolicies:\n  \
           - id: release-approval\n    \
             providers: [{provider_id}]\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> Sources {
        Sources {
            mcp_server: mcp_server_resource(
                "xops-approvals",
                "https://xops.example/mcp",
                "XOPS_TOKEN",
            ),
            manifest: "scaffold:\n  mcpServers:\n    - xops-approvals\n".into(),
            approvals: provider_entry("xops", "xops-approvals"),
            flows: flow_reference("xops"),
        }
    }

    #[test]
    fn 四样齐全时不报缺() {
        assert!(missing("xops", "xops-approvals", &complete()).is_empty());
        assert_eq!(Piece::ALL.len(), 4, "XFG-021：缺一样门就不存在");
    }

    #[test]
    fn 缺第四样能被检出来() {
        // **这一条是 doctor 不管的那条**：provider 装好了、连得上、没人引用。
        let mut sources = complete();
        sources.flows = String::new();
        assert_eq!(
            missing("xops", "xops-approvals", &sources),
            vec![Piece::FlowReference]
        );
        assert!(
            Piece::FlowReference.consequence().contains("静默失效"),
            "它的危险之处要说出来"
        );
    }

    #[test]
    fn 缺第三样也能被检出来() {
        let mut sources = complete();
        sources.approvals = "approvals:\n  providers: []\n".into();
        assert_eq!(
            missing("xops", "xops-approvals", &sources),
            vec![Piece::ProviderEntry]
        );
    }

    #[test]
    fn 前两样缺了也报但它们本来就会加载失败() {
        let sources = Sources::default();
        let gone = missing("xops", "xops-approvals", &sources);
        assert_eq!(gone.len(), 4);
        assert!(Piece::ManifestEntry.consequence().contains("加载失败"));
    }

    #[test]
    fn provider条目里的角色就是xops那三个() {
        let entry = provider_entry("xops", "xops-approvals");
        for role in crate::registration::XOPS_ROLES {
            assert!(entry.contains(role), "XFG-019：{role}");
        }
    }
}
