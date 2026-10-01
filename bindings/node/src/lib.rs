//! napi-rs 绑定：把 `app-mcp-native` 暴露给 Node（`@app-mcp/node` 在其上封装 JS API）。
//!
//! # 线程模型
//!
//! 原生运行时在自己的分发线程上触发回调（工具调用、资源读取、取消、状态、配对、日志）。
//! 本绑定把每个 JS 回调包装成 [`ThreadsafeFunction`]，在分发线程上以非阻塞方式投递到
//! Node 事件循环，回调本身在 JS 主线程执行。JS 完成 handler 后调用 `Call.complete` /
//! `Call.fail`（可在任意时刻、任意次数尝试，重复完成返回 `ALREADY_COMPLETED` 错误）。
//!
//! # 进程退出
//!
//! 所有 ThreadsafeFunction 都以 **weak** 模式创建（`napi_unref_threadsafe_function`），
//! 不会阻止 Node 进程退出；是否保持进程存活由 JS 封装层决定（`keepAlive` 选项）。
//! ThreadsafeFunction 在最后一个 Rust 引用被丢弃时释放：工具 / 资源注销后原生运行时丢弃
//! handler，`stop()` 时丢弃客户端监听器。
//!
//! # 错误
//!
//! 所有方法抛出的 JS `Error` 的 `code` 为 [`NativeError`] 对应的大写代码
//! （如 `DUPLICATE_NAME`、`STOPPED`），参数不合法时为 `INVALID_ARG`。

#![deny(clippy::all)]

use std::sync::{Arc, Mutex};

use app_mcp_native as native;
use napi::Status;
use napi::bindgen_prelude::Unknown;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use native::{
    Activation, CancelReason, ClientKind, ErrorKind, LifecycleMode, LifecyclePolicy, LogLevel, NativeError,
    Residency, Risk, SleepReason, StateInfo, StateStatus, Visibility, WakeDescriptor, WakeKind, WakeReason,
};

/// 本绑定抛出的错误：`status` 字符串成为 JS 错误的 `code`（napi-derive 按名称 `Result` 识别返回类型）。
use napi::Result;

/// 不阻止进程退出（weak）、不带 error-first 参数（callee_handled = false）的 ThreadsafeFunction。
type WeakTsfn<T> = ThreadsafeFunction<T, Unknown<'static>, T, Status, false, true>;

// ---------------------------------------------------------------------------
// 错误与枚举转换
// ---------------------------------------------------------------------------

fn native_error_code(e: &NativeError) -> &'static str {
    match e {
        NativeError::InvalidName(_) => "INVALID_NAME",
        NativeError::InvalidSchema(_) => "INVALID_SCHEMA",
        NativeError::DuplicateName(_) => "DUPLICATE_NAME",
        NativeError::InvalidJson(_) => "INVALID_JSON",
        NativeError::InvalidConfig(_) => "INVALID_CONFIG",
        NativeError::AlreadyCompleted => "ALREADY_COMPLETED",
        NativeError::Disposed => "DISPOSED",
        NativeError::Stopped => "STOPPED",
        NativeError::Internal(_) => "INTERNAL",
    }
}

fn to_js_error(e: NativeError) -> napi::Error<String> {
    napi::Error::new(native_error_code(&e).to_string(), e.to_string())
}

fn invalid_arg(message: String) -> napi::Error<String> {
    napi::Error::new("INVALID_ARG".to_string(), message)
}

fn parse_risk(s: &str) -> Result<Risk, String> {
    Ok(match s {
        "read" => Risk::Read,
        "write" => Risk::Write,
        "destructive" => Risk::Destructive,
        "payment" => Risk::Payment,
        "os-sensitive" => Risk::OsSensitive,
        other => return Err(invalid_arg(format!("未知的 risk：{other:?}"))),
    })
}

fn parse_activation(s: &str) -> Result<Activation, String> {
    Ok(match s {
        "headless" => Activation::Headless,
        "background" => Activation::Background,
        "foreground" => Activation::Foreground,
        other => return Err(invalid_arg(format!("未知的 activation：{other:?}"))),
    })
}

fn parse_visibility(s: &str) -> Result<Visibility, String> {
    Ok(match s {
        "visible" => Visibility::Visible,
        "hidden" => Visibility::Hidden,
        "frozen" => Visibility::Frozen,
        other => return Err(invalid_arg(format!("未知的 visibility：{other:?}"))),
    })
}

