//! uniffi 绑定（proc-macro 方式）：把 `app-mcp-native` 暴露给 Kotlin、Swift、Python。
//!
//! # 设计
//!
//! - 对象（`uniffi::Object`）：[`AppMcpClient`]、[`Scope`]、[`Tool`]、[`Resource`]、[`Call`]、[`Read`]、
//!   [`Hold`]，分别包装原生运行时的 `NativeClient` 与各类句柄。
//! - 记录（`uniffi::Record`）：[`ClientConfig`]、[`AppOverview`]、[`ToolSpec`]、[`ResourceSpec`]、[`StateInfo`]、
//!   [`LifecyclePolicy`]、[`WakeDescriptor`]。
//! - 枚举（`uniffi::Enum`）：[`Risk`]、[`Activation`]、[`Visibility`]、[`ClientKind`]、[`CancelReason`]、
//!   [`StateStatus`]、[`LogLevel`]、[`LifecycleMode`]、[`Residency`]、[`WakeKind`]、[`WakeReason`]、[`SleepReason`]。
//! - 函数：[`error_kinds`]、[`parse_wake_token`]。
//! - 错误（`uniffi::Error`）：[`AppMcpError`]。
//! - 由外部语言实现的回调接口（`#[uniffi::export(foreign)]`，即旧写法 `with_foreign` 的规范形式）：
//!   [`ToolHandler`]、[`ResourceReader`]、[`CancelListener`]、[`ClientListener`]。
//!
//! # 线程模型
//!
//! 与原生运行时一致，**回调是同步的**：`ToolHandler::invoke` 等在原生运行时的分发线程上调用，
//! 必须尽快返回。语言封装层在回调里把工作切换到合适的线程（UI 线程、线程池、协程），
//! 之后从任意线程调用 [`Call::complete`] / [`Call::fail`] 完成调用。
//!
//! 外部回调抛出的未预期异常在 uniffi 里会变成 panic；这里的适配器用 `catch_unwind` 兜底，
//! 工具调用转为 `HANDLER_ERROR`，其余回调忽略，保证不会让分发线程崩溃。
//!
//! # 错误类别
//!
//! 在 FFI 上用协议字符串表示（如 `"HANDLER_ERROR"`、`"INVALID_INPUT"`），见 [`error_kinds`]。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use app_mcp_native as native;
use native::ErrorKind;

uniffi::setup_scaffolding!();

// ---------------------------------------------------------------------------
// 枚举
// ---------------------------------------------------------------------------

/// 风险等级，决定 Host 的确认策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Risk {
    Read,
    Write,
    Destructive,
    Payment,
    OsSensitive,
}

/// 调用时 App 需要的激活方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Activation {
    Headless,
    Background,
    Foreground,
}

/// 实例可见性。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Visibility {
    Visible,
    Hidden,
    Frozen,
}

/// 客户端类型。原生 App 用 `Native`（默认）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ClientKind {
    Web,
    Native,
    Hybrid,
}

/// 调用被取消的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum CancelReason {
    /// Host 请求取消。
    Requested,
    /// 调用超时。
    Timeout,
    /// 与 Host 断开连接。
    Disconnected,
    /// 客户端已停止。
    Stopped,
}

/// 日志级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// 连接状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum StateStatus {
    Idle,
    Connecting,
    Handshaking,
    PendingPairing,
    Connected,
    Backoff,
    Rejected,
    Stopped,
    Dormant,
    Waking,
}

/// 生命周期模式（spec/lifecycle.md 第 3 节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LifecycleMode {
    /// 不休眠（兼容现有行为）。
    Persistent,
    /// 启动即连接；空闲后休眠；唤醒后回连。
    Idle,
    /// 启动时不连接；被唤醒或 `connect_now` 时连接，任务完成后经过 `grace_ms` 休眠。
    OnDemand,
}

/// 休眠后的进程驻留策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Residency {
    /// 只断开连接，进程照常运行。
    Keep,
    /// 仅当本进程由唤醒冷启动时，休眠后回调 `on_idle_exit`。
    ExitWhenIdle,
    /// 每次休眠后都回调 `on_idle_exit`（无界面的辅助进程）。
    ExitAlways,
}

/// 唤醒方式（spec/lifecycle.md 第 5 节）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum WakeKind {
    /// 自定义 URL scheme：`<scheme>://app-mcp/wake?token=`。
    Uri,
    /// Windows 打包应用 AUMID。
    Aumid,
    /// macOS Apple Event。
    AppleEvent,
    /// Linux D-Bus `org.freedesktop.Application.ActivateAction`。
    Dbus,
    /// Android 显式广播。
    AndroidIntent,
    /// 网页 URL。
    WebUrl,
    /// 不可唤醒（Host 回退到清单 `launch`）。
    None,
}

/// 回连原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum WakeReason {
    /// 由 Host 通过操作系统激活机制唤醒。
    OsActivation,
    /// App 主动回连。
    App,
    /// 窗口 / 页面重新可见。
    Visible,
    /// 进程启动后的首次连接。
    ColdStart,
}

