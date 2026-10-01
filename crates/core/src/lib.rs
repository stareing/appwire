//! app-mcp-core：sans-IO 客户端核心。
//!
//! 本 crate 不做任何 I/O，也不依赖异步运行时。它是一个确定性的状态机：
//!
//! - **输入**：注册 / 注销请求、连接建立 / 断开、收到的文本消息、定时器到期、handler 完成。
//! - **输出**：通过 [`Client::poll_event`] 取出的 [`Event`]（要发送的消息、要调用的 handler、
//!   状态变化等），以及 [`Client::poll_timeout`] 给出的下一次定时器时刻。
//!
//! 驱动层（浏览器 JS、原生 tokio 线程、测试代码）负责真正的 I/O 与计时，时间统一以
//! 调用方提供的单调毫秒数 [`Millis`] 表示。
//!
//! 典型驱动循环：
//!
//! ```text
//! client.start(now);
//! loop {
//!     while let Some(ev) = client.poll_event() { 处理 ev }
//!     等待 { 网络消息 | 连接变化 | handler 完成 | 到达 client.poll_timeout() }
//!     调用对应的 handle_* / complete_* 方法
//! }
//! ```
//!
//! 行为规范见 `spec/protocol.md` 的"SDK 行为"一节。本文件中的公开 API 是
//! 核心与各语言绑定之间的契约，修改前需同步更新绑定层。

mod calls;
mod connection;
mod lifecycle;
mod registry;

use std::collections::VecDeque;

use app_mcp_protocol as proto;
use serde_json::Value;

pub use proto::{
    Activation, AppOverview, ClientKind, ConnectionErrorCode, ConnectionIssue, DiagnosticParams, Risk, SleepReason,
    ToolError, Visibility, WakeDescriptor, WakeKind, WakeReason,
};
pub use lifecycle::parse_wake_token;

/// 调用方提供的单调时钟（毫秒）。
pub type Millis = u64;

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ClientConfig {
    pub app_id: String,
    pub app_name: String,
    /// 每个标签页或进程唯一，页面刷新后应保持不变（由驱动层负责持久化）。
    pub instance_id: String,
    pub client_kind: ClientKind,
    pub sdk_version: String,
    pub app_version: Option<String>,
    pub origin: Option<String>,
    pub instance_title: Option<String>,
    pub instance_url: Option<String>,
    /// 之前配对得到的 token（由驱动层持久化，见 [`Event::Paired`]）。
    pub token: Option<String>,
    /// 由 Host 唤醒时携带的一次性 token。
    pub launch_token: Option<String>,
    /// App 总览，握手时发送给 Host，由 Host 在模型首次接触该 App 时附带。
    pub overview: Option<AppOverview>,
    pub reconnect: ReconnectPolicy,
    pub heartbeat: HeartbeatPolicy,
    /// 同时执行的调用上限；超出的调用按到达顺序排队。默认 1（串行）。
    pub max_concurrent_calls: usize,
    /// 同一资源两次 `resources/updated` 通知之间的最小间隔。默认 100ms。
    pub resource_update_throttle_ms: Millis,
    /// 发送 `app/hello` 后等待结果的最长时间，超时断开并重连。默认 10s；0 表示不限。
    pub handshake_timeout_ms: Millis,
    /// 生命周期策略（spec/lifecycle.md 第 3 节）。默认 `persistent`（不休眠）。
    pub lifecycle: LifecyclePolicy,
    /// 期望的 Host 用户（spec/protocol.md 1.6）：握手结果的 `user` 与之不同时进入
    /// [`ConnectionState::HostMismatch`]。`None`（默认）不核对用户（网页、Android / iOS）；
    /// 原生运行时填入 [`app_mcp_protocol::identity::expected_host_user`]。
    pub expected_host_user: Option<String>,
}