fn parse_client_kind(s: &str) -> Result<ClientKind, String> {
    Ok(match s {
        "native" => ClientKind::Native,
        "hybrid" => ClientKind::Hybrid,
        "web" => ClientKind::Web,
        other => return Err(invalid_arg(format!("未知的 clientKind：{other:?}"))),
    })
}

fn parse_error_kind(s: &str) -> Result<ErrorKind, String> {
    Ok(match s {
        "TOOL_NOT_FOUND" => ErrorKind::ToolNotFound,
        "TOOL_DISABLED" => ErrorKind::ToolDisabled,
        "INVALID_INPUT" => ErrorKind::InvalidInput,
        "USER_REJECTED" => ErrorKind::UserRejected,
        "TIMEOUT" => ErrorKind::Timeout,
        "HANDLER_ERROR" => ErrorKind::HandlerError,
        "CANCELLED" => ErrorKind::Cancelled,
        "APP_DISCONNECTED" => ErrorKind::AppDisconnected,
        "APP_NOT_INSTALLED" => ErrorKind::AppNotInstalled,
        "LAUNCH_FAILED" => ErrorKind::LaunchFailed,
        "APP_NOT_RESPONDING" => ErrorKind::AppNotResponding,
        "INSTANCE_FROZEN" => ErrorKind::InstanceFrozen,
        "RESOURCE_NOT_FOUND" => ErrorKind::ResourceNotFound,
        "UNAUTHORIZED" => ErrorKind::Unauthorized,
        "UNSUPPORTED_PROTOCOL" => ErrorKind::UnsupportedProtocol,
        other => return Err(invalid_arg(format!("未知的错误类别：{other:?}"))),
    })
}

fn parse_lifecycle_mode(s: &str) -> Result<LifecycleMode, String> {
    Ok(match s {
        "persistent" => LifecycleMode::Persistent,
        "idle" => LifecycleMode::Idle,
        "on-demand" => LifecycleMode::OnDemand,
        other => return Err(invalid_arg(format!("未知的 lifecycle.mode：{other:?}"))),
    })
}

fn parse_residency(s: &str) -> Result<Residency, String> {
    Ok(match s {
        "keep" => Residency::Keep,
        "exit-when-idle" => Residency::ExitWhenIdle,
        "exit-always" => Residency::ExitAlways,
        other => return Err(invalid_arg(format!("未知的 lifecycle.residency：{other:?}"))),
    })
}

fn parse_wake_kind(s: &str) -> Result<WakeKind, String> {
    Ok(match s {
        "uri" => WakeKind::Uri,
        "aumid" => WakeKind::Aumid,
        "apple-event" => WakeKind::AppleEvent,
        "dbus" => WakeKind::Dbus,
        "android-intent" => WakeKind::AndroidIntent,
        "web-url" => WakeKind::WebUrl,
        "none" => WakeKind::None,
        other => return Err(invalid_arg(format!("未知的 wake.kind：{other:?}"))),
    })
}

fn parse_wake_reason(s: &str) -> Result<WakeReason, String> {
    Ok(match s {
        "os-activation" => WakeReason::OsActivation,
        "app" => WakeReason::App,
        "visible" => WakeReason::Visible,
        "cold-start" => WakeReason::ColdStart,
        other => return Err(invalid_arg(format!("未知的唤醒原因：{other:?}"))),
    })
}

fn parse_sleep_reason(s: &str) -> Result<SleepReason, String> {
    Ok(match s {
        "idle" => SleepReason::Idle,
        "grace" => SleepReason::Grace,
        "background" => SleepReason::Background,
        "app" => SleepReason::App,
        other => return Err(invalid_arg(format!("未知的休眠原因：{other:?}"))),
    })
}

/// JS 数字 → 毫秒数（非负、有限；小数向下取整）。
fn parse_millis(field: &str, v: f64) -> Result<u64, String> {
    if !v.is_finite() || v < 0.0 {
        return Err(invalid_arg(format!("{field} 必须是非负有限数：{v}")));
    }
    // 已检查非负有限；超出 u64 范围时饱和。
    Ok(v.floor().min(u64::MAX as f64) as u64)
}