/// 休眠原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum SleepReason {
    Idle,
    Grace,
    /// 进入后台。
    Background,
    /// App 主动请求。
    App,
}

impl From<LifecycleMode> for native::LifecycleMode {
    fn from(v: LifecycleMode) -> Self {
        match v {
            LifecycleMode::Persistent => native::LifecycleMode::Persistent,
            LifecycleMode::Idle => native::LifecycleMode::Idle,
            LifecycleMode::OnDemand => native::LifecycleMode::OnDemand,
        }
    }
}

impl From<Residency> for native::Residency {
    fn from(v: Residency) -> Self {
        match v {
            Residency::Keep => native::Residency::Keep,
            Residency::ExitWhenIdle => native::Residency::ExitWhenIdle,
            Residency::ExitAlways => native::Residency::ExitAlways,
        }
    }
}

impl From<WakeKind> for native::WakeKind {
    fn from(v: WakeKind) -> Self {
        match v {
            WakeKind::Uri => native::WakeKind::Uri,
            WakeKind::Aumid => native::WakeKind::Aumid,
            WakeKind::AppleEvent => native::WakeKind::AppleEvent,
            WakeKind::Dbus => native::WakeKind::Dbus,
            WakeKind::AndroidIntent => native::WakeKind::AndroidIntent,
            WakeKind::WebUrl => native::WakeKind::WebUrl,
            WakeKind::None => native::WakeKind::None,
        }
    }
}

impl From<WakeReason> for native::WakeReason {
    fn from(v: WakeReason) -> Self {
        match v {
            WakeReason::OsActivation => native::WakeReason::OsActivation,
            WakeReason::App => native::WakeReason::App,
            WakeReason::Visible => native::WakeReason::Visible,
            WakeReason::ColdStart => native::WakeReason::ColdStart,
        }
    }
}

impl From<SleepReason> for native::SleepReason {
    fn from(v: SleepReason) -> Self {
        match v {
            SleepReason::Idle => native::SleepReason::Idle,
            SleepReason::Grace => native::SleepReason::Grace,
            SleepReason::Background => native::SleepReason::Background,
            SleepReason::App => native::SleepReason::App,
        }
    }
}

impl From<Risk> for native::Risk {
    fn from(v: Risk) -> Self {
        match v {
            Risk::Read => native::Risk::Read,
            Risk::Write => native::Risk::Write,
            Risk::Destructive => native::Risk::Destructive,
            Risk::Payment => native::Risk::Payment,
            Risk::OsSensitive => native::Risk::OsSensitive,
        }
    }
}

impl From<Activation> for native::Activation {
    fn from(v: Activation) -> Self {
        match v {
            Activation::Headless => native::Activation::Headless,
            Activation::Background => native::Activation::Background,
            Activation::Foreground => native::Activation::Foreground,
        }
    }
}

impl From<Visibility> for native::Visibility {
    fn from(v: Visibility) -> Self {
        match v {
            Visibility::Visible => native::Visibility::Visible,
            Visibility::Hidden => native::Visibility::Hidden,
            Visibility::Frozen => native::Visibility::Frozen,
        }
    }
}

impl From<ClientKind> for native::ClientKind {
    fn from(v: ClientKind) -> Self {
        match v {
            ClientKind::Web => native::ClientKind::Web,
            ClientKind::Native => native::ClientKind::Native,
            ClientKind::Hybrid => native::ClientKind::Hybrid,
        }
    }
}

impl From<native::CancelReason> for CancelReason {
    fn from(v: native::CancelReason) -> Self {
        match v {
            native::CancelReason::Requested => CancelReason::Requested,
            native::CancelReason::Timeout => CancelReason::Timeout,
            native::CancelReason::Disconnected => CancelReason::Disconnected,
            native::CancelReason::Stopped => CancelReason::Stopped,
        }
    }
}

impl From<native::LogLevel> for LogLevel {
    fn from(v: native::LogLevel) -> Self {
        match v {
            native::LogLevel::Debug => LogLevel::Debug,
            native::LogLevel::Info => LogLevel::Info,
            native::LogLevel::Warn => LogLevel::Warn,
            native::LogLevel::Error => LogLevel::Error,
        }
    }
}

