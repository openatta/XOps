//! 身份、项目与审计的地基。
//!
//! 守住的核心性质只有一条：
//!
//! > **每一条记录都能回答"谁做的"，而这个答案由凭据给出，不由调用方自述。**
//!
//! MCP 协议本身不认证调用者——管道那头的 agent 说自己是谁没有任何约束力。
//! 这里松了，后面所有东西都是装饰。
//!
//! 归属：RP-02。**本包不知道调用它的是 MCP 还是别的什么**——它只提供
//! 「令牌 → 身份」与「身份 + 项目 + 动作 → 能不能」两个答案。

pub mod directory;
pub mod permission;
pub mod project;
pub mod token;
pub mod user;

pub use directory::{
    Directory, Identity, MEMBERS, PLATFORM_TABLES, PROJECTS, Snapshot, TOKENS, USERS,
};
pub use permission::{Action, can, can_in};
pub use project::{Member, MemberChange, Project, ProjectId, Slug, owners_after};
pub use token::{Token, TokenId, TokenSecret};
pub use user::{
    BuiltinProvider, ExternalAccount, ExternalProfile, IdentityProvider, OAuthProvider,
    ProfileExchange, ProviderId, User, UserId,
};