fn status_str(s: StateStatus) -> &'static str {
    match s {
        StateStatus::Idle => "idle",
        StateStatus::Connecting => "connecting",
        StateStatus::Handshaking => "handshaking",
        StateStatus::PendingPairing => "pending-pairing",
        StateStatus::Connected => "connected",
        StateStatus::Backoff => "backoff",
        StateStatus::Rejected => "rejected",
        StateStatus::Stopped => "stopped",
        StateStatus::Dormant => "dormant",
        StateStatus::Waking => "waking",
        StateStatus::HostMismatch => "host-mismatch",
    }
}

fn cancel_reason_str(r: CancelReason) -> &'static str {
    match r {
        CancelReason::Requested => "requested",
        CancelReason::Timeout => "timeout",
        CancelReason::Disconnected => "disconnected",
        CancelReason::Stopped => "stopped",
    }
}

fn log_level_str(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

// ---------------------------------------------------------------------------
// JS 对象
// ---------------------------------------------------------------------------

/// `new NativeClient(config, listener?)` 的配置。
#[napi(object)]
pub struct ClientConfig {
    pub app_id: String,
    pub app_name: String,
    pub instance_id: Option<String>,
    /// `'native'`（默认）| `'hybrid'` | `'web'`。
    pub client_kind: Option<String>,
    pub host_url: Option<String>,
    pub app_version: Option<String>,
    pub instance_title: Option<String>,
    pub token: Option<String>,
    pub launch_token: Option<String>,
    pub max_concurrent_calls: Option<u32>,
    /// App 总览（spec/protocol.md 第 7 节）。
    pub overview: Option<OverviewInit>,
    /// 生命周期策略（spec/lifecycle.md 第 3 节）。缺省 `persistent`。
    pub lifecycle: Option<LifecycleInit>,
    /// 建立 WebSocket 连接的超时（毫秒），默认 5000。
    pub connect_timeout_ms: Option<u32>,
}

/// 生命周期策略。未提供的字段取默认值（见 spec/lifecycle.md 第 3 节）。
#[napi(object)]
pub struct LifecycleInit {
    /// `'persistent'`（默认）| `'idle'` | `'on-demand'`。
    pub mode: Option<String>,
    pub idle_timeout_ms: Option<f64>,
    pub hidden_idle_timeout_ms: Option<f64>,
    pub grace_ms: Option<f64>,
    /// `'keep'`（默认）| `'exit-when-idle'` | `'exit-always'`。
    pub residency: Option<String>,
    /// 本实例的唤醒描述，随 `app/sleep` 上报。
    pub wake: Option<WakeInit>,
}

/// 唤醒描述（spec/lifecycle.md 第 5 节）。
#[napi(object)]
pub struct WakeInit {
    /// `'uri' | 'aumid' | 'apple-event' | 'dbus' | 'android-intent' | 'web-url' | 'none'`。
    pub kind: String,
    pub target: Option<String>,
    pub background: Option<bool>,
}

impl LifecycleInit {
    fn into_policy(self) -> Result<LifecyclePolicy, String> {
        let mut p = LifecyclePolicy::default();
        if let Some(mode) = self.mode.as_deref() {
            p.mode = parse_lifecycle_mode(mode)?;
        }
        if let Some(v) = self.idle_timeout_ms {
            p.idle_timeout_ms = parse_millis("lifecycle.idleTimeoutMs", v)?;
        }
        if let Some(v) = self.hidden_idle_timeout_ms {
            p.hidden_idle_timeout_ms = parse_millis("lifecycle.hiddenIdleTimeoutMs", v)?;
        }
        if let Some(v) = self.grace_ms {
            p.grace_ms = parse_millis("lifecycle.graceMs", v)?;
        }
        if let Some(r) = self.residency.as_deref() {
            p.residency = parse_residency(r)?;
        }
        if let Some(w) = self.wake {
            p.wake = Some(WakeDescriptor {
                kind: parse_wake_kind(&w.kind)?,
                target: w.target,
                background: w.background.unwrap_or(false),
            });
        }
        Ok(p)
    }
}

/// App 总览：`summary` 一句话简介，`body` Markdown 正文，`locale` 如 `'zh-CN'`。
#[napi(object)]
pub struct OverviewInit {
    pub summary: String,
    pub body: Option<String>,
    pub locale: Option<String>,
}

impl From<OverviewInit> for native::AppOverview {
    fn from(o: OverviewInit) -> Self {
        native::AppOverview { summary: o.summary, body: o.body, locale: o.locale }
    }
}

/// 连接状态。`status` 与 `@app-mcp/web` 的 `ConnectionState.status` 取值一致。
#[napi(object)]
pub struct JsStateInfo {
    pub status: String,
    pub retry_in_ms: Option<f64>,
    pub reason: Option<String>,
    /// 与 `reason` 对应的错误码（spec/protocol.md 10.1）。
    pub code: Option<String>,
}

impl From<StateInfo> for JsStateInfo {
    fn from(s: StateInfo) -> Self {
        Self {
            status: status_str(s.status).to_string(),
            retry_in_ms: s.retry_in_ms.map(|v| v as f64),
            reason: s.reason,
            code: s.code,
        }
    }
}

/// 客户端事件：`type` 为 `'state'`（带 `state`）、`'paired'`（带 `token`）、`'log'`（带 `level`、`message`）
/// 或 `'idle-exit'`（已休眠且驻留策略允许退出进程，无其他字段）。
#[napi(object)]
pub struct ClientEvent {
    #[napi(js_name = "type")]
    pub kind: String,
    pub state: Option<JsStateInfo>,
    pub token: Option<String>,
    pub level: Option<String>,
    pub message: Option<String>,
}

/// 工具定义。未提供的字段取默认值：无参数、risk = write、enabled = true。
#[napi(object)]
pub struct ToolSpecInit {
    pub name: String,
    pub description: String,
    /// JSON Schema 文本（顶层 `type` 为 `"object"`）。
    pub input_schema_json: Option<String>,
    pub risk: Option<String>,
    pub activation: Option<String>,
    pub title: Option<String>,
    pub enabled: Option<bool>,
}

impl ToolSpecInit {
    fn into_spec(self) -> Result<native::ToolSpec, String> {
        let mut spec = native::ToolSpec::new(self.name, self.description);
        spec.input_schema_json = self.input_schema_json;
        if let Some(risk) = self.risk.as_deref() {
            spec.risk = parse_risk(risk)?;
        }
        spec.activation = self.activation.as_deref().map(parse_activation).transpose()?;
        spec.title = self.title;
        spec.enabled = self.enabled.unwrap_or(true);
        Ok(spec)
    }
}

#[napi(object)]
pub struct ResourceSpecInit {
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
}

impl From<ResourceSpecInit> for native::ResourceSpec {
    fn from(s: ResourceSpecInit) -> Self {
        native::ResourceSpec { name: s.name, description: s.description, mime_type: s.mime_type }
    }
}

// ---------------------------------------------------------------------------
// 回调适配：原生分发线程 → Node 事件循环
// ---------------------------------------------------------------------------

struct JsToolHandler {
    tsfn: WeakTsfn<Call>,
}

impl native::ToolHandler for JsToolHandler {
    fn invoke(&self, call: native::CallHandle) {
        let status = self.tsfn.call(Call { inner: call.clone() }, ThreadsafeFunctionCallMode::NonBlocking);
        if status != Status::Ok {
            // 事件循环已关闭（进程正在退出）：直接失败，避免 Host 等到超时。
            let _ = call.fail(ErrorKind::AppNotResponding, "Node 事件循环不可用");
        }
    }
}

struct JsResourceReader {
    tsfn: WeakTsfn<Read>,
}

impl native::ResourceReader for JsResourceReader {
    fn read(&self, read: native::ReadHandle) {
        let status = self.tsfn.call(Read { inner: read.clone() }, ThreadsafeFunctionCallMode::NonBlocking);
        if status != Status::Ok {
            let _ = read.fail(ErrorKind::AppNotResponding, "Node 事件循环不可用");
        }
    }
}

struct JsCancelListener {
    tsfn: WeakTsfn<String>,
}

impl native::CancelListener for JsCancelListener {
    fn on_cancel(&self, reason: CancelReason) {
        let _ = self.tsfn.call(cancel_reason_str(reason).to_string(), ThreadsafeFunctionCallMode::NonBlocking);
    }
}

/// 客户端监听器。`stop()` 后清空，以便尽早释放 ThreadsafeFunction。
struct JsClientListener {
    tsfn: Mutex<Option<Arc<WeakTsfn<ClientEvent>>>>,
}

impl JsClientListener {
    fn emit(&self, event: ClientEvent) {
        let tsfn = match self.tsfn.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        if let Some(tsfn) = tsfn {
            let _ = tsfn.call(event, ThreadsafeFunctionCallMode::NonBlocking);
        }
    }

    fn release(&self) {
        let taken = match self.tsfn.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        drop(taken);
    }
}

impl native::ClientListener for JsClientListener {
    fn on_state_changed(&self, state: StateInfo) {
        self.emit(ClientEvent {
            kind: "state".to_string(),
            state: Some(state.into()),
            token: None,
            level: None,
            message: None,
        });
    }

    fn on_paired(&self, token: String) {
        self.emit(ClientEvent { kind: "paired".to_string(), state: None, token: Some(token), level: None, message: None });
    }

    fn on_log(&self, level: LogLevel, message: String) {
        self.emit(ClientEvent {
            kind: "log".to_string(),
            state: None,
            token: None,
            level: Some(log_level_str(level).to_string()),
            message: Some(message),
        });
    }

    fn on_idle_exit(&self) {
        self.emit(ClientEvent { kind: "idle-exit".to_string(), state: None, token: None, level: None, message: None });
    }
}

// ---------------------------------------------------------------------------
// 句柄类
// ---------------------------------------------------------------------------

/// 一次工具调用（由原生运行时创建，JS 不能构造）。
#[napi]
pub struct Call {
    inner: native::CallHandle,
}

#[napi]
impl Call {
    #[napi(getter)]
    pub fn call_id(&self) -> String {
        self.inner.call_id()
    }

    #[napi(getter)]
    pub fn tool_name(&self) -> String {
        self.inner.tool_name()
    }

    /// 已由 Host 校验过的参数（JSON 对象文本）。
    #[napi(getter)]
    pub fn arguments_json(&self) -> String {
        self.inner.arguments_json()
    }

    #[napi]
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    /// 设置取消监听：`listener(reason)`，reason 为 `'requested' | 'timeout' | 'disconnected' | 'stopped'`。
    /// 已取消时也会回调一次（经事件循环异步投递）。
    #[napi]
    pub fn set_cancel_listener(&self, listener: WeakTsfn<String>) {
        self.inner.set_cancel_listener(Arc::new(JsCancelListener { tsfn: listener }));
    }

    /// 成功完成。`dataJson` 为 `null` / 省略表示 `null`。
    #[napi]
    pub fn complete(&self, data_json: Option<String>, state_hints: Option<Vec<String>>) -> Result<(), String> {
        self.inner.complete(data_json.as_deref(), state_hints.unwrap_or_default()).map_err(to_js_error)
    }

    /// 失败完成。`kind` 为协议错误类别（如 `'HANDLER_ERROR'`）。
    #[napi]
    pub fn fail(&self, kind: String, message: String) -> Result<(), String> {
        let kind = parse_error_kind(&kind)?;
        self.inner.fail(kind, &message).map_err(to_js_error)
    }

    /// 失败完成并附带结构化详情（JSON 文本；对象的字段合并进错误的 `data`，其他值放在 `data.details`）。
    /// `detailsJson` 为 `null` / 省略时等同于 `fail`。
    #[napi]
    pub fn fail_with_details(&self, kind: String, message: String, details_json: Option<String>) -> Result<(), String> {
        let kind = parse_error_kind(&kind)?;
        self.inner.fail_with_details(kind, &message, details_json.as_deref()).map_err(to_js_error)
    }

    /// 调用完成后仍阻止自动休眠（handler 发起的长任务），直到返回的 `Hold` 被 `release()`。
    /// 调用已结束时抛出 `ALREADY_COMPLETED`。
    #[napi]
    pub fn hold(&self) -> Result<Hold, String> {
        let inner = self.inner.hold().map_err(to_js_error)?;
        Ok(Hold { inner })
    }
}

/// 阻止自动休眠的持有。`release()` 幂等；JS 对象被垃圾回收时也会释放（不要依赖这一点）。
#[napi]
pub struct Hold {
    inner: native::HoldHandle,
}

#[napi]
impl Hold {
    #[napi]
    pub fn release(&self) {
        self.inner.release();
    }
}

/// 一次资源读取。
#[napi]
pub struct Read {
    inner: native::ReadHandle,
}

#[napi]
impl Read {
    #[napi(getter)]
    pub fn resource_name(&self) -> String {
        self.inner.resource_name()
    }

    #[napi]
    pub fn complete(&self, contents_json: String) -> Result<(), String> {
        self.inner.complete(&contents_json).map_err(to_js_error)
    }

    #[napi]
    pub fn fail(&self, kind: String, message: String) -> Result<(), String> {
        let kind = parse_error_kind(&kind)?;
        self.inner.fail(kind, &message).map_err(to_js_error)
    }
}

/// 已注册的工具。
#[napi]
pub struct Tool {
    inner: native::ToolHandle,
}

#[napi]
impl Tool {
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.name()
    }

    /// 整体替换定义（`spec.name` 被忽略）。
    #[napi]
    pub fn update(&self, spec: ToolSpecInit) -> Result<(), String> {
        self.inner.update(spec.into_spec()?).map_err(to_js_error)
    }

    #[napi]
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.inner.set_enabled(enabled).map_err(to_js_error)
    }

    #[napi]
    pub fn dispose(&self) {
        self.inner.dispose();
    }
}