impl From<native::StateStatus> for StateStatus {
    fn from(v: native::StateStatus) -> Self {
        match v {
            native::StateStatus::Idle => StateStatus::Idle,
            native::StateStatus::Connecting => StateStatus::Connecting,
            native::StateStatus::Handshaking => StateStatus::Handshaking,
            native::StateStatus::PendingPairing => StateStatus::PendingPairing,
            native::StateStatus::Connected => StateStatus::Connected,
            native::StateStatus::Backoff => StateStatus::Backoff,
            native::StateStatus::Rejected => StateStatus::Rejected,
            native::StateStatus::Stopped => StateStatus::Stopped,
            native::StateStatus::Dormant => StateStatus::Dormant,
            native::StateStatus::Waking => StateStatus::Waking,
        }
    }
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// 绑定层的统一错误。
#[derive(Clone, Debug, PartialEq, thiserror::Error, uniffi::Error)]
pub enum AppMcpError {
    #[error("名称不合法：{detail}")]
    InvalidName { detail: String },
    #[error("inputSchema 不合法：{detail}")]
    InvalidSchema { detail: String },
    #[error("名为 {name:?} 的工具或资源已注册")]
    DuplicateName { name: String },
    #[error("JSON 不合法：{detail}")]
    InvalidJson { detail: String },
    #[error("配置不合法：{detail}")]
    InvalidConfig { detail: String },
    #[error("未知的错误类别：{kind:?}")]
    UnknownErrorKind { kind: String },
    #[error("调用或读取已完成或已取消")]
    AlreadyCompleted,
    #[error("句柄已释放")]
    Disposed,
    #[error("客户端已停止")]
    Stopped,
    #[error("内部错误：{detail}")]
    Internal { detail: String },
}

impl From<native::NativeError> for AppMcpError {
    fn from(e: native::NativeError) -> Self {
        use native::NativeError as N;
        match e {
            N::InvalidName(detail) => AppMcpError::InvalidName { detail },
            N::InvalidSchema(detail) => AppMcpError::InvalidSchema { detail },
            N::DuplicateName(name) => AppMcpError::DuplicateName { name },
            N::InvalidJson(detail) => AppMcpError::InvalidJson { detail },
            N::InvalidConfig(detail) => AppMcpError::InvalidConfig { detail },
            N::AlreadyCompleted => AppMcpError::AlreadyCompleted,
            N::Disposed => AppMcpError::Disposed,
            N::Stopped => AppMcpError::Stopped,
            N::Internal(detail) => AppMcpError::Internal { detail },
        }
    }
}

/// 所有协议错误类别（FFI 上的字符串形式）。
const ALL_ERROR_KINDS: [ErrorKind; 15] = [
    ErrorKind::ToolNotFound,
    ErrorKind::ToolDisabled,
    ErrorKind::InvalidInput,
    ErrorKind::UserRejected,
    ErrorKind::Timeout,
    ErrorKind::HandlerError,
    ErrorKind::Cancelled,
    ErrorKind::AppDisconnected,
    ErrorKind::AppNotInstalled,
    ErrorKind::LaunchFailed,
    ErrorKind::AppNotResponding,
    ErrorKind::InstanceFrozen,
    ErrorKind::ResourceNotFound,
    ErrorKind::Unauthorized,
    ErrorKind::UnsupportedProtocol,
];

fn parse_error_kind(kind: &str) -> Result<ErrorKind, AppMcpError> {
    ALL_ERROR_KINDS
        .iter()
        .copied()
        .find(|k| k.as_str() == kind)
        .ok_or_else(|| AppMcpError::UnknownErrorKind {
            kind: kind.to_owned(),
        })
}

/// 返回所有合法的错误类别字符串（如 `"HANDLER_ERROR"`）。
#[uniffi::export]
pub fn error_kinds() -> Vec<String> {
    ALL_ERROR_KINDS
        .iter()
        .map(|k| k.as_str().to_owned())
        .collect()
}

// ---------------------------------------------------------------------------
// 记录
// ---------------------------------------------------------------------------

/// 客户端配置。可选字段为空时使用原生运行时的默认值。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ClientConfig {
    /// App 标识，`[a-z][a-z0-9-]{0,62}`。
    pub app_id: String,
    pub app_name: String,
    /// 为空时自动生成（每个进程一个）。
    #[uniffi(default = None)]
    pub instance_id: Option<String>,
    /// 为空时为 `Native`。
    #[uniffi(default = None)]
    pub client_kind: Option<ClientKind>,
    /// 为空时为 `ws://127.0.0.1:7717`。
    #[uniffi(default = None)]
    pub host_url: Option<String>,
    #[uniffi(default = None)]
    pub app_version: Option<String>,
    #[uniffi(default = None)]
    pub instance_title: Option<String>,
    /// 之前配对得到的 token（见 `ClientListener.on_paired`）。
    #[uniffi(default = None)]
    pub token: Option<String>,
    /// 为空时读取环境变量 `APP_MCP_LAUNCH_TOKEN`。
    #[uniffi(default = None)]
    pub launch_token: Option<String>,
    /// 同时执行的调用上限。
    #[uniffi(default = 1)]
    pub max_concurrent_calls: u32,
    /// App 总览：握手时发给 Host，模型在会话中首次接触本 App 时由 Host 附带。
    #[uniffi(default = None)]
    pub overview: Option<AppOverview>,
    /// 生命周期策略（spec/lifecycle.md）。为空时为 `persistent`（不休眠）。
    #[uniffi(default = None)]
    pub lifecycle: Option<LifecyclePolicy>,
    /// 建立连接的超时（毫秒）。为空时为 5000。
    #[uniffi(default = None)]
    pub connect_timeout_ms: Option<u32>,
}

