//! app-mcp-native：原生 App 的共用运行时。
//!
//! 在后台线程上驱动 sans-IO 核心（`app-mcp-core`），负责连接 Host（默认走本地 IPC：Unix 域套接字 /
//! Windows 命名管道；显式配置时也可用 `ws://` / `wss://`，见 [`NativeConfig::host_url`]）、计时与回调分发。所有语言绑定都建在它之上：
//!
//! - `bindings/c`（C ABI）→ C、C++、C#（P/Invoke）、Dart（dart:ffi）
//! - `bindings/uniffi` → Kotlin、Swift、Python
//! - `bindings/node`（napi-rs）→ Node、Electron 主进程
//! - Rust App（Tauri、egui 等）直接依赖本 crate
//!
//! # 线程模型
//!
//! - [`NativeClient`] 及所有句柄都是 `Send + Sync`，可在任意线程调用。
//! - 注册类方法同步返回结果（如重名错误）。
//! - [`ToolHandler::invoke`]、[`ResourceReader::read`]、[`ClientListener`] 的回调在库的
//!   **分发线程**上执行，调用时不持有任何内部锁。回调必须尽快返回：需要在 UI 线程执行的
//!   handler，由语言封装层切换线程后，再从任意线程调用 [`CallHandle::complete`] / [`CallHandle::fail`]。
//! - 每个 [`CallHandle`] 必须且只能完成一次；调用被取消后完成会返回 [`NativeError::AlreadyCompleted`]。
//!
//! # 生命周期（spec/lifecycle.md）
//!
//! [`NativeConfig::lifecycle`] 为 `idle` / `on-demand` 时，空闲后与 Host 完成 `app/sleep` 握手并进入
//! [`StateStatus::Dormant`]：关闭 socket、销毁 tokio 运行时（运行时线程在条件变量上阻塞，无定时器），
//! 分发线程阻塞等待。[`NativeClient::handle_wake`] / [`NativeClient::wake`] 时重建运行时并回连。
//!
//! 本文件中的公开 API 是原生层与各绑定之间的契约，修改前需同步更新绑定层。

use std::sync::Arc;

pub use app_mcp_core::{
    Activation, AppOverview, Audience, CallDedupPolicy, ClientKind, ContentAnnotations, HeartbeatMode, LifecycleMode, LifecyclePolicy,
    Residency, ResultStatus, Risk, SleepReason, ToolAnnotations, ToolSurface, TransportKind, Visibility, WakeDescriptor,
    WakeKind, WakeReason, parse_wake_token,
};
pub use app_mcp_protocol::{ConnectionErrorCode, ErrorKind, navigation_reason, user_action_reason};

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct NativeConfig {
    pub app_id: String,
    pub app_name: String,
    /// 为 `None` 时自动生成（每个进程一个）。
    pub instance_id: Option<String>,
    /// 默认 [`ClientKind::Native`]；Electron / Tauri 主进程用 [`ClientKind::Hybrid`]。
    pub client_kind: ClientKind,
    /// Host 端点（spec/protocol.md 第 1 节）：`unix:<绝对路径>`、`pipe:\\.\pipe\<名称>`、`ws://…` 或 `wss://…`。
    ///
    /// [`NativeConfig::new`] 的默认值（创建配置时解析一次）：环境变量 `APP_MCP_ENDPOINT`（非空时）→
    /// 登记文件 `~/.app-mcp/run/endpoints.json`（运行中的 Host 写下的实际端点；`APP_MCP_HOME` 可改配置目录）→
    /// 平台默认 IPC 端点（Linux `$XDG_RUNTIME_DIR/app-mcp/hub.sock`，否则 `~/.app-mcp/run/hub.sock`；macOS
    /// `~/.app-mcp/run/hub.sock`；Windows `\\.\pipe\app-mcp-<用户 SID>`）→ `ws://127.0.0.1:7717/app`
    /// （Android / iOS 等没有默认 IPC 端点的平台）。连不上时按退避重连同一端点，不换用其他传输。
    pub host_url: String,
    pub app_version: Option<String>,
    pub instance_title: Option<String>,
    /// 之前配对得到的 token（由 App 持久化，见 [`ClientListener::on_paired`]）。
    pub token: Option<String>,
    /// 为 `None` 时读取环境变量 `APP_MCP_LAUNCH_TOKEN`（Host 唤醒 App 时设置）。
    pub launch_token: Option<String>,
    /// 同时执行的调用上限，默认 1。
    pub max_concurrent_calls: u32,
    /// App 总览：握手时发给 Host，模型在会话中首次接触本 App 时由 Host 附带（spec/protocol.md 第 7 节）。
    pub overview: Option<AppOverview>,
    /// 生命周期策略（spec/lifecycle.md）。默认 `persistent`（不休眠）。
    pub lifecycle: LifecyclePolicy,
    /// 建立 WebSocket 连接（含 TLS 握手）的超时，默认 5000ms；超时按连接失败处理（进入重连退避）。
    pub connect_timeout_ms: u32,
    /// 心跳策略（spec/lifecycle.md 第 11 节）。默认 `Auto`：按端点的传输类别
    /// （[`app_mcp_protocol::Endpoint::transport_kind`]）决定，本地 IPC 与桌面本机回环不发心跳。
    pub heartbeat: HeartbeatMode,
    /// 调用去重（spec/protocol.md 3.3）：同一 `callId` 在有效期内只执行一次。默认保留 5 分钟、最多 64 条。
    pub call_dedup: CallDedupPolicy,
    /// 在系统名字服务登记本 App（spec/naming.md，"按名寻址"）：Hub 按名拨号时由本客户端接受通道（App 不必常驻、
    /// 不必主动连接 Hub；进程未运行时由系统激活）。Linux：D-Bus 会话总线名 `dev.appmcp.App.<appId>`（需要激活文件，
    /// `app-mcp-host app install` 生成）。默认 `false`。
    ///
    /// 登记在 [`NativeClient::start`] 之后进行、[`NativeClient::stop`] / 客户端被丢弃时注销；登记期间运行时线程保持
    /// 与名字服务的连接（阻塞等待，无定时器）。本平台不支持或登记失败时经 [`ClientListener::on_log`] 报告，其余照常。
    /// 通常与 `lifecycle.mode = OnDemand` 同用：App 不主动连接，只在 Hub 拨入时连接。
    pub register_name: bool,
    /// 登记实例名（spec/naming.md 2.1，`[a-z][a-z0-9-]{0,31}`，不能是 `default`）：`Some` 时另登记实例名字
    /// （D-Bus `dev.appmcp.App.<appId>.<instance>`），供 `appmcp://<appId>/<instance>` 寻址。默认 `None`。
    pub name_instance: Option<String>,
    /// 名字服务地址（Linux：D-Bus 地址，如 `unix:path=/run/user/1000/bus`）。`None`（默认）按环境：由名字服务激活时用
    /// `DBUS_STARTER_ADDRESS`，否则 `DBUS_SESSION_BUS_ADDRESS`。测试用私有总线时设置。
    pub name_service_address: Option<String>,
}

impl NativeConfig {
    pub fn new(app_id: impl Into<String>, app_name: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            app_name: app_name.into(),
            instance_id: None,
            client_kind: ClientKind::Native,
            host_url: app_mcp_protocol::endpoint::default_endpoint(),
            app_version: None,
            instance_title: None,
            token: None,
            launch_token: None,
            max_concurrent_calls: 1,
            overview: None,
            lifecycle: LifecyclePolicy::default(),
            connect_timeout_ms: 5_000,
            heartbeat: HeartbeatMode::Auto,
            call_dedup: CallDedupPolicy::default(),
            register_name: false,
            name_instance: None,
            name_service_address: None,
        }
    }
}

// ---------------------------------------------------------------------------
// 定义
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    /// App 内唯一，`[a-zA-Z0-9_.-]{1,64}`。
    pub name: String,
    pub description: String,
    /// JSON Schema 文本，`type` 必须为 `"object"`；为 `None` 时表示无参数。
    pub input_schema_json: Option<String>,
    pub risk: Risk,
    pub activation: Option<Activation>,
    pub title: Option<String>,
    pub enabled: bool,
}

