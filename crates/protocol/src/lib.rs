//! app-mcp SDK ↔ Host 协议（v1）。
//!
//! 本 crate 是 Host 与各语言 SDK 共用的**唯一协议定义**，规范文本见 `spec/protocol.md`。
//! 修改这里的任何类型都视为协议变更，必须同步更新规范。
//!
//! - [`jsonrpc`]：JSON-RPC 2.0 信封（请求、通知、响应）的解析与序列化。
//! - [`messages`]：各方法名与参数、结果类型。
//! - [`error`]：协议错误码与错误类别。
//! - [`hash`]：工具摘要 `toolsHash`（spec/lifecycle.md 第 6 节）。

pub mod error;
pub mod hash;
pub mod jsonrpc;
pub mod messages;

pub use error::{ErrorKind, ToolError};
pub use hash::{canonical_json, tools_hash};
pub use jsonrpc::{Message, Notification, ParseError, Request, RequestId, Response, RpcError};
pub use messages::*;

/// 当前协议版本。握手时双方交换，不一致时 Host 返回 `rejected`。
pub const PROTOCOL_VERSION: &str = "1";

/// 默认 WebSocket 监听地址。
pub const DEFAULT_WS_ADDR: &str = "127.0.0.1:7717";
