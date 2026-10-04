//! C 结构体 / 整数枚举 → 原生运行时类型的转换，以及按 `struct_size` 读取扩展选项。
//! 另含出参、句柄释放等指针工具。

use std::ffi::{c_char, c_int};
use std::panic::{AssertUnwindSafe, catch_unwind};

use app_mcp_native::{
    Activation, AppOverview, BusyPolicy, CachePolicy, CacheScope, CallDedupPolicy, CallResult, ClientKind, ContentAnnotations, Deprecation, ErrorKind, HeartbeatMode,
    LifecycleMode, LifecyclePolicy, NativeConfig, Residency, ResourceOptions, ResourceSpec, ResultStatus,
    Risk, SleepReason, ToolAnnotations, ToolOptions, ToolSpec, ToolSurface, UndoAction, Visibility, WakeDescriptor, WakeKind,
    WakeReason,
};

use crate::callbacks::AmIdleExitFn;
use crate::ffi_types::{
    AmCallResult, AmClientConfig, AmClientOptions, AmLifecycle, AmResourceOptions, AmResourceSpec, AmToolOptions,
    AmToolSpec,
};
use crate::status::{AmStatus, FfiError, FfiResult};
use crate::strings::{opt_str, req_str};

// ---------------------------------------------------------------------------
// 枚举转换
// ---------------------------------------------------------------------------

pub(crate) fn client_kind_from(v: c_int) -> FfiResult<ClientKind> {
    match v {
        0 => Ok(ClientKind::Native),
        1 => Ok(ClientKind::Hybrid),
        _ => Err(FfiError::invalid_argument(format!(
            "非法的 client_kind：{v}"
        ))),
    }
}

pub(crate) fn risk_from(v: c_int) -> FfiResult<Risk> {
    match v {
        0 => Ok(Risk::Read),
        1 => Ok(Risk::Write),
        2 => Ok(Risk::Destructive),
        3 => Ok(Risk::Payment),
        4 => Ok(Risk::OsSensitive),
        _ => Err(FfiError::invalid_argument(format!("非法的 risk：{v}"))),
    }
}

pub(crate) fn result_status_from(v: c_int) -> FfiResult<ResultStatus> {
    match v {
        0 => Ok(ResultStatus::Done),
        1 => Ok(ResultStatus::Pending),
        2 => Ok(ResultStatus::Partial),
        3 => Ok(ResultStatus::Noop),
        _ => Err(FfiError::invalid_argument(format!("非法的 status：{v}"))),
    }
}