impl ToolSpec {
    /// 其余字段：无参数、risk = write、enabled = true。
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema_json: None,
            risk: Risk::Write,
            activation: None,
            title: None,
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceSpec {
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
}

/// 工具的附加选项（`register_tool_with` / [`ToolHandle::update_with`]，spec/protocol.md 第 3 节）。
///
/// @compat 不放进 [`ToolSpec`]：给结构体加字段会破坏现有绑定与 App 的结构体字面量；以后的工具选项都加在这里。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolOptions {
    /// 标准 MCP 工具注解，原样转发给 Agent；`None` = 未声明（Hub 按 `risk` 推导）。
    pub annotations: Option<ToolAnnotations>,
    /// 结果的 JSON Schema 文本（MCP `outputSchema`）；`None` = 未声明。
    pub output_schema_json: Option<String>,
    /// 对界面的依赖（spec/protocol.md 3.4）：缺省 `App`；`View` 表示只在所在界面可见且处于最上层时注册（由封装层决定注册时机）。
    pub surface: ToolSurface,
    /// 所在页面名（`[a-zA-Z0-9_.-]{1,64}`）；Hub 在该工具未注册时据此导航（[`NativeClient::set_navigation_handler`]）。
    pub page: Option<String>,
    /// 后台替代（spec/protocol.md 3.4）：同一 App 中一个 `App` 工具的名称；本 `View` 工具因 App 在后台不可调用时
    /// Hub 改调它。`None` = 未声明。
    pub background_tool: Option<String>,
}

/// 调用成功的完整结果（[`CallHandle::complete_with`]，spec/protocol.md 3.2）。`Default` = 无返回值、`done`。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CallResult {
    /// 返回值 JSON 文本；`None` 表示无返回值（`null`，Hub 对模型输出"已完成"）。
    pub data_json: Option<String>,
    /// 调用后内容可能已变化的资源名。
    pub state_hints: Vec<String>,
    /// 业务状态：`Pending`（已受理、待 App 内确认或异步完成）/ `Partial` / `Noop`；缺省 `Done`。
    pub status: ResultStatus,
    /// `Pending` 时可读取后续状态的资源名。
    pub state_resource: Option<String>,
    /// 一句面向模型 / 用户的结论。
    pub summary: Option<String>,
    /// 结果内容的标注（MCP 内容注解），Hub 原样转发。
    pub annotations: Option<ContentAnnotations>,
}

/// 资源的附加选项（`register_resource_with`）。
///
/// @compat 不放进 [`ResourceSpec`]：给结构体加字段会破坏现有绑定与 App 的结构体字面量；以后的资源选项都加在这里。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResourceOptions {
    /// 需实时推送（spec/lifecycle.md 第 13 节 B3）：被 Host 订阅时阻止休眠，休眠期间变化时回连推送。
    /// 默认 `false`：订阅不阻止休眠，变化在下次连接时补发。
    pub realtime: bool,    /// 资源内容的标注（MCP 内容注解），Hub 放到 MCP `resources/list` 的资源注解上；`None` = 未声明。
    pub annotations: Option<ContentAnnotations>,
}

// ---------------------------------------------------------------------------
// 回调接口（由 App / 语言封装层实现）
// ---------------------------------------------------------------------------

/// 工具 handler。在分发线程上调用，必须尽快返回；结果通过 `call` 异步提交。
pub trait ToolHandler: Send + Sync + 'static {
    fn invoke(&self, call: CallHandle);
}

/// 资源读取。在分发线程上调用，结果通过 `read` 异步提交。
pub trait ResourceReader: Send + Sync + 'static {
    fn read(&self, read: ReadHandle);
}

/// 导航回调（Host 的 `app/navigate`，spec/protocol.md 3.4）。在分发线程上调用，必须尽快返回；
/// 切换界面后通过 `request` 提交结果（界面切换完成、新页面的工具注册之后再 `complete` 更好，Hub 会等待目标工具出现）。
pub trait NavigationHandler: Send + Sync + 'static {
    fn navigate(&self, request: NavigateHandle);
}

/// 调用被取消时通知（Host 取消、超时、断线、停止）。在分发线程上调用。
pub trait CancelListener: Send + Sync + 'static {
    fn on_cancel(&self, reason: CancelReason);
}

