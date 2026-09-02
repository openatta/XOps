//! 这次执行准碰哪些路径。
//!
//! # 为什么必须有这一段
//!
//! 引擎的 `Builder` 拿不到 `Permission` 时用的是一个 **`AllowAll`**——一律放行。
//! XOps 从来没传过，于是：
//!
//! ```text
//! 技能内容：用 Read 读 /private/tmp/xops-demo/仓外的秘密.txt
//! 产出：    机密-不该被技能读到-1788315172
//! ```
//!
//! **一个技能能读 xopsd 这个进程能读的任何文件**——XOps 自己的库、别的项目的工作区、
//! `.env` 里的模型 key、`~/.ssh`。实测撞出来的，不是推的。
//!
//! 工具那一侧其实拦了一道：`Read` 对越界路径回的是 `PermissionDecision::Ask`。
//! 可**无人值守的执行里没有人可问**，而这一层的默认是放行——
//! "要确认"就这么退化成了"随便读"。⚠️ **默认值站在哪一边，是这类洞唯一的成因。**
//!
//! # 判定
//!
//! `I-I`：**一次执行的可见范围完全由其声明的数据源决定，不存在隐式扩权**（`EXE-012`）。
//! 落到这里就两条：
//!
//! ```text
//! 声明了代码仓  →  只准碰那份只读工作区里的路径
//! 没有声明      →  一个文件都不准碰
//! ```
//!
//! 第二条容易被漏掉:不声明代码仓的技能，`project_root` 是 xopsd 自己的 cwd——
//! **不拦的话它读的正是 XOps 的源码。**
//!
//! # 怎么认出"路径"
//!
//! **不按工具枚举字段名。** `Read` 的路径叫 `file_path`，`Glob`/`Grep` 的叫 `path`，
//! 而新工具会带新的名字——一张要跟着上游走的表，迟早会漏一格，
//! **漏的那一格不报错**。
//!
//! 所以反过来：**参数里每一个字符串都当成可能的路径去验**。
//! 代价是误拒——比如 Grep 的 pattern 里正好写了 `/etc/passwd` 这几个字。
//! 这个方向是对的:与 `FLW-008`③ 同一条口径，**证不出安全就当作不安全**。

use std::path::{Component, Path, PathBuf};

use attacore_core::interface::permission::{Permission, PermissionOutcome};

/// 这次执行的路径边界。
#[derive(Debug, Clone)]
pub struct Confine {
    /// 那份只读工作区。**`None` 表示这次执行没有声明代码仓。**
    root: Option<PathBuf>,
}

impl Confine {
    #[must_use]
    pub fn new(root: Option<PathBuf>) -> Self {
        Self {
            root: root.map(|root| normalize(&root)),
        }
    }

    /// 这个字符串当成路径看，落在边界里吗。
    fn allows(&self, root: &Path, raw: &str) -> bool {
        if raw.is_empty() {
            return true;
        }
        let candidate = Path::new(raw);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root.join(candidate)
        };
        normalize(&joined).starts_with(root)
    }
}

#[async_trait::async_trait]
impl Permission for Confine {
    async fn check(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        _cwd: &Path,
        _session_id: &str,
    ) -> PermissionOutcome {
        // ⚠️ **用自己记着的 root，不用传进来的 cwd。** 没声明代码仓时
        // cwd 是 xopsd 自己的目录，照它判等于把 XOps 的源码放出去。
        let Some(root) = &self.root else {
            return PermissionOutcome::Deny {
                reason: format!(
                    "这次执行没有声明代码仓，{tool_name} 碰不了任何文件（EXE-012、I-I）"
                ),
            };
        };
        let mut offending = Vec::new();
        collect_strings(tool_input, &mut offending);
        for raw in offending {
            if !self.allows(root, &raw) {
                return PermissionOutcome::Deny {
                    reason: format!(
                        "{raw} 在这次执行的只读工作区之外。\
                         一次执行的可见范围由它声明的数据源决定（I-I）"
                    ),
                };
            }
        }
        PermissionOutcome::Permit
    }
}

/// 把参数里所有字符串收出来。**不认字段名**——见模块注释。
fn collect_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => out.push(text.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_strings(item, out);
            }
        }
        serde_json::Value::Object(fields) => {
            for field in fields.values() {
                collect_strings(field, out);
            }
        }
        _ => {}
    }
}

/// 只按字面消掉 `.` 与 `..`，**不碰文件系统**。
///
/// ⚠️ 不用 `canonicalize`:它要求路径存在，而"不存在的越界路径"照样要拦；
/// 而且它会跟符号链接走——**跟着走就等于让符号链接说了算**。
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 判(confine: &Confine, input: serde_json::Value) -> bool {
        let outcome = futures_lite_block_on(confine.check("Read", &input, Path::new("/x"), "s"));
        matches!(outcome, PermissionOutcome::Permit)
    }

    /// 一个够用的 block_on —— 这几条判定里没有任何真正的异步。
    fn futures_lite_block_on(
        future: impl std::future::Future<Output = PermissionOutcome>,
    ) -> PermissionOutcome {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn 工作区里的路径放行() {
        let confine = Confine::new(Some(PathBuf::from("/ws/ws-1")));
        assert!(判(
            &confine,
            serde_json::json!({"file_path": "app/reader.py"})
        ));
        assert!(判(
            &confine,
            serde_json::json!({"file_path": "/ws/ws-1/README.md"})
        ));
        assert!(判(&confine, serde_json::json!({"pattern": "**/*.py"})));
    }

    #[test]
    fn 越界的路径拦下来() {
        // 这条是实测撞出来的那个洞:**技能读到了工作区之外的文件。**
        let confine = Confine::new(Some(PathBuf::from("/ws/ws-1")));
        assert!(!判(
            &confine,
            serde_json::json!({"file_path": "/etc/passwd"})
        ));
        assert!(!判(
            &confine,
            serde_json::json!({"file_path": "../ws-2/秘密"})
        ));
        assert!(!判(
            &confine,
            serde_json::json!({"file_path": "/ws/ws-1/../../etc/passwd"})
        ));
        // 藏在嵌套里的也要收出来 —— 收的是**每一个**字符串，不是某几个字段。
        assert!(!判(
            &confine,
            serde_json::json!({"paths": [{"p": "/etc/hosts"}]})
        ));
    }

    #[test]
    fn 前缀相同但不是同一个目录不算在里面() {
        // `/ws/ws-10` 以 `/ws/ws-1` 开头，可它是**另一份工作区**。
        let confine = Confine::new(Some(PathBuf::from("/ws/ws-1")));
        assert!(!判(
            &confine,
            serde_json::json!({"file_path": "/ws/ws-10/x"})
        ));
    }

    #[test]
    fn 没声明代码仓就一个文件都不准碰() {
        // ⚠️ 最容易漏的一条:不声明代码仓时 `project_root` 是 xopsd 自己的 cwd，
        // 不拦的话技能读的正是 XOps 的源码。
        let confine = Confine::new(None);
        assert!(!判(&confine, serde_json::json!({"file_path": "任何东西"})));
        assert!(!判(&confine, serde_json::json!({})));
    }
}