pub(crate) fn activation_from(v: c_int) -> FfiResult<Option<Activation>> {
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

pub(crate) fn visibility_from(v: c_int) -> FfiResult<Visibility> {
    match v {
        0 => Ok(Visibility::Visible),
        1 => Ok(Visibility::Hidden),
        2 => Ok(Visibility::Frozen),
        _ => Err(FfiError::invalid_argument(format!(
            "非法的 visibility：{v}"
        ))),
    }
}

pub(crate) fn lifecycle_mode_from(v: c_int) -> FfiResult<LifecycleMode> {
    match v {
        0 => Ok(LifecycleMode::Persistent),
        1 => Ok(LifecycleMode::Idle),
        2 => Ok(LifecycleMode::OnDemand),
        _ => Err(FfiError::invalid_argument(format!(
            "非法的 lifecycle mode：{v}"
        ))),
    }
}

pub(crate) fn residency_from(v: c_int) -> FfiResult<Residency> {
    match v {
        0 => Ok(Residency::Keep),
        1 => Ok(Residency::ExitWhenIdle),
        2 => Ok(Residency::ExitAlways),
        _ => Err(FfiError::invalid_argument(format!("非法的 residency：{v}"))),
    }
}

/// `-1`（`AM_WAKE_UNSET`）→ `None`。
pub(crate) fn wake_kind_from(v: c_int) -> FfiResult<Option<WakeKind>> {
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

pub(crate) fn wake_reason_from(v: c_int) -> FfiResult<WakeReason> {
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

pub(crate) fn sleep_reason_from(v: c_int) -> FfiResult<SleepReason> {
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

pub(crate) fn busy_policy_from(v: c_int) -> FfiResult<BusyPolicy> {
    match v {
        0 => Ok(BusyPolicy::Reject),
        1 => Ok(BusyPolicy::Queue),
        _ => Err(FfiError::invalid_argument(format!("非法的 busy policy：{v}"))),
    }
}

pub(crate) fn heartbeat_mode_from(v: c_int) -> FfiResult<HeartbeatMode> {
    match v {
        0 => Ok(HeartbeatMode::Auto),
        1 => Ok(HeartbeatMode::Always),
        2 => Ok(HeartbeatMode::Off),
        _ => Err(FfiError::invalid_argument(format!("非法的 heartbeat：{v}"))),
    }
}

pub(crate) unsafe fn convert_lifecycle(l: &AmLifecycle) -> FfiResult<LifecyclePolicy> {
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
pub(crate) struct OptionsView {
    pub(crate) lifecycle: *const AmLifecycle,
    pub(crate) connect_timeout_ms: u32,
    pub(crate) on_idle_exit: Option<AmIdleExitFn>,
    pub(crate) heartbeat: c_int,
    pub(crate) host_absent_retries: i32,
    pub(crate) legacy_timers: bool,
    pub(crate) merge_window_ms: i64,
    pub(crate) sleep_on_background: bool,
    pub(crate) call_dedup_ttl_ms: i64,
    pub(crate) call_dedup_max_entries: i32,
    pub(crate) register_name: bool,
    pub(crate) name_instance: *const c_char,
    pub(crate) max_queued_calls: i32,
}

impl Default for OptionsView {
    fn default() -> Self {
        Self {
            lifecycle: std::ptr::null(),
            connect_timeout_ms: 0,
            on_idle_exit: None,
            heartbeat: 0,
            host_absent_retries: 0,
            legacy_timers: false,
            merge_window_ms: 0,
            sleep_on_background: false,
            call_dedup_ttl_ms: 0,
            call_dedup_max_entries: 0,
            register_name: false,
            name_instance: std::ptr::null(),
            max_queued_calls: 0,
        }
    }
}

/// 按 `struct_size` 读取扩展选项：只读取完整包含在调用方结构体中的字段。
pub(crate) unsafe fn read_options(p: *const AmClientOptions) -> FfiResult<OptionsView> {
    use std::mem::{offset_of, size_of};
    let mut view = OptionsView::default();
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
    if has(offset_of!(AmClientOptions, register_name), size_of::<bool>()) {
        view.register_name = unsafe { std::ptr::addr_of!((*p).register_name).read() };
    }
    if has(offset_of!(AmClientOptions, name_instance), size_of::<*const c_char>()) {
        view.name_instance = unsafe { std::ptr::addr_of!((*p).name_instance).read() };
    }
    if has(offset_of!(AmClientOptions, max_queued_calls), size_of::<i32>()) {
        view.max_queued_calls = unsafe { std::ptr::addr_of!((*p).max_queued_calls).read() };
    }
    Ok(view)
}

/// v18 排队上限：0 = 保留默认值，负数 = 不限（0），正数按字面使用。
pub(crate) fn max_queued_from(base: u32, value: i32) -> u32 {
    match value {
        0 => base,
        n if n < 0 => 0,
        n => n.unsigned_abs(),
    }
}

/// v13 调用去重：0 = 保留默认值，负数 = 关闭（对应字段取 0），正数按字面使用。
pub(crate) fn call_dedup_from(base: CallDedupPolicy, ttl_ms: i64, max_entries: i32) -> CallDedupPolicy {
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
pub(crate) unsafe fn read_resource_options(p: *const AmResourceOptions) -> FfiResult<ResourceOptions> {
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
    if offset_of!(AmResourceOptions, cache_scope) + size_of::<c_int>() <= size {
        let ttl_ms = unsafe { std::ptr::addr_of!((*p).cache_ttl_ms).read() };
        let scope = unsafe { std::ptr::addr_of!((*p).cache_scope).read() };
        options.cache = cache_policy_from(ttl_ms, scope)?;
    }
    Ok(options)
}

/// 读取 `struct_size`：至少要含 `struct_size` 字段本身。
///
/// # Safety
/// `p` 非空，指向以 `u32` 的 `struct_size` 开头的结构体。
pub(crate) unsafe fn struct_size_of(p: *const u32, what: &str) -> FfiResult<usize> {
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
pub(crate) unsafe fn opt_json<T: serde::de::DeserializeOwned>(p: *const c_char, what: &str) -> FfiResult<Option<T>> {
    let text = unsafe { opt_str(p, what) }
        .map_err(|_| FfiError::new(AmStatus::InvalidJson, format!("{what} 不是合法的 UTF-8")))?;
    let Some(text) = text else { return Ok(None) };
    serde_json::from_str::<Option<T>>(text)
        .map_err(|e| FfiError::new(AmStatus::InvalidJson, format!("{what} 不合法：{e}")))
}

/// 按 `struct_size` 读取工具选项；`p` 为 NULL 时为空选项（不声明 / 清除）。
pub(crate) unsafe fn read_tool_options(p: *const AmToolOptions) -> FfiResult<ToolOptions> {
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
    if size >= offset_of!(AmToolOptions, concurrency) + size_of::<u32>() {
        options.concurrency = unsafe { std::ptr::addr_of!((*p).concurrency).read() };
    }
    if has(offset_of!(AmToolOptions, exclusive)) {
        let text = unsafe { std::ptr::addr_of!((*p).exclusive).read() };
        options.exclusive = unsafe { opt_str(text, "options->exclusive") }?.map(str::to_owned);
    }
    if size >= offset_of!(AmToolOptions, implements_len) + size_of::<usize>() {
        let items = unsafe { std::ptr::addr_of!((*p).implements).read() };
        let len = unsafe { std::ptr::addr_of!((*p).implements_len).read() };
        options.implements = unsafe { read_str_array(items, len, "options->implements") }?;
    }
    if size >= offset_of!(AmToolOptions, cache_scope) + size_of::<c_int>() {
        let ttl_ms = unsafe { std::ptr::addr_of!((*p).cache_ttl_ms).read() };
        let scope = unsafe { std::ptr::addr_of!((*p).cache_scope).read() };
        options.cache = cache_policy_from(ttl_ms, scope)?;
    }
    if has(offset_of!(AmToolOptions, deprecated_until)) {
        let message = unsafe { std::ptr::addr_of!((*p).deprecated_message).read() };
        let replacement = unsafe { std::ptr::addr_of!((*p).deprecated_replacement).read() };
        let until = unsafe { std::ptr::addr_of!((*p).deprecated_until).read() };
        options.deprecated = unsafe { deprecation_from(message, replacement, until) }?;
    }
    if size >= offset_of!(AmToolOptions, undoable) + size_of::<bool>() {
        options.undoable = unsafe { std::ptr::addr_of!((*p).undoable).read() };
    }
    Ok(options)
}

/// v23 弃用声明（spec/protocol.md 3.7）：三个指针均为 NULL = 未声明。
///
/// @why 只给 replacement / until 而 message 为 NULL 时按空 message 交给核心，由核心统一报 `InvalidConfig`
/// （P-04，唯一校验点），不在此另设规则。
/// @error 非法 UTF-8 → `AM_ERR_INVALID_ARGUMENT`。
///
/// # Safety
/// 各指针为 NULL 或有效的 C 字符串。
pub(crate) unsafe fn deprecation_from(
    message: *const c_char,
    replacement: *const c_char,
    until: *const c_char,
) -> FfiResult<Option<Deprecation>> {
    let message = unsafe { opt_str(message, "options->deprecated_message") }?;
    let replacement = unsafe { opt_str(replacement, "options->deprecated_replacement") }?.map(str::to_owned);
    let until = unsafe { opt_str(until, "options->deprecated_until") }?.map(str::to_owned);
    if message.is_none() && replacement.is_none() && until.is_none() {
        return Ok(None);
    }
    Ok(Some(Deprecation { message: message.unwrap_or_default().to_owned(), replacement, until }))
}

/// v22 结果缓存声明（spec/protocol.md 3.6）：`ttl_ms` 0 = 未声明。
///
/// @error `scope` 不是 `AmCacheScope` 取值 → `AM_ERR_INVALID_ARGUMENT`（`ttl_ms` 为 0 时也检查）。
/// `ttl_ms` 的范围不在此校验：由核心注册时返回 `InvalidConfig`（P-04，唯一校验点）。
pub(crate) fn cache_policy_from(ttl_ms: u64, scope: c_int) -> FfiResult<Option<CachePolicy>> {
    let scope = match scope {
        0 => CacheScope::Private,
        1 => CacheScope::Shared,
        other => return Err(FfiError::invalid_argument(format!("options->cache_scope 取值无效：{other}"))),
    };
    Ok((ttl_ms > 0).then_some(CachePolicy { ttl_ms, scope }))
}

/// 按 `struct_size` 读取调用结果；`p` 为 NULL 时为默认结果（无返回值、done）。
/// 只有 `AM_ERR_INVALID_JSON` 表示可重试（调用不应被消费）。
pub(crate) unsafe fn read_call_result(p: *const AmCallResult) -> FfiResult<CallResult> {
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
    if has(offset_of!(AmCallResult, undo_label), ptr_len) {
        let tool = unsafe { std::ptr::addr_of!((*p).undo_tool).read() };
        let arguments = unsafe { std::ptr::addr_of!((*p).undo_arguments_json).read() };
        let label = unsafe { std::ptr::addr_of!((*p).undo_label).read() };
        result.undo = unsafe { undo_from(tool, arguments, label) }?;
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

/// v24 撤销信息（spec/protocol.md 3.8）：三个指针均为 NULL = 不可撤销；`arguments` 为 NULL 时取 `{}`。
///
/// @why 格式（局部名、参数为对象且不超长、label 长度）不在此校验：原样交给核心，由核心去掉不合法的撤销信息并告警
/// （P-04，唯一校验点 `UndoAction::validate`），结果照常发送；`tool` 为 NULL 而另两项不为 NULL 时以空名交给核心。
/// @error `arguments` 不是合法 JSON / UTF-8 → `AM_ERR_INVALID_JSON`（可重试）；`tool` / `label` 非法 UTF-8 →
/// `AM_ERR_INVALID_ARGUMENT`。
///
/// # Safety
/// 各指针为 NULL 或有效的 C 字符串。
pub(crate) unsafe fn undo_from(
    tool: *const c_char,
    arguments: *const c_char,
    label: *const c_char,
) -> FfiResult<Option<UndoAction>> {
    let arguments_value = unsafe { opt_str(arguments, "result->undo_arguments_json") }
        .map_err(|_| FfiError::new(AmStatus::InvalidJson, "result->undo_arguments_json 不是合法的 UTF-8"))?
        .map(serde_json::from_str::<serde_json::Value>)
        .transpose()
        .map_err(|e| FfiError::new(AmStatus::InvalidJson, format!("result->undo_arguments_json 不合法：{e}")))?;
    let tool = unsafe { opt_str(tool, "result->undo_tool") }?;
    let label = unsafe { opt_str(label, "result->undo_label") }?.map(str::to_owned);
    if tool.is_none() && arguments_value.is_none() && label.is_none() {
        return Ok(None);
    }
    let mut action = UndoAction::new(tool.unwrap_or_default());
    if let Some(value) = arguments_value {
        action.arguments = value;
    }
    action.label = label;
    Ok(Some(action))
}

/// 协议错误类别字符串 → [`ErrorKind`]；NULL 或未知值按 `HANDLER_ERROR` 处理。
pub(crate) fn error_kind_from(kind: Option<&str>) -> ErrorKind {
    kind.and_then(|k| serde_json::from_value(serde_json::Value::String(k.to_owned())).ok())
        .unwrap_or(ErrorKind::HandlerError)
}

pub(crate) unsafe fn convert_config(c: &AmClientConfig) -> FfiResult<NativeConfig> {
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
pub(crate) unsafe fn convert_tool_spec(s: &AmToolSpec, name_override: Option<String>) -> FfiResult<ToolSpec> {
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

pub(crate) unsafe fn convert_resource_spec(s: &AmResourceSpec) -> FfiResult<ResourceSpec> {
    Ok(ResourceSpec {
        name: unsafe { req_str(s.name, "spec->name") }?.to_owned(),
        description: unsafe { req_str(s.description, "spec->description") }?.to_owned(),
        mime_type: unsafe { opt_str(s.mime_type, "spec->mime_type") }?.map(str::to_owned),
    })
}

/// 检查输出参数非空并先置为 NULL。
pub(crate) unsafe fn prepare_out<T>(out: *mut *mut T) -> FfiResult<&'static mut *mut T> {
    // SAFETY: 调用方保证 out 为 NULL 或指向可写的指针。
    let out = unsafe { out.as_mut() }.ok_or_else(|| FfiError::null("out"))?;
    *out = std::ptr::null_mut();
    Ok(out)
}

pub(crate) unsafe fn read_hints(hints: *const *const c_char, len: usize) -> FfiResult<Vec<String>> {
    unsafe { read_str_array(hints, len, "state_hints") }
}

/// C 字符串数组（指针 + 长度）→ `Vec<String>`；`len` 为 0 时 `items` 可为 NULL。
///
/// # Safety
/// `items` 为 NULL 或指向 `len` 个 NULL / 有效 C 字符串指针。
pub(crate) unsafe fn read_str_array(items: *const *const c_char, len: usize, what: &str) -> FfiResult<Vec<String>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if items.is_null() {
        return Err(FfiError::null(what));
    }
    // SAFETY: 调用方保证 items 指向 len 个元素。
    let slice = unsafe { std::slice::from_raw_parts(items, len) };
    slice
        .iter()
        .enumerate()
        .map(|(i, p)| unsafe { req_str(*p, &format!("{what}[{i}]")) }.map(str::to_owned))
        .collect()
}

/// 消费（释放）一个 Box 句柄；释放过程中的 panic 被吞掉。
pub(crate) unsafe fn consume<T>(p: *mut T) {
    // SAFETY: p 来自 Box::into_raw，且调用方保证不再使用。
    let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(p) })));
}