/// 已注册的资源。
#[napi]
pub struct Resource {
    inner: native::ResourceHandle,
}

#[napi]
impl Resource {
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.name()
    }

    #[napi]
    pub fn notify_changed(&self) -> Result<(), String> {
        self.inner.notify_changed().map_err(to_js_error)
    }

    #[napi]
    pub fn dispose(&self) {
        self.inner.dispose();
    }
}

/// Scope：注销时递归注销其下全部工具、资源与子 scope。
#[napi]
pub struct Scope {
    inner: native::ScopeHandle,
}

#[napi]
impl Scope {
    #[napi]
    pub fn register_tool(&self, spec: ToolSpecInit, handler: WeakTsfn<Call>) -> Result<Tool, String> {
        let spec = spec.into_spec()?;
        let inner =
            self.inner.register_tool(spec, Arc::new(JsToolHandler { tsfn: handler })).map_err(to_js_error)?;
        Ok(Tool { inner })
    }

    #[napi]
    pub fn register_resource(&self, spec: ResourceSpecInit, reader: WeakTsfn<Read>) -> Result<Resource, String> {
        let inner = self
            .inner
            .register_resource(spec.into(), Arc::new(JsResourceReader { tsfn: reader }))
            .map_err(to_js_error)?;
        Ok(Resource { inner })
    }