/// 本实例的唤醒描述，随 `app/sleep` 上报。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct WakeDescriptor {
    pub kind: WakeKind,
    /// 各 kind 的定位信息（scheme、组件名、D-Bus 名等）。
    #[uniffi(default = None)]
    pub target: Option<String>,
    /// 能否不把窗口带到前台就唤醒。
    #[uniffi(default = false)]
    pub background: bool,
}

impl From<WakeDescriptor> for native::WakeDescriptor {
    fn from(w: WakeDescriptor) -> Self {
        native::WakeDescriptor {
            kind: w.kind.into(),
            target: w.target,
            background: w.background,
        }
    }
}

/// 生命周期策略（spec/lifecycle.md 第 3 节）。为空的字段取原生默认值。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct LifecyclePolicy {
    /// 为空时为 `Persistent`。
    #[uniffi(default = None)]
    pub mode: Option<LifecycleMode>,
    /// 空闲多久进入休眠（`Idle` 模式）。
    #[uniffi(default = 60000)]
    pub idle_timeout_ms: u64,
    /// 可见性为 hidden / frozen 时的空闲时间。
    #[uniffi(default = 15000)]
    pub hidden_idle_timeout_ms: u64,
    /// `OnDemand` 模式下任务完成后保留连接的时间。
    #[uniffi(default = 10000)]
    pub grace_ms: u64,
    /// 为空时为 `Keep`。
    #[uniffi(default = None)]
    pub residency: Option<Residency>,
    /// 为空时不上报（Host 回退到清单 `launch`）。
    #[uniffi(default = None)]
    pub wake: Option<WakeDescriptor>,
}

impl From<LifecyclePolicy> for native::LifecyclePolicy {
    fn from(p: LifecyclePolicy) -> Self {
        let d = native::LifecyclePolicy::default();
        native::LifecyclePolicy {
            mode: p.mode.map_or(d.mode, Into::into),
            idle_timeout_ms: p.idle_timeout_ms,
            hidden_idle_timeout_ms: p.hidden_idle_timeout_ms,
            grace_ms: p.grace_ms,
            residency: p.residency.map_or(d.residency, Into::into),
            wake: p.wake.map(Into::into),
        }
    }
}

/// 从操作系统激活参数 / URL 中提取唤醒令牌（`app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=`、
/// `#app-mcp-wake=<token>`）。不是唤醒参数时返回空。
#[uniffi::export]
pub fn parse_wake_token(args: String) -> Option<String> {
    native::parse_wake_token(&args)
}

/// App 总览（spec/protocol.md 第 7 节）。只描述能力，不授权。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AppOverview {
    /// 一句话简介（≤ 100 字符）。
    pub summary: String,
    /// 总览正文（Markdown，≤ 2000 字符）。
    #[uniffi(default = None)]
    pub body: Option<String>,
    /// 语言，如 `zh-CN`。
    #[uniffi(default = None)]
    pub locale: Option<String>,
}

impl From<AppOverview> for native::AppOverview {
    fn from(o: AppOverview) -> Self {
        native::AppOverview {
            summary: o.summary,
            body: o.body,
            locale: o.locale,
        }
    }
}

impl From<ClientConfig> for native::NativeConfig {
    fn from(c: ClientConfig) -> Self {
        let mut n = native::NativeConfig::new(c.app_id, c.app_name);
        n.instance_id = c.instance_id;
        if let Some(kind) = c.client_kind {
            n.client_kind = kind.into();
        }
        if let Some(url) = c.host_url {
            n.host_url = url;
        }
        n.app_version = c.app_version;
        n.instance_title = c.instance_title;
        n.token = c.token;
        n.launch_token = c.launch_token;
        n.max_concurrent_calls = c.max_concurrent_calls;
        n.overview = c.overview.map(Into::into);
        if let Some(lifecycle) = c.lifecycle {
            n.lifecycle = lifecycle.into();
        }
        if let Some(ms) = c.connect_timeout_ms {
            n.connect_timeout_ms = ms;
        }
        n
    }
}

/// 工具定义。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ToolSpec {
    /// App 内唯一，`[a-zA-Z0-9_.-]{1,64}`。
    pub name: String,
    pub description: String,
    /// JSON Schema 文本，`type` 必须为 `"object"`；为空表示无参数。
    #[uniffi(default = None)]
    pub input_schema_json: Option<String>,
    /// 为空时为 `Write`。
    #[uniffi(default = None)]
    pub risk: Option<Risk>,
    #[uniffi(default = None)]
    pub activation: Option<Activation>,
    #[uniffi(default = None)]
    pub title: Option<String>,
    #[uniffi(default = true)]
    pub enabled: bool,
}

impl From<ToolSpec> for native::ToolSpec {
    fn from(s: ToolSpec) -> Self {
        let mut n = native::ToolSpec::new(s.name, s.description);
        n.input_schema_json = s.input_schema_json;
        if let Some(risk) = s.risk {
            n.risk = risk.into();
        }
        n.activation = s.activation.map(Into::into);
        n.title = s.title;
        n.enabled = s.enabled;
        n
    }
}

/// 资源定义。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ResourceSpec {
    pub name: String,
    pub description: String,
    #[uniffi(default = None)]
    pub mime_type: Option<String>,
}

