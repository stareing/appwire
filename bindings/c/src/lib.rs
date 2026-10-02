//! C ABI 实现，契约见 `include/app_mcp.h`。
//!
//! - 所有入口用 [`status::guard`] 包住：错误写入线程局部存储（`am_last_error_message`），
//!   panic 转为 `AM_ERR_PANIC`，不会跨越 FFI 边界。
//! - 回调包装见 [`callbacks`]；句柄定义见 [`handles`]。
//! - `user_data` 的所有权：传入 `am_client_new` / `am_tool_register` / `am_resource_register` /
//!   `am_call_set_cancel_callback` 后，无论成功与否都归库所有，库在不再使用时调用 `free_user_data`
//!   （失败时在函数返回前调用）。
//!
//! 各函数的安全约定（指针有效性、所有权）统一写在头文件中，这里不再逐个重复。
#![allow(clippy::missing_safety_doc)]

mod callbacks;
mod handles;
mod status;
mod strings;

use std::ffi::{c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use app_mcp_native::{
    Activation, AppOverview, CallDedupPolicy, CallResult, ClientKind, ClientListener, ContentAnnotations, ErrorKind, HeartbeatMode,
    LifecycleMode, LifecyclePolicy, NativeClient, NativeConfig, Residency, ResourceOptions, ResourceSpec, ResultStatus,
    Risk, SleepReason, ToolAnnotations, ToolOptions, ToolSpec, ToolSurface, Visibility, WakeDescriptor, WakeKind,
    WakeReason,
};

pub use callbacks::{
    AmCancelFn, AmCancelReason, AmFreeFn, AmIdleExitFn, AmLogFn, AmLogLevel, AmNavigateFn, AmPairedFn, AmReadFn,
    AmStateFn, AmStateStatus, AmToolFn,
};
use callbacks::{CCancelListener, CClientListener, CNavigationHandler, CResourceReader, CToolHandler, UserData};
use handles::ScopeKind;
pub use handles::{AmCall, AmClient, AmHold, AmNavigate, AmRead, AmResource, AmScope, AmTool};
pub use status::AmStatus;
use status::{FfiError, FfiResult, guard, guard_value, last_error_ptr};
use strings::{into_raw_cstring, lossy_str, opt_str, req_str};

/// 与头文件 `AM_API_VERSION` 一致。
pub const AM_API_VERSION: u32 = 3;

// ---------------------------------------------------------------------------
// 配置与定义（与头文件逐字段对应；枚举字段用 c_int 接收，避免非法值造成未定义行为）
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct AmClientConfig {
    pub app_id: *const c_char,
    pub app_name: *const c_char,
    pub instance_id: *const c_char,
    pub host_url: *const c_char,
    pub app_version: *const c_char,
    pub instance_title: *const c_char,
    pub token: *const c_char,
    pub launch_token: *const c_char,
    pub client_kind: c_int,
    pub max_concurrent_calls: u32,
    pub overview_summary: *const c_char,
    pub overview_body: *const c_char,
    pub overview_locale: *const c_char,
}

#[repr(C)]
pub struct AmClientCallbacks {
    pub on_state: Option<AmStateFn>,
    pub on_paired: Option<AmPairedFn>,
    pub on_log: Option<AmLogFn>,
    pub user_data: *mut c_void,
    pub free_user_data: Option<AmFreeFn>,
}

/// v3：生命周期策略（枚举字段用 c_int 接收）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AmLifecycle {
    pub mode: c_int,
    pub idle_timeout_ms: u64,
    pub hidden_idle_timeout_ms: u64,
    pub grace_ms: u64,
    pub residency: c_int,
    pub wake_kind: c_int,
    pub wake_target: *const c_char,
    pub wake_background: bool,
}

/// v3：`am_client_new_ex` 的扩展选项（带 `struct_size`，按调用方给出的大小读取）。
#[repr(C)]
pub struct AmClientOptions {
    pub struct_size: u32,
    pub lifecycle: *const AmLifecycle,
    pub connect_timeout_ms: u32,
    pub on_idle_exit: Option<AmIdleExitFn>,
    /// v7（4e，spec/lifecycle.md 第 11 节）：`AmHeartbeatMode`，0 = AUTO。
    pub heartbeat: c_int,
    /// v7：连续多少次"Host 不在"后转休眠；0 = 默认（3），负数 = 一直重连。
    pub host_absent_retries: i32,
    /// v7：回退到 4e 之前的定时器行为。
    pub legacy_timers: bool,
    /// v8（4e 第二部分，spec/lifecycle.md 第 13 节 B1）：调用后的合并窗口；0 = 默认（2000），负数 = 不留窗口。
    pub merge_window_ms: i64,
    /// v8（B4）：进入后台且空闲时立即休眠。
    pub sleep_on_background: bool,
    /// v13（spec/protocol.md 3.3 调用去重）：首次结果的保留时长；0 = 默认（300000），负数 = 关闭去重。
    pub call_dedup_ttl_ms: i64,
    /// v13：最多保留的结果数；0 = 默认（64），负数 = 关闭去重。
    pub call_dedup_max_entries: i32,
}

/// v8：`am_resource_register_ex` 的资源选项（带 `struct_size`，按调用方给出的大小读取）。
#[repr(C)]
pub struct AmResourceOptions {
    pub struct_size: u32,
    /// 需实时推送（spec/lifecycle.md 第 13 节 B3）。
    pub realtime: bool,
    /// v13：资源内容的标注（MCP 内容注解 JSON 对象）；NULL = 未声明。
    pub annotations_json: *const c_char,
}

#[repr(C)]
pub struct AmToolSpec {
    pub name: *const c_char,
    pub description: *const c_char,
    pub input_schema_json: *const c_char,
    pub risk: c_int,
    pub activation: c_int,
    pub title: *const c_char,
    pub enabled: bool,
}

/// v9：`am_tool_register_ex` / `am_tool_update_ex` 的工具选项（带 `struct_size`，按调用方给出的大小读取）。
#[repr(C)]
pub struct AmToolOptions {
    pub struct_size: u32,
    /// 标准 MCP 工具注解（JSON 对象文本）；NULL = 未声明。
    pub annotations_json: *const c_char,
    /// 结果的 JSON Schema 文本（MCP `outputSchema`）；NULL = 未声明。
    pub output_schema_json: *const c_char,
    /// v14：所在页面名；NULL = 未声明。
    pub page: *const c_char,
    /// v14：`AmToolSurface`（0 = APP，1 = VIEW）。
    pub surface: c_int,
    /// v15：App 在后台时代替本工具（view 工具）调用的同 App app 工具本地名；NULL = 未声明。
    pub background_tool: *const c_char,
}

/// v9：`am_call_complete_ex` 的调用结果（带 `struct_size`，按调用方给出的大小读取；`status` 用 c_int 接收）。
#[repr(C)]
pub struct AmCallResult {
    pub struct_size: u32,
    pub data_json: *const c_char,
    pub state_hints: *const *const c_char,
    pub state_hints_len: usize,
    pub status: c_int,
    pub state_resource: *const c_char,
    pub summary: *const c_char,
    /// 内容注解（MCP `Annotations`，JSON 对象文本）；NULL = 无。
    pub annotations_json: *const c_char,
}

#[repr(C)]
pub struct AmResourceSpec {
    pub name: *const c_char,
    pub description: *const c_char,
    pub mime_type: *const c_char,
}

// ---------------------------------------------------------------------------
// 枚举转换
// ---------------------------------------------------------------------------

fn client_kind_from(v: c_int) -> FfiResult<ClientKind> {
    match v {
        0 => Ok(ClientKind::Native),
        1 => Ok(ClientKind::Hybrid),
        _ => Err(FfiError::invalid_argument(format!(
            "非法的 client_kind：{v}"
        ))),
    }
}

