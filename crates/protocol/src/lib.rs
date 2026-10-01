//! app-mcp SDK ↔ Host 协议（v1）。
//!
//! 本 crate 是 Host 与各语言 SDK 共用的**唯一协议定义**，规范文本见 `spec/protocol.md`。
//! 修改这里的任何类型都视为协议变更，必须同步更新规范。
//!
//! - [`jsonrpc`]：JSON-RPC 2.0 信封（请求、通知、响应）的解析与序列化。
//! - [`messages`]：各方法名与参数、结果类型。
//! - [`error`]：协议错误码与错误类别。
//! - [`hash`]：工具摘要 `toolsHash`（spec/lifecycle.md 第 6 节）。
//! - [`endpoint`]：传输端点（WebSocket / Unix 域套接字 / 命名管道）的格式与默认位置（第 1 节）。
//! - [`mux`]：一条连接承载多个实例的多路复用帧（第 9 节）。
//! - [`identity`]：Host 身份（`service` / `version` / `user` / `pid`）与 SDK 侧核对（1.6）。
//! - [`registry`]：单实例锁与登记文件 `~/.app-mcp/run/endpoints.json`（1.7）。
//! - [`diagnostic`]：连接级错误码与 `app/diagnostic` 上报（第 10 节）。

pub mod diagnostic;
pub mod endpoint;
pub mod error;
pub mod hash;
pub mod identity;
pub mod jsonrpc;
pub mod messages;
pub mod mux;
pub mod platform;
pub mod registry;

pub use diagnostic::{ConnectionErrorCode, ConnectionIssue, DiagnosticParams, IssueKind};
pub use endpoint::{Endpoint, TransportKind};
pub use error::{ErrorKind, ToolError, navigation_reason, user_action_reason};
pub use hash::{canonical_json, tools_hash};
pub use jsonrpc::{Message, Notification, ParseError, Request, RequestId, Response, RpcError};
pub use messages::*;
pub use mux::{MUX_MAX_CHANNELS, MUX_VERSION, MuxFrame, MuxParams, MuxResult};

/// 当前协议版本。握手时双方交换，不一致时 Host 返回 `rejected`。
pub const PROTOCOL_VERSION: &str = "1";

/// Host 的默认 HTTP 监听地址（spec/protocol.md 1.3）：同一端口承载 `/app`（App 的 WebSocket 连接）、
/// `/mcp`（MCP Streamable HTTP）与 `/healthz`。
pub const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:7717";

/// 与 [`DEFAULT_LISTEN_ADDR`] 相同（App 连接与 MCP 合并到同一端口之前的名称）。
pub const DEFAULT_WS_ADDR: &str = DEFAULT_LISTEN_ADDR;

/// 默认端口被占用时 Host 依次尝试、网页 SDK 依次握手的端口（spec/protocol.md 1.3）。
pub const LISTEN_CANDIDATE_PORTS: [u16; 3] = [7717, 7737, 7757];

/// App 连接（WebSocket 升级）的 HTTP 路径。
pub const APP_PATH: &str = "/app";
/// MCP Streamable HTTP 的路径。
pub const MCP_PATH: &str = "/mcp";
/// 健康检查路径（返回 Host 身份与监听信息）。
pub const HEALTH_PATH: &str = "/healthz";
/// 运行状态路径（实例、最近错误、SDK 上报；需要本地 IPC 或令牌，spec/protocol.md 1.3）。
pub const STATUS_PATH: &str = "/status";

/// 默认 WebSocket 端点：`ws://127.0.0.1:7717/app`（网页 SDK；没有本地 IPC 的原生平台）。
pub const DEFAULT_WS_URL: &str = "ws://127.0.0.1:7717/app";