impl From<ResourceSpec> for native::ResourceSpec {
    fn from(s: ResourceSpec) -> Self {
        native::ResourceSpec {
            name: s.name,
            description: s.description,
            mime_type: s.mime_type,
        }
    }
}

/// 连接状态信息。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct StateInfo {
    pub status: StateStatus,
    /// `Backoff` 时距下一次重连的毫秒数。
    pub retry_in_ms: Option<u64>,
    /// `Rejected` 时的原因。
    pub reason: Option<String>,
}

impl From<native::StateInfo> for StateInfo {
    fn from(s: native::StateInfo) -> Self {
        StateInfo {
            status: s.status.into(),
            retry_in_ms: s.retry_in_ms,
            reason: s.reason,
        }
    }
}

// ---------------------------------------------------------------------------
// 回调接口（由外部语言实现）
// ---------------------------------------------------------------------------

/// 工具 handler。在分发线程上同步调用，必须尽快返回；结果通过 `call` 提交。
#[uniffi::export(foreign)]
pub trait ToolHandler: Send + Sync {
    fn invoke(&self, call: Arc<Call>);
}

/// 资源读取。在分发线程上同步调用，结果通过 `read` 提交。
#[uniffi::export(foreign)]
pub trait ResourceReader: Send + Sync {
    fn read(&self, read: Arc<Read>);
}

/// 调用被取消时通知。在分发线程上调用（已取消时在设置监听的线程上立即调用）。
#[uniffi::export(foreign)]
pub trait CancelListener: Send + Sync {
    fn on_cancel(&self, reason: CancelReason);
}

/// 客户端事件。在分发线程上调用。
#[uniffi::export(foreign)]
pub trait ClientListener: Send + Sync {
    fn on_state_changed(&self, state: StateInfo);
    /// 配对成功并获得新 token，App 应持久化，下次放入 `ClientConfig.token`。
    fn on_paired(&self, token: String);
    fn on_log(&self, level: LogLevel, message: String);
    /// 已进入休眠，且驻留策略允许退出进程。App 自行决定是否退出。
    fn on_idle_exit(&self);
}

/// 执行外部回调，吞掉 panic（包括 uniffi 把外部未预期异常转换成的 panic）。返回是否正常结束。
fn guarded(f: impl FnOnce()) -> bool {
    catch_unwind(AssertUnwindSafe(f)).is_ok()
}

struct ToolHandlerAdapter(Arc<dyn ToolHandler>);

impl native::ToolHandler for ToolHandlerAdapter {
    fn invoke(&self, call: native::CallHandle) {
        let wrapped = Arc::new(Call {
            inner: call.clone(),
        });
        if !guarded(|| self.0.invoke(wrapped)) {
            // 外部 handler 在同步部分抛出异常：转为 HANDLER_ERROR（若已完成则忽略）。
            let _ = call.fail(ErrorKind::HandlerError, "handler 抛出了未处理的异常");
        }
    }
}

struct ResourceReaderAdapter(Arc<dyn ResourceReader>);

impl native::ResourceReader for ResourceReaderAdapter {
    fn read(&self, read: native::ReadHandle) {
        let wrapped = Arc::new(Read {
            inner: read.clone(),
        });
        if !guarded(|| self.0.read(wrapped)) {
            let _ = read.fail(ErrorKind::HandlerError, "资源读取抛出了未处理的异常");
        }
    }
}

struct CancelListenerAdapter(Arc<dyn CancelListener>);

impl native::CancelListener for CancelListenerAdapter {
    fn on_cancel(&self, reason: native::CancelReason) {
        guarded(|| self.0.on_cancel(reason.into()));
    }
}

struct ClientListenerAdapter(Arc<dyn ClientListener>);

impl native::ClientListener for ClientListenerAdapter {
    fn on_state_changed(&self, state: native::StateInfo) {
        guarded(|| self.0.on_state_changed(state.into()));
    }
    fn on_paired(&self, token: String) {
        guarded(|| self.0.on_paired(token));
    }
    fn on_log(&self, level: native::LogLevel, message: String) {
        guarded(|| self.0.on_log(level.into(), message));
    }
    fn on_idle_exit(&self) {
        guarded(|| self.0.on_idle_exit());
    }
}

// ---------------------------------------------------------------------------
// 对象
// ---------------------------------------------------------------------------

/// 一次工具调用。可跨线程传递；`complete` / `fail` 只能成功一次。
#[derive(Debug, uniffi::Object)]
pub struct Call {
    inner: native::CallHandle,
}