fn risk_from(v: c_int) -> FfiResult<Risk> {
    match v {
        0 => Ok(Risk::Read),
        1 => Ok(Risk::Write),
        2 => Ok(Risk::Destructive),
        3 => Ok(Risk::Payment),
        4 => Ok(Risk::OsSensitive),
        _ => Err(FfiError::invalid_argument(format!("非法的 risk：{v}"))),
    }
}

fn result_status_from(v: c_int) -> FfiResult<ResultStatus> {
    match v {
        0 => Ok(ResultStatus::Done),
        1 => Ok(ResultStatus::Pending),
        2 => Ok(ResultStatus::Partial),
        3 => Ok(ResultStatus::Noop),
        _ => Err(FfiError::invalid_argument(format!("非法的 status：{v}"))),
    }
}

fn activation_from(v: c_int) -> FfiResult<Option<Activation>> {
    match v {
        -1 => Ok(None),
        0 => Ok(Some(Activation::Headless)),
        1 => Ok(Some(Activation::Background)),
        2 => Ok(Some(Activation::Foreground)),
        _ => Err(FfiError::invalid_argument(format!(
            "非法的 activation：{v}"
        ))),
    }
}

fn visibility_from(v: c_int) -> FfiResult<Visibility> {
    match v {
        0 => Ok(Visibility::Visible),
        1 => Ok(Visibility::Hidden),
        2 => Ok(Visibility::Frozen),
        _ => Err(FfiError::invalid_argument(format!(
            "非法的 visibility：{v}"
        ))),
    }
}

fn lifecycle_mode_from(v: c_int) -> FfiResult<LifecycleMode> {
    match v {
        0 => Ok(LifecycleMode::Persistent),
        1 => Ok(LifecycleMode::Idle),
        2 => Ok(LifecycleMode::OnDemand),
        _ => Err(FfiError::invalid_argument(format!(
            "非法的 lifecycle mode：{v}"
        ))),
    }
}

fn residency_from(v: c_int) -> FfiResult<Residency> {
    match v {
        0 => Ok(Residency::Keep),
        1 => Ok(Residency::ExitWhenIdle),
        2 => Ok(Residency::ExitAlways),
        _ => Err(FfiError::invalid_argument(format!("非法的 residency：{v}"))),
    }
}

/// `-1`（`AM_WAKE_UNSET`）→ `None`。
fn wake_kind_from(v: c_int) -> FfiResult<Option<WakeKind>> {
    match v {
        -1 => Ok(None),
        0 => Ok(Some(WakeKind::None)),
        1 => Ok(Some(WakeKind::Uri)),
        2 => Ok(Some(WakeKind::Aumid)),
        3 => Ok(Some(WakeKind::AppleEvent)),
        4 => Ok(Some(WakeKind::Dbus)),
        5 => Ok(Some(WakeKind::AndroidIntent)),
        6 => Ok(Some(WakeKind::WebUrl)),
        _ => Err(FfiError::invalid_argument(format!("非法的 wake_kind：{v}"))),
    }
}

fn wake_reason_from(v: c_int) -> FfiResult<WakeReason> {
    match v {
        0 => Ok(WakeReason::OsActivation),
        1 => Ok(WakeReason::App),
        2 => Ok(WakeReason::Visible),
        3 => Ok(WakeReason::ColdStart),
        _ => Err(FfiError::invalid_argument(format!(
            "非法的 wake reason：{v}"
        ))),
    }
}

fn sleep_reason_from(v: c_int) -> FfiResult<SleepReason> {
    match v {
        0 => Ok(SleepReason::Idle),
        1 => Ok(SleepReason::Grace),
        2 => Ok(SleepReason::Background),
        3 => Ok(SleepReason::App),
        _ => Err(FfiError::invalid_argument(format!(
            "非法的 sleep reason：{v}"
        ))),
    }
}

fn heartbeat_mode_from(v: c_int) -> FfiResult<HeartbeatMode> {
    match v {
        0 => Ok(HeartbeatMode::Auto),
        1 => Ok(HeartbeatMode::Always),
        2 => Ok(HeartbeatMode::Off),
        _ => Err(FfiError::invalid_argument(format!("非法的 heartbeat：{v}"))),
    }
}

unsafe fn convert_lifecycle(l: &AmLifecycle) -> FfiResult<LifecyclePolicy> {
    let wake = match wake_kind_from(l.wake_kind)? {
        None => None,
        Some(kind) => Some(WakeDescriptor {
            kind,
            // SAFETY: 调用方保证 wake_target 为 NULL 或有效的 C 字符串。
            target: unsafe { opt_str(l.wake_target, "lifecycle->wake_target") }?.map(str::to_owned),
            background: l.wake_background,
        }),
    };
    Ok(LifecyclePolicy {
        mode: lifecycle_mode_from(l.mode)?,
        idle_timeout_ms: l.idle_timeout_ms,
        hidden_idle_timeout_ms: l.hidden_idle_timeout_ms,
        grace_ms: l.grace_ms,
        residency: residency_from(l.residency)?,
        wake,
        ..LifecyclePolicy::default()
    })
}

/// 从调用方给出的 `AmClientOptions` 中读出的字段（按 `struct_size` 截断）。
#[derive(Default)]
struct OptionsView {
    lifecycle: *const AmLifecycle,
    connect_timeout_ms: u32,
    on_idle_exit: Option<AmIdleExitFn>,
    heartbeat: c_int,
    host_absent_retries: i32,
    legacy_timers: bool,
    merge_window_ms: i64,
    sleep_on_background: bool,
    call_dedup_ttl_ms: i64,
    call_dedup_max_entries: i32,
}

/// 按 `struct_size` 读取扩展选项：只读取完整包含在调用方结构体中的字段。
unsafe fn read_options(p: *const AmClientOptions) -> FfiResult<OptionsView> {
    use std::mem::{offset_of, size_of};
    let mut view = OptionsView {
        lifecycle: std::ptr::null(),
        ..OptionsView::default()
    };
    if p.is_null() {
        return Ok(view);
    }
    // SAFETY: 调用方保证 p 指向至少 struct_size 字节（struct_size 字段本身总在开头）。
    let size = unsafe { std::ptr::addr_of!((*p).struct_size).read_unaligned() } as usize;
    if size < size_of::<u32>() {
        return Err(FfiError::invalid_argument(
            "options->struct_size 必须设为 sizeof(AmClientOptions)",
        ));
    }
    let has = |offset: usize, len: usize| size >= offset + len;
    if has(
        offset_of!(AmClientOptions, lifecycle),
        size_of::<*const AmLifecycle>(),
    ) {
        view.lifecycle = unsafe { std::ptr::addr_of!((*p).lifecycle).read() };
    }
    if has(
        offset_of!(AmClientOptions, connect_timeout_ms),
        size_of::<u32>(),
    ) {
        view.connect_timeout_ms = unsafe { std::ptr::addr_of!((*p).connect_timeout_ms).read() };
    }
    if has(
        offset_of!(AmClientOptions, on_idle_exit),
        size_of::<Option<AmIdleExitFn>>(),
    ) {
        view.on_idle_exit = unsafe { std::ptr::addr_of!((*p).on_idle_exit).read() };
    }
    if has(offset_of!(AmClientOptions, heartbeat), size_of::<c_int>()) {
        view.heartbeat = unsafe { std::ptr::addr_of!((*p).heartbeat).read() };
    }
    if has(offset_of!(AmClientOptions, host_absent_retries), size_of::<i32>()) {
        view.host_absent_retries = unsafe { std::ptr::addr_of!((*p).host_absent_retries).read() };
    }
    if has(offset_of!(AmClientOptions, legacy_timers), size_of::<bool>()) {
        view.legacy_timers = unsafe { std::ptr::addr_of!((*p).legacy_timers).read() };
    }
    if has(offset_of!(AmClientOptions, merge_window_ms), size_of::<i64>()) {
        view.merge_window_ms = unsafe { std::ptr::addr_of!((*p).merge_window_ms).read() };
    }
    if has(offset_of!(AmClientOptions, sleep_on_background), size_of::<bool>()) {
        view.sleep_on_background = unsafe { std::ptr::addr_of!((*p).sleep_on_background).read() };
    }
    if has(offset_of!(AmClientOptions, call_dedup_ttl_ms), size_of::<i64>()) {
        view.call_dedup_ttl_ms = unsafe { std::ptr::addr_of!((*p).call_dedup_ttl_ms).read() };
    }
    if has(offset_of!(AmClientOptions, call_dedup_max_entries), size_of::<i32>()) {
        view.call_dedup_max_entries = unsafe { std::ptr::addr_of!((*p).call_dedup_max_entries).read() };
    }
    Ok(view)
}

