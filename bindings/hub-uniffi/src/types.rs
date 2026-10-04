//! FFI 数据类型（`uniffi::Record` / `uniffi::Enum` / `uniffi::Error`）及与 `app_mcp_hub` 类型的转换。
//!
//! 约定：任意 JSON（参数、结果、schema、details）在 FFI 上以 JSON 文本（`String`）传递；
//! 时长以毫秒数（`u64`）传递；错误类别用协议字符串（如 `"USER_REJECTED"`）。

macro_rules! enum_map {
    ($ffi:ident <=> $hub:path { $($v:ident),* $(,)? }) => {
        impl From<$ffi> for $hub {
            fn from(v: $ffi) -> Self {
                match v { $($ffi::$v => <$hub>::$v,)* }
            }
        }
        impl From<$hub> for $ffi {
            fn from(v: $hub) -> Self {
                match v { $(<$hub>::$v => $ffi::$v,)* }
            }
        }
    };
}

mod agents;
mod annotations;
mod app_events;
mod apps;
mod cache;
mod call;
mod catalog;
mod config;
mod enums;
mod error;
mod events;
mod intents;
mod calls_status;
mod locks;
mod policy;
mod schema_changes;
mod status;
mod undo;
mod usage;

pub use agents::AgentCredential;
pub(crate) use agents::agents_config;
pub use annotations::*;
pub use app_events::*;
pub use apps::*;
pub use cache::*;
pub use call::*;
pub use catalog::*;
pub use config::*;
pub use enums::*;
pub use error::*;
pub use events::*;
pub use intents::*;
pub use calls_status::*;
pub use locks::*;
pub use policy::*;
pub use schema_changes::*;
pub use status::*;
pub use undo::*;
pub use usage::*;

#[cfg(test)]
mod tests;