impl ClientConfig {
    /// 其余字段取默认值。
    pub fn new(
        app_id: impl Into<String>,
        app_name: impl Into<String>,
        instance_id: impl Into<String>,
        client_kind: ClientKind,
    ) -> Self {
        Self {
            app_id: app_id.into(),
            app_name: app_name.into(),
            instance_id: instance_id.into(),
            client_kind,
            sdk_version: env!("CARGO_PKG_VERSION").to_owned(),
            app_version: None,
            origin: None,
            instance_title: None,
            instance_url: None,
            token: None,
            launch_token: None,
            overview: None,
            reconnect: ReconnectPolicy::default(),
            heartbeat: HeartbeatPolicy::default(),
            max_concurrent_calls: 1,
            resource_update_throttle_ms: 100,
            handshake_timeout_ms: 10_000,
            lifecycle: LifecyclePolicy::default(),
            expected_host_user: None,
        }
    }
}

/// 生命周期模式（spec/lifecycle.md 第 3 节）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LifecycleMode {
    /// 不休眠（兼容现有行为）。
    #[default]
    Persistent,
    /// 启动即连接；空闲 `idle_timeout_ms` 后休眠；唤醒后回连。
    Idle,
    /// 启动时不连接（进入 `Dormant`）；被唤醒或 `connect_now()` 时连接，任务完成后经过 `grace_ms` 休眠。
    OnDemand,
}

/// 休眠后的进程驻留策略。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Residency {
    /// 只断开连接，进程照常运行。
    #[default]
    Keep,
    /// 仅当本进程由唤醒冷启动时，休眠后发出 [`Event::IdleExit`]，由 App 决定是否退出。
    ExitWhenIdle,
    /// 每次休眠后都发出 [`Event::IdleExit`]（无界面的辅助进程）。
    ExitAlways,
}

/// 生命周期策略。
#[derive(Clone, Debug, PartialEq)]
pub struct LifecyclePolicy {
    pub mode: LifecycleMode,
    /// `idle` 模式下空闲多久进入休眠。默认 60s。
    pub idle_timeout_ms: Millis,
    /// 可见性为 `hidden` / `frozen` 时使用的空闲时间（与模式对应的超时取较小值）。默认 15s。
    pub hidden_idle_timeout_ms: Millis,
    /// `on-demand` 模式下任务完成后保留连接的时间。默认 10s。
    pub grace_ms: Millis,
    pub residency: Residency,
    /// 本实例的唤醒描述，随 `app/sleep` 上报；`None` 时 Host 回退到清单 `launch`。
    pub wake: Option<WakeDescriptor>,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            mode: LifecycleMode::Persistent,
            idle_timeout_ms: 60_000,
            hidden_idle_timeout_ms: 15_000,
            grace_ms: 10_000,
            residency: Residency::Keep,
            wake: None,
        }
    }
}

/// 断线重连的指数退避。第 n 次重试的延迟为 `min(initial * multiplier^n, max)`。
#[derive(Clone, Debug, PartialEq)]
pub struct ReconnectPolicy {
    pub initial_delay_ms: Millis,
    pub max_delay_ms: Millis,
    pub multiplier: f64,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self { initial_delay_ms: 500, max_delay_ms: 30_000, multiplier: 2.0 }
    }
}

/// 心跳：连接期间每隔 `interval_ms` 发送一次 `ping`，
/// 超过超时时间未收到响应即视为断开。
#[derive(Clone, Debug, PartialEq)]
pub struct HeartbeatPolicy {
    pub interval_ms: Millis,
    /// 实例可见时的超时。
    pub timeout_ms: Millis,
    /// 实例隐藏或冻结时的超时（后台页面定时器会被浏览器限流）。
    pub hidden_timeout_ms: Millis,
}

impl Default for HeartbeatPolicy {
    fn default() -> Self {
        Self { interval_ms: 15_000, timeout_ms: 10_000, hidden_timeout_ms: 120_000 }
    }
}

// ---------------------------------------------------------------------------
// 句柄与定义
// ---------------------------------------------------------------------------

/// 工具句柄。在一个 [`Client`] 内唯一，不复用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToolId(pub u64);

/// 资源句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(pub u64);

/// Scope 句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopeId(pub u64);

/// 一次资源读取请求的句柄。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReadId(pub u64);

