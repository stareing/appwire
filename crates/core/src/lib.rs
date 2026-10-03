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
//! 行为规范见 `spec/protocol.md` 的"SDK 行为"一节。本文件及其重导出的 `config` / `handles` / `state` 中的公开 API 是
//! 核心与各语言绑定之间的契约，修改前需同步更新绑定层。

mod calls;
mod config;
mod connection;
mod dedup;
mod events;
mod handles;
mod lifecycle;
mod registry;
mod state;
mod vec_map;

use std::collections::VecDeque;

use app_mcp_protocol as proto;
use serde_json::Value;

pub use proto::{
    Activation, AppOverview, Audience, ClientKind, ConnectionErrorCode, ConnectionIssue, ContentAnnotations,
    DiagnosticParams, EventInfo, LifecycleMode, MAX_EVENT_PAYLOAD_BYTES, ResultStatus, Risk, SleepReason, ToolAnnotations, ToolError, ToolSurface, TransportKind,
    Visibility, WakeDescriptor, WakeKind, WakeReason, navigation_reason,
};
pub use config::*;
pub use dedup::CallDedupPolicy;
pub use handles::*;
pub use lifecycle::parse_wake_token;
pub use state::*;

/// 调用方提供的单调时钟（毫秒）。
pub type Millis = u64;

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
    /// 已开始执行的调用的首次结果（跨连接保留，spec/protocol.md 3.3）。
    dedup: dedup::DedupTable,
    session: connection::Session,
    visibility: Visibility,
    focused: bool,
    /// App 声明用户正在操作（[`Client::set_busy`]）。
    busy: bool,
    next_request_id: i64,
    next_read_id: u64,
    next_navigate_id: u64,
    /// 自上次成功握手以来的重试次数。
    retry_count: u32,
    /// 生命周期的跨连接状态（持有、恢复令牌、唤醒原因等）。
    life: lifecycle::Life,
    /// 待上报的连接问题（[`Client::report_issue`]），下次握手成功后以 `app/diagnostic` 发出。
    diagnostics: Vec<DiagnosticParams>,
    /// 运行时声明的事件与 `eventId` 计数（第 16 项 N3）。
    declared_events: events::Events,
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        let life = lifecycle::Life {
            launched_by_wake: config.launch_token.is_some() || config.launched_by_activation,
            ..Default::default()
        };
        Self {
            life,
            token: config.token.clone(),
            launch_token: config.launch_token.clone(),
            config,
            state: ConnectionState::Idle,
            events: VecDeque::new(),
            registry: registry::Registry::default(),
            calls: calls::Calls::default(),
            dedup: dedup::DedupTable::default(),
            session: connection::Session::default(),
            visibility: Visibility::Visible,
            focused: true,
            busy: false,
            next_request_id: 0,
            next_read_id: 0,
            next_navigate_id: 0,
            retry_count: 0,
            diagnostics: Vec::new(),
            declared_events: events::Events::default(),
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

    /// 更新工具。放宽了并发声明（`concurrency` / `exclusive`）时排队中的调用随即可能开始。
    pub fn update_tool(&mut self, tool: ToolId, update: ToolUpdate) -> Result<(), CoreError> {
        self.registry.update_tool(tool, update)?;
        self.settle_busy();
        Ok(())
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
    ///
    /// 未连接时（或回连后 Host 重新订阅之前），上次连接断开时被订阅的资源记为"已变化"，Host 重新订阅时补发；
    /// `Dormant` 中声明了 `realtime` 的这类资源变化会以 [`WakeReason::App`] 回连（spec/lifecycle.md 第 13 节 B3）。
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

    // ---- 事件（第 16 项 N3，spec/protocol.md 3.5）--------------------------

    /// 声明本实例可发出的事件；同名替换。已连接且声明有变化时随即重发全量 `events/sync`，否则在下次握手成功后发送。
    /// 名称规则同工具名，不合法返回 [`CoreError::InvalidName`]。
    pub fn declare_event(&mut self, info: EventInfo) -> Result<(), CoreError> {
        self.on_declare_event(info)
    }

    /// 撤销事件声明；未声明过返回 `false`。已连接时随即重发全量 `events/sync`。
    pub fn remove_event(&mut self, name: &str) -> bool {
        self.on_remove_event(name)
    }

    /// 发出已声明的事件（`events/emit`），`eventId` 由核心生成、实例内单调唯一。
    ///
    /// 已连接时发送并返回 `true`；未连接（休眠、断线、重连中、握手中）丢弃并返回 `false`——不缓存、不触发连接，
    /// 也不算调用活动（不推迟空闲休眠）。
    ///
    /// @error 名称不合法 → [`CoreError::InvalidName`]；未声明 → [`CoreError::UnknownEvent`]；`payload` 不是对象
    /// 或序列化后超过 [`MAX_EVENT_PAYLOAD_BYTES`] → [`CoreError::InvalidEventPayload`]。出错时不发送。
    pub fn emit_event(&mut self, name: &str, payload: Option<Value>) -> Result<bool, CoreError> {
        self.on_emit_event(name, payload)
    }

    // ---- 可见性 ---------------------------------------------------------

    /// 实例可见性或焦点变化。连接期间发送 `app/visibility`（值未变化时不发送），
    /// 并据此选择心跳超时。
    /// 可见性变化会重新开始空闲计时（隐藏时使用 `hidden_idle_timeout_ms`）。
    /// [`LifecyclePolicy::sleep_on_background`] 开启时，从可见变为隐藏 / 冻结后空闲即以 `background` 休眠（B4）。
    pub fn set_visibility(&mut self, visibility: Visibility, focused: bool, now: Millis) {
        self.life.last_now = now;
        if self.visibility == visibility && self.focused == focused {
            return;
        }
        let visibility_changed = self.visibility != visibility;
        let was_visible = self.visibility == Visibility::Visible;
        self.visibility = visibility;
        self.focused = focused;
        if self.state == ConnectionState::Connected {
            self.notify(proto::method::VISIBILITY, &proto::VisibilityParams { visibility, focused });
        }
        if visibility_changed {
            self.restart_idle_timer(now);
            self.on_visibility_changed(was_visible, now);
        }
    }

    // ---- 驱动层输入 -----------------------------------------------------

    /// 连接已建立：发送 `app/hello`，状态变为 `Handshaking`。
    pub fn handle_connected(&mut self, now: Millis) {
        self.life.last_now = now;
        if !matches!(self.state, ConnectionState::Connecting | ConnectionState::Waking) {
            self.warn(format!("当前状态 {} 下收到连接建立通知，已忽略", self.state.name()));
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

    /// 报告进行中调用的进度（`tools/progress`，spec/protocol.md 3.3）。
    ///
    /// 未连接时丢弃（进度只是提示）；`progress` 不是有限数时丢弃并产生 [`Event::Warning`]，非有限的 `total` 视为未知。
    /// 调用不在执行中（排队、已完成、已取消）返回 [`CoreError::UnknownCall`]。
    pub fn report_progress(
        &mut self,
        call_id: &str,
        progress: f64,
        total: Option<f64>,
        message: Option<String>,
        now: Millis,
    ) -> Result<(), CoreError> {
        self.life.last_now = now;
        if !self.calls.is_running(call_id) {
            return Err(CoreError::UnknownCall(call_id.to_owned()));
        }
        self.send_progress(call_id, progress, total, message);
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

    /// 导航完成（[`Event::Navigate`]）：`Ok` 回复 `{ok: true}`；失败用 `NAVIGATION_FAILED` / `NAVIGATION_DENIED`
    /// （[`ToolError::navigation_failed`] / [`ToolError::navigation_denied`]），其他类别原样回复。
    pub fn complete_navigate(&mut self, navigate: NavigateId, outcome: Result<(), ToolError>) -> Result<(), CoreError> {
        let request_id = self.session.navigations.remove(&navigate).ok_or(CoreError::UnknownNavigate(navigate))?;
        self.finish_navigate(request_id, outcome);
        self.request_idle_recheck();
        Ok(())
    }

    /// 是否处理 Host 的 `app/navigate`（[`ClientConfig::navigation`]）。握手时声明，已连接时修改在下次连接生效；
    /// 关闭后新到的导航请求以 `NAVIGATION_FAILED`（`unsupported`）回复，进行中的不受影响。
    pub fn set_navigation(&mut self, enabled: bool) {
        self.config.navigation = enabled;
    }

    /// 不可见时导航请求是否仍交给导航回调（[`ClientConfig::navigate_in_background`]）。随时生效，只影响之后到达的请求。
    pub fn set_navigate_in_background(&mut self, enabled: bool) {
        self.config.navigate_in_background = enabled;
    }

    /// 声明用户正在 / 不再在 App 内操作（第 16 项 N6，spec/protocol.md 5.3）：期间写调用按 [`ClientConfig::busy_policy`]
    /// 拒绝或排队，只读调用与已开始的调用不受影响。设为 `busy` 且策略为拒绝时，排队中的写调用随即被拒绝；取消后排队中的调用随即开始。
    /// 何时算"正在操作"由 App 决定（如编辑框获得焦点、拖拽中），本库不推断。
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        self.settle_busy();
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// 修改 [`ClientConfig::busy_policy`]，随即对排队中的调用生效。
    pub fn set_busy_policy(&mut self, policy: BusyPolicy) {
        self.config.busy_policy = policy;
        self.settle_busy();
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

    /// Hub 经系统名字服务拨入了一条通道（spec/naming.md 第 3 节，"被连接方"）：驱动层已持有该通道、尚未在其上
    /// 收发任何数据。产生 [`Event::Connect`]，驱动层在这条通道上（而不是拨号到 Host 端点）完成连接后照常调用
    /// [`Client::handle_connected`]——之后与 App 拨出的连接走同一个状态机：SDK 先发 `app/hello`（`wakeReason:
    /// "os-activation"`），握手方向不变。
    ///
    /// 这条通道上（直到断开）：不发心跳、不做 App 端空闲计时（关闭时机由 Hub 决定；App 仍可 [`Client::sleep`]
    /// 请 Hub 提前关闭）；断开后不重连——`persistent` 回到 `Backoff` 照常重连 Host 端点，其余模式进入 `Dormant`
    /// （[`Residency`] 规则同休眠）。
    ///
    /// 只在 `Dormant`、`Backoff`、`HostMismatch` 状态下接受；尚未 `start`、已停止 / 被拒绝、已有连接（含建立中）时
    /// 返回 `false`，驱动层应拒绝这次拨号（spec/naming.md 9.1 `CHANNEL_LIMIT`）。
    pub fn accept_channel(&mut self, now: Millis) -> bool {
        self.life.last_now = now;
        self.on_accept_channel()
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
        self.registry.peek_tools_hash()
    }

    /// 上次休眠时 Host 返回、尚未用于握手的恢复令牌。
    pub fn resume_token(&self) -> Option<&str> {
        self.life.resume_token.as_deref()
    }

    /// 本连接 SDK 发送心跳的间隔；`None` = 不发心跳（spec/lifecycle.md 第 11 节 A3）。
    ///
    /// `legacy_timers` 或 [`HeartbeatMode::Always`] → 发；[`HeartbeatMode::Off`] → 不发；[`HeartbeatMode::Auto`] →
    /// 传输为 [`TransportKind::Ipc`] / [`TransportKind::Loopback`] 时不发，其余发。`interval_ms` 为 0 时不发。
    pub fn heartbeat_interval(&self) -> Option<Millis> {
        let hb = &self.config.heartbeat;
        let on = self.config.lifecycle.legacy_timers
            || match hb.mode {
                HeartbeatMode::Always => true,
                HeartbeatMode::Off => false,
                HeartbeatMode::Auto => !matches!(self.config.transport, TransportKind::Ipc | TransportKind::Loopback),
            };
        // 名字服务通道是本地传输，存活由 EOF 与系统对端死亡通知判断（spec/naming.md 第 3 节）。
        let on = on && !self.life.inbound;
        Some(hb.interval_ms).filter(|ms| on && *ms > 0)
    }

    /// 调试用：是否已发送 `app/sleep`、正在等待结果（对外状态仍为 `Connected`）。
    pub fn is_sleep_pending(&self) -> bool {
        self.session.sleeping.is_some()
    }
}