    #[napi]
    pub fn create_scope(&self, name: String) -> Result<Scope, String> {
        let inner = self.inner.create_scope(&name).map_err(to_js_error)?;
        Ok(Scope { inner })
    }

    #[napi]
    pub fn dispose(&self) {
        self.inner.dispose();
    }
}

/// 原生客户端。构造时启动后台运行时线程（不连接），`start()` 后开始连接 Host。
#[napi(js_name = "NativeClient")]
pub struct JsNativeClient {
    inner: native::NativeClient,
    listener: Option<Arc<JsClientListener>>,
}

#[napi]
impl JsNativeClient {
    /// `listener(event)` 在 Node 事件循环上接收状态、配对与日志事件。
    #[napi(constructor)]
    pub fn new(config: ClientConfig, listener: Option<WeakTsfn<ClientEvent>>) -> Result<Self, String> {
        let mut cfg = native::NativeConfig::new(config.app_id, config.app_name);
        cfg.instance_id = config.instance_id;
        if let Some(kind) = config.client_kind.as_deref() {
            cfg.client_kind = parse_client_kind(kind)?;
        }
        if let Some(url) = config.host_url {
            cfg.host_url = url;
        }
        cfg.app_version = config.app_version;
        cfg.instance_title = config.instance_title;
        cfg.token = config.token;
        cfg.launch_token = config.launch_token;
        if let Some(n) = config.max_concurrent_calls {
            cfg.max_concurrent_calls = n;
        }
        cfg.overview = config.overview.map(Into::into);
        if let Some(lifecycle) = config.lifecycle {
            cfg.lifecycle = lifecycle.into_policy()?;
        }
        if let Some(ms) = config.connect_timeout_ms {
            cfg.connect_timeout_ms = ms;
        }

        let listener = listener.map(|tsfn| Arc::new(JsClientListener { tsfn: Mutex::new(Some(Arc::new(tsfn))) }));
        let dyn_listener = listener.clone().map(|l| l as Arc<dyn native::ClientListener>);
        let inner = native::NativeClient::new(cfg, dyn_listener).map_err(to_js_error)?;
        Ok(Self { inner, listener })
    }