/// 阻止休眠的持有句柄（[`Client::hold`]），用 [`Client::release_hold`] 释放。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HoldId(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub struct ToolDef {
    /// App 内唯一，`[a-zA-Z0-9_.-]{1,64}`。
    pub name: String,
    pub description: String,
    /// JSON Schema，`type` 必须为 `"object"`。
    pub input_schema: Value,
    pub risk: Risk,
    pub activation: Option<Activation>,
    pub title: Option<String>,
    /// 为 false 时不同步给 Host（等同于从 Host 的角度看不存在）。
    pub enabled: bool,
    /// 所属 scope；scope 被销毁时工具自动注销。
    pub scope: Option<ScopeId>,
}

/// 对已注册工具的部分更新；`None` 表示不变。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolUpdate {
    pub description: Option<String>,
    pub input_schema: Option<Value>,
    pub risk: Option<Risk>,
    pub activation: Option<Option<Activation>>,
    pub title: Option<Option<String>>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceDef {
    /// App 内唯一，`[a-zA-Z0-9_.-]{1,64}`。
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
    pub scope: Option<ScopeId>,
}

/// handler 成功返回的内容。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CallOutput {
    pub data: Value,
    /// 调用后内容可能已变化的资源名。
    pub state_hints: Vec<String>,
}

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
    /// 调用 handler。完成后调用 [`Client::complete_call`]。
    InvokeTool { call_id: String, tool: ToolId, name: String, arguments: Value },
    /// 取消调用：驱动层应中止 handler（如触发 AbortSignal）。之后的 `complete_call` 会被忽略。
    CancelTool { call_id: String, reason: CancelReason },
    /// 读取资源。完成后调用 [`Client::complete_read`]。
    ReadResource { read: ReadId, resource: ResourceId, name: String },
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
}