/// v13 调用去重：0 = 保留默认值，负数 = 关闭（对应字段取 0），正数按字面使用。
fn call_dedup_from(base: CallDedupPolicy, ttl_ms: i64, max_entries: i32) -> CallDedupPolicy {
    let ttl_ms = match ttl_ms {
        0 => base.ttl_ms,
        n if n < 0 => 0,
        n => n.unsigned_abs(),
    };
    let max_entries = match max_entries {
        0 => base.max_entries,
        n if n < 0 => 0,
        n => n.unsigned_abs() as usize,
    };
    CallDedupPolicy { ttl_ms, max_entries }
}

/// 按 `struct_size` 读取资源选项；`p` 为 NULL 时取默认值。
unsafe fn read_resource_options(p: *const AmResourceOptions) -> FfiResult<ResourceOptions> {
    use std::mem::{offset_of, size_of};
    let mut options = ResourceOptions::default();
    if p.is_null() {
        return Ok(options);
    }
    // SAFETY: 调用方保证 p 指向至少 struct_size 字节（struct_size 字段本身总在开头）。
    let size = unsafe { std::ptr::addr_of!((*p).struct_size).read_unaligned() } as usize;
    if size < size_of::<u32>() {
        return Err(FfiError::invalid_argument(
            "options->struct_size 必须设为 sizeof(AmResourceOptions)",
        ));
    }
    if offset_of!(AmResourceOptions, realtime) + size_of::<bool>() <= size {
        options.realtime = unsafe { std::ptr::addr_of!((*p).realtime).read() };
    }
    if offset_of!(AmResourceOptions, annotations_json) + size_of::<*const c_char>() <= size {
        let text = unsafe { std::ptr::addr_of!((*p).annotations_json).read() };
        options.annotations = unsafe { opt_json::<ContentAnnotations>(text, "options->annotations_json") }?;
    }
    Ok(options)
}

/// 读取 `struct_size`：至少要含 `struct_size` 字段本身。
///
/// # Safety
/// `p` 非空，指向以 `u32` 的 `struct_size` 开头的结构体。
unsafe fn struct_size_of(p: *const u32, what: &str) -> FfiResult<usize> {
    // SAFETY: 由调用方保证。
    let size = unsafe { p.read_unaligned() } as usize;
    if size < std::mem::size_of::<u32>() {
        return Err(FfiError::invalid_argument(format!("{what}->struct_size 必须设为 sizeof 结构体")));
    }
    Ok(size)
}

/// 可为 NULL 的 JSON 文本 → `T`（`null` 文本也视为未提供）；非法 UTF-8 / JSON 返回 `AM_ERR_INVALID_JSON`。
///
/// # Safety
/// `p` 为 NULL 或有效的 C 字符串。
unsafe fn opt_json<T: serde::de::DeserializeOwned>(p: *const c_char, what: &str) -> FfiResult<Option<T>> {
    let text = unsafe { opt_str(p, what) }
        .map_err(|_| FfiError::new(AmStatus::InvalidJson, format!("{what} 不是合法的 UTF-8")))?;
    let Some(text) = text else { return Ok(None) };
    serde_json::from_str::<Option<T>>(text)
        .map_err(|e| FfiError::new(AmStatus::InvalidJson, format!("{what} 不合法：{e}")))
}

/// 按 `struct_size` 读取工具选项；`p` 为 NULL 时为空选项（不声明 / 清除）。
unsafe fn read_tool_options(p: *const AmToolOptions) -> FfiResult<ToolOptions> {
    use std::mem::{offset_of, size_of};
    let mut options = ToolOptions::default();
    if p.is_null() {
        return Ok(options);
    }
    // SAFETY: 调用方保证 p 指向至少 struct_size 字节（struct_size 字段本身总在开头）。
    let size = unsafe { struct_size_of(std::ptr::addr_of!((*p).struct_size), "options") }?;
    let has = |offset: usize| size >= offset + size_of::<*const c_char>();
    if has(offset_of!(AmToolOptions, annotations_json)) {
        let text = unsafe { std::ptr::addr_of!((*p).annotations_json).read() };
        options.annotations = unsafe { opt_json::<ToolAnnotations>(text, "options->annotations_json") }?;
    }
    if has(offset_of!(AmToolOptions, output_schema_json)) {
        let text = unsafe { std::ptr::addr_of!((*p).output_schema_json).read() };
        options.output_schema_json =
            unsafe { opt_str(text, "options->output_schema_json") }?.map(str::to_owned);
    }
    if has(offset_of!(AmToolOptions, page)) {
        let text = unsafe { std::ptr::addr_of!((*p).page).read() };
        options.page = unsafe { opt_str(text, "options->page") }?.map(str::to_owned);
    }
    if size >= offset_of!(AmToolOptions, surface) + size_of::<c_int>() {
        let surface = unsafe { std::ptr::addr_of!((*p).surface).read() };
        options.surface = match surface {
            0 => ToolSurface::App,
            1 => ToolSurface::View,
            other => return Err(FfiError::invalid_argument(format!("options->surface 取值无效：{other}"))),
        };
    }
    if has(offset_of!(AmToolOptions, background_tool)) {
        let text = unsafe { std::ptr::addr_of!((*p).background_tool).read() };
        options.background_tool = unsafe { opt_str(text, "options->background_tool") }?.map(str::to_owned);
    }
    Ok(options)
}

/// 按 `struct_size` 读取调用结果；`p` 为 NULL 时为默认结果（无返回值、done）。
/// 只有 `AM_ERR_INVALID_JSON` 表示可重试（调用不应被消费）。
unsafe fn read_call_result(p: *const AmCallResult) -> FfiResult<CallResult> {
    use std::mem::{offset_of, size_of};
    let mut result = CallResult::default();
    if p.is_null() {
        return Ok(result);
    }
    // SAFETY（本函数内各处）：调用方保证 p 指向至少 struct_size 字节，字符串字段为 NULL 或有效的 C 字符串。
    let size = unsafe { struct_size_of(std::ptr::addr_of!((*p).struct_size), "result") }?;
    let has = |offset: usize, len: usize| size >= offset + len;
    let ptr_len = size_of::<*const c_char>();
    if has(offset_of!(AmCallResult, data_json), ptr_len) {
        let text = unsafe { std::ptr::addr_of!((*p).data_json).read() };
        result.data_json = unsafe { opt_str(text, "result->data_json") }
            .map_err(|_| FfiError::new(AmStatus::InvalidJson, "result->data_json 不是合法的 UTF-8"))?
            .map(str::to_owned);
    }
    if has(offset_of!(AmCallResult, annotations_json), ptr_len) {
        let text = unsafe { std::ptr::addr_of!((*p).annotations_json).read() };
        result.annotations = unsafe { opt_json::<ContentAnnotations>(text, "result->annotations_json") }?;
    }
    if has(offset_of!(AmCallResult, state_hints_len), size_of::<usize>()) {
        let hints = unsafe { std::ptr::addr_of!((*p).state_hints).read() };
        let len = unsafe { std::ptr::addr_of!((*p).state_hints_len).read() };
        result.state_hints = unsafe { read_hints(hints, len) }?;
    }
    if has(offset_of!(AmCallResult, status), size_of::<c_int>()) {
        result.status = result_status_from(unsafe { std::ptr::addr_of!((*p).status).read() })?;
    }
    if has(offset_of!(AmCallResult, state_resource), ptr_len) {
        let text = unsafe { std::ptr::addr_of!((*p).state_resource).read() };
        result.state_resource = unsafe { opt_str(text, "result->state_resource") }?.map(str::to_owned);
    }
    if has(offset_of!(AmCallResult, summary), ptr_len) {
        let text = unsafe { std::ptr::addr_of!((*p).summary).read() };
        result.summary = unsafe { opt_str(text, "result->summary") }?.map(str::to_owned);
    }
    Ok(result)
}

