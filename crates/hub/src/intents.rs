//! 标准意图（spec/intents.md 第 4 节，第 16 项 N4）：内置工具 `apps.intents` 与机主默认表。
//!
//! - [`defaults`]：机主默认表（`HubConfig::intent_defaults`、`<home>/intents.json` 的格式）的校验与查找。
//! - `collect`：按意图分组列出实现者（纯函数，候选来源同 `apps.search`）。
//! - `builtin`：参数校验、结果与渐进暴露。
//!
//! @invariant 只读注册表、休眠快照、清单与页面目录：不唤醒 App、不新增定时器或后台任务；Hub 不按默认表路由，
//! 也不提供"按动词调用"的入口（选哪个 App 归 Agent）。

mod builtin;
mod collect;
pub mod defaults;

pub use defaults::{IntentsConfig, IntentsStatus, MAX_INTENT_DEFAULTS};

/// `apps.intents` 的 `intent` 参数的最大字符数（`<动词>@<主版本>`，动词至多 64 字符）。
pub const MAX_INTENT_ARG_CHARS: usize = 80;