#[uniffi::export]
impl Call {
    pub fn call_id(&self) -> String {
        self.inner.call_id()
    }
    pub fn tool_name(&self) -> String {
        self.inner.tool_name()
    }
    /// 已由 Host 按 inputSchema 校验过的参数（JSON 对象文本）。
    pub fn arguments_json(&self) -> String {
        self.inner.arguments_json()
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    /// 设置取消监听。已取消时立即（在当前线程）回调一次。
    pub fn set_cancel_listener(&self, listener: Arc<dyn CancelListener>) {
        self.inner
            .set_cancel_listener(Arc::new(CancelListenerAdapter(listener)));
    }
    /// 成功完成。`data_json` 为空表示 `null`；非法 JSON 返回 `InvalidJson`（调用仍未完成）。
    pub fn complete(
        &self,
        data_json: Option<String>,
        state_hints: Vec<String>,
    ) -> Result<(), AppMcpError> {
        Ok(self.inner.complete(data_json.as_deref(), state_hints)?)
    }
    /// 失败完成。`kind` 为错误类别字符串（如 `"HANDLER_ERROR"`），未知类别返回 `UnknownErrorKind`。
    pub fn fail(&self, kind: String, message: String) -> Result<(), AppMcpError> {
        let kind = parse_error_kind(&kind)?;
        Ok(self.inner.fail(kind, &message)?)
    }
    /// 失败完成并附带结构化详情（JSON 文本；对象的字段合并进错误的 `data`，其他值放在 `data.details`）。
    /// `details_json` 为空等同于 `fail`；非法 JSON 返回 `InvalidJson`（调用仍未完成）。
    pub fn fail_with_details(
        &self,
        kind: String,
        message: String,
        details_json: Option<String>,
    ) -> Result<(), AppMcpError> {
        let kind = parse_error_kind(&kind)?;
        Ok(self
            .inner
            .fail_with_details(kind, &message, details_json.as_deref())?)
    }
    /// 延长持有：调用完成后仍阻止自动休眠（handler 发起的长任务），直到返回的 `Hold` 被释放。
    /// 调用已结束时返回 `AlreadyCompleted`。
    pub fn hold(&self) -> Result<Arc<Hold>, AppMcpError> {
        Ok(Arc::new(Hold {
            inner: self.inner.hold()?,
        }))
    }
}

/// 阻止自动休眠的持有。`release` 幂等；对象被外部语言释放时自动释放。
#[derive(Debug, uniffi::Object)]
pub struct Hold {
    inner: native::HoldHandle,
}

#[uniffi::export]
impl Hold {
    /// 释放持有。重复调用无效果。
    pub fn release(&self) {
        self.inner.release()
    }
}

/// 一次资源读取。完成只能一次。
#[derive(Debug, uniffi::Object)]
pub struct Read {
    inner: native::ReadHandle,
}

#[uniffi::export]
impl Read {
    pub fn resource_name(&self) -> String {
        self.inner.resource_name()
    }
    /// 以 JSON 文本完成读取。
    pub fn complete(&self, contents_json: String) -> Result<(), AppMcpError> {
        Ok(self.inner.complete(&contents_json)?)
    }
    pub fn fail(&self, kind: String, message: String) -> Result<(), AppMcpError> {
        let kind = parse_error_kind(&kind)?;
        Ok(self.inner.fail(kind, &message)?)
    }
}

/// 已注册的工具。`dispose` 幂等；丢弃对象**不会**注销工具。
#[derive(Debug, uniffi::Object)]
pub struct Tool {
    inner: native::ToolHandle,
}

#[uniffi::export]
impl Tool {
    pub fn name(&self) -> String {
        self.inner.name()
    }
    /// 用新定义整体替换（名称不可变，`spec.name` 被忽略）。
    pub fn update(&self, spec: ToolSpec) -> Result<(), AppMcpError> {
        Ok(self.inner.update(spec.into())?)
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<(), AppMcpError> {
        Ok(self.inner.set_enabled(enabled)?)
    }
    pub fn dispose(&self) {
        self.inner.dispose()
    }
}

/// 已注册的资源。`dispose` 幂等。
#[derive(Debug, uniffi::Object)]
pub struct Resource {
    inner: native::ResourceHandle,
}

#[uniffi::export]
impl Resource {
    pub fn name(&self) -> String {
        self.inner.name()
    }
    pub fn notify_changed(&self) -> Result<(), AppMcpError> {
        Ok(self.inner.notify_changed()?)
    }
    pub fn dispose(&self) {
        self.inner.dispose()
    }
}

/// Scope：注销时递归注销其下所有工具、资源与子 scope。`dispose` 幂等。
#[derive(Debug, uniffi::Object)]
pub struct Scope {
    inner: native::ScopeHandle,
}

#[uniffi::export]
impl Scope {
    pub fn register_tool(
        &self,
        spec: ToolSpec,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<Arc<Tool>, AppMcpError> {
        let inner = self
            .inner
            .register_tool(spec.into(), Arc::new(ToolHandlerAdapter(handler)))?;
        Ok(Arc::new(Tool { inner }))
    }
    pub fn register_resource(
        &self,
        spec: ResourceSpec,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<Arc<Resource>, AppMcpError> {
        let inner = self
            .inner
            .register_resource(spec.into(), Arc::new(ResourceReaderAdapter(reader)))?;
        Ok(Arc::new(Resource { inner }))
    }
    pub fn create_scope(&self, name: String) -> Result<Arc<Scope>, AppMcpError> {
        Ok(Arc::new(Scope {
            inner: self.inner.create_scope(&name)?,
        }))
    }
    pub fn dispose(&self) {
        self.inner.dispose()
    }
}

/// 客户端。创建时启动后台运行时（不连接），`start` 后开始连接 Host。
/// 对象被外部语言释放（最后一个引用消失）时自动停止。
#[derive(Debug, uniffi::Object)]
pub struct AppMcpClient {
    inner: native::NativeClient,
}

#[uniffi::export]
impl AppMcpClient {
    #[uniffi::constructor]
    pub fn new(
        config: ClientConfig,
        listener: Option<Arc<dyn ClientListener>>,
    ) -> Result<Arc<Self>, AppMcpError> {
        let listener: Option<Arc<dyn native::ClientListener>> =
            listener.map(|l| Arc::new(ClientListenerAdapter(l)) as Arc<dyn native::ClientListener>);
        let inner = native::NativeClient::new(config.into(), listener)?;
        Ok(Arc::new(Self { inner }))
    }
    pub fn instance_id(&self) -> String {
        self.inner.instance_id()
    }
    pub fn state(&self) -> StateInfo {
        self.inner.state().into()
    }
    /// 当前 token（配置带入的或配对后获得的）。
    pub fn token(&self) -> Option<String> {
        self.inner.token()
    }
    /// 开始连接。重复调用无效果。
    pub fn start(&self) {
        self.inner.start()
    }
    /// 停止：取消所有调用、断开连接、不再重连。之后注册类方法返回 `Stopped`。
    pub fn stop(&self) {
        self.inner.stop()
    }
    pub fn set_visibility(&self, visibility: Visibility, focused: bool) {
        self.inner.set_visibility(visibility.into(), focused)
    }
    /// 在根作用域注册工具。
    pub fn register_tool(
        &self,
        spec: ToolSpec,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<Arc<Tool>, AppMcpError> {
        let inner = self
            .inner
            .register_tool(spec.into(), Arc::new(ToolHandlerAdapter(handler)))?;
        Ok(Arc::new(Tool { inner }))
    }
    pub fn register_resource(
        &self,
        spec: ResourceSpec,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<Arc<Resource>, AppMcpError> {
        let inner = self
            .inner
            .register_resource(spec.into(), Arc::new(ResourceReaderAdapter(reader)))?;
        Ok(Arc::new(Resource { inner }))
    }
    pub fn create_scope(&self, name: String) -> Result<Arc<Scope>, AppMcpError> {
        Ok(Arc::new(Scope {
            inner: self.inner.create_scope(&name)?,
        }))
    }

    // ---- 生命周期（spec/lifecycle.md 第 8 节） ----

    /// 处理操作系统激活参数 / URL。不是本 SDK 的唤醒返回 `false`。可以在 `start` 之前调用。
    pub fn handle_wake(&self, args: String) -> bool {
        self.inner.handle_wake(&args)
    }
    /// App 主动回连。返回是否因此发起了回连。
    pub fn wake(&self) -> bool {
        self.inner.wake()
    }
    /// 以指定原因回连（窗口重新可见时用 `Visible`）。
    pub fn wake_with_reason(&self, reason: WakeReason) -> bool {
        self.inner.wake_with_reason(reason.into())
    }
    /// `OnDemand` 模式下主动连接；尚未 `start` 时等同于 `start`。
    pub fn connect_now(&self) -> bool {
        self.inner.connect_now()
    }
    /// App 主动请求休眠（原因 `App`）。返回是否有效果。
    pub fn sleep(&self) -> bool {
        self.inner.sleep()
    }
    /// 以指定原因请求休眠（进入后台时用 `Background`）。
    pub fn sleep_with_reason(&self, reason: SleepReason) -> bool {
        self.inner.sleep_with_reason(reason.into())
    }
    /// 临时阻止自动休眠，直到返回的 `Hold` 被释放。
    pub fn hold(&self) -> Arc<Hold> {
        Arc::new(Hold {
            inner: self.inner.hold(),
        })
    }
    /// 当前工具与资源定义的摘要。
    pub fn tools_hash(&self) -> String {
        self.inner.tools_hash()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_kinds_roundtrip() {
        let kinds = error_kinds();
        assert_eq!(kinds.len(), 15);
        for k in &kinds {
            assert_eq!(parse_error_kind(k).map(|e| e.as_str()), Ok(k.as_str()));
        }
        assert!(kinds.contains(&"HANDLER_ERROR".to_owned()));
        assert_eq!(
            parse_error_kind("NOPE"),
            Err(AppMcpError::UnknownErrorKind {
                kind: "NOPE".into()
            })
        );
    }

    #[test]
    fn config_defaults_follow_native() {
        let cfg = ClientConfig {
            app_id: "shop".into(),
            app_name: "Shop".into(),
            instance_id: None,
            client_kind: None,
            host_url: None,
            app_version: None,
            instance_title: None,
            token: None,
            launch_token: None,
            max_concurrent_calls: 1,
            overview: None,
            lifecycle: None,
            connect_timeout_ms: None,
        };
        let n: native::NativeConfig = cfg.clone().into();
        assert_eq!(n, native::NativeConfig::new("shop", "Shop"));

        let n: native::NativeConfig = ClientConfig {
            client_kind: Some(ClientKind::Hybrid),
            host_url: Some("ws://127.0.0.1:1".into()),
            max_concurrent_calls: 4,
            overview: Some(AppOverview {
                summary: "网店".into(),
                body: None,
                locale: Some("zh-CN".into()),
            }),
            ..cfg
        }
        .into();
        assert_eq!(
            n.overview.as_ref().map(|o| o.summary.as_str()),
            Some("网店")
        );
        assert_eq!(n.client_kind, native::ClientKind::Hybrid);
        assert_eq!(n.host_url, "ws://127.0.0.1:1");
        assert_eq!(n.max_concurrent_calls, 4);
    }

    #[test]
    fn tool_spec_conversion() {
        let spec = ToolSpec {
            name: "cart.add".into(),
            description: "加入购物车".into(),
            input_schema_json: Some(r#"{"type":"object"}"#.into()),
            risk: None,
            activation: Some(Activation::Background),
            title: None,
            enabled: false,
        };
        let n: native::ToolSpec = spec.clone().into();
        assert_eq!(n.risk, native::Risk::Write);
        assert_eq!(n.activation, Some(native::Activation::Background));
        assert!(!n.enabled);
        let n: native::ToolSpec = ToolSpec {
            risk: Some(Risk::OsSensitive),
            ..spec
        }
        .into();
        assert_eq!(n.risk, native::Risk::OsSensitive);
    }

    #[test]
    fn native_error_mapping() {
        assert_eq!(
            AppMcpError::from(native::NativeError::DuplicateName("a".into())),
            AppMcpError::DuplicateName { name: "a".into() }
        );
        assert_eq!(
            AppMcpError::from(native::NativeError::Stopped),
            AppMcpError::Stopped
        );
    }

    #[test]
    fn state_conversion() {
        let s = StateInfo::from(native::StateInfo {
            status: native::StateStatus::Backoff,
            retry_in_ms: Some(500),
            reason: None,
        });
        assert_eq!(s.status, StateStatus::Backoff);
        assert_eq!(s.retry_in_ms, Some(500));
    }

    #[test]
    fn lifecycle_conversion() {
        let p = LifecyclePolicy {
            mode: None,
            idle_timeout_ms: 60_000,
            hidden_idle_timeout_ms: 15_000,
            grace_ms: 10_000,
            residency: None,
            wake: None,
        };
        assert_eq!(native::LifecyclePolicy::from(p.clone()), native::LifecyclePolicy::default());
        let n: native::LifecyclePolicy = LifecyclePolicy {
            mode: Some(LifecycleMode::Idle),
            hidden_idle_timeout_ms: 0,
            residency: Some(Residency::ExitWhenIdle),
            wake: Some(WakeDescriptor {
                kind: WakeKind::AndroidIntent,
                target: Some("pkg/dev.appmcp.android.WakeReceiver".into()),
                background: true,
            }),
            ..p
        }
        .into();
        assert_eq!(n.mode, native::LifecycleMode::Idle);
        assert_eq!(n.hidden_idle_timeout_ms, 0);
        assert_eq!(n.residency, native::Residency::ExitWhenIdle);
        let wake = n.wake.expect("wake");
        assert_eq!(wake.kind, native::WakeKind::AndroidIntent);
        assert!(wake.background);
    }

    #[test]
    fn wake_token_parsing() {
        assert_eq!(parse_wake_token("app-mcp-wake:abc".into()), Some("abc".into()));
        assert_eq!(
            parse_wake_token("shop://app-mcp/wake?token=x1".into()),
            Some("x1".into())
        );
        assert_eq!(parse_wake_token("--foo".into()), None);
    }

    #[test]
    fn client_lifecycle_api() {
        let cfg = ClientConfig {
            app_id: "shop".into(),
            app_name: "Shop".into(),
            instance_id: None,
            client_kind: None,
            host_url: Some("ws://127.0.0.1:9".into()),
            app_version: None,
            instance_title: None,
            token: None,
            launch_token: None,
            max_concurrent_calls: 1,
            overview: None,
            lifecycle: Some(LifecyclePolicy {
                mode: Some(LifecycleMode::OnDemand),
                idle_timeout_ms: 60_000,
                hidden_idle_timeout_ms: 15_000,
                grace_ms: 10_000,
                residency: None,
                wake: None,
            }),
            connect_timeout_ms: Some(1000),
        };
        let client = AppMcpClient::new(cfg, None).expect("client");
        assert!(!client.handle_wake("not-a-wake".into()));
        assert_eq!(client.tools_hash().len(), 16);
        let hold = client.hold();
        hold.release();
        hold.release();
        client.stop();
    }

    #[test]
    fn guarded_catches_panic() {
        assert!(guarded(|| {}));
        assert!(!guarded(|| panic!("boom")));
    }
}
