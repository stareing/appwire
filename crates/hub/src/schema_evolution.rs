//! 工具演进（第 16 项 O4，spec/hub-api.md 3.21）：Hub 只呈现与告知，**不拦截、不保留旧 schema、不按弃用改变路由**。
//!
//! - [`present`]：弃用呈现（MCP 描述前缀、工具 `_meta`）与参数不符时的提示（`INVALID_INPUT` 带 `schemaHash`）。
//! - [`log`]：运行时告警记录（[`SchemaChangeRecord`]，最近 [`MAX_SCHEMA_CHANGES`] 条，只在内存）。
//! - [`detect`]：`tools/sync` / `tools/changed` 写注册表前，按 [`app_mcp_protocol::schema_compat::compare_tool`] 比较同名工具
//!   相对 Hub 此前已知定义（与结果缓存的 sync 比较同一基准，`registry/declared.rs`）的变化。
//!
//! `schemaHash` 在 [`crate::tool_def::ToolDef`] 构造时算一次（列表热路径不重算）。
//!
//! @invariant 不新增定时器或线程；记录只在收到 App 声明时追加（CLAUDE.md「召之即来」）。

mod detect;
mod log;
mod present;
#[cfg(test)]
mod tests;

pub use log::{MAX_SCHEMA_CHANGES, SchemaChangeRecord};
pub use present::DEPRECATED_PREFIX;

pub(crate) use log::SchemaChangeLog;
#[cfg(feature = "mcp-server")]
pub(crate) use present::{mcp_description, mcp_tool_meta};
pub(crate) use present::invalid_arguments;
