//! 连接状态、事件与错误类型。

use super::*;

// ---------------------------------------------------------------------------
// 状态与事件
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionState {
    /// 尚未调用 [`Client::start`]。
    Idle,
    /// 已请求驱动层建立连接，等待 [`Client::handle_connected`]。
    Connecting,
    /// 连接已建立，已发送 `app/hello`，等待结果。
    Handshaking,
    /// Host 返回 `pending`，等待用户确认配对。
    PendingPairing,
    /// 已配对并完成同步，可以接收调用。
    Connected,
    /// 连接断开，将在 `retry_at` 时重连。`reason` / `code`：本次断开或连接失败的原因与错误码
    /// （spec/protocol.md 10.1；驱动层经 [`Client::handle_connect_failed`] / [`Client::handle_disconnected_with`]
    /// 报告、握手 / 心跳超时）；驱动层未给出原因的断线（[`Client::handle_disconnected`]）为 `None`。
    Backoff { retry_at: Millis, reason: Option<String>, code: Option<ConnectionErrorCode> },
    /// 被 Host 拒绝（配对拒绝或协议不兼容），不再自动重连。`code` 来自 Host 的 `HelloResult.code` /
    /// `PairingResultParams.code`，缺省或不认识时为 [`ConnectionErrorCode::Rejected`]。
    Rejected { reason: String, code: ConnectionErrorCode },
    /// 已调用 [`Client::stop`]。
    Stopped,
    /// 休眠：已与 Host 完成 `app/sleep` 握手后断开（或 `on-demand` 模式启动后尚未连接）。
    /// 不重连、无定时器，注册表保留；等待 [`Client::handle_wake`] / [`Client::wake`]。
    Dormant,
    /// 收到唤醒后正在回连：已产生 [`Event::Connect`]，等待 [`Client::handle_connected`]。
    Waking,
    /// 对端不是期望的 Host（spec/protocol.md 1.6）：不是 app-mcp（`service` 不符、不认识 `app/hello`、
    /// 握手结果无法解析），或 Host 属于其他用户。已断开且不再自动重连；[`Client::wake`] /
    /// [`Client::connect_now`] 时再试一次。
    HostMismatch { reason: String, code: ConnectionErrorCode },
}

impl ConnectionState {
    /// 状态名（与 JS 的 `status` 一致），用于日志与警告。
    pub fn name(&self) -> &'static str {
        match self {
            ConnectionState::Idle => "idle",
            ConnectionState::Connecting => "connecting",
            ConnectionState::Handshaking => "handshaking",
            ConnectionState::PendingPairing => "pending-pairing",
            ConnectionState::Connected => "connected",
            ConnectionState::Backoff { .. } => "backoff",
            ConnectionState::Rejected { .. } => "rejected",
            ConnectionState::Stopped => "stopped",
            ConnectionState::Dormant => "dormant",
            ConnectionState::Waking => "waking",
            ConnectionState::HostMismatch { .. } => "host-mismatch",
        }
    }

    /// 状态携带的错误码（`Backoff` 可能没有）。
    pub fn code(&self) -> Option<ConnectionErrorCode> {
        match self {
            ConnectionState::Backoff { code, .. } => *code,
            ConnectionState::Rejected { code, .. } | ConnectionState::HostMismatch { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// 状态携带的原因说明（`Backoff` 可能没有）。
    pub fn reason(&self) -> Option<&str> {
        match self {
            ConnectionState::Backoff { reason, .. } => reason.as_deref(),
            ConnectionState::Rejected { reason, .. } | ConnectionState::HostMismatch { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

/// [`Client::report_issue`] 最多积累的不同错误码数。
pub const MAX_PENDING_DIAGNOSTICS: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// 请求驱动层建立连接（首次或重连）。建立后调用 [`Client::handle_connected`]，
    /// 失败则调用 [`Client::handle_disconnected`]。
    Connect,
    /// 请求驱动层关闭当前连接。关闭后**不需要**再调用 `handle_disconnected`。
    Disconnect,
    /// 通过当前连接发送一条文本消息。只会在连接建立期间产生。
    Send(String),
    /// 调用 handler。完成后调用 [`Client::complete_call`]。`idempotency_key` 为 Agent 给出的幂等键（原样，
    /// spec/protocol.md 3.3），供 handler 上下文读取；App 决定如何使用。
    InvokeTool { call_id: String, tool: ToolId, name: String, arguments: Value, idempotency_key: Option<String> },
    /// 取消调用：驱动层应中止 handler（如触发 AbortSignal）。之后的 `complete_call` 会被忽略。
    CancelTool { call_id: String, reason: CancelReason },
    /// 读取资源。完成后调用 [`Client::complete_read`]。
    ReadResource { read: ReadId, resource: ResourceId, name: String },
    /// Host 请求导航到页面（`app/navigate`，spec/protocol.md 3.4；仅在 [`ClientConfig::navigation`] 为 true 时产生）。
    /// `params` 缺省为 `null`。完成后调用 [`Client::complete_navigate`]；连接断开后完成返回 [`CoreError::UnknownNavigate`]。
    Navigate { navigate: NavigateId, page: String, params: Value },
    /// 连接状态变化。
    StateChanged(ConnectionState),
    /// 配对成功并获得新 token，驱动层应持久化，下次创建 Client 时放入配置。
    Paired { token: String },
    /// 非致命问题（解析失败、未知方法等），仅用于日志。
    Warning(String),
    /// 已进入休眠，且驻留策略允许退出进程（[`Residency`]）。App 自行决定是否退出。
    IdleExit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    /// Host 发来 `tools/cancel`。
    Requested,
    /// 超过 `timeoutMs`。
    Timeout,
    /// 连接断开。
    Disconnected,
    /// Client 被停止。
    Stopped,
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum CoreError {
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("input schema must be a json object with \"type\": \"object\"")]
    InvalidSchema,
    #[error("a tool or resource named {0:?} is already registered")]
    DuplicateName(String),
    #[error("unknown tool {0:?}")]
    UnknownTool(ToolId),
    #[error("unknown resource {0:?}")]
    UnknownResource(ResourceId),
    #[error("unknown scope {0:?}")]
    UnknownScope(ScopeId),
    #[error("unknown or finished call {0:?}")]
    UnknownCall(String),
    #[error("unknown or finished read {0:?}")]
    UnknownRead(ReadId),
    #[error("unknown or finished navigation {0:?}")]
    UnknownNavigate(NavigateId),
    /// 发出未声明的事件（[`Client::emit_event`]）。
    #[error("unknown event {0:?}: declare it before emitting")]
    UnknownEvent(String),
    /// 事件载荷不是 JSON 对象或超过 [`MAX_EVENT_PAYLOAD_BYTES`]。
    #[error("invalid event payload: {0}")]
    InvalidEventPayload(String),
    /// 工具 `implements` 格式不合法、重复或超过 4 项（spec/intents.md 第 1 节）。
    #[error("invalid implements: {0}")]
    InvalidImplements(String),
}

impl ConnectionState {
    /// 连接是否处于"已建立或正在建立"（包括握手中）。
    pub(crate) fn is_link_up(&self) -> bool {
        matches!(
            self,
            ConnectionState::Connecting
                | ConnectionState::Waking
                | ConnectionState::Handshaking
                | ConnectionState::PendingPairing
                | ConnectionState::Connected
        )
    }
}
