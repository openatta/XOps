//! Git 仓绑定。
//!
//! **XOps 在任何代码路径上都不持有、不请求仓库的写权限**（`RPO-013`、`I-G`）：
//! 不推分支、不提 PR、不打 tag、不改任何仓库内容。这条不是靠自觉——
//! 绑定之前会**实际推一次 dry-run**，写得进去就拒绝绑（`RPO-002`）。
//!
//! 归属：RP-08。**容器与挂载归 RP-07**，本包只把工作区备好。

pub mod binding;
pub mod credential;
pub mod platform;
pub mod service;
pub mod tools;
pub mod workspace;

pub use binding::Binding;
pub use credential::{KEY_ENV, SealedCredential, Sealer, Secret};
pub use platform::{GitHub, GitPlatform, WriteProbe};
pub use service::{BINDINGS_TABLE, Deps, Repos};
pub use workspace::{Budget, Workspace};