impl Default for AmLifecycle {
    fn default() -> Self {
        let d = LifecyclePolicy::default();
        Self {
            mode: 0,
            idle_timeout_ms: d.idle_timeout_ms,
            hidden_idle_timeout_ms: d.hidden_idle_timeout_ms,
            grace_ms: d.grace_ms,
            residency: 0,
            wake_kind: -1,
            wake_target: std::ptr::null(),
            wake_background: false,
        }
    }
}

/// 协议错误类别字符串 → [`ErrorKind`]；NULL 或未知值按 `HANDLER_ERROR` 处理。
fn error_kind_from(kind: Option<&str>) -> ErrorKind {
    kind.and_then(|k| serde_json::from_value(serde_json::Value::String(k.to_owned())).ok())
        .unwrap_or(ErrorKind::HandlerError)
}

unsafe fn convert_config(c: &AmClientConfig) -> FfiResult<NativeConfig> {
    // SAFETY（本函数内各处）：字符串字段由调用方保证为 NULL 或有效的 C 字符串。
    let app_id = unsafe { req_str(c.app_id, "config->app_id") }?;
    let app_name = unsafe { req_str(c.app_name, "config->app_name") }?;
    let mut cfg = NativeConfig::new(app_id, app_name);
    cfg.instance_id = unsafe { opt_str(c.instance_id, "config->instance_id") }?.map(str::to_owned);
    if let Some(url) = unsafe { opt_str(c.host_url, "config->host_url") }? {
        cfg.host_url = url.to_owned();
    }
    cfg.app_version = unsafe { opt_str(c.app_version, "config->app_version") }?.map(str::to_owned);
    cfg.instance_title =
        unsafe { opt_str(c.instance_title, "config->instance_title") }?.map(str::to_owned);
    cfg.token = unsafe { opt_str(c.token, "config->token") }?.map(str::to_owned);
    cfg.launch_token =
        unsafe { opt_str(c.launch_token, "config->launch_token") }?.map(str::to_owned);
    cfg.client_kind = client_kind_from(c.client_kind)?;
    cfg.max_concurrent_calls = if c.max_concurrent_calls == 0 {
        1
    } else {
        c.max_concurrent_calls
    };
    // overview_summary 为 NULL 时没有总览，body / locale 被忽略。
    if let Some(summary) = unsafe { opt_str(c.overview_summary, "config->overview_summary") }? {
        cfg.overview = Some(AppOverview {
            summary: summary.to_owned(),
            body: unsafe { opt_str(c.overview_body, "config->overview_body") }?.map(str::to_owned),
            locale: unsafe { opt_str(c.overview_locale, "config->overview_locale") }?
                .map(str::to_owned),
        });
    }
    Ok(cfg)
}

/// `name_override` 不为 `None` 时忽略 `spec->name`（用于 `am_tool_update`）。
unsafe fn convert_tool_spec(s: &AmToolSpec, name_override: Option<String>) -> FfiResult<ToolSpec> {
    let name = match name_override {
        Some(n) => n,
        None => unsafe { req_str(s.name, "spec->name") }?.to_owned(),
    };
    let description = unsafe { req_str(s.description, "spec->description") }?;
    let mut spec = ToolSpec::new(name, description);
    spec.input_schema_json =
        unsafe { opt_str(s.input_schema_json, "spec->input_schema_json") }?.map(str::to_owned);
    spec.risk = risk_from(s.risk)?;
    spec.activation = activation_from(s.activation)?;
    spec.title = unsafe { opt_str(s.title, "spec->title") }?.map(str::to_owned);
    spec.enabled = s.enabled;
    Ok(spec)
}

unsafe fn convert_resource_spec(s: &AmResourceSpec) -> FfiResult<ResourceSpec> {
    Ok(ResourceSpec {
        name: unsafe { req_str(s.name, "spec->name") }?.to_owned(),
        description: unsafe { req_str(s.description, "spec->description") }?.to_owned(),
        mime_type: unsafe { opt_str(s.mime_type, "spec->mime_type") }?.map(str::to_owned),
    })
}

/// 检查输出参数非空并先置为 NULL。
unsafe fn prepare_out<T>(out: *mut *mut T) -> FfiResult<&'static mut *mut T> {
    // SAFETY: 调用方保证 out 为 NULL 或指向可写的指针。
    let out = unsafe { out.as_mut() }.ok_or_else(|| FfiError::null("out"))?;
    *out = std::ptr::null_mut();
    Ok(out)
}

unsafe fn read_hints(hints: *const *const c_char, len: usize) -> FfiResult<Vec<String>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if hints.is_null() {
        return Err(FfiError::null("state_hints"));
    }
    // SAFETY: 调用方保证 hints 指向 len 个元素。
    let slice = unsafe { std::slice::from_raw_parts(hints, len) };
    slice
        .iter()
        .enumerate()
        .map(|(i, p)| unsafe { req_str(*p, &format!("state_hints[{i}]")) }.map(str::to_owned))
        .collect()
}

/// 消费（释放）一个 Box 句柄；释放过程中的 panic 被吞掉。
unsafe fn consume<T>(p: *mut T) {
    // SAFETY: p 来自 Box::into_raw，且调用方保证不再使用。
    let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(p) })));
}

// ---------------------------------------------------------------------------
// 通用
// ---------------------------------------------------------------------------

static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

#[unsafe(no_mangle)]
pub extern "C" fn am_version() -> *const c_char {
    VERSION.as_ptr().cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn am_last_error_message() -> *const c_char {
    last_error_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    // SAFETY: s 由本库通过 CString::into_raw 返回。
    unsafe { consume_cstring(s) };
}

unsafe fn consume_cstring(s: *mut c_char) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(unsafe { std::ffi::CString::from_raw(s) })
    }));
}

