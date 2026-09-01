//! 技能资产。
//!
//! **技能内容是不可信输入**（`SKL-006`、G7）：平台不解析其语义、不因其内容改变控制流，
//! 只负责交给执行运行时。这一条决定了本包里没有任何"看看技能想干什么"的代码。
//!
//! 归属：RP-09。

pub mod declaration;
pub mod service;
pub mod skill;
pub mod tools;

pub use declaration::{Declaration, Input, InputType, OutputShape};
pub use service::{Resolved, SKILLS_TABLE, Skills, VERSIONS_TABLE};
pub use skill::{Ownership, Skill, SkillId, State, Version};