impl ConnectionState {
    /// 连接是否处于"已建立或正在建立"（包括握手中）。
    fn is_link_up(&self) -> bool {
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

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// sans-IO 客户端。非线程安全，由驱动层串行调用。
#[derive(Debug)]
pub struct Client {
    config: ClientConfig,
    state: ConnectionState,
    token: Option<String>,
    /// 一次性唤醒 token；首次成功握手后清除。
    launch_token: Option<String>,
    events: VecDeque<Event>,
    registry: registry::Registry,
    calls: calls::Calls,
    session: connection::Session,
    visibility: Visibility,
    focused: bool,
    next_request_id: i64,
    next_read_id: u64,
    /// 自上次成功握手以来的重试次数。
    retry_count: u32,
    /// 生命周期的跨连接状态（持有、恢复令牌、唤醒原因等）。
    life: lifecycle::Life,
    /// 待上报的连接问题（[`Client::report_issue`]），下次握手成功后以 `app/diagnostic` 发出。
    diagnostics: Vec<DiagnosticParams>,
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        let life = lifecycle::Life { launched_by_wake: config.launch_token.is_some(), ..Default::default() };
        Self {
            life,
            token: config.token.clone(),
            launch_token: config.launch_token.clone(),
            config,
            state: ConnectionState::Idle,
            events: VecDeque::new(),
            registry: registry::Registry::default(),
            calls: calls::Calls::default(),
            session: connection::Session::default(),
            visibility: Visibility::Visible,
            focused: true,
            next_request_id: 0,
            next_read_id: 0,
            retry_count: 0,
            diagnostics: Vec::new(),
        }
    }

    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    pub fn state(&self) -> &ConnectionState {
        &self.state
    }

    /// Host 为当前连接分配的连接 ID（`HelloResult.connectionId`，spec/protocol.md 10.3）；
    /// 未连接、或旧 Host 不提供时为 `None`。驱动层在日志中带上它，便于与 Host 日志对照。
    pub fn connection_id(&self) -> Option<&str> {
        self.session.connection_id.as_deref()
    }

    /// 记录一次连接问题（如网页被浏览器拦截），下次握手成功后以 `app/diagnostic` 上报给 Host
    /// （spec/protocol.md 10.2）。同一 `code` 合并计数并保留最新说明；最多保留
    /// [`MAX_PENDING_DIAGNOSTICS`] 个不同的码（更多的丢弃）。已连接时在下一轮事件中直接发出。
    pub fn report_issue(&mut self, code: &str, message: &str) {
        if let Some(d) = self.diagnostics.iter_mut().find(|d| d.code == code) {
            d.count = d.count.saturating_add(1);
            d.message = message.to_owned();
        } else if self.diagnostics.len() < MAX_PENDING_DIAGNOSTICS {
            self.diagnostics.push(DiagnosticParams { code: code.to_owned(), message: message.to_owned(), count: 1 });
        }
        if self.connected() {
            self.flush_diagnostics();
        }
    }

    /// 当前已配对的 token（配置中带入的或配对后获得的）。
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// 开始工作：产生 [`Event::Connect`]，状态变为 `Connecting`。重复调用无效果。
    ///
    /// `on-demand` 模式下（且之前没有收到唤醒、配置中没有 launch token）不连接，直接进入 `Dormant`。
    pub fn start(&mut self, now: Millis) {
        self.life.last_now = now;
        if self.state == ConnectionState::Idle {
            self.on_start();
        }
    }

    /// 停止：取消所有进行中与排队的调用（[`CancelReason::Stopped`]），
    /// 连接中则产生 [`Event::Disconnect`]，状态变为 `Stopped`，不再重连。
    /// 停止前已排队的消息（如刚完成的调用结果）保留在 [`Event::Disconnect`] 之前，驱动层有机会先发出。
    pub fn stop(&mut self, now: Millis) {
        self.life.last_now = now;
        if self.state == ConnectionState::Stopped {
            return;
        }
        let link_up = self.link_up();
        self.teardown(CancelReason::Stopped, true);
        if link_up {
            self.events.push_back(Event::Disconnect);
        }
        self.set_state(ConnectionState::Stopped);
    }

    // ---- scope ----------------------------------------------------------

    /// 创建 scope。`name` 仅用于调试，不影响工具名。
    pub fn create_scope(&mut self, name: &str, parent: Option<ScopeId>) -> Result<ScopeId, CoreError> {
        self.registry.create_scope(name, parent)
    }

    /// 销毁 scope：递归注销其下全部子 scope、工具与资源。
    pub fn dispose_scope(&mut self, scope: ScopeId) -> Result<(), CoreError> {
        self.registry.dispose_scope(scope)?;
        self.prune_subscriptions();
        Ok(())
    }

    /// 调试用：scope 创建时的名称；scope 不存在（或已销毁）时为 `None`。
    pub fn scope_name(&self, scope: ScopeId) -> Option<&str> {
        self.registry.scope_name(scope)
    }

    // ---- 工具 -----------------------------------------------------------

    /// 注册工具。已存在同名工具时返回 [`CoreError::DuplicateName`]（与是否启用无关）。
    /// 连接期间，变更在下一次 [`Client::poll_event`] 时合并为一条 `tools/changed`。
    ///
    /// 工具名是 App 内的局部名，Host 对外暴露为 `<appId>.<局部名>`（spec/protocol.md 3.1）。
    /// 名称以 `<appId>.` 开头时仍按原样注册（协议语义不变），但产生 [`Event::Warning`]——
    /// 这通常是误把全名写成了局部名，对外会变成 `<appId>.<appId>.…`。
    pub fn register_tool(&mut self, def: ToolDef) -> Result<ToolId, CoreError> {
        let prefixed = proto::has_app_id_prefix(&def.name, &self.config.app_id).then(|| def.name.clone());
        let id = self.registry.register_tool(def)?;
        if let Some(name) = prefixed {
            self.warn(proto::app_id_prefix_warning(&name, &self.config.app_id));
        }
        Ok(id)
    }

    pub fn update_tool(&mut self, tool: ToolId, update: ToolUpdate) -> Result<(), CoreError> {
        self.registry.update_tool(tool, update)
    }

    /// 注销工具。该工具进行中的调用不受影响，照常完成。
    pub fn unregister_tool(&mut self, tool: ToolId) -> Result<(), CoreError> {
        self.registry.unregister_tool(tool)
    }

    /// 调试用：当前工具定义。
    pub fn tool_def(&self, tool: ToolId) -> Option<&ToolDef> {
        self.registry.tool(tool)
    }

    // ---- 资源 -----------------------------------------------------------

    pub fn register_resource(&mut self, def: ResourceDef) -> Result<ResourceId, CoreError> {
        self.registry.register_resource(def)
    }

    /// 资源内容变化。仅当 Host 订阅了该资源时才发送 `resources/updated`，
    /// 并按 `resource_update_throttle_ms` 节流（节流期内的多次变化合并为一次）。
    pub fn notify_resource_changed(&mut self, resource: ResourceId, now: Millis) -> Result<(), CoreError> {
        if self.registry.resource(resource).is_none() {
            return Err(CoreError::UnknownResource(resource));
        }
        self.on_resource_changed(resource, now);
        Ok(())
    }

    pub fn unregister_resource(&mut self, resource: ResourceId) -> Result<(), CoreError> {
        self.registry.unregister_resource(resource)?;
        self.prune_subscriptions();
        Ok(())
    }

    /// 已注销资源的订阅随之移除。
    fn prune_subscriptions(&mut self) {
        let registry = &self.registry;
        let before = self.session.subscriptions.len();
        self.session.subscriptions.retain(|name, _| registry.resource_by_name(name).is_some());
        if self.session.subscriptions.len() != before {
            // 没有时间参数：请驱动层尽快调用 handle_timeout，届时重新判定空闲。
            self.request_idle_recheck();
        }
    }

    /// 调试用：Host 当前是否订阅了该资源。
    pub fn is_subscribed(&self, resource: ResourceId) -> bool {
        self.registry.resource(resource).is_some_and(|d| self.session.subscriptions.contains_key(&d.name))
    }

    // ---- 可见性 ---------------------------------------------------------

    /// 实例可见性或焦点变化。连接期间发送 `app/visibility`（值未变化时不发送），
    /// 并据此选择心跳超时。
    /// 可见性变化会重新开始空闲计时（隐藏时使用 `hidden_idle_timeout_ms`）。
    pub fn set_visibility(&mut self, visibility: Visibility, focused: bool, now: Millis) {
        self.life.last_now = now;
        if self.visibility == visibility && self.focused == focused {
            return;
        }
        let visibility_changed = self.visibility != visibility;
        self.visibility = visibility;
        self.focused = focused;
        if self.state == ConnectionState::Connected {
            self.notify(proto::method::VISIBILITY, &proto::VisibilityParams { visibility, focused });
        }
        if visibility_changed {
            self.restart_idle_timer(now);
        }
    }

    // ---- 驱动层输入 -----------------------------------------------------

    /// 连接已建立：发送 `app/hello`，状态变为 `Handshaking`。
    pub fn handle_connected(&mut self, now: Millis) {
        self.life.last_now = now;
        if !matches!(self.state, ConnectionState::Connecting | ConnectionState::Waking) {
            self.warn(format!("当前状态 {:?} 下收到连接建立通知，已忽略", self.state));
            return;
        }
        self.send_hello();
        let timeout = self.config.handshake_timeout_ms;
        self.session.handshake_deadline = (timeout > 0).then(|| now.saturating_add(timeout));
        self.set_state(ConnectionState::Handshaking);
    }

    /// 连接断开（或建立失败）：取消所有进行中与排队的调用（[`CancelReason::Disconnected`]），
    /// 放弃进行中的资源读取，进入 `Backoff`。`Stopped` / `Rejected` 状态下不重连。
    pub fn handle_disconnected(&mut self, now: Millis) {
        self.disconnected(None, now);
    }

    /// 建立连接失败，带驱动层归类的原因（如 `HOST_NOT_RUNNING`、`IPC_PERMISSION_DENIED`，spec/protocol.md 10.1）：
    /// 与 [`Client::handle_disconnected`] 相同，进入的 `Backoff` 状态带 `reason` / `code`。
    pub fn handle_connect_failed(&mut self, issue: ConnectionIssue, now: Millis) {
        self.disconnected(Some(issue), now);
    }

    /// 已建立的连接断开，带驱动层归类的原因（`CONNECTION_CLOSED` / `CONNECTION_LOST`，spec/protocol.md 10.1）：
    /// 与 [`Client::handle_disconnected`] 相同，进入的 `Backoff` 状态带 `reason` / `code`。
    pub fn handle_disconnected_with(&mut self, issue: ConnectionIssue, now: Millis) {
        self.disconnected(Some(issue), now);
    }

    fn disconnected(&mut self, issue: Option<ConnectionIssue>, now: Millis) {
        self.life.last_now = now;
        if self.link_up() {
            self.teardown(CancelReason::Disconnected, false);
            self.enter_backoff(now, issue);
        }
    }

    /// 处理一条收到的文本消息。格式错误产生 [`Event::Warning`]，不会 panic。
    pub fn handle_message(&mut self, text: &str, now: Millis) {
        self.life.last_now = now;
        self.on_message(text, now);
        self.refresh_idle(now);
    }

    /// 定时器到期（心跳、重连、调用超时、资源节流）。可以多调用，没有到期项时无效果。
    pub fn handle_timeout(&mut self, now: Millis) {
        self.life.last_now = now;
        self.on_timeout(now);
    }

    /// handler 完成。已取消或已超时的调用返回 [`CoreError::UnknownCall`]，不发送任何消息。
    pub fn complete_call(
        &mut self,
        call_id: &str,
        outcome: Result<CallOutput, ToolError>,
        now: Millis,
    ) -> Result<(), CoreError> {
        self.life.last_now = now;
        let call = self.calls.take_running(call_id).ok_or_else(|| CoreError::UnknownCall(call_id.to_owned()))?;
        self.finish_call(call, outcome);
        self.refresh_idle(now);
        Ok(())
    }

    /// 资源读取完成。
    pub fn complete_read(&mut self, read: ReadId, outcome: Result<Value, ToolError>) -> Result<(), CoreError> {
        let pending = self.session.reads.remove(&read).ok_or(CoreError::UnknownRead(read))?;
        self.finish_read(pending, outcome);
        // 没有时间参数：请驱动层尽快调用 handle_timeout，届时重新判定空闲。
        self.request_idle_recheck();
        Ok(())
    }

    /// 调试用：正在执行的调用数。
    pub fn running_call_count(&self) -> usize {
        self.calls.running_len()
    }

    /// 调试用：排队等待执行的调用数。
    pub fn queued_call_count(&self) -> usize {
        self.calls.queued_len()
    }

    // ---- 驱动层输出 -----------------------------------------------------

    /// 取出下一个事件。驱动层应在每次调用输入方法后循环取到 `None` 为止。
    pub fn poll_event(&mut self) -> Option<Event> {
        self.flush_changes();
        self.events.pop_front()
    }

    /// 下一次需要调用 [`Client::handle_timeout`] 的时刻；没有待处理定时器时为 `None`。
    /// `Dormant` 状态下始终为 `None`。
    pub fn poll_timeout(&self) -> Option<Millis> {
        self.next_timeout()
    }

    // ---- 生命周期（spec/lifecycle.md）------------------------------------

    /// 处理操作系统激活参数 / URL。识别 `app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=<token>`
    /// （以及 `<scheme>:app-mcp/wake?token=`）、URL 片段 `#app-mcp-wake=<token>`；`args` 可以是完整命令行。
    ///
    /// 不是本 SDK 的唤醒时返回 `false`，不改变任何状态。识别成功时令牌作为下次握手的 `launchToken`：
    /// `Dormant` → 回连（`Waking`）；`Backoff` → 立即重连；`Idle`（尚未 `start`）→ 记录，`start` 时连接
    /// （`on-demand` 模式也会连接）；休眠握手进行中 → 休眠完成后立即回连；已连接（不在休眠握手中）→ 只重新开始
    /// 空闲计时，令牌丢弃（Host 按实例 ID 认领本连接，不影响之后的休眠）。
    pub fn handle_wake(&mut self, args: &str, now: Millis) -> bool {
        self.life.last_now = now;
        let Some(token) = parse_wake_token(args) else { return false };
        self.on_wake_token(token, now);
        true
    }

    /// App 主动回连（原因 `app`），等同于 `wake_with_reason(WakeReason::App, now)`。
    pub fn wake(&mut self, now: Millis) -> bool {
        self.wake_with_reason(WakeReason::App, now)
    }

    /// 以指定原因回连（Web：页面重新可见时用 [`WakeReason::Visible`]）。
    ///
    /// `Dormant` → `Waking` 并产生 [`Event::Connect`]；`Backoff` → 立即重连；休眠握手进行中 → 休眠完成后立即回连；
    /// 已连接时重新开始空闲计时。返回是否因此发起了（或安排了）回连。
    pub fn wake_with_reason(&mut self, reason: WakeReason, now: Millis) -> bool {
        self.life.last_now = now;
        self.on_wake(reason, now)
    }

    /// `on-demand` 模式下主动连接（原因 `app`）。与 [`Client::wake`] 相同，另外在尚未 `start` 时等同于 `start`。
    pub fn connect_now(&mut self, now: Millis) -> bool {
        self.life.last_now = now;
        if self.state == ConnectionState::Idle {
            self.life.pending_wake = Some(WakeReason::App);
            self.on_start();
            return true;
        }
        self.on_wake(WakeReason::App, now)
    }

    /// App 主动请求休眠（原因 `app`），等同于 `sleep_with_reason(SleepReason::App, now)`。
    pub fn sleep(&mut self, now: Millis) -> bool {
        self.sleep_with_reason(SleepReason::App, now)
    }

    /// 以指定原因请求休眠（不看空闲条件，也不受 `hold` 影响；任何模式都可用）。
    ///
    /// 已连接：发送 `app/sleep`，被拒绝时按 `retryAfterMs`（缺省 5s）重试；连接建立中或握手中：直接断开进入
    /// `Dormant`；`Backoff`：停止重连进入 `Dormant`。返回是否有效果。
    pub fn sleep_with_reason(&mut self, reason: SleepReason, now: Millis) -> bool {
        self.life.last_now = now;
        self.on_sleep_requested(reason, now)
    }

    /// 临时阻止自动休眠（如 App 在做长任务），用 [`Client::release_hold`] 释放。
    /// 休眠握手进行中时，休眠完成后会立即回连。
    pub fn hold(&mut self, now: Millis) -> HoldId {
        self.life.last_now = now;
        self.add_hold(None, now)
    }

    /// 为某个调用延长持有（handler 发起的长任务在调用完成后仍需保持连接）。调用不存在时返回
    /// [`CoreError::UnknownCall`]。持有与调用的生命周期无关，需要显式释放。
    pub fn hold_for_call(&mut self, call_id: &str, now: Millis) -> Result<HoldId, CoreError> {
        self.life.last_now = now;
        if !self.calls.contains(call_id) {
            return Err(CoreError::UnknownCall(call_id.to_owned()));
        }
        Ok(self.add_hold(Some(call_id.to_owned()), now))
    }

    /// 释放持有。已释放或未知的句柄返回 `false`。
    pub fn release_hold(&mut self, hold: HoldId, now: Millis) -> bool {
        self.life.last_now = now;
        let removed = self.life.holds.remove(&hold.0).is_some();
        if removed {
            self.refresh_idle(now);
        }
        removed
    }

    /// 调试用：当前持有数。
    pub fn hold_count(&self) -> usize {
        self.life.holds.len()
    }

    /// 当前工具与资源定义的摘要 `toolsHash`（spec/lifecycle.md 第 6 节）。
    pub fn tools_hash(&self) -> String {
        let (tools, resources) = self.registry.snapshot();
        proto::tools_hash(&tools, &resources)
    }

    /// 上次休眠时 Host 返回、尚未用于握手的恢复令牌。
    pub fn resume_token(&self) -> Option<&str> {
        self.life.resume_token.as_deref()
    }

    /// 调试用：是否已发送 `app/sleep`、正在等待结果（对外状态仍为 `Connected`）。
    pub fn is_sleep_pending(&self) -> bool {
        self.session.sleeping.is_some()
    }
}
