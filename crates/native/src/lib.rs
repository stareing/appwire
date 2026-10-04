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
    Activation, AppOverview, Audience, BusyPolicy, CachePolicy, CacheScope, CallDedupPolicy, ClientKind, ContentAnnotations, EventInfo, HeartbeatMode, LifecycleMode, LifecyclePolicy,
    Residency, ResultStatus, Risk, SleepReason, ToolAnnotations, ToolSurface, TransportKind, Visibility, WakeDescriptor,
    WakeKind, WakeReason, MAX_CACHE_TTL_MS, MAX_EVENT_PAYLOAD_BYTES, parse_wake_token,
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
    /// 排队中的调用上限（spec/protocol.md 5.3），默认 64；0 表示不限。超出时新调用以 `RATE_LIMITED` 拒绝。
    pub max_queued_calls: u32,
    /// 用户正在操作（[`NativeClient::set_busy`]）期间写调用的处理方式，默认 [`BusyPolicy::Reject`]（spec/protocol.md 5.3）。
    pub busy_policy: BusyPolicy,
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
    /// `app-mcp-host app install` 生成）；Windows：命名管道 `\\.\pipe\appmcp-<用户 SID>-<appId>`（需要 App 登记文件
    /// `%LOCALAPPDATA%\app-mcp\apps\<appId>.json`，同一命令生成）。默认 `false`。
    ///
    /// 登记在 [`NativeClient::start`] 之后进行、[`NativeClient::stop`] / 客户端被丢弃时注销；登记期间运行时线程保持
    /// 与名字服务的连接（阻塞等待，无定时器）。本平台不支持或登记失败时经 [`ClientListener::on_log`] 报告，其余照常。
    /// 通常与 `lifecycle.mode = OnDemand` 同用：App 不主动连接，只在 Hub 拨入时连接。
    pub register_name: bool,
    /// 登记实例名（spec/naming.md 2.1，`[a-z][a-z0-9-]{0,31}`，不能是 `default`）：`Some` 时另登记实例名字
    /// （D-Bus `dev.appmcp.App.<appId>.<instance>`、Windows 管道 `…-<appId>.<instance>`），供 `appmcp://<appId>/<instance>`
    /// 寻址。默认 `None`。
    pub name_instance: Option<String>,
    /// 名字服务地址（Linux：D-Bus 地址，如 `unix:path=/run/user/1000/bus`；Windows 不使用）。`None`（默认）按环境：由名字服务激活时用
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
            max_queued_calls: app_mcp_core::DEFAULT_MAX_QUEUED_CALLS as u32,
            busy_policy: BusyPolicy::Reject,
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
    /// 实现的标准意图（spec/intents.md），每项 `"<动词>@<主版本>"`，最多 4 项、不重复；格式不合法时注册 / 更新返回
    /// [`NativeError::InvalidName`]。空（缺省）= 未声明。
    pub implements: Vec<String>,
    /// 结果缓存声明（spec/protocol.md 3.6）：只对生效注解只读的工具生效（否则照常注册并记警告）；`ttlMs` 越界时注册 / 更新返回
    /// [`NativeError::InvalidConfig`]。`None`（缺省）= 未声明。
    pub cache: Option<CachePolicy>,
    /// 本工具同时执行的调用上限（spec/protocol.md 5.3）：0（缺省）= 不单独限制，只受 `max_concurrent_calls` 约束。只在 SDK 内生效。
    pub concurrency: u32,
    /// 互斥组（spec/protocol.md 5.3，`[a-zA-Z0-9_.-]{1,64}`）：同组的工具同一时刻至多一个在执行。`None` = 不互斥。只在 SDK 内生效。
    pub exclusive: Option<String>,
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
    pub realtime: bool,
    /// 资源内容的标注（MCP 内容注解），Hub 放到 MCP `resources/list` 的资源注解上；`None` = 未声明。
    pub annotations: Option<ContentAnnotations>,
    /// 读取结果缓存声明（spec/protocol.md 3.6）；`ttlMs` 越界时注册返回 [`NativeError::InvalidConfig`]。`None`（缺省）= 未声明。
    pub cache: Option<CachePolicy>,
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

/// App 拒绝一条由 Hub 拨入的通道的原因（[`NativeClient::accept_channel`]、D-Bus `Open()`，spec/naming.md 4.1、4.2、9.1）。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChannelRefusal {
    /// 已有连接或通道（含建立中；同时只接受一条，spec/naming.md U-16），Hub 记 `CHANNEL_LIMIT`。
    #[error("App 已有连接，同时只接受一条通道")]
    Busy,
    /// 客户端尚未 `start`、已停止或已被丢弃。
    #[error("App 的 SDK 尚未启动或已停止")]
    Stopped,
    /// 交来的 fd 不是可用的流式套接字。
    #[error("通道不可用：{0}")]
    Invalid(String),
}

// ---------------------------------------------------------------------------
// 句柄
// ---------------------------------------------------------------------------

/// 一次工具调用。可克隆、可跨线程传递；完成（`complete` / `fail`）只能一次。
#[derive(Clone, Debug)]
pub struct CallHandle {
    inner: Arc<CallInner>,
}

/// 阻止自动休眠的持有（[`NativeClient::hold`]、[`CallHandle::hold`]）。可克隆；`release` 幂等；
/// 最后一个克隆被丢弃时自动释放。
#[derive(Clone, Debug)]
pub struct HoldHandle {
    inner: Arc<HoldInner>,
}

/// 一次资源读取。完成只能一次。
#[derive(Clone, Debug)]
pub struct ReadHandle {
    inner: Arc<ReadInner>,
}

/// 一次导航请求（[`NavigationHandler::navigate`]）。可克隆、可跨线程传递；完成只能一次。
#[derive(Clone, Debug)]
pub struct NavigateHandle {
    inner: Arc<NavigateInner>,
}

/// 已注册的工具。可克隆；`dispose` 幂等。丢弃句柄**不会**注销工具。
#[derive(Clone, Debug)]
pub struct ToolHandle {
    inner: Arc<ToolInner>,
}

/// 已注册的资源。可克隆；`dispose` 幂等。
#[derive(Clone, Debug)]
pub struct ResourceHandle {
    inner: Arc<ResourceInner>,
}

/// Scope：注销时递归注销其下所有工具、资源与子 scope。可克隆；`dispose` 幂等。
#[derive(Clone, Debug)]
pub struct ScopeHandle {
    inner: Arc<ScopeInner>,
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
mod client;
mod convert;
mod handle_inner;
mod handles;
mod shared;

use convert::{
    build_core_config, cancel_reason, cid_prefix, core_error, parse_output_schema, parse_schema,
    spec_update, state_info,
};

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

/// 所有 `NativeClient` 克隆共享；最后一个克隆被丢弃时停止并回收后台线程。
struct ClientOwner {
    shared: Arc<Shared>,
    /// 运行时线程与分发线程。
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
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

struct ReadInner {
    shared: Arc<Shared>,
    read: ReadId,
    name: String,
    done: AtomicBool,
}

struct NavigateInner {
    shared: Arc<Shared>,
    navigate: NavigateId,
    page: String,
    params_json: Option<String>,
    done: AtomicBool,
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

#[derive(Debug)]
struct ScopeInner {
    shared: Arc<Shared>,
    id: ScopeId,
    disposed: AtomicBool,
}

#[cfg(test)]
mod tests;