// ---------------------------------------------------------------------------
// 客户端
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_new(
    config: *const AmClientConfig,
    callbacks: *const AmClientCallbacks,
    out: *mut *mut AmClient,
) -> AmStatus {
    unsafe { am_client_new_ex(config, callbacks, std::ptr::null(), out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_new_ex(
    config: *const AmClientConfig,
    callbacks: *const AmClientCallbacks,
    options: *const AmClientOptions,
    out: *mut *mut AmClient,
) -> AmStatus {
    guard(|| {
        // 先接管 user_data，保证任何失败路径都会调用 free_user_data。
        // SAFETY: callbacks 为 NULL 或指向有效结构体。
        let mut listener = unsafe { callbacks.as_ref() }.map(|cb| CClientListener {
            on_state: cb.on_state,
            on_paired: cb.on_paired,
            on_log: cb.on_log,
            on_idle_exit: None,
            user_data: UserData::new(cb.user_data, cb.free_user_data),
        });
        let out = unsafe { prepare_out(out) }?;
        // SAFETY: options 为 NULL 或指向至少 struct_size 字节。
        let opts = unsafe { read_options(options) }?;
        if let Some(f) = opts.on_idle_exit {
            listener
                .get_or_insert_with(|| CClientListener {
                    on_state: None,
                    on_paired: None,
                    on_log: None,
                    on_idle_exit: None,
                    user_data: UserData::new(std::ptr::null_mut(), None),
                })
                .on_idle_exit = Some(f);
        }
        // SAFETY: config 为 NULL 或指向有效结构体。
        let config = unsafe { config.as_ref() }.ok_or_else(|| FfiError::null("config"))?;
        let mut cfg = unsafe { convert_config(config) }?;
        // SAFETY: lifecycle 为 NULL 或指向有效结构体。
        if let Some(l) = unsafe { opts.lifecycle.as_ref() } {
            cfg.lifecycle = unsafe { convert_lifecycle(l) }?;
        }
        if opts.connect_timeout_ms != 0 {
            cfg.connect_timeout_ms = opts.connect_timeout_ms;
        }
        cfg.heartbeat = heartbeat_mode_from(opts.heartbeat)?;
        match opts.host_absent_retries {
            0 => {}
            n if n < 0 => cfg.lifecycle.host_absent_retries = 0,
            n => cfg.lifecycle.host_absent_retries = n.unsigned_abs(),
        }
        cfg.lifecycle.legacy_timers = opts.legacy_timers;
        match opts.merge_window_ms {
            0 => {}
            n if n < 0 => cfg.lifecycle.merge_window_ms = 0,
            n => cfg.lifecycle.merge_window_ms = n.unsigned_abs(),
        }
        cfg.lifecycle.sleep_on_background = opts.sleep_on_background;
        cfg.call_dedup = call_dedup_from(cfg.call_dedup, opts.call_dedup_ttl_ms, opts.call_dedup_max_entries);
        let listener: Option<Arc<dyn ClientListener>> = match listener {
            Some(l) if l.has_any() => Some(Arc::new(l)),
            _ => None,
        };
        let client = NativeClient::new(cfg, listener)?;
        *out = Box::into_raw(Box::new(AmClient::new(client)));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_free(client: *mut AmClient) {
    if client.is_null() {
        return;
    }
    let _ = guard(|| {
        // SAFETY: client 来自 am_client_new，调用方保证不再使用。
        let client = unsafe { Box::from_raw(client) };
        client.shared.shutdown();
        Ok(())
    });
}

unsafe fn client_ref<'a>(client: *const AmClient) -> FfiResult<&'a AmClient> {
    // SAFETY: client 为 NULL 或来自 am_client_new 且尚未释放。
    unsafe { client.as_ref() }.ok_or_else(|| FfiError::null("client"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_start(client: *mut AmClient) -> AmStatus {
    guard(|| {
        unsafe { client_ref(client) }?.shared.client()?.start();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_stop(client: *mut AmClient) -> AmStatus {
    guard(|| {
        unsafe { client_ref(client) }?.shared.client()?.stop();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_set_visibility(
    client: *mut AmClient,
    visibility: c_int,
    focused: bool,
) -> AmStatus {
    guard(|| {
        let c = unsafe { client_ref(client) }?;
        let v = visibility_from(visibility)?;
        c.shared.client()?.set_visibility(v, focused);
        Ok(())
    })
}

/// v14：设置导航回调；`handler` 为 NULL 时清除。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_set_navigation_handler(
    client: *mut AmClient,
    handler: Option<AmNavigateFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
) -> AmStatus {
    guard(|| {
        let ud = UserData::new(user_data, free_user_data);
        let c = unsafe { client_ref(client) }?;
        let handler = handler.map(|f| Arc::new(CNavigationHandler { f, user_data: ud }) as Arc<dyn app_mcp_native::NavigationHandler>);
        c.shared.client()?.set_navigation_handler(handler);
        Ok(())
    })
}

/// v15：App 在后台（Hidden / Frozen）时是否仍把导航请求交给导航回调；false 时直接以
/// `USER_ACTION_REQUIRED`（reason "foreground"）回复。默认值随平台（桌面 true，移动端 false）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_set_navigate_in_background(client: *mut AmClient, enabled: bool) -> AmStatus {
    guard(|| {
        unsafe { client_ref(client) }?.shared.client()?.set_navigate_in_background(enabled);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_state(
    client: *const AmClient,
    status: *mut AmStateStatus,
    retry_in_ms: *mut u64,
    reason: *mut *mut c_char,
) -> AmStatus {
    guard(|| {
        // SAFETY: 输出指针为 NULL 或可写。
        if !reason.is_null() {
            unsafe { *reason = std::ptr::null_mut() };
        }
        let c = unsafe { client_ref(client) }?;
        let status = unsafe { status.as_mut() }.ok_or_else(|| FfiError::null("status"))?;
        let st = c.shared.client()?.state();
        *status = st.status.into();
        if let Some(r) = unsafe { retry_in_ms.as_mut() } {
            *r = if st.status == app_mcp_native::StateStatus::Backoff {
                st.retry_in_ms.unwrap_or(0)
            } else {
                0
            };
        }
        if !reason.is_null() {
            let text = match st.status {
                app_mcp_native::StateStatus::Rejected | app_mcp_native::StateStatus::HostMismatch => {
                    Some(st.reason.unwrap_or_default())
                }
                app_mcp_native::StateStatus::Backoff => st.reason,
                _ => None,
            };
            if let Some(text) = text {
                unsafe { *reason = into_raw_cstring(&text) };
            }
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_instance_id(client: *const AmClient) -> *mut c_char {
    guard_value(std::ptr::null_mut(), || {
        let id = unsafe { client_ref(client) }?
            .shared
            .client()?
            .instance_id();
        Ok(into_raw_cstring(&id))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_token(client: *const AmClient) -> *mut c_char {
    guard_value(std::ptr::null_mut(), || {
        let token = unsafe { client_ref(client) }?.shared.client()?.token();
        Ok(token.map_or(std::ptr::null_mut(), |t| into_raw_cstring(&t)))
    })
}

/// v6：当前状态的错误码（spec/protocol.md 10.1）。
///
/// # Safety
/// `client` 为 NULL 或有效客户端；`code` 为 NULL 或可写。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_state_code(client: *const AmClient, code: *mut *mut c_char) -> AmStatus {
    guard(|| {
        // SAFETY: 输出指针为 NULL 或可写。
        let out = unsafe { code.as_mut() }.ok_or_else(|| FfiError::null("code"))?;
        *out = std::ptr::null_mut();
        let st = unsafe { client_ref(client) }?.shared.client()?.state();
        if let Some(c) = st.code {
            *out = into_raw_cstring(&c);
        }
        Ok(())
    })
}

/// v6：Host 为当前连接分配的连接 ID（spec/protocol.md 10.3）。
///
/// # Safety
/// `client` 为 NULL 或有效客户端；`id` 为 NULL 或可写。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_connection_id(client: *const AmClient, id: *mut *mut c_char) -> AmStatus {
    guard(|| {
        // SAFETY: 输出指针为 NULL 或可写。
        let out = unsafe { id.as_mut() }.ok_or_else(|| FfiError::null("id"))?;
        *out = std::ptr::null_mut();
        if let Some(cid) = unsafe { client_ref(client) }?.shared.client()?.connection_id() {
            *out = into_raw_cstring(&cid);
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// 客户端：生命周期（v3）
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_lifecycle_init(lifecycle: *mut AmLifecycle) {
    // SAFETY: lifecycle 为 NULL 或指向可写的结构体。
    if let Some(l) = unsafe { lifecycle.as_mut() } {
        *l = AmLifecycle::default();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_handle_wake(client: *mut AmClient, args: *const c_char) -> bool {
    guard_value(false, || {
        let c = unsafe { client_ref(client) }?;
        let args = unsafe { req_str(args, "args") }?;
        Ok(c.shared.client()?.handle_wake(args))
    })
}

/// 写入可选的 bool 输出参数。
unsafe fn write_flag(out: *mut bool, value: bool) {
    // SAFETY: out 为 NULL 或可写。
    if let Some(o) = unsafe { out.as_mut() } {
        *o = value;
    }
}

/// 执行一个返回 bool 的客户端操作，结果写入可选输出参数。
unsafe fn client_flag_op(
    client: *mut AmClient,
    out: *mut bool,
    f: impl FnOnce(&NativeClient) -> FfiResult<bool>,
) -> AmStatus {
    unsafe { write_flag(out, false) };
    guard(|| {
        let c = unsafe { client_ref(client) }?.shared.client()?;
        let v = f(&c)?;
        unsafe { write_flag(out, v) };
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_wake(client: *mut AmClient, started: *mut bool) -> AmStatus {
    unsafe { client_flag_op(client, started, |c| Ok(c.wake())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_wake_with_reason(
    client: *mut AmClient,
    reason: c_int,
    started: *mut bool,
) -> AmStatus {
    unsafe {
        client_flag_op(client, started, |c| {
            Ok(c.wake_with_reason(wake_reason_from(reason)?))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_connect_now(
    client: *mut AmClient,
    started: *mut bool,
) -> AmStatus {
    unsafe { client_flag_op(client, started, |c| Ok(c.connect_now())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_sleep(client: *mut AmClient, changed: *mut bool) -> AmStatus {
    unsafe { client_flag_op(client, changed, |c| Ok(c.sleep())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_sleep_with_reason(
    client: *mut AmClient,
    reason: c_int,
    changed: *mut bool,
) -> AmStatus {
    unsafe {
        client_flag_op(client, changed, |c| {
            Ok(c.sleep_with_reason(sleep_reason_from(reason)?))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_hold(client: *mut AmClient, out: *mut *mut AmHold) -> AmStatus {
    guard(|| {
        let out = unsafe { prepare_out(out) }?;
        let c = unsafe { client_ref(client) }?.shared.client()?;
        *out = Box::into_raw(Box::new(AmHold { handle: c.hold() }));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hold_release(hold: *mut AmHold) {
    if hold.is_null() {
        return;
    }
    let _ = guard(|| {
        // SAFETY: hold 来自本库，调用方保证不再使用。
        let h = unsafe { Box::from_raw(hold) };
        h.handle.release();
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_tools_hash(client: *const AmClient) -> *mut c_char {
    guard_value(std::ptr::null_mut(), || {
        let hash = unsafe { client_ref(client) }?.shared.client()?.tools_hash();
        Ok(into_raw_cstring(&hash))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_parse_wake_token(args: *const c_char) -> *mut c_char {
    guard_value(std::ptr::null_mut(), || {
        let args = unsafe { req_str(args, "args") }?;
        Ok(app_mcp_native::parse_wake_token(args)
            .map_or(std::ptr::null_mut(), |t| into_raw_cstring(&t)))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_root_scope(
    client: *mut AmClient,
    out: *mut *mut AmScope,
) -> AmStatus {
    guard(|| {
        let out = unsafe { prepare_out(out) }?;
        let c = unsafe { client_ref(client) }?;
        c.shared.check_alive()?;
        *out = Box::into_raw(Box::new(AmScope {
            kind: ScopeKind::Root,
            shared: c.shared.clone(),
        }));
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

unsafe fn scope_ref<'a>(scope: *const AmScope) -> FfiResult<&'a AmScope> {
    // SAFETY: scope 为 NULL 或有效句柄。
    unsafe { scope.as_ref() }.ok_or_else(|| FfiError::null("scope"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_scope_create(
    parent: *mut AmScope,
    name: *const c_char,
    out: *mut *mut AmScope,
) -> AmStatus {
    guard(|| {
        let out = unsafe { prepare_out(out) }?;
        let parent = unsafe { scope_ref(parent) }?;
        let name = unsafe { req_str(name, "name") }?;
        let handle = match &parent.kind {
            ScopeKind::Root => {
                let h = parent.shared.client()?.create_scope(name)?;
                parent.shared.track_scope(&h);
                h
            }
            ScopeKind::Child(s) => {
                parent.shared.check_alive()?;
                s.create_scope(name)?
            }
        };
        *out = Box::into_raw(Box::new(AmScope {
            kind: ScopeKind::Child(handle),
            shared: parent.shared.clone(),
        }));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_scope_dispose(scope: *mut AmScope) -> AmStatus {
    guard(|| {
        let scope = unsafe { scope_ref(scope) }?;
        scope.shared.check_alive()?;
        match &scope.kind {
            ScopeKind::Root => scope.shared.dispose_root(),
            ScopeKind::Child(s) => s.dispose(),
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_scope_free(scope: *mut AmScope) {
    if !scope.is_null() {
        // SAFETY: scope 来自本库，调用方保证不再使用。
        unsafe { consume(scope) };
    }
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_register(
    scope: *mut AmScope,
    spec: *const AmToolSpec,
    handler: Option<AmToolFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
    out: *mut *mut AmTool,
) -> AmStatus {
    // SAFETY: 参数约定与 am_tool_register_ex 相同，options 为 NULL。
    unsafe { am_tool_register_ex(scope, spec, std::ptr::null(), handler, user_data, free_user_data, out) }
}

/// v9：同 `am_tool_register`，另带工具选项（`options` 可为 NULL）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_register_ex(
    scope: *mut AmScope,
    spec: *const AmToolSpec,
    options: *const AmToolOptions,
    handler: Option<AmToolFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
    out: *mut *mut AmTool,
) -> AmStatus {
    guard(|| {
        let ud = UserData::new(user_data, free_user_data);
        let out = unsafe { prepare_out(out) }?;
        let scope = unsafe { scope_ref(scope) }?;
        let spec = unsafe { spec.as_ref() }.ok_or_else(|| FfiError::null("spec"))?;
        let f = handler.ok_or_else(|| FfiError::null("handler"))?;
        let spec = unsafe { convert_tool_spec(spec, None) }?;
        // SAFETY: options 为 NULL 或指向至少 struct_size 字节。
        let options = unsafe { read_tool_options(options) }?;
        let handler = Arc::new(CToolHandler { f, user_data: ud });
        let handle = match &scope.kind {
            ScopeKind::Root => {
                let h = scope.shared.client()?.register_tool_with(spec, options, handler)?;
                scope.shared.track_tool(&h);
                h
            }
            ScopeKind::Child(s) => {
                scope.shared.check_alive()?;
                s.register_tool_with(spec, options, handler)?
            }
        };
        *out = Box::into_raw(Box::new(AmTool {
            handle,
            shared: scope.shared.clone(),
        }));
        Ok(())
    })
}

unsafe fn tool_ref<'a>(tool: *const AmTool) -> FfiResult<&'a AmTool> {
    let t = unsafe { tool.as_ref() }.ok_or_else(|| FfiError::null("tool"))?;
    t.shared.check_alive()?;
    Ok(t)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_update(tool: *mut AmTool, spec: *const AmToolSpec) -> AmStatus {
    guard(|| {
        let tool = unsafe { tool_ref(tool) }?;
        let spec = unsafe { spec.as_ref() }.ok_or_else(|| FfiError::null("spec"))?;
        let spec = unsafe { convert_tool_spec(spec, Some(tool.handle.name())) }?;
        tool.handle.update(spec)?;
        Ok(())
    })
}

/// v9：用新定义与选项整体替换（`options` 为 NULL 或字段为 NULL 表示清除该声明）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_update_ex(
    tool: *mut AmTool,
    spec: *const AmToolSpec,
    options: *const AmToolOptions,
) -> AmStatus {
    guard(|| {
        let tool = unsafe { tool_ref(tool) }?;
        let spec = unsafe { spec.as_ref() }.ok_or_else(|| FfiError::null("spec"))?;
        let spec = unsafe { convert_tool_spec(spec, Some(tool.handle.name())) }?;
        // SAFETY: options 为 NULL 或指向至少 struct_size 字节。
        let options = unsafe { read_tool_options(options) }?;
        tool.handle.update_with(spec, options)?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_set_enabled(tool: *mut AmTool, enabled: bool) -> AmStatus {
    guard(|| {
        unsafe { tool_ref(tool) }?.handle.set_enabled(enabled)?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_dispose(tool: *mut AmTool) -> AmStatus {
    guard(|| {
        unsafe { tool_ref(tool) }?.handle.dispose();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_tool_free(tool: *mut AmTool) {
    if !tool.is_null() {
        unsafe { consume(tool) };
    }
}

// ---------------------------------------------------------------------------
// 资源
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_resource_register(
    scope: *mut AmScope,
    spec: *const AmResourceSpec,
    reader: Option<AmReadFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
    out: *mut *mut AmResource,
) -> AmStatus {
    // SAFETY: 参数约定与 am_resource_register_ex 相同，options 为 NULL。
    unsafe { am_resource_register_ex(scope, spec, std::ptr::null(), reader, user_data, free_user_data, out) }
}

/// v8：同 `am_resource_register`，另带资源选项（`options` 可为 NULL）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_resource_register_ex(
    scope: *mut AmScope,
    spec: *const AmResourceSpec,
    options: *const AmResourceOptions,
    reader: Option<AmReadFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
    out: *mut *mut AmResource,
) -> AmStatus {
    guard(|| {
        let ud = UserData::new(user_data, free_user_data);
        let out = unsafe { prepare_out(out) }?;
        let scope = unsafe { scope_ref(scope) }?;
        let spec = unsafe { spec.as_ref() }.ok_or_else(|| FfiError::null("spec"))?;
        let f = reader.ok_or_else(|| FfiError::null("reader"))?;
        let spec = unsafe { convert_resource_spec(spec) }?;
        // SAFETY: options 为 NULL 或指向至少 struct_size 字节。
        let options = unsafe { read_resource_options(options) }?;
        let reader = Arc::new(CResourceReader { f, user_data: ud });
        let handle = match &scope.kind {
            ScopeKind::Root => {
                let h = scope.shared.client()?.register_resource_with(spec, options, reader)?;
                scope.shared.track_resource(&h);
                h
            }
            ScopeKind::Child(s) => {
                scope.shared.check_alive()?;
                s.register_resource_with(spec, options, reader)?
            }
        };
        *out = Box::into_raw(Box::new(AmResource {
            handle,
            shared: scope.shared.clone(),
        }));
        Ok(())
    })
}

unsafe fn resource_ref<'a>(r: *const AmResource) -> FfiResult<&'a AmResource> {
    let r = unsafe { r.as_ref() }.ok_or_else(|| FfiError::null("resource"))?;
    r.shared.check_alive()?;
    Ok(r)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_resource_notify_changed(resource: *mut AmResource) -> AmStatus {
    guard(|| {
        unsafe { resource_ref(resource) }?.handle.notify_changed()?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_resource_dispose(resource: *mut AmResource) -> AmStatus {
    guard(|| {
        unsafe { resource_ref(resource) }?.handle.dispose();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_resource_free(resource: *mut AmResource) {
    if !resource.is_null() {
        unsafe { consume(resource) };
    }
}

// ---------------------------------------------------------------------------
// 调用
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_id(call: *const AmCall) -> *const c_char {
    // SAFETY: call 为 NULL 或尚未消费的调用。
    unsafe { call.as_ref() }.map_or(std::ptr::null(), |c| c.id.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_tool_name(call: *const AmCall) -> *const c_char {
    unsafe { call.as_ref() }.map_or(std::ptr::null(), |c| c.tool_name.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_arguments_json(call: *const AmCall) -> *const c_char {
    unsafe { call.as_ref() }.map_or(std::ptr::null(), |c| c.arguments.as_ptr())
}

/// v16：Agent 幂等键（spec/protocol.md 3.3）；没有时返回 NULL。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_idempotency_key(call: *const AmCall) -> *const c_char {
    unsafe { call.as_ref() }
        .and_then(|c| c.idempotency_key.as_ref())
        .map_or(std::ptr::null(), |k| k.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_is_cancelled(call: *const AmCall) -> bool {
    guard_value(false, || {
        Ok(unsafe { call.as_ref() }.is_some_and(|c| c.handle.is_cancelled()))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_set_cancel_callback(
    call: *mut AmCall,
    on_cancel: Option<AmCancelFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
) -> AmStatus {
    guard(|| {
        let ud = UserData::new(user_data, free_user_data);
        let call = unsafe { call.as_ref() }.ok_or_else(|| FfiError::null("call"))?;
        let f = on_cancel.ok_or_else(|| FfiError::null("on_cancel"))?;
        call.handle
            .set_cancel_listener(Arc::new(CCancelListener { f, user_data: ud }));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_complete(
    call: *mut AmCall,
    data_json: *const c_char,
    state_hints: *const *const c_char,
    state_hints_len: usize,
) -> AmStatus {
    if call.is_null() {
        return guard(|| Err(FfiError::null("call")));
    }
    let status = guard(|| {
        // SAFETY: call 非空且尚未消费。
        let c = unsafe { &*call };
        let data = unsafe { opt_str(data_json, "data_json") }
            .map_err(|_| FfiError::new(AmStatus::InvalidJson, "data_json 不是合法的 UTF-8"))?;
        let hints = match unsafe { read_hints(state_hints, state_hints_len) } {
            Ok(h) => h,
            Err(e) => {
                // call 仍会被消费，因此以 HANDLER_ERROR 结束调用，避免调用悬挂到超时。
                let _ = c.handle.fail(
                    ErrorKind::HandlerError,
                    &format!("state_hints 非法：{}", e.message),
                );
                return Err(e);
            }
        };
        c.handle.complete(data, hints)?;
        Ok(())
    });
    if status != AmStatus::InvalidJson {
        unsafe { consume(call) };
    }
    status
}

/// v9：成功完成并消费 call，附带业务状态、摘要与内容注解（`result` 可为 NULL）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_complete_ex(call: *mut AmCall, result: *const AmCallResult) -> AmStatus {
    if call.is_null() {
        return guard(|| Err(FfiError::null("call")));
    }
    let status = guard(|| {
        // SAFETY: call 非空且尚未消费。
        let c = unsafe { &*call };
        // SAFETY: result 为 NULL 或指向至少 struct_size 字节。
        let result = match unsafe { read_call_result(result) } {
            Ok(r) => r,
            Err(e) if e.status == AmStatus::InvalidJson => return Err(e),
            Err(e) => {
                // call 仍会被消费，因此以 HANDLER_ERROR 结束调用，避免调用悬挂到超时。
                let _ = c.handle.fail(ErrorKind::HandlerError, &format!("调用结果非法：{}", e.message));
                return Err(e);
            }
        };
        c.handle.complete_with(result)?;
        Ok(())
    });
    if status != AmStatus::InvalidJson {
        unsafe { consume(call) };
    }
    status
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_fail(
    call: *mut AmCall,
    kind: *const c_char,
    message: *const c_char,
) -> AmStatus {
    if call.is_null() {
        return guard(|| Err(FfiError::null("call")));
    }
    let status = guard(|| {
        let c = unsafe { &*call };
        let kind = error_kind_from(unsafe { lossy_str(kind) }.as_deref());
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        c.handle.fail(kind, &message)?;
        Ok(())
    });
    unsafe { consume(call) };
    status
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_fail_with_details(
    call: *mut AmCall,
    kind: *const c_char,
    message: *const c_char,
    details_json: *const c_char,
) -> AmStatus {
    if call.is_null() {
        return guard(|| Err(FfiError::null("call")));
    }
    let status = guard(|| {
        let c = unsafe { &*call };
        let kind = error_kind_from(unsafe { lossy_str(kind) }.as_deref());
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        let details = unsafe { opt_str(details_json, "details_json") }
            .map_err(|_| FfiError::new(AmStatus::InvalidJson, "details_json 不是合法的 UTF-8"))?;
        c.handle.fail_with_details(kind, &message, details)?;
        Ok(())
    });
    if status != AmStatus::InvalidJson {
        unsafe { consume(call) };
    }
    status
}

/// v11：以 `USER_ACTION_REQUIRED` 失败完成并消费 call；reason / uri 为 NULL 时不出现在错误的 data 中。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_fail_user_action(
    call: *mut AmCall,
    message: *const c_char,
    reason: *const c_char,
    uri: *const c_char,
) -> AmStatus {
    if call.is_null() {
        return guard(|| Err(FfiError::null("call")));
    }
    let status = guard(|| {
        let c = unsafe { &*call };
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        let reason = unsafe { lossy_str(reason) };
        let uri = unsafe { lossy_str(uri) };
        c.handle.fail_user_action(&message, reason.as_deref(), uri.as_deref())?;
        Ok(())
    });
    unsafe { consume(call) };
    status
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_hold(call: *const AmCall, out: *mut *mut AmHold) -> AmStatus {
    guard(|| {
        let out = unsafe { prepare_out(out) }?;
        let c = unsafe { call.as_ref() }.ok_or_else(|| FfiError::null("call"))?;
        *out = Box::into_raw(Box::new(AmHold {
            handle: c.handle.hold()?,
        }));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_progress(
    call: *const AmCall,
    progress: f64,
    total: f64,
    message: *const c_char,
) -> AmStatus {
    guard(|| {
        let c = unsafe { call.as_ref() }.ok_or_else(|| FfiError::null("call"))?;
        let message = unsafe { opt_str(message, "message") }?;
        let total = (total >= 0.0).then_some(total);
        c.handle.report_progress(progress, total, message)?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// 资源读取
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_read_resource_name(read: *const AmRead) -> *const c_char {
    unsafe { read.as_ref() }.map_or(std::ptr::null(), |r| r.resource_name.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_read_complete(
    read: *mut AmRead,
    contents_json: *const c_char,
) -> AmStatus {
    if read.is_null() {
        return guard(|| Err(FfiError::null("read")));
    }
    let status = guard(|| {
        let r = unsafe { &*read };
        let contents = unsafe { opt_str(contents_json, "contents_json") }
            .map_err(|_| FfiError::new(AmStatus::InvalidJson, "contents_json 不是合法的 UTF-8"))?
            .ok_or_else(|| FfiError::new(AmStatus::InvalidJson, "contents_json 不能为 NULL"))?;
        r.handle.complete(contents)?;
        Ok(())
    });
    if status != AmStatus::InvalidJson {
        unsafe { consume(read) };
    }
    status
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_read_fail(
    read: *mut AmRead,
    kind: *const c_char,
    message: *const c_char,
) -> AmStatus {
    if read.is_null() {
        return guard(|| Err(FfiError::null("read")));
    }
    let status = guard(|| {
        let r = unsafe { &*read };
        let kind = error_kind_from(unsafe { lossy_str(kind) }.as_deref());
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        r.handle.fail(kind, &message)?;
        Ok(())
    });
    unsafe { consume(read) };
    status
}

/// v12：失败完成并消费 read，附带结构化详情；语义同 [`am_call_fail_with_details`]（`details_json` 为 NULL 等同
/// [`am_read_fail`]；非法时返回 `AM_ERR_INVALID_JSON` 且不消费 read）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_read_fail_with_details(
    read: *mut AmRead,
    kind: *const c_char,
    message: *const c_char,
    details_json: *const c_char,
) -> AmStatus {
    if read.is_null() {
        return guard(|| Err(FfiError::null("read")));
    }
    let status = guard(|| {
        let r = unsafe { &*read };
        let kind = error_kind_from(unsafe { lossy_str(kind) }.as_deref());
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        let details = unsafe { opt_str(details_json, "details_json") }
            .map_err(|_| FfiError::new(AmStatus::InvalidJson, "details_json 不是合法的 UTF-8"))?;
        r.handle.fail_with_details(kind, &message, details)?;
        Ok(())
    });
    if status != AmStatus::InvalidJson {
        unsafe { consume(read) };
    }
    status
}

/// v12：以 `USER_ACTION_REQUIRED` 失败完成并消费 read；语义同 [`am_call_fail_user_action`]。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_read_fail_user_action(
    read: *mut AmRead,
    message: *const c_char,
    reason: *const c_char,
    uri: *const c_char,
) -> AmStatus {
    if read.is_null() {
        return guard(|| Err(FfiError::null("read")));
    }
    let status = guard(|| {
        let r = unsafe { &*read };
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        let reason = unsafe { lossy_str(reason) };
        let uri = unsafe { lossy_str(uri) };
        r.handle.fail_user_action(&message, reason.as_deref(), uri.as_deref())?;
        Ok(())
    });
    unsafe { consume(read) };
    status
}

// ---------------------------------------------------------------------------
// 导航（v14）
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_page(navigate: *const AmNavigate) -> *const c_char {
    unsafe { navigate.as_ref() }.map_or(std::ptr::null(), |n| n.page.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_params_json(navigate: *const AmNavigate) -> *const c_char {
    unsafe { navigate.as_ref() }
        .and_then(|n| n.params_json.as_ref())
        .map_or(std::ptr::null(), |p| p.as_ptr())
}

/// 完成并消费 `navigate`：`f` 对句柄提交结果。
unsafe fn finish_navigate(
    navigate: *mut AmNavigate,
    f: impl FnOnce(&AmNavigate) -> Result<(), app_mcp_native::NativeError>,
) -> AmStatus {
    if navigate.is_null() {
        return guard(|| Err(FfiError::null("navigate")));
    }
    let status = guard(|| {
        f(unsafe { &*navigate })?;
        Ok(())
    });
    unsafe { consume(navigate) };
    status
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_complete(navigate: *mut AmNavigate) -> AmStatus {
    unsafe { finish_navigate(navigate, |n| n.handle.complete()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_fail(navigate: *mut AmNavigate, message: *const c_char) -> AmStatus {
    let message = unsafe { lossy_str(message) }.unwrap_or_default();
    unsafe { finish_navigate(navigate, |n| n.handle.fail(&message)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_deny(navigate: *mut AmNavigate, message: *const c_char) -> AmStatus {
    let message = unsafe { lossy_str(message) }.unwrap_or_default();
    unsafe { finish_navigate(navigate, |n| n.handle.deny(&message)) }
}

/// v15：以 `USER_ACTION_REQUIRED` 结束导航并消费 `navigate`；reason / uri 为 NULL 时不出现在错误的 data 中。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_fail_user_action(
    navigate: *mut AmNavigate,
    message: *const c_char,
    reason: *const c_char,
    uri: *const c_char,
) -> AmStatus {
    let message = unsafe { lossy_str(message) }.unwrap_or_default();
    let reason = unsafe { lossy_str(reason) };
    let uri = unsafe { lossy_str(uri) };
    unsafe { finish_navigate(navigate, |n| n.handle.fail_user_action(&message, reason.as_deref(), uri.as_deref())) }
}

#[cfg(test)]
mod tests;