    #[napi(getter)]
    pub fn instance_id(&self) -> String {
        self.inner.instance_id()
    }

    #[napi(getter)]
    pub fn state(&self) -> JsStateInfo {
        self.inner.state().into()
    }

    /// Host 为当前连接分配的连接 ID（spec/protocol.md 10.3）；未连接时为 `undefined`。
    #[napi(getter)]
    pub fn connection_id(&self) -> Option<String> {
        self.inner.connection_id()
    }

    /// 当前 token（配置带入的或配对后获得的）。
    #[napi(getter)]
    pub fn token(&self) -> Option<String> {
        self.inner.token()
    }

    #[napi]
    pub fn start(&self) {
        self.inner.start();
    }

    /// 停止：取消所有调用、断开连接、不再重连，并释放监听器的 ThreadsafeFunction。
    ///
    /// 停止后不再投递状态事件（`stopped` 状态由 JS 封装层自行设置）。
    #[napi]
    pub fn stop(&self) {
        self.inner.stop();
        if let Some(listener) = &self.listener {
            listener.release();
        }
    }

    // ---- 生命周期（spec/lifecycle.md 第 8 节）----------------------------

    /// 处理 OS 激活参数 / URL（`app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=`、`#app-mcp-wake=`）。
    /// 不是本 SDK 的唤醒返回 `false`。可在 `start()` 之前调用。
    #[napi]
    pub fn handle_wake(&self, args: String) -> bool {
        self.inner.handle_wake(&args)
    }