/// 客户端事件。在分发线程上调用。
pub trait ClientListener: Send + Sync + 'static {
    fn on_state_changed(&self, state: StateInfo);
    /// 配对成功并获得新 token，App 应持久化，下次放入 [`NativeConfig::token`]。
    fn on_paired(&self, token: String);
    fn on_log(&self, level: LogLevel, message: String) {
        let _ = (level, message);
    }
    /// 已进入休眠，且驻留策略（[`Residency`]）允许退出进程。App 自行决定是否退出。
    fn on_idle_exit(&self) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelReason {
    Requested,
    Timeout,
    Disconnected,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateStatus {
    Idle,
    Connecting,
    Handshaking,
    PendingPairing,
    Connected,
    Backoff,
    Rejected,
    Stopped,
    /// 休眠：无连接、无定时器、运行时已释放，等待唤醒。
    Dormant,
    /// 收到唤醒后正在回连。
    Waking,
    /// 对端不是期望的 Host（不是 app-mcp，或属于其他用户；spec/protocol.md 1.6）。不再自动重连，
    /// [`NativeClient::wake`] / [`NativeClient::connect_now`] 时再试一次；原因见 [`StateInfo::reason`]。
    HostMismatch,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StateInfo {
    pub status: StateStatus,
    /// `Backoff` 时距下一次重连的毫秒数。
    pub retry_in_ms: Option<u64>,
    /// `Rejected` / `HostMismatch` 时的原因；`Backoff` 时为本次连接失败 / 断开的原因（有的话）。
    pub reason: Option<String>,
    /// 与 `reason` 对应的错误码（spec/protocol.md 10.1，如 `HOST_NOT_RUNNING`、`HOST_NOT_APP_MCP`）。
    pub code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum NativeError {
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("invalid input schema: {0}")]
    InvalidSchema(String),
    #[error("a tool or resource named {0:?} is already registered")]
    DuplicateName(String),
    #[error("invalid json: {0}")]
    InvalidJson(String),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("call or read already completed or cancelled")]
    AlreadyCompleted,
    #[error("handle already disposed")]
    Disposed,
    #[error("client stopped")]
    Stopped,
    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// 句柄
// ---------------------------------------------------------------------------

/// 一次工具调用。可克隆、可跨线程传递；完成（`complete` / `fail`）只能一次。
#[derive(Clone, Debug)]
pub struct CallHandle {
    inner: Arc<CallInner>,
}

impl CallHandle {
    pub fn call_id(&self) -> String {
        self.inner.call_id.clone()
    }
    pub fn tool_name(&self) -> String {
        self.inner.tool_name.clone()
    }
    /// 已由 Host 按 inputSchema 校验过的参数（JSON 对象文本）。
    pub fn arguments_json(&self) -> String {
        self.inner.arguments_json.clone()
    }
    /// Agent 给出的幂等键（原样，spec/protocol.md 3.3）；没有时为 `None`。App 决定如何使用（如作为业务去重键）。
    pub fn idempotency_key(&self) -> Option<String> {
        self.inner.idempotency_key.clone()
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.lock().cancelled.is_some()
    }
    /// 设置取消监听。已取消时立即（在当前线程）回调一次。
    pub fn set_cancel_listener(&self, listener: Arc<dyn CancelListener>) {
        let mut st = self.inner.lock();
        match st.cancelled {
            Some(reason) => {
                drop(st);
                listener.on_cancel(reason);
            }
            None => st.listener = Some(listener),
        }
    }
    /// 成功完成。`data_json` 为 `None` 表示 `null`；非法 JSON 返回 [`NativeError::InvalidJson`]（调用仍未完成）。
    pub fn complete(
        &self,
        data_json: Option<&str>,
        state_hints: Vec<String>,
    ) -> Result<(), NativeError> {
        self.complete_with(CallResult {
            data_json: data_json.map(str::to_owned),
            state_hints,
            ..CallResult::default()
        })
    }
    /// 成功完成，附带业务状态、摘要与内容标注（spec/protocol.md 3.2）。非法 JSON 返回
    /// [`NativeError::InvalidJson`]（调用仍未完成）。
    pub fn complete_with(&self, result: CallResult) -> Result<(), NativeError> {
        let data = match result.data_json.as_deref() {
            None => Value::Null,
            Some(text) => {
                serde_json::from_str(text).map_err(|e| NativeError::InvalidJson(e.to_string()))?
            }
        };
        self.inner.finish(Ok(CallOutput {
            data,
            state_hints: result.state_hints,
            annotations: result.annotations,
            status: result.status,
            state_resource: result.state_resource,
            summary: result.summary,
        }))
    }
    /// 失败完成。
    pub fn fail(&self, kind: ErrorKind, message: &str) -> Result<(), NativeError> {
        self.inner.finish(Err(ToolError::new(kind, message)))
    }
    /// 失败完成，附带结构化详情（JSON 文本；对象的字段合并进错误的 `data`，其他值放在 `data.details`）。`details_json` 为 `None`
    /// 等同于 [`CallHandle::fail`]；非法 JSON 返回 [`NativeError::InvalidJson`]（调用仍未完成）。
    pub fn fail_with_details(
        &self,
        kind: ErrorKind,
        message: &str,
        details_json: Option<&str>,
    ) -> Result<(), NativeError> {
        self.inner.finish(Err(tool_error_with_details(kind, message, details_json)?))
    }
    /// 以 `USER_ACTION_REQUIRED` 失败完成（spec/protocol.md 第 4 节）：需要用户本人操作后才能继续
    /// （登录过期、系统权限未授予、需切到前台、需在 App 内确认等）。
    ///
    /// @input message 面向用户的说明（Agent 转告用户）。
    /// @input reason 可选类别，建议取 `user_action_reason` 中的值（`login` / `permission` / `foreground` / `confirm`）。
    /// @input uri 可选的 App 内入口（深链接等）。
    /// @output `None` 的字段不出现在错误的 `data` 中。
    pub fn fail_user_action(
        &self,
        message: &str,
        reason: Option<&str>,
        uri: Option<&str>,
    ) -> Result<(), NativeError> {
        self.inner
            .finish(Err(ToolError::user_action_required(message, reason, uri)))
    }
    /// 报告进度（spec/protocol.md 3.3）：Host 合并后转发给 Agent（MCP `notifications/progress`）。`progress` 应递增；
    /// `total` 未知时为 `None`。未连接时丢弃；调用已结束（完成、取消）时返回 [`NativeError::AlreadyCompleted`]。
    pub fn report_progress(
        &self,
        progress: f64,
        total: Option<f64>,
        message: Option<&str>,
    ) -> Result<(), NativeError> {
        if self.inner.lock().cancelled.is_some() {
            return Err(NativeError::AlreadyCompleted);
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        st.client
            .report_progress(&self.inner.call_id, progress, total, message.map(str::to_owned), now_ms())
            .map_err(core_error)?;
        drop(st);
        shared.wake();
        Ok(())
    }
    /// 延长持有：调用完成后仍阻止自动休眠（handler 发起的长任务），直到返回的句柄被释放。
    /// 调用已结束（完成、取消）时返回 [`NativeError::AlreadyCompleted`]。
    pub fn hold(&self) -> Result<HoldHandle, NativeError> {
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        let id = st
            .client
            .hold_for_call(&self.inner.call_id, now_ms())
            .map_err(core_error)?;
        drop(st);
        shared.wake();
        Ok(HoldHandle::new(shared.clone(), id))
    }
}

/// 阻止自动休眠的持有（[`NativeClient::hold`]、[`CallHandle::hold`]）。可克隆；`release` 幂等；
/// 最后一个克隆被丢弃时自动释放。
#[derive(Clone, Debug)]
pub struct HoldHandle {
    inner: Arc<HoldInner>,
}

impl HoldHandle {
    fn new(shared: Arc<Shared>, id: HoldId) -> Self {
        Self {
            inner: Arc::new(HoldInner {
                shared,
                id,
                released: AtomicBool::new(false),
            }),
        }
    }
    /// 释放持有。重复调用无效果。
    pub fn release(&self) {
        self.inner.release();
    }
}

/// 一次资源读取。完成只能一次。
#[derive(Clone, Debug)]
pub struct ReadHandle {
    inner: Arc<ReadInner>,
}

impl ReadHandle {
    pub fn resource_name(&self) -> String {
        self.inner.name.clone()
    }
    pub fn complete(&self, contents_json: &str) -> Result<(), NativeError> {
        let contents: Value = serde_json::from_str(contents_json)
            .map_err(|e| NativeError::InvalidJson(e.to_string()))?;
        self.inner.finish(Ok(contents))
    }
    pub fn fail(&self, kind: ErrorKind, message: &str) -> Result<(), NativeError> {
        self.inner.finish(Err(ToolError::new(kind, message)))
    }
    /// 失败完成，附带结构化详情；语义同 [`CallHandle::fail_with_details`]（非法 JSON 返回
    /// [`NativeError::InvalidJson`]，读取仍未完成）。
    pub fn fail_with_details(
        &self,
        kind: ErrorKind,
        message: &str,
        details_json: Option<&str>,
    ) -> Result<(), NativeError> {
        self.inner.finish(Err(tool_error_with_details(kind, message, details_json)?))
    }
    /// 以 `USER_ACTION_REQUIRED` 失败完成；语义同 [`CallHandle::fail_user_action`]（`None` 的字段不出现在错误的 `data` 中）。
    pub fn fail_user_action(
        &self,
        message: &str,
        reason: Option<&str>,
        uri: Option<&str>,
    ) -> Result<(), NativeError> {
        self.inner
            .finish(Err(ToolError::user_action_required(message, reason, uri)))
    }
}

/// 带可选详情（JSON 文本）的错误；`details_json` 非法时返回 [`NativeError::InvalidJson`]。
fn tool_error_with_details(
    kind: ErrorKind,
    message: &str,
    details_json: Option<&str>,
) -> Result<ToolError, NativeError> {
    let err = ToolError::new(kind, message);
    let Some(text) = details_json else {
        return Ok(err);
    };
    let details: Value =
        serde_json::from_str(text).map_err(|e| NativeError::InvalidJson(e.to_string()))?;
    Ok(err.with_details(details))
}

/// 一次导航请求（[`NavigationHandler::navigate`]）。可克隆、可跨线程传递；完成只能一次。
#[derive(Clone, Debug)]
pub struct NavigateHandle {
    inner: Arc<NavigateInner>,
}

impl NavigateHandle {
    /// 目标页面名。
    pub fn page(&self) -> String {
        self.inner.page.clone()
    }
    /// 页面参数（JSON 文本）；Host 没有给出时为 `None`。
    pub fn params_json(&self) -> Option<String> {
        self.inner.params_json.clone()
    }
    /// 导航完成。
    pub fn complete(&self) -> Result<(), NativeError> {
        self.inner.finish(Ok(()))
    }
    /// 导航失败（`NAVIGATION_FAILED`，`data.reason` = `error`）：页面不存在、参数不合法等。
    pub fn fail(&self, message: &str) -> Result<(), NativeError> {
        self.inner.finish(Err(ToolError::navigation_failed(message, navigation_reason::ERROR)))
    }
    /// 拒绝本次导航（`NAVIGATION_DENIED`）：如用户正在输入。`message` 面向模型 / 用户。
    pub fn deny(&self, message: &str) -> Result<(), NativeError> {
        self.inner.finish(Err(ToolError::navigation_denied(message)))
    }
    /// 需要用户本人操作（`USER_ACTION_REQUIRED`，spec/protocol.md 3.4）：如 App 在后台、已发通知请用户点开
    /// （`reason` 取 `foreground`，`uri` 为该页面的 App 内入口）。语义同 [`CallHandle::fail_user_action`]。
    pub fn fail_user_action(&self, message: &str, reason: Option<&str>, uri: Option<&str>) -> Result<(), NativeError> {
        self.inner.finish(Err(ToolError::user_action_required(message, reason, uri)))
    }
}

/// 已注册的工具。可克隆；`dispose` 幂等。丢弃句柄**不会**注销工具。
#[derive(Clone, Debug)]
pub struct ToolHandle {
    inner: Arc<ToolInner>,
}

impl ToolHandle {
    pub fn name(&self) -> String {
        self.inner.name.clone()
    }
    /// 用新定义整体替换（名称不可变，`spec.name` 被忽略）。已声明的 [`ToolOptions`] 保持不变。
    pub fn update(&self, spec: ToolSpec) -> Result<(), NativeError> {
        self.apply(spec_update(spec)?)
    }
    /// 用新定义与选项整体替换（选项中的 `None` 表示清除该声明）。
    pub fn update_with(&self, spec: ToolSpec, options: ToolOptions) -> Result<(), NativeError> {
        let output_schema = parse_output_schema(options.output_schema_json.as_deref())?;
        self.apply(ToolUpdate {
            annotations: Some(options.annotations),
            output_schema: Some(output_schema),
            surface: Some(options.surface),
            page: Some(options.page),
            background_tool: Some(options.background_tool),
            ..spec_update(spec)?
        })
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<(), NativeError> {
        self.apply(ToolUpdate {
            enabled: Some(enabled),
            ..ToolUpdate::default()
        })
    }
    pub fn dispose(&self) {
        if self.inner.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.tools.remove(&self.inner.id).is_some() {
            let _ = st.client.unregister_tool(self.inner.id);
        }
        drop(st);
        shared.wake();
    }

    fn apply(&self, update: ToolUpdate) -> Result<(), NativeError> {
        if self.inner.disposed.load(Ordering::SeqCst) {
            return Err(NativeError::Disposed);
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        if !st.tools.contains_key(&self.inner.id) {
            return Err(NativeError::Disposed);
        }
        st.client
            .update_tool(self.inner.id, update)
            .map_err(core_error)?;
        drop(st);
        shared.wake();
        Ok(())
    }
}

/// 已注册的资源。可克隆；`dispose` 幂等。
#[derive(Clone, Debug)]
pub struct ResourceHandle {
    inner: Arc<ResourceInner>,
}

impl ResourceHandle {
    pub fn name(&self) -> String {
        self.inner.name.clone()
    }
    pub fn notify_changed(&self) -> Result<(), NativeError> {
        if self.inner.disposed.load(Ordering::SeqCst) {
            return Err(NativeError::Disposed);
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        if !st.resources.contains_key(&self.inner.id) {
            return Err(NativeError::Disposed);
        }
        st.client
            .notify_resource_changed(self.inner.id, now_ms())
            .map_err(core_error)?;
        drop(st);
        shared.wake();
        Ok(())
    }
    pub fn dispose(&self) {
        if self.inner.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.resources.remove(&self.inner.id).is_some() {
            let _ = st.client.unregister_resource(self.inner.id);
        }
        drop(st);
        shared.wake();
    }
}

/// Scope：注销时递归注销其下所有工具、资源与子 scope。可克隆；`dispose` 幂等。
#[derive(Clone, Debug)]
pub struct ScopeHandle {
    inner: Arc<ScopeInner>,
}

impl ScopeHandle {
    pub fn register_tool(
        &self,
        spec: ToolSpec,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<ToolHandle, NativeError> {
        self.register_tool_with(spec, ToolOptions::default(), handler)
    }
    /// 同 [`ScopeHandle::register_tool`]，另带工具选项（MCP 注解、输出 schema）。
    pub fn register_tool_with(
        &self,
        spec: ToolSpec,
        options: ToolOptions,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<ToolHandle, NativeError> {
        self.check()?;
        self.inner
            .shared
            .register_tool(Some(self.inner.id), spec, options, handler)
    }
    pub fn register_resource(
        &self,
        spec: ResourceSpec,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<ResourceHandle, NativeError> {
        self.register_resource_with(spec, ResourceOptions::default(), reader)
    }
    /// 同 [`ScopeHandle::register_resource`]，另带资源选项（如 `realtime`）。
    pub fn register_resource_with(
        &self,
        spec: ResourceSpec,
        options: ResourceOptions,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<ResourceHandle, NativeError> {
        self.check()?;
        self.inner
            .shared
            .register_resource(Some(self.inner.id), spec, options, reader)
    }
    pub fn create_scope(&self, name: &str) -> Result<ScopeHandle, NativeError> {
        self.check()?;
        self.inner.shared.create_scope(Some(self.inner.id), name)
    }
    pub fn dispose(&self) {
        if self.inner.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        st.dispose_scope(self.inner.id);
        drop(st);
        shared.wake();
    }

    fn check(&self) -> Result<(), NativeError> {
        if self.inner.disposed.load(Ordering::SeqCst) {
            Err(NativeError::Disposed)
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// 客户端
// ---------------------------------------------------------------------------

/// 原生客户端。创建时启动后台运行时线程（不连接），`start` 后开始连接 Host。
/// 可克隆（共享同一个客户端）。最后一个克隆被丢弃时自动停止并结束后台线程。
#[derive(Clone)]
pub struct NativeClient {
    owner: Arc<ClientOwner>,
}

impl NativeClient {
    pub fn new(
        config: NativeConfig,
        listener: Option<Arc<dyn ClientListener>>,
    ) -> Result<Self, NativeError> {
        let connect_timeout = std::time::Duration::from_millis(u64::from(config.connect_timeout_ms.max(1)));
        let (core_config, host_url, name_request) = build_core_config(config)?;
        let instance_id = core_config.instance_id.clone();
        let shared = Arc::new(Shared {
            state: Mutex::new(CoreState {
                client: Client::new(core_config),
                stopped: false,
                shutdown: false,
                tools: HashMap::new(),
                resources: HashMap::new(),
                scopes: HashMap::new(),
                calls: HashMap::new(),
                navigation: None,
                name_request,
                channel: None,
            }),
            wake: tokio::sync::Notify::new(),
            park: Mutex::new(0),
            park_cv: Condvar::new(),
            runtime_active: AtomicBool::new(false),
            listener,
            instance_id,
        });
        let _ = epoch();

        let rt = runtime::build_runtime()
            .map_err(|e| NativeError::Internal(format!("无法创建运行时：{e}")))?;
        let (job_tx, job_rx) = std::sync::mpsc::channel::<Job>();
        let dispatcher = std::thread::Builder::new()
            .name("app-mcp-dispatch".to_owned())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    // 用户回调 panic 不应拖垮分发线程。
                    let _ = std::panic::catch_unwind(AssertUnwindSafe(job));
                }
            })
            .map_err(|e| NativeError::Internal(format!("无法创建分发线程：{e}")))?;
        let driver_shared = shared.clone();
        let runtime = std::thread::Builder::new()
            .name("app-mcp-runtime".to_owned())
            .spawn(move || {
                runtime::run(
                    rt,
                    driver_shared,
                    job_tx,
                    runtime::Target {
                        endpoint: host_url,
                        connect_timeout,
                    },
                )
            })
            .map_err(|e| NativeError::Internal(format!("无法创建运行时线程：{e}")))?;

        Ok(Self {
            owner: Arc::new(ClientOwner {
                shared,
                threads: Mutex::new(vec![runtime, dispatcher]),
            }),
        })
    }
    /// 设置导航回调（spec/protocol.md 3.4）：`Some` 时握手声明 `capabilities.navigate`，Host 的 `app/navigate` 交给回调；
    /// `None` 时导航请求以 `NAVIGATION_FAILED`（`unsupported`）回复。能力在握手时声明——连接后才设置的回调在下次连接
    /// （回连 / 唤醒）时生效，建议在 `start` 之前设置。
    pub fn set_navigation_handler(&self, handler: Option<Arc<dyn NavigationHandler>>) {
        let mut st = self.owner.shared.lock();
        st.client.set_navigation(handler.is_some());
        st.navigation = handler;
    }

    /// 实例不可见时导航请求是否仍交给导航回调（spec/protocol.md 3.4）。缺省按平台：桌面 `true`（App / 封装层能把窗口
    /// 带到前台）；Android、iOS、鸿蒙 `false`（直接以 `USER_ACTION_REQUIRED`（`foreground`）回复，不等超时）。
    /// App 要在后台自行处理（如发通知请用户点开，再以 [`NavigateHandle::fail_user_action`] 回复）时置为 `true`。随时生效。
    pub fn set_navigate_in_background(&self, enabled: bool) {
        self.owner.shared.lock().client.set_navigate_in_background(enabled);
    }

    pub fn instance_id(&self) -> String {
        self.owner.shared.instance_id.clone()
    }
    pub fn state(&self) -> StateInfo {
        let st = self.owner.shared.lock();
        state_info(st.client.state(), now_ms())
    }
    /// Host 为当前连接分配的连接 ID（spec/protocol.md 10.3）；未连接或旧 Host 时为 `None`。
    /// 连接期间库产生的日志（[`ClientListener::on_log`]）都以 `[连接 ID] ` 开头。
    pub fn connection_id(&self) -> Option<String> {
        self.owner.shared.lock().client.connection_id().map(str::to_owned)
    }
    /// 当前 token（配置带入的或配对后获得的）。
    pub fn token(&self) -> Option<String> {
        self.owner.shared.lock().client.token().map(str::to_owned)
    }
    /// 开始连接。重复调用无效果。
    pub fn start(&self) {
        let shared = &self.owner.shared;
        shared.lock().client.start(now_ms());
        shared.wake();
    }
    /// 停止：取消所有调用、断开连接、不再重连。之后注册类方法返回 [`NativeError::Stopped`]。
    pub fn stop(&self) {
        let shared = &self.owner.shared;
        shared.lock().stop(false);
        shared.wake();
    }
    pub fn set_visibility(&self, visibility: Visibility, focused: bool) {
        let shared = &self.owner.shared;
        shared
            .lock()
            .client
            .set_visibility(visibility, focused, now_ms());
        shared.wake();
    }
    /// 在根作用域注册工具。
    pub fn register_tool(
        &self,
        spec: ToolSpec,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<ToolHandle, NativeError> {
        self.register_tool_with(spec, ToolOptions::default(), handler)
    }
    /// 同 [`NativeClient::register_tool`]，另带工具选项（MCP 注解、输出 schema，spec/protocol.md 第 3 节）。
    pub fn register_tool_with(
        &self,
        spec: ToolSpec,
        options: ToolOptions,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<ToolHandle, NativeError> {
        self.owner.shared.register_tool(None, spec, options, handler)
    }
    pub fn register_resource(
        &self,
        spec: ResourceSpec,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<ResourceHandle, NativeError> {
        self.register_resource_with(spec, ResourceOptions::default(), reader)
    }
    /// 同 [`NativeClient::register_resource`]，另带资源选项（如 `realtime`，spec/lifecycle.md 第 13 节 B3）。
    pub fn register_resource_with(
        &self,
        spec: ResourceSpec,
        options: ResourceOptions,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<ResourceHandle, NativeError> {
        self.owner.shared.register_resource(None, spec, options, reader)
    }
    pub fn create_scope(&self, name: &str) -> Result<ScopeHandle, NativeError> {
        self.owner.shared.create_scope(None, name)
    }

    // ---- 生命周期 -------------------------------------------------------

    /// 处理操作系统激活参数 / URL（命令行、`onOpenURL`、D-Bus action 参数等），识别
    /// `app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=`、`#app-mcp-wake=<token>`。
    /// 不是本 SDK 的唤醒返回 `false`。可以在 `start` 之前调用（冷启动唤醒：`on-demand` 模式也会连接）。
    pub fn handle_wake(&self, args: &str) -> bool {
        let shared = &self.owner.shared;
        let recognized = shared.lock().client.handle_wake(args, now_ms());
        shared.wake();
        recognized
    }
    /// App 主动回连（如用户打开了相关界面）。返回是否因此发起了回连。
    pub fn wake(&self) -> bool {
        self.wake_with_reason(WakeReason::App)
    }
    /// 以指定原因回连（窗口重新可见时用 [`WakeReason::Visible`]）。
    pub fn wake_with_reason(&self, reason: WakeReason) -> bool {
        let shared = &self.owner.shared;
        let started = shared.lock().client.wake_with_reason(reason, now_ms());
        shared.wake();
        started
    }
    /// `on-demand` 模式下主动连接；尚未 `start` 时等同于 `start`。
    pub fn connect_now(&self) -> bool {
        let shared = &self.owner.shared;
        let started = shared.lock().client.connect_now(now_ms());
        shared.wake();
        started
    }
    /// App 主动请求休眠（原因 `app`，不受空闲条件与持有影响）。返回是否有效果。
    pub fn sleep(&self) -> bool {
        self.sleep_with_reason(SleepReason::App)
    }
    /// 以指定原因请求休眠（进入后台时用 [`SleepReason::Background`]）。
    pub fn sleep_with_reason(&self, reason: SleepReason) -> bool {
        let shared = &self.owner.shared;
        let changed = shared.lock().client.sleep_with_reason(reason, now_ms());
        shared.wake();
        changed
    }
    /// 临时阻止自动休眠，直到返回的句柄被释放（或最后一个克隆被丢弃）。
    pub fn hold(&self) -> HoldHandle {
        let shared = &self.owner.shared;
        let id = shared.lock().client.hold(now_ms());
        shared.wake();
        HoldHandle::new(shared.clone(), id)
    }
    /// 当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）。
    pub fn tools_hash(&self) -> String {
        self.owner.shared.lock().client.tools_hash()
    }
    /// 调试 / 测试用：tokio 运行时当前是否存在（休眠时为 `false`）。
    #[doc(hidden)]
    pub fn runtime_active(&self) -> bool {
        self.owner.shared.runtime_active.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for NativeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeClient").finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// 内部实现
// ---------------------------------------------------------------------------
//
// 结构：
// - `Shared`：核心 `Client` 与 handler 表放在一个 `Mutex<CoreState>` 里。注册类方法在调用线程上
//   加锁直接调用核心（从而同步返回错误），然后通过 `Notify` 唤醒运行时线程取出新事件。
// - 运行时线程（`runtime::drive`）：tokio current-thread runtime，负责 WebSocket 连接、计时与
//   取事件。取事件时持锁，只把事件翻译成「动作」；锁释放后再执行 I/O 或把用户回调投递到分发线程。
// - 分发线程：按顺序执行所有用户回调（handler、reader、listener、cancel listener），不持有任何锁。
// - `CallHandle::complete` 等可在任意线程调用：同样加锁直接调用核心的 `complete_call`，再唤醒运行时线程。
//
// 锁顺序：`CoreState` 锁 → `CallInner` 锁（运行时标记取消时）；任何地方都不会在持有 `CallInner`
// 锁时再去拿 `CoreState` 锁，也不会在持锁时调用用户回调。

mod names;
mod runtime;
/// 测试支持（只供本仓库测试，不属于公开 API 契约）。
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod test_support;
/// 测试支持：临时私有 D-Bus 会话总线（只供本仓库测试，不属于公开 API 契约）。
#[cfg(all(feature = "test-support", target_os = "linux"))]
#[doc(hidden)]
pub mod test_bus;

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use app_mcp_core::{
    CallOutput, Client, ClientConfig, ConnectionState, CoreError, Event, HoldId, Millis, NavigateId, ReadId,
    ResourceDef, ResourceId, ScopeId, ToolDef, ToolError, ToolId, ToolUpdate,
};
use serde_json::{Value, json};

/// 投递到分发线程的用户回调。
type Job = Box<dyn FnOnce() + Send + 'static>;

/// 进程级单调时钟起点。
fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// 进程启动（首次使用本库）以来的单调毫秒数。
fn now_ms() -> Millis {
    Millis::try_from(epoch().elapsed().as_millis()).unwrap_or(Millis::MAX)
}

/// 每个进程一个的自动实例 ID。
fn process_instance_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| format!("native-{:032x}", rand::random::<u128>()))
        .clone()
}

fn lock_ignore_poison<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 校验端点：格式合法，且本平台支持该传输。
fn parse_endpoint(text: &str) -> Result<app_mcp_protocol::Endpoint, NativeError> {
    use app_mcp_protocol::Endpoint;
    let endpoint =
        Endpoint::parse(text).map_err(|e| NativeError::InvalidConfig(format!("host_url 无效：{e}")))?;
    let supported = match &endpoint {
        Endpoint::WebSocket(_) => true,
        Endpoint::Unix(_) => cfg!(unix),
        Endpoint::Pipe(_) => cfg!(windows),
    };
    if !supported {
        return Err(NativeError::InvalidConfig(format!(
            "本平台不支持端点 {text:?}"
        )));
    }
    Ok(endpoint)
}

/// 校验配置并转换为核心配置；同时返回 Host 端点。
fn build_core_config(
    config: NativeConfig,
) -> Result<(ClientConfig, app_mcp_protocol::Endpoint, Option<names::NameRequest>), NativeError> {
    let endpoint = parse_endpoint(&config.host_url)?;
    if let Some(inst) = &config.name_instance
        && !app_mcp_protocol::naming::is_valid_instance(inst)
    {
        return Err(NativeError::InvalidConfig(format!(
            "name_instance 必须匹配 [a-z][a-z0-9-]{{0,31}} 且不是 default：{inst:?}"
        )));
    }
    let name_request = config.register_name.then(|| names::NameRequest {
        app_id: config.app_id.clone(),
        instance: config.name_instance.clone(),
        address: config.name_service_address.clone().filter(|a| !a.is_empty()),
    });
    if !app_mcp_protocol::is_valid_app_id(&config.app_id) {
        return Err(NativeError::InvalidConfig(format!(
            "app_id 必须匹配 [a-z][a-z0-9-]{{0,62}}：{:?}",
            config.app_id
        )));
    }
    if config.max_concurrent_calls == 0 {
        return Err(NativeError::InvalidConfig(
            "max_concurrent_calls 必须大于 0".to_owned(),
        ));
    }
    let instance_id = match config.instance_id {
        Some(id) if !id.is_empty() => id,
        Some(_) => {
            return Err(NativeError::InvalidConfig(
                "instance_id 不能为空".to_owned(),
            ));
        }
        None => process_instance_id(),
    };
    let launch_token = config
        .launch_token
        .or_else(|| std::env::var("APP_MCP_LAUNCH_TOKEN").ok())
        .filter(|t| !t.is_empty());

    let mut inner = ClientConfig::new(
        config.app_id,
        config.app_name,
        instance_id,
        config.client_kind,
    );
    inner.app_version = config.app_version;
    inner.instance_title = config.instance_title;
    inner.token = config.token;
    inner.launch_token = launch_token;
    inner.overview = config.overview;
    inner.lifecycle = config.lifecycle;
    inner.heartbeat.mode = config.heartbeat;
    inner.transport = endpoint.transport_kind(&app_mcp_protocol::platform::Target::CURRENT);
    inner.expected_host_user = app_mcp_protocol::identity::expected_host_user();
    inner.max_concurrent_calls = usize::try_from(config.max_concurrent_calls).unwrap_or(usize::MAX);
    inner.call_dedup = config.call_dedup;
    inner.navigate_in_background = app_mcp_protocol::platform::Target::CURRENT.allows_self_foreground();
    inner.launched_by_activation = name_request.is_some()
        && std::env::args().any(|a| a == app_mcp_protocol::naming::dbus::ACTIVATION_ARG);
    Ok((inner, endpoint, name_request))
}

/// 核心错误 → 原生错误。未知句柄一律视为已注销，未知调用 / 读取视为已完成。
fn core_error(e: CoreError) -> NativeError {
    match e {
        CoreError::InvalidName(n) => NativeError::InvalidName(n),
        CoreError::InvalidSchema => {
            NativeError::InvalidSchema(CoreError::InvalidSchema.to_string())
        }
        CoreError::DuplicateName(n) => NativeError::DuplicateName(n),
        CoreError::UnknownTool(_) | CoreError::UnknownResource(_) | CoreError::UnknownScope(_) => {
            NativeError::Disposed
        }
        CoreError::UnknownCall(_) | CoreError::UnknownRead(_) | CoreError::UnknownNavigate(_) => {
            NativeError::AlreadyCompleted
        }
    }
}

/// 解析 inputSchema 文本；`None` 表示无参数。`type: object` 由核心校验。
fn parse_output_schema(text: Option<&str>) -> Result<Option<Value>, NativeError> {
    text.map(|t| {
        serde_json::from_str(t).map_err(|e| NativeError::InvalidSchema(format!("outputSchema 不是合法 JSON：{e}")))
    })
    .transpose()
}

/// [`ToolSpec`] 对应的整体更新（不含 [`ToolOptions`] 的字段）。
fn spec_update(spec: ToolSpec) -> Result<ToolUpdate, NativeError> {
    let input_schema = parse_schema(spec.input_schema_json.as_deref())?;
    Ok(ToolUpdate {
        description: Some(spec.description),
        input_schema: Some(input_schema),
        risk: Some(spec.risk),
        activation: Some(spec.activation),
        title: Some(spec.title),
        enabled: Some(spec.enabled),
        ..ToolUpdate::default()
    })
}

fn parse_schema(text: Option<&str>) -> Result<Value, NativeError> {
    match text {
        None => Ok(json!({ "type": "object", "properties": {} })),
        Some(t) => serde_json::from_str(t)
            .map_err(|e| NativeError::InvalidSchema(format!("不是合法 JSON：{e}"))),
    }
}

fn state_info(state: &ConnectionState, now: Millis) -> StateInfo {
    let (status, retry_in_ms) = match state {
        ConnectionState::Idle => (StateStatus::Idle, None),
        ConnectionState::Connecting => (StateStatus::Connecting, None),
        ConnectionState::Handshaking => (StateStatus::Handshaking, None),
        ConnectionState::PendingPairing => (StateStatus::PendingPairing, None),
        ConnectionState::Connected => (StateStatus::Connected, None),
        ConnectionState::Backoff { retry_at, .. } => (StateStatus::Backoff, Some(retry_at.saturating_sub(now))),
        ConnectionState::Rejected { .. } => (StateStatus::Rejected, None),
        ConnectionState::Stopped => (StateStatus::Stopped, None),
        ConnectionState::Dormant => (StateStatus::Dormant, None),
        ConnectionState::Waking => (StateStatus::Waking, None),
        ConnectionState::HostMismatch { .. } => (StateStatus::HostMismatch, None),
    };
    StateInfo {
        status,
        retry_in_ms,
        reason: state.reason().map(str::to_owned),
        code: state.code().map(|c| c.as_str().to_owned()),
    }
}

/// 连接期间的日志前缀 `[连接 ID] `（spec/protocol.md 10.3）。
fn cid_prefix(client: &app_mcp_core::Client) -> String {
    client.connection_id().map(|c| format!("[{c}] ")).unwrap_or_default()
}

fn cancel_reason(r: app_mcp_core::CancelReason) -> CancelReason {
    match r {
        app_mcp_core::CancelReason::Requested => CancelReason::Requested,
        app_mcp_core::CancelReason::Timeout => CancelReason::Timeout,
        app_mcp_core::CancelReason::Disconnected => CancelReason::Disconnected,
        app_mcp_core::CancelReason::Stopped => CancelReason::Stopped,
    }
}

/// 所有 `NativeClient` 克隆共享；最后一个克隆被丢弃时停止并回收后台线程。
struct ClientOwner {
    shared: Arc<Shared>,
    /// 运行时线程与分发线程。
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl Drop for ClientOwner {
    fn drop(&mut self) {
        self.shared.lock().stop(true);
        self.shared.wake();
        let threads = std::mem::take(&mut *lock_ignore_poison(&self.threads));
        let me = std::thread::current().id();
        for t in threads {
            // 最后一个克隆可能在分发线程（用户回调）里被丢弃：不能 join 自己，直接分离。
            // 运行时线程退出后发送端被丢弃，分发线程执行完剩余回调后自行结束。
            if t.thread().id() != me {
                let _ = t.join();
            }
        }
    }
}

/// 驱动层与各句柄共享的状态。
struct Shared {
    state: Mutex<CoreState>,
    /// 唤醒运行时线程（有新输入，需要重新取事件）。
    wake: tokio::sync::Notify,
    /// 休眠时运行时线程在此条件变量上阻塞；计数在每次 `wake()` 时递增。
    park: Mutex<u64>,
    park_cv: Condvar,
    /// tokio 运行时当前是否存在。
    runtime_active: AtomicBool,
    listener: Option<Arc<dyn ClientListener>>,
    instance_id: String,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("instance_id", &self.instance_id)
            .finish_non_exhaustive()
    }
}

struct ToolEntry {
    handler: Arc<dyn ToolHandler>,
    scope: Option<ScopeId>,
}

struct ResourceEntry {
    reader: Arc<dyn ResourceReader>,
    scope: Option<ScopeId>,
}

struct CoreState {
    client: Client,
    /// `stop` 已调用（或客户端已被丢弃）：注册类方法返回 `Stopped`。
    stopped: bool,
    /// 客户端已被丢弃：运行时线程处理完剩余事件后退出。
    shutdown: bool,
    tools: HashMap<ToolId, ToolEntry>,
    resources: HashMap<ResourceId, ResourceEntry>,
    /// scope → 父 scope。
    scopes: HashMap<ScopeId, Option<ScopeId>>,
    /// 进行中的调用（用于取消通知）。
    calls: HashMap<String, Arc<CallInner>>,
    /// 导航回调（[`NativeClient::set_navigation_handler`]）；`None` = 不支持导航。
    navigation: Option<Arc<dyn NavigationHandler>>,
    /// 要在名字服务登记的名字（[`NativeConfig::register_name`]）；`None` = 不登记。
    name_request: Option<names::NameRequest>,
    /// Hub 拨入、等待运行时取走的通道（[`Client::accept_channel`] 已接受，下一个 `Connect` 动作在其上连接）。
    channel: Option<names::Channel>,
}

/// 运行时线程取出事件后要执行的动作。
enum Action {
    Connect,
    Send(String),
    Disconnect,
    Dispatch(Job),
}

/// 一轮取事件的结果。
struct Batch {
    actions: Vec<Action>,
    timeout: Option<Millis>,
    shutdown: bool,
    /// 核心处于休眠态（可以释放运行时）。
    dormant: bool,
}

impl CoreState {
    fn stop(&mut self, shutdown: bool) {
        self.stopped = true;
        self.shutdown |= shutdown;
        self.client.stop(now_ms());
        // 释放 handler，打破「handler 持有 NativeClient」形成的引用环。
        self.navigation = None;
        self.tools.clear();
        self.resources.clear();
        self.scopes.clear();
    }

    fn dispose_scope(&mut self, scope: ScopeId) {
        if !self.scopes.contains_key(&scope) {
            return;
        }
        // 收集 scope 及全部后代。
        let mut doomed: HashSet<ScopeId> = HashSet::from([scope]);
        loop {
            let before = doomed.len();
            for (id, parent) in &self.scopes {
                if parent.is_some_and(|p| doomed.contains(&p)) {
                    doomed.insert(*id);
                }
            }
            if doomed.len() == before {
                break;
            }
        }
        let _ = self.client.dispose_scope(scope);
        let in_doomed = |s: &Option<ScopeId>| s.is_some_and(|s| doomed.contains(&s));
        self.tools.retain(|_, e| !in_doomed(&e.scope));
        self.resources.retain(|_, e| !in_doomed(&e.scope));
        self.scopes.retain(|id, _| !doomed.contains(id));
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, CoreState> {
        lock_ignore_poison(&self.state)
    }

    fn wake(&self) {
        self.wake.notify_one();
        *lock_ignore_poison(&self.park) += 1;
        self.park_cv.notify_all();
    }

    /// 是否应在名字服务登记：配置了登记、已 `start` 且未停止。
    fn name_request(&self) -> Option<names::NameRequest> {
        let st = self.lock();
        let started = *st.client.state() != ConnectionState::Idle;
        st.name_request.clone().filter(|_| started && !st.stopped)
    }

    /// 取走待连接的拨入通道。
    fn take_channel(&self) -> Option<names::Channel> {
        self.lock().channel.take()
    }
}

impl Shared {

    fn register_tool(
        self: &Arc<Self>,
        scope: Option<ScopeId>,
        spec: ToolSpec,
        options: ToolOptions,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<ToolHandle, NativeError> {
        let mut st = self.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        let input_schema = parse_schema(spec.input_schema_json.as_deref())?;
        let output_schema = parse_output_schema(options.output_schema_json.as_deref())?;
        let name = spec.name.clone();
        let id = st
            .client
            .register_tool(ToolDef {
                name: spec.name,
                description: spec.description,
                input_schema,
                risk: spec.risk,
                activation: spec.activation,
                title: spec.title,
                enabled: spec.enabled,
                scope,
                annotations: options.annotations,
                output_schema,
                surface: options.surface,
                page: options.page,
                background_tool: options.background_tool,
            })
            .map_err(core_error)?;
        st.tools.insert(id, ToolEntry { handler, scope });
        drop(st);
        self.wake();
        Ok(ToolHandle {
            inner: Arc::new(ToolInner {
                shared: self.clone(),
                id,
                name,
                disposed: AtomicBool::new(false),
            }),
        })
    }

    fn register_resource(
        self: &Arc<Self>,
        scope: Option<ScopeId>,
        spec: ResourceSpec,
        options: ResourceOptions,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<ResourceHandle, NativeError> {
        let mut st = self.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        let name = spec.name.clone();
        let id = st
            .client
            .register_resource(ResourceDef {
                name: spec.name,
                description: spec.description,
                mime_type: spec.mime_type,
                scope,
                realtime: options.realtime,
                annotations: options.annotations,
            })
            .map_err(core_error)?;
        st.resources.insert(id, ResourceEntry { reader, scope });
        drop(st);
        self.wake();
        Ok(ResourceHandle {
            inner: Arc::new(ResourceInner {
                shared: self.clone(),
                id,
                name,
                disposed: AtomicBool::new(false),
            }),
        })
    }

    fn create_scope(
        self: &Arc<Self>,
        parent: Option<ScopeId>,
        name: &str,
    ) -> Result<ScopeHandle, NativeError> {
        let mut st = self.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        let id = st.client.create_scope(name, parent).map_err(core_error)?;
        st.scopes.insert(id, parent);
        Ok(ScopeHandle {
            inner: Arc::new(ScopeInner {
                shared: self.clone(),
                id,
                disposed: AtomicBool::new(false),
            }),
        })
    }

    /// 在分发线程上调用 listener。
    fn listener_job(&self, f: impl FnOnce(&dyn ClientListener) + Send + 'static) -> Option<Action> {
        let listener = self.listener.clone()?;
        Some(Action::Dispatch(Box::new(move || f(listener.as_ref()))))
    }

    /// 持锁取出全部事件并翻译成动作。不在这里调用任何用户回调。
    fn drain(self: &Arc<Self>) -> Batch {
        let mut st = self.lock();
        let mut actions = Vec::new();
        while let Some(event) = st.client.poll_event() {
            match event {
                Event::Connect => actions.push(Action::Connect),
                Event::Send(text) => actions.push(Action::Send(text)),
                Event::Disconnect => actions.push(Action::Disconnect),
                Event::InvokeTool {
                    call_id,
                    tool,
                    name,
                    arguments,
                    idempotency_key,
                } => {
                    let Some(handler) = st.tools.get(&tool).map(|e| e.handler.clone()) else {
                        let err =
                            ToolError::new(ErrorKind::ToolNotFound, format!("工具 {name} 已注销"));
                        let _ = st.client.complete_call(&call_id, Err(err), now_ms());
                        continue;
                    };
                    let call = Arc::new(CallInner {
                        shared: self.clone(),
                        call_id: call_id.clone(),
                        tool_name: name,
                        arguments_json: arguments.to_string(),
                        idempotency_key,
                        state: Mutex::new(CallState::default()),
                    });
                    st.calls.insert(call_id, call.clone());
                    let handle = CallHandle { inner: call };
                    actions.push(Action::Dispatch(Box::new(move || {
                        let fallback = handle.clone();
                        if std::panic::catch_unwind(AssertUnwindSafe(|| handler.invoke(handle)))
                            .is_err()
                        {
                            let _ =
                                fallback.fail(ErrorKind::HandlerError, "handler 执行时发生 panic");
                        }
                    })));
                }
                Event::CancelTool { call_id, reason } => {
                    let Some(call) = st.calls.remove(&call_id) else {
                        continue;
                    };
                    let reason = cancel_reason(reason);
                    if let Some(listener) = call.mark_cancelled(reason) {
                        actions.push(Action::Dispatch(Box::new(move || {
                            listener.on_cancel(reason)
                        })));
                    }
                }
                Event::ReadResource {
                    read,
                    resource,
                    name,
                } => {
                    let Some(reader) = st.resources.get(&resource).map(|e| e.reader.clone()) else {
                        let err = ToolError::new(
                            ErrorKind::ResourceNotFound,
                            format!("资源 {name} 已注销"),
                        );
                        let _ = st.client.complete_read(read, Err(err));
                        continue;
                    };
                    let handle = ReadHandle {
                        inner: Arc::new(ReadInner {
                            shared: self.clone(),
                            read,
                            name,
                            done: AtomicBool::new(false),
                        }),
                    };
                    actions.push(Action::Dispatch(Box::new(move || {
                        let fallback = handle.clone();
                        if std::panic::catch_unwind(AssertUnwindSafe(|| reader.read(handle)))
                            .is_err()
                        {
                            let _ = fallback.fail(ErrorKind::HandlerError, "资源读取时发生 panic");
                        }
                    })));
                }
                Event::Navigate { navigate, page, params } => {
                    let Some(handler) = st.navigation.clone() else {
                        // 回调在请求到达后被清除：按不支持回复。
                        let err = ToolError::navigation_failed(
                            format!("App 不支持由 Agent 导航（页面「{page}」）。"),
                            navigation_reason::UNSUPPORTED,
                        );
                        let _ = st.client.complete_navigate(navigate, Err(err));
                        continue;
                    };
                    let handle = NavigateHandle {
                        inner: Arc::new(NavigateInner {
                            shared: self.clone(),
                            navigate,
                            page,
                            params_json: (!params.is_null()).then(|| params.to_string()),
                            done: AtomicBool::new(false),
                        }),
                    };
                    actions.push(Action::Dispatch(Box::new(move || {
                        let fallback = handle.clone();
                        if std::panic::catch_unwind(AssertUnwindSafe(|| handler.navigate(handle))).is_err() {
                            let _ = fallback.fail("导航回调执行时发生 panic");
                        }
                    })));
                }
                Event::StateChanged(state) => {
                    if state == ConnectionState::Connected {
                        let message = match st.client.connection_id() {
                            Some(cid) => format!("[{cid}] 已连接 Host（连接 ID {cid}）"),
                            None => "已连接 Host（Host 未提供连接 ID）".to_owned(),
                        };
                        actions.extend(self.listener_job(move |l| l.on_log(LogLevel::Info, message)));
                    }
                    let info = state_info(&state, now_ms());
                    actions.extend(self.listener_job(move |l| l.on_state_changed(info)));
                }
                Event::Paired { token } => {
                    actions.extend(self.listener_job(move |l| l.on_paired(token)))
                }
                Event::Warning(message) => {
                    let message = format!("{}{message}", cid_prefix(&st.client));
                    actions.extend(self.listener_job(move |l| l.on_log(LogLevel::Warn, message)));
                }
                Event::IdleExit => actions.extend(self.listener_job(|l| l.on_idle_exit())),
            }
        }
        Batch {
            actions,
            timeout: st.client.poll_timeout(),
            shutdown: st.shutdown,
            dormant: *st.client.state() == ConnectionState::Dormant,
        }
    }
}

#[derive(Default)]
struct CallState {
    /// 已调用过 complete / fail。
    finished: bool,
    cancelled: Option<CancelReason>,
    listener: Option<Arc<dyn CancelListener>>,
}

struct CallInner {
    shared: Arc<Shared>,
    call_id: String,
    tool_name: String,
    arguments_json: String,
    idempotency_key: Option<String>,
    state: Mutex<CallState>,
}

impl std::fmt::Debug for CallInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallInner")
            .field("call_id", &self.call_id)
            .field("tool_name", &self.tool_name)
            .finish_non_exhaustive()
    }
}

impl CallInner {
    fn lock(&self) -> MutexGuard<'_, CallState> {
        lock_ignore_poison(&self.state)
    }

    /// 标记已取消，返回需要通知的监听器。
    fn mark_cancelled(&self, reason: CancelReason) -> Option<Arc<dyn CancelListener>> {
        let mut st = self.lock();
        if st.cancelled.is_some() {
            return None;
        }
        st.cancelled = Some(reason);
        st.listener.take()
    }

    fn finish(&self, outcome: Result<CallOutput, ToolError>) -> Result<(), NativeError> {
        {
            let mut st = self.lock();
            if st.finished || st.cancelled.is_some() {
                return Err(NativeError::AlreadyCompleted);
            }
            st.finished = true;
        }
        let mut core = self.shared.lock();
        core.calls.remove(&self.call_id);
        let result = core.client.complete_call(&self.call_id, outcome, now_ms());
        drop(core);
        self.shared.wake();
        result.map_err(core_error)
    }
}

struct ReadInner {
    shared: Arc<Shared>,
    read: ReadId,
    name: String,
    done: AtomicBool,
}

impl std::fmt::Debug for ReadInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadInner")
            .field("read", &self.read)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl ReadInner {
    fn finish(&self, outcome: Result<Value, ToolError>) -> Result<(), NativeError> {
        if self.done.swap(true, Ordering::SeqCst) {
            return Err(NativeError::AlreadyCompleted);
        }
        let result = self.shared.lock().client.complete_read(self.read, outcome);
        self.shared.wake();
        result.map_err(core_error)
    }
}

struct NavigateInner {
    shared: Arc<Shared>,
    navigate: NavigateId,
    page: String,
    params_json: Option<String>,
    done: AtomicBool,
}

impl std::fmt::Debug for NavigateInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NavigateInner")
            .field("navigate", &self.navigate)
            .field("page", &self.page)
            .finish_non_exhaustive()
    }
}

impl NavigateInner {
    fn finish(&self, outcome: Result<(), ToolError>) -> Result<(), NativeError> {
        if self.done.swap(true, Ordering::SeqCst) {
            return Err(NativeError::AlreadyCompleted);
        }
        let result = self.shared.lock().client.complete_navigate(self.navigate, outcome);
        self.shared.wake();
        result.map_err(core_error)
    }
}

#[derive(Debug)]
struct ToolInner {
    shared: Arc<Shared>,
    id: ToolId,
    name: String,
    disposed: AtomicBool,
}

#[derive(Debug)]
struct ResourceInner {
    shared: Arc<Shared>,
    id: ResourceId,
    name: String,
    disposed: AtomicBool,
}

#[derive(Debug)]
struct HoldInner {
    shared: Arc<Shared>,
    id: HoldId,
    released: AtomicBool,
}

impl HoldInner {
    fn release(&self) {
        if self.released.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shared.lock().client.release_hold(self.id, now_ms());
        self.shared.wake();
    }
}

impl Drop for HoldInner {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Debug)]
struct ScopeInner {
    shared: Arc<Shared>,
    id: ScopeId,
    disposed: AtomicBool,
}

#[cfg(test)]
mod tests;