    /// App 主动回连。`reason` 缺省 `'app'`（窗口重新可见时用 `'visible'`）。返回是否因此发起了回连。
    #[napi]
    pub fn wake(&self, reason: Option<String>) -> Result<bool, String> {
        Ok(match reason.as_deref() {
            None => self.inner.wake(),
            Some(r) => self.inner.wake_with_reason(parse_wake_reason(r)?),
        })
    }

    /// `on-demand` 模式下主动连接；尚未 `start()` 时等同于 `start()`。
    #[napi]
    pub fn connect_now(&self) -> bool {
        self.inner.connect_now()
    }

    /// 主动请求休眠。`reason` 缺省 `'app'`（进入后台时用 `'background'`）。返回是否有效果。
    #[napi]
    pub fn sleep(&self, reason: Option<String>) -> Result<bool, String> {
        Ok(match reason.as_deref() {
            None => self.inner.sleep(),
            Some(r) => self.inner.sleep_with_reason(parse_sleep_reason(r)?),
        })
    }

    /// 临时阻止自动休眠，直到返回的 `Hold` 被 `release()`。
    #[napi]
    pub fn hold(&self) -> Hold {
        Hold { inner: self.inner.hold() }
    }

    /// 当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）。
    #[napi]
    pub fn tools_hash(&self) -> String {
        self.inner.tools_hash()
    }

    /// 测试用：tokio 运行时当前是否存在（休眠时为 `false`）。
    #[napi]
    pub fn runtime_active(&self) -> bool {
        self.inner.runtime_active()
    }

    #[napi]
    pub fn set_visibility(&self, visibility: String, focused: bool) -> Result<(), String> {
        self.inner.set_visibility(parse_visibility(&visibility)?, focused);
        Ok(())
    }

    #[napi]
    pub fn register_tool(&self, spec: ToolSpecInit, handler: WeakTsfn<Call>) -> Result<Tool, String> {
        let spec = spec.into_spec()?;
        let inner =
            self.inner.register_tool(spec, Arc::new(JsToolHandler { tsfn: handler })).map_err(to_js_error)?;
        Ok(Tool { inner })
    }

    #[napi]
    pub fn register_resource(&self, spec: ResourceSpecInit, reader: WeakTsfn<Read>) -> Result<Resource, String> {
        let inner = self
            .inner
            .register_resource(spec.into(), Arc::new(JsResourceReader { tsfn: reader }))
            .map_err(to_js_error)?;
        Ok(Resource { inner })
    }

    #[napi]
    pub fn create_scope(&self, name: String) -> Result<Scope, String> {
        let inner = self.inner.create_scope(&name).map_err(to_js_error)?;
        Ok(Scope { inner })
    }
}
