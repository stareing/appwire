//! Hub SDK 的 C ABI 实现，契约见 `include/app_mcp_hub.h`（spec/hub-api.md 第 6 节）。
//!
//! - 每个 [`AmHub`] 自带一个 tokio 多线程运行时与一个回调分发线程（[`dispatch::Dispatcher`]）；
//!   回调从不在 tokio 工作线程或调用方线程上执行。
//! - 所有入口用 [`ffi_util::guard`] 包住：错误写入线程局部存储（`am_hub_last_error_message`），
//!   panic 转为 `AM_HUB_ERR_PANIC`，不会跨越 FFI 边界。
//! - 异步操作（调用、读资源、dispatch）在运行时上以任务执行，结果经 [`dispatch::ResultCb`] 投递，
//!   保证恰好一次（Hub 停止时中止任务并以 `CANCELLED` 兜底结果回调）。
//! - 审批 / 配对 / 唤醒：Hub 的 async trait 映射为“回调 + 完成句柄”；句柄内部是 oneshot 发送端，
//!   未完成就被丢弃时接收端得到错误，按拒绝（唤醒：`LAUNCH_FAILED`）处理。
//!
//! 所有导出符号带 `am_hub_` 前缀，可与 `app_mcp`（App 端 C ABI）同时链接进一个进程。
//! 各函数的安全约定（指针有效性、所有权）统一写在头文件中，这里不再逐个重复。
#![allow(clippy::missing_safety_doc)]

mod config;
mod dispatch;
mod ffi_util;

use std::ffi::{c_char, c_int, c_void};
use std::future::Future;
use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use hub::{
    ApprovalHandler, ApprovalRequest, CallOutcome, CallRequest, ErrorKind, Hub, HubError,
    PairingHandler, PairingRequest, ProgressUpdate, ToolError, ToolFilter, ToolFormat, WakeRequest,
    Waker, async_trait, format,
};
use serde_json::{Value, json};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinSet;

pub use dispatch::{AmHubFreeFn, AmHubResultFn};
use dispatch::{CallbackEntry, Dispatcher, ResultCb, Slot, UserData, lock};
pub use ffi_util::AmHubStatus;
use ffi_util::{
    FfiError, FfiResult, guard, guard_value, into_raw_cstring, last_error_ptr, opt_str, req_str,
    write_out_str,
};

/// 与头文件 `AM_HUB_API_VERSION` 一致。3：合并端口（`am_hub_listen_addr`、配置 `listen`）。
pub const AM_HUB_API_VERSION: u32 = 3;

/// Hub 停止时未完成的调用使用的错误说明。
const STOPPED_MESSAGE: &str = "Hub 已停止，调用未完成。";

pub type AmHubEventFn = unsafe extern "C" fn(user_data: *mut c_void, event_json: *mut c_char);
pub type AmHubApprovalFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    request_json: *mut c_char,
    approval: *mut AmHubApproval,
);
pub type AmHubPairingFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    request_json: *mut c_char,
    pairing: *mut AmHubPairing,
);
pub type AmHubWakerFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    request_json: *mut c_char,
    wake: *mut AmHubWake,
);

// ---------------------------------------------------------------------------
// 句柄
// ---------------------------------------------------------------------------

/// 一次审批（头文件 `AmHubApproval`）。
pub struct AmHubApproval {
    tx: oneshot::Sender<bool>,
}

/// 一次配对（头文件 `AmHubPairing`）。
pub struct AmHubPairing {
    tx: oneshot::Sender<bool>,
}

/// 一次唤醒（头文件 `AmHubWake`）。
pub struct AmHubWake {
    tx: oneshot::Sender<Result<(), ToolError>>,
}

/// 运行中的 Hub（头文件 `AmHub`）。
pub struct AmHub {
    rt: Mutex<Option<Runtime>>,
    handle: Handle,
    /// `None` = 已停止。异步操作在读锁下登记任务，停止时在写锁下取走，保证不遗漏。
    hub: RwLock<Option<Arc<Hub>>>,
    /// 进行中的异步操作；停止时全部中止（结果回调以兜底结果触发）。
    ops: Mutex<JoinSet<()>>,
    listen_addr: Option<SocketAddr>,
    ipc_endpoint: Option<String>,
    dispatcher: Dispatcher,
    events: Arc<Slot<AmHubEventFn>>,
    approval: Arc<Slot<AmHubApprovalFn>>,
    pairing: Arc<Slot<AmHubPairingFn>>,
    pairing_installed: AtomicBool,
    waker: Arc<Slot<AmHubWakerFn>>,
    seq: AtomicU64,
}

impl AmHub {
    fn hub(&self) -> FfiResult<Arc<Hub>> {
        self.hub
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(FfiError::stopped)
    }

    /// 在运行时上执行异步操作，结果经 `rc` 投递。Hub 已停止时不回调并返回 `STOPPED`。
    fn spawn_result<Fut>(&self, rc: ResultCb, f: impl FnOnce(Arc<Hub>) -> Fut) -> FfiResult<()>
    where
        Fut: Future<Output = String> + Send + 'static,
    {
        let guard = self
            .hub
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(hub) = guard.as_ref() else {
            rc.disarm();
            return Err(FfiError::stopped());
        };
        let fut = f(hub.clone());
        let mut ops = lock(&self.ops);
        while ops.try_join_next().is_some() {}
        ops.spawn_on(
            async move {
                let json = fut.await;
                rc.fire(json);
            },
            &self.handle,
        );
        Ok(())
    }

    /// 在本 Hub 的运行时上阻塞执行。当前线程已处于某个 tokio 运行时中时，改在临时线程上执行。
    fn block_on<T: Send + 'static>(&self, fut: impl Future<Output = T> + Send + 'static) -> T {
        if Handle::try_current().is_ok() {
            let handle = self.handle.clone();
            let joined = std::thread::scope(|s| s.spawn(move || handle.block_on(fut)).join());
            match joined {
                Ok(v) => v,
                Err(p) => std::panic::resume_unwind(p),
            }
        } else {
            self.handle.block_on(fut)
        }
    }

    fn next_call_id(&self) -> String {
        format!("am-hub-{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// 停止 Hub：中止进行中的操作、关闭 App 连接与上游。幂等。
    fn shutdown(&self) {
        let (hub, ops) = {
            let mut w = self
                .hub
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(hub) = w.take() else { return };
            (hub, std::mem::take(&mut *lock(&self.ops)))
        };
        self.block_on(async move {
            let mut ops = ops;
            ops.shutdown().await;
            // 同步查询可能短暂持有引用：稍等其释放后正常关闭；等不到则丢弃（Drop 同样中止后台任务）。
            let mut hub = hub;
            for _ in 0..500 {
                match Arc::try_unwrap(hub) {
                    Ok(h) => {
                        h.shutdown().await;
                        return;
                    }
                    Err(h) => {
                        hub = h;
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                }
            }
        });
    }

    fn teardown(&self) {
        self.shutdown();
        if let Some(rt) = lock(&self.rt).take() {
            if Handle::try_current().is_ok() {
                rt.shutdown_background();
            } else {
                rt.shutdown_timeout(Duration::from_secs(2));
            }
        }
        // 运行时关闭时被丢弃的任务已把兜底结果排进队列；等分发线程处理完再释放 user_data。
        self.dispatcher.close_and_join();
        self.events.set(None);
        self.approval.set(None);
        self.pairing.set(None);
        self.waker.set(None);
    }
}

// ---------------------------------------------------------------------------
// 审批 / 配对：async trait → 回调 + 完成句柄
// ---------------------------------------------------------------------------

/// 把请求投递到分发线程上的回调，等待完成句柄的结果。回调未设置、句柄被丢弃 → `false`。
async fn ask<F: Copy + Send + Sync + 'static>(
    slot: &Arc<Slot<F>>,
    dispatcher: &Dispatcher,
    request_json: String,
    invoke: fn(F, *mut c_void, *mut c_char, oneshot::Sender<bool>),
) -> bool {
    if slot.get().is_none() {
        return false;
    }
    let (tx, rx) = oneshot::channel();
    let slot = slot.clone();
    let job = Box::new(move || {
        // 执行时再读取：期间被清除则丢弃 tx（按拒绝处理）。
        if let Some(entry) = slot.get() {
            invoke(
                entry.f,
                entry.user_data.ptr(),
                into_raw_cstring(&request_json),
                tx,
            );
        }
    });
    if dispatcher.post(job).is_err() {
        return false;
    }
    rx.await.unwrap_or(false)
}

fn invoke_approval(f: AmHubApprovalFn, ud: *mut c_void, json: *mut c_char, tx: oneshot::Sender<bool>) {
    let handle = Box::into_raw(Box::new(AmHubApproval { tx }));
    // SAFETY: 调用方注册的回调；字符串与句柄的所有权转移给回调方。
    unsafe { f(ud, json, handle) };
}

fn invoke_pairing(f: AmHubPairingFn, ud: *mut c_void, json: *mut c_char, tx: oneshot::Sender<bool>) {
    let handle = Box::into_raw(Box::new(AmHubPairing { tx }));
    // SAFETY: 同上。
    unsafe { f(ud, json, handle) };
}

struct CApprovalHandler {
    slot: Arc<Slot<AmHubApprovalFn>>,
    dispatcher: Dispatcher,
}

#[async_trait]
impl ApprovalHandler for CApprovalHandler {
    async fn approve(&self, req: ApprovalRequest) -> bool {
        let json = serde_json::to_string(&req).unwrap_or_default();
        ask(&self.slot, &self.dispatcher, json, invoke_approval).await
    }
}

struct CPairingHandler {
    slot: Arc<Slot<AmHubPairingFn>>,
    dispatcher: Dispatcher,
}

#[async_trait]
impl PairingHandler for CPairingHandler {
    async fn pair(&self, req: PairingRequest) -> bool {
        let json = serde_json::to_string(&req).unwrap_or_default();
        ask(&self.slot, &self.dispatcher, json, invoke_pairing).await
    }
}

/// 唤醒：把 WakeRequest 投递给 C 回调，等待 `am_hub_waker_complete`。
/// 回调被清除、句柄未完成就被释放 → `LAUNCH_FAILED`。
struct CWaker {
    slot: Arc<Slot<AmHubWakerFn>>,
    dispatcher: Dispatcher,
}

fn waker_dropped() -> ToolError {
    ToolError::new(ErrorKind::LaunchFailed, "唤醒回调未完成（句柄被丢弃或回调已清除）。")
}

#[async_trait]
impl Waker for CWaker {
    async fn wake(&self, req: WakeRequest) -> Result<(), HubError> {
        let json = serde_json::to_string(&req).unwrap_or_default();
        let (tx, rx) = oneshot::channel();
        let slot = self.slot.clone();
        let job = Box::new(move || {
            if let Some(entry) = slot.get() {
                let handle = Box::into_raw(Box::new(AmHubWake { tx }));
                // SAFETY: 调用方注册的回调；字符串与句柄的所有权转移给回调方。
                unsafe { (entry.f)(entry.user_data.ptr(), into_raw_cstring(&json), handle) };
            }
        });
        if self.dispatcher.post(job).is_err() {
            return Err(waker_dropped().into());
        }
        match rx.await {
            Ok(r) => r.map_err(Into::into),
            Err(_) => Err(waker_dropped().into()),
        }
    }
}

// ---------------------------------------------------------------------------
// JSON 辅助
// ---------------------------------------------------------------------------

fn error_json(e: &ToolError) -> Value {
    let mut v = json!({ "kind": e.kind, "message": e.message });
    if let Some(d) = &e.details {
        v["details"] = d.clone();
    }
    v
}

/// 以 `CallOutcome` 形式表示的失败结果。
fn outcome_error_json(call_id: &str, e: ToolError) -> String {
    let o = CallOutcome {
        call_id: call_id.to_owned(),
        result: Err(e),
        state_hints: Vec::new(),
        instance_id: None,
        overview: None,
        status: hub::ResultStatus::Done,
        state_resource: None,
        summary: None,
        annotations: None,
    };
    serde_json::to_string(&o).unwrap_or_default()
}

/// dispatch 的兜底结果（Hub 停止）：该格式的错误结果消息。
fn dispatch_fallback(format: ToolFormat, call: &Value) -> String {
    let parsed = format::parse_call(format, call).unwrap_or_else(|p| p);
    let text = format!("{}: {STOPPED_MESSAGE}", error_kind_str(ErrorKind::Cancelled));
    let result = json!({ "content": [{ "type": "text", "text": text }], "isError": true });
    match serde_json::from_value(result) {
        Ok(r) => format::render_result(format, &parsed, &r).to_string(),
        Err(_) => json!({ "error": { "kind": ErrorKind::Cancelled, "message": STOPPED_MESSAGE } })
            .to_string(),
    }
}

fn error_kind_str(k: ErrorKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn to_json<T: serde::Serialize>(v: &T) -> FfiResult<String> {
    serde_json::to_string(v)
        .map_err(|e| FfiError::new(AmHubStatus::Internal, format!("序列化失败：{e}")))
}

fn parse_filter(text: Option<&str>) -> FfiResult<ToolFilter> {
    match text.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => serde_json::from_str(t).map_err(|e| FfiError::json("filter_json", e)),
        None => Ok(ToolFilter::default()),
    }
}

fn format_from(v: c_int) -> FfiResult<ToolFormat> {
    Ok(match v {
        0 => ToolFormat::Mcp,
        1 => ToolFormat::OpenAiChat,
        2 => ToolFormat::OpenAiResponses,
        3 => ToolFormat::Anthropic,
        4 => ToolFormat::Gemini,
        _ => return Err(FfiError::invalid_argument(format!("非法的 format：{v}"))),
    })
}

/// # Safety
/// `p` 为 NULL 或 `am_hub_start` 返回且尚未释放的句柄。
unsafe fn hub_ref<'a>(p: *const AmHub) -> FfiResult<&'a AmHub> {
    // SAFETY: 由调用方保证。
    unsafe { p.as_ref() }.ok_or_else(|| FfiError::null("hub"))
}

/// 同步查询的公共部分：检查 out、执行、写出字符串。
unsafe fn query(
    hub: *const AmHub,
    out: *mut *mut c_char,
    f: impl FnOnce(&AmHub, &Hub) -> FfiResult<String>,
) -> AmHubStatus {
    guard(|| {
        if out.is_null() {
            return Err(FfiError::null("out_json"));
        }
        // SAFETY: out 非 NULL，由调用方保证可写。
        unsafe { write_out_str(out, None) };
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        let inner = h.hub()?;
        let _rt = h.handle.enter();
        let s = f(h, &inner)?;
        // SAFETY: 同上。
        unsafe { write_out_str(out, Some(&s)) };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// 通用
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn am_hub_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn am_hub_last_error_message() -> *const c_char {
    last_error_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: s 由本库 CString::into_raw 分配，按约定只释放一次。
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { std::ffi::CString::from_raw(s) })));
    }
}

// ---------------------------------------------------------------------------
// 生命周期
// ---------------------------------------------------------------------------

fn start(config_json: Option<&str>) -> FfiResult<Box<AmHub>> {
    let cfg = config::parse(config_json)?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cfg.worker_threads)
        .thread_name("am-hub-worker")
        .enable_all()
        .build()
        .map_err(|e| FfiError::new(AmHubStatus::Internal, format!("创建 tokio 运行时失败：{e}")))?;
    let dispatcher = Dispatcher::start()
        .map_err(|e| FfiError::new(AmHubStatus::Internal, format!("创建分发线程失败：{e}")))?;
    let started = if Handle::try_current().is_ok() {
        let rt_ref = &rt;
        std::thread::scope(|s| s.spawn(move || rt_ref.block_on(Hub::start(cfg.hub))).join())
            .unwrap_or_else(|p| std::panic::resume_unwind(p))
    } else {
        rt.block_on(Hub::start(cfg.hub))
    };
    let hub = match started {
        Ok(h) => h,
        Err(e) => {
            dispatcher.close_and_join();
            rt.shutdown_background();
            return Err(FfiError::io("启动 Hub 失败", &e));
        }
    };
    let events = Arc::new(Slot::<AmHubEventFn>::new());
    let approval = Arc::new(Slot::<AmHubApprovalFn>::new());
    hub.set_approval_handler(Arc::new(CApprovalHandler {
        slot: approval.clone(),
        dispatcher: dispatcher.clone(),
    }));

    // 事件转发：回调未设置时丢弃；执行时再读取回调（被清除后不再调用旧回调）。
    let mut rx = hub.events();
    let (slot, d) = (events.clone(), dispatcher.clone());
    rt.spawn(async move {
        loop {
            let json = match rx.recv().await {
                Ok(ev) => serde_json::to_string(&ev).unwrap_or_default(),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    json!({ "type": "lagged", "skipped": n }).to_string()
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if slot.get().is_none() {
                continue;
            }
            let slot = slot.clone();
            let _ = d.post(Box::new(move || {
                if let Some(entry) = slot.get() {
                    // SAFETY: 调用方注册的回调；字符串所有权转移给回调方。
                    unsafe { (entry.f)(entry.user_data.ptr(), into_raw_cstring(&json)) };
                }
            }));
        }
    });

    Ok(Box::new(AmHub {
        handle: rt.handle().clone(),
        rt: Mutex::new(Some(rt)),
        listen_addr: hub.listen_addr(),
        ipc_endpoint: hub.ipc_endpoint().map(str::to_owned),
        hub: RwLock::new(Some(Arc::new(hub))),
        ops: Mutex::new(JoinSet::new()),
        dispatcher,
        events,
        approval,
        pairing: Arc::new(Slot::new()),
        pairing_installed: AtomicBool::new(false),
        waker: Arc::new(Slot::new()),
        seq: AtomicU64::new(0),
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_start(config_json: *const c_char, out_hub: *mut *mut AmHub) -> AmHubStatus {
    guard(|| {
        if out_hub.is_null() {
            return Err(FfiError::null("out_hub"));
        }
        // SAFETY: out_hub 非 NULL，由调用方保证可写。
        unsafe { *out_hub = std::ptr::null_mut() };
        // SAFETY: 由调用方保证。
        let cfg = unsafe { opt_str(config_json, "config_json") }?;
        let hub = start(cfg)?;
        // SAFETY: 同上。
        unsafe { *out_hub = Box::into_raw(hub) };
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_shutdown(hub: *mut AmHub) {
    let _ = guard(|| {
        // SAFETY: 由调用方保证。
        unsafe { hub_ref(hub) }?.shutdown();
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_free(hub: *mut AmHub) {
    if hub.is_null() {
        return;
    }
    // SAFETY: hub 由 am_hub_start 分配，按约定只释放一次。
    let b = unsafe { Box::from_raw(hub) };
    let _ = guard(move || {
        b.teardown();
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_listen_addr(hub: *const AmHub) -> *mut c_char {
    guard_value(std::ptr::null_mut(), || {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        h.hub()?;
        Ok(h.listen_addr.map_or(std::ptr::null_mut(), |a| into_raw_cstring(&a.to_string())))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_ipc_endpoint(hub: *const AmHub) -> *mut c_char {
    guard_value(std::ptr::null_mut(), || {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        h.hub()?;
        Ok(h.ipc_endpoint.as_deref().map_or(std::ptr::null_mut(), into_raw_cstring))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_serve_http(
    hub: *mut AmHub,
    addr: *const c_char,
    allow_remote: bool,
    out_addr: *mut *mut c_char,
) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        unsafe { write_out_str(out_addr, None) };
        // SAFETY: 同上。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let addr = unsafe { req_str(addr, "addr") }?.to_owned();
        let inner = h.hub()?;
        let local = h
            .block_on(async move { inner.serve_http(&addr, allow_remote).await })
            .map_err(|e| FfiError::io("启动 HTTP 出口失败", &e))?;
        // SAFETY: 同上。
        unsafe { write_out_str(out_addr, Some(&local.to_string())) };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// 查询
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_apps_json(hub: *const AmHub, out_json: *mut *mut c_char) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe { query(hub, out_json, |_, h| to_json(&h.apps())) }
}

/// 运行状态（HubStatus，与 `GET /status` 相同，spec/hub-api.md 3.9）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_status_json(hub: *const AmHub, out_json: *mut *mut c_char) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe { query(hub, out_json, |_, h| to_json(&h.status())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_tools_json(
    hub: *const AmHub,
    filter_json: *const c_char,
    out_json: *mut *mut c_char,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe {
        query(hub, out_json, |_, h| {
            let filter = parse_filter(opt_str(filter_json, "filter_json")?)?;
            to_json(&h.tools(&filter))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_resources_json(
    hub: *const AmHub,
    out_json: *mut *mut c_char,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe { query(hub, out_json, |_, h| to_json(&h.resources())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_overview_json(
    hub: *const AmHub,
    app_id: *const c_char,
    out_json: *mut *mut c_char,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe {
        query(hub, out_json, |_, h| {
            let app_id = req_str(app_id, "app_id")?;
            to_json(&h.overview(app_id))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_export_tools(
    hub: *const AmHub,
    format: c_int,
    filter_json: *const c_char,
    out_json: *mut *mut c_char,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe {
        query(hub, out_json, |_, h| {
            let format = format_from(format)?;
            let filter = parse_filter(opt_str(filter_json, "filter_json")?)?;
            to_json(&h.export_tools(format, &filter))
        })
    }
}

// ---------------------------------------------------------------------------
// 调用
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_call(
    hub: *mut AmHub,
    request_json: *const c_char,
    cb: Option<AmHubResultFn>,
    user_data: *mut c_void,
    out_call_id: *mut *mut c_char,
) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        unsafe { write_out_str(out_call_id, None) };
        // SAFETY: 同上。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let text = unsafe { req_str(request_json, "request_json") }?;
        let cb = cb.ok_or_else(|| FfiError::null("cb"))?;
        let mut req: CallRequest =
            serde_json::from_str(text).map_err(|e| FfiError::json("request_json", e))?;
        let call_id = req.call_id.get_or_insert_with(|| h.next_call_id()).clone();
        let fallback =
            outcome_error_json(&call_id, ToolError::new(ErrorKind::Cancelled, STOPPED_MESSAGE));
        let rc = ResultCb::new(cb, user_data, h.dispatcher.clone(), fallback);
        let id = call_id.clone();
        h.spawn_result(rc, move |hub| async move {
            match hub.call_tool(req).await {
                Ok(o) => serde_json::to_string(&o).unwrap_or_default(),
                Err(e) => outcome_error_json(&id, e.0),
            }
        })?;
        // SAFETY: 同上。
        unsafe { write_out_str(out_call_id, Some(&call_id)) };
        Ok(())
    })
}

/// v11：调用进度回调（spec/hub-api.md 3.12）。`progress_json` 归回调方所有。
pub type AmHubProgressFn = unsafe extern "C" fn(user_data: *mut c_void, progress_json: *mut c_char);

/// 一条进度 → `{"callId", "progress", "total"?, "message"?}`。
fn progress_json(call_id: &str, p: &ProgressUpdate) -> String {
    let mut v = json!({ "callId": call_id, "progress": p.progress });
    if let Some(t) = p.total {
        v["total"] = json!(t);
    }
    if let Some(m) = &p.message {
        v["message"] = json!(m);
    }
    v.to_string()
}

/// 把一条进度排到分发线程上回调（与结果回调同一队列，因此先于结果到达）。
fn post_progress(dispatcher: &Dispatcher, f: AmHubProgressFn, user_data: dispatch::SendPtr, json: String) {
    let job: Box<dyn FnOnce() + Send> = Box::new(move || {
        let ud = user_data;
        // SAFETY: 调用方提供的回调；字符串所有权转移给回调方。
        unsafe { f(ud.0, ffi_util::into_raw_cstring(&json)) };
    });
    // 分发线程已关闭（Hub 释放中）时丢弃进度：进度不保证送达。
    let _ = dispatcher.post(job);
}

/// v11：同 [`am_hub_call`]，并接收调用进度（`Hub::call_tool_with_progress`）。
///
/// @input on_progress 可为 NULL（等同 [`am_hub_call`]）；与 `cb` 共用 `user_data`。
/// @invariant 进度回调与结果回调在同一分发线程上串行执行，全部进度回调先于结果回调；结果回调之后不再有进度回调。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_call_with_progress(
    hub: *mut AmHub,
    request_json: *const c_char,
    cb: Option<AmHubResultFn>,
    on_progress: Option<AmHubProgressFn>,
    user_data: *mut c_void,
    out_call_id: *mut *mut c_char,
) -> AmHubStatus {
    let Some(on_progress) = on_progress else {
        // SAFETY: 由调用方保证。
        return unsafe { am_hub_call(hub, request_json, cb, user_data, out_call_id) };
    };
    guard(|| {
        // SAFETY: 由调用方保证。
        unsafe { write_out_str(out_call_id, None) };
        // SAFETY: 同上。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let text = unsafe { req_str(request_json, "request_json") }?;
        let cb = cb.ok_or_else(|| FfiError::null("cb"))?;
        let mut req: CallRequest =
            serde_json::from_str(text).map_err(|e| FfiError::json("request_json", e))?;
        let call_id = req.call_id.get_or_insert_with(|| h.next_call_id()).clone();
        let fallback =
            outcome_error_json(&call_id, ToolError::new(ErrorKind::Cancelled, STOPPED_MESSAGE));
        let rc = ResultCb::new(cb, user_data, h.dispatcher.clone(), fallback);
        let id = call_id.clone();
        let dispatcher = h.dispatcher.clone();
        let ud = dispatch::SendPtr(user_data);
        h.spawn_result(rc, move |hub| async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProgressUpdate>();
            let call = hub.call_tool_with_progress(req, tx);
            tokio::pin!(call);
            let res = loop {
                tokio::select! {
                    biased;
                    Some(p) = rx.recv() => post_progress(&dispatcher, on_progress, ud, progress_json(&id, &p)),
                    res = &mut call => break res,
                }
            };
            while let Ok(p) = rx.try_recv() {
                post_progress(&dispatcher, on_progress, ud, progress_json(&id, &p));
            }
            match res {
                Ok(o) => serde_json::to_string(&o).unwrap_or_default(),
                Err(e) => outcome_error_json(&id, e.0),
            }
        })?;
        // SAFETY: 同上。
        unsafe { write_out_str(out_call_id, Some(&call_id)) };
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_cancel_call(hub: *mut AmHub, call_id: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let id = unsafe { req_str(call_id, "call_id") }?;
        h.hub()?.cancel_call(id);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_read_resource(
    hub: *mut AmHub,
    uri: *const c_char,
    cb: Option<AmHubResultFn>,
    user_data: *mut c_void,
) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let uri = unsafe { req_str(uri, "uri") }?.to_owned();
        let cb = cb.ok_or_else(|| FfiError::null("cb"))?;
        let fallback = json!({
            "error": error_json(&ToolError::new(ErrorKind::Cancelled, STOPPED_MESSAGE))
        })
        .to_string();
        let rc = ResultCb::new(cb, user_data, h.dispatcher.clone(), fallback);
        h.spawn_result(rc, move |hub| async move {
            match hub.read_resource(&uri).await {
                Ok(c) => json!({ "ok": c }).to_string(),
                Err(e) => json!({ "error": error_json(&e.0) }).to_string(),
            }
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_subscribe(hub: *mut AmHub, uri: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let uri = unsafe { req_str(uri, "uri") }?;
        let inner = h.hub()?;
        let _rt = h.handle.enter();
        inner.subscribe(uri)?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_unsubscribe(hub: *mut AmHub, uri: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let uri = unsafe { req_str(uri, "uri") }?;
        let inner = h.hub()?;
        let _rt = h.handle.enter();
        inner.unsubscribe(uri);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_select_instance(
    hub: *mut AmHub,
    app_id: *const c_char,
    instance_id: *const c_char,
) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let app_id = unsafe { req_str(app_id, "app_id") }?;
        // SAFETY: 同上。
        let instance_id = unsafe { opt_str(instance_id, "instance_id") }?;
        h.hub()?.select_instance(app_id, instance_id);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_reset_session(hub: *mut AmHub, session: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let session = unsafe { opt_str(session, "session") }?;
        h.hub()?.reset_session(session);
        Ok(())
    })
}

/// 替换策略规则集（v10，spec/hub-api.md 3.13）。
///
/// @error JSON 不合法或有未知字段 → `InvalidJson`；规则不合法 → `InvalidConfig`（之前的规则继续生效，原因记入
/// `HubStatus.policy.lastError`）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_set_policy(hub: *mut AmHub, policy_json: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let text = unsafe { req_str(policy_json, "policy_json") }?;
        let policy: hub::PolicyConfig = serde_json::from_str(text).map_err(|e| FfiError::json("policy_json", e))?;
        h.hub()?
            .set_policy(policy)
            .map_err(|e| FfiError::new(AmHubStatus::InvalidConfig, e.0.message))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_dispatch(
    hub: *mut AmHub,
    format: c_int,
    tool_call_json: *const c_char,
    session: *const c_char,
    cb: Option<AmHubResultFn>,
    user_data: *mut c_void,
) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        let format = format_from(format)?;
        // SAFETY: 同上。
        let text = unsafe { req_str(tool_call_json, "tool_call_json") }?;
        // SAFETY: 同上。
        let session = unsafe { opt_str(session, "session") }?.map(str::to_owned);
        let cb = cb.ok_or_else(|| FfiError::null("cb"))?;
        let call: Value =
            serde_json::from_str(text).map_err(|e| FfiError::json("tool_call_json", e))?;
        let fallback = dispatch_fallback(format, &call);
        let rc = ResultCb::new(cb, user_data, h.dispatcher.clone(), fallback);
        h.spawn_result(rc, move |hub| async move {
            hub.dispatch_in_session(format, call, session.as_deref())
                .await
                .to_string()
        })
    })
}

// ---------------------------------------------------------------------------
// 事件与策略回调
// ---------------------------------------------------------------------------

/// 设置常驻回调的公共部分：user_data 无论成败都归库所有。
unsafe fn set_slot<F>(
    hub: *mut AmHub,
    cb: Option<F>,
    user_data: *mut c_void,
    free_user_data: Option<AmHubFreeFn>,
    pick: impl FnOnce(&AmHub) -> &Arc<Slot<F>>,
) -> AmHubStatus {
    let ud = UserData::new(user_data, free_user_data);
    guard(move || {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        h.hub()?;
        pick(h).set(cb.map(|f| CallbackEntry { f, user_data: ud }));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_set_event_cb(
    hub: *mut AmHub,
    cb: Option<AmHubEventFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmHubFreeFn>,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe { set_slot(hub, cb, user_data, free_user_data, |h| &h.events) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_set_approval_cb(
    hub: *mut AmHub,
    cb: Option<AmHubApprovalFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmHubFreeFn>,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe { set_slot(hub, cb, user_data, free_user_data, |h| &h.approval) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_set_pairing_cb(
    hub: *mut AmHub,
    cb: Option<AmHubPairingFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmHubFreeFn>,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    let status = unsafe { set_slot(hub, cb, user_data, free_user_data, |h| &h.pairing) };
    if status == AmHubStatus::Ok && cb.is_some() {
        // 首次设置时才向 Hub 注册处理器（未设置时保持 Hub 默认的白名单行为）。
        return guard(|| {
            // SAFETY: 由调用方保证；set_slot 已检查非 NULL。
            let h = unsafe { hub_ref(hub) }?;
            if !h.pairing_installed.swap(true, Ordering::SeqCst) {
                h.hub()?.set_pairing_handler(Arc::new(CPairingHandler {
                    slot: h.pairing.clone(),
                    dispatcher: h.dispatcher.clone(),
                }));
            }
            Ok(())
        });
    }
    status
}

fn complete(tx: oneshot::Sender<bool>, approved: bool) -> FfiResult<()> {
    tx.send(approved).map_err(|_| {
        FfiError::new(AmHubStatus::AlreadyCompleted, "请求已超时或已取消，结果被忽略")
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_approval_complete(approval: *mut AmHubApproval, approved: bool) -> AmHubStatus {
    guard(|| {
        if approval.is_null() {
            return Err(FfiError::null("approval"));
        }
        // SAFETY: 由回调交出，按约定只消费一次。
        let b = unsafe { Box::from_raw(approval) };
        complete(b.tx, approved)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_pairing_complete(pairing: *mut AmHubPairing, approved: bool) -> AmHubStatus {
    guard(|| {
        if pairing.is_null() {
            return Err(FfiError::null("pairing"));
        }
        // SAFETY: 由回调交出，按约定只消费一次。
        let b = unsafe { Box::from_raw(pairing) };
        complete(b.tx, approved)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_set_waker_cb(
    hub: *mut AmHub,
    cb: Option<AmHubWakerFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmHubFreeFn>,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    let status = unsafe { set_slot(hub, cb, user_data, free_user_data, |h| &h.waker) };
    if status != AmHubStatus::Ok {
        return status;
    }
    guard(|| {
        // SAFETY: 由调用方保证；set_slot 已检查非 NULL。
        let h = unsafe { hub_ref(hub) }?;
        let inner = h.hub()?;
        if cb.is_some() {
            inner.set_waker(Arc::new(CWaker {
                slot: h.waker.clone(),
                dispatcher: h.dispatcher.clone(),
            }));
        } else {
            // 清除：恢复配置 waker 决定的实现（默认系统唤醒）。
            inner.reset_waker();
        }
        Ok(())
    })
}

/// 协议错误类别字符串 → [`ErrorKind`]；NULL / 空 / 未知 → `LAUNCH_FAILED`。
fn parse_error_kind(s: Option<&str>) -> ErrorKind {
    s.filter(|k| !k.is_empty())
        .and_then(|k| serde_json::from_value(Value::String(k.to_owned())).ok())
        .unwrap_or(ErrorKind::LaunchFailed)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_waker_complete(
    wake: *mut AmHubWake,
    ok: bool,
    error_kind: *const c_char,
    message: *const c_char,
) -> AmHubStatus {
    guard(|| {
        if wake.is_null() {
            return Err(FfiError::null("wake"));
        }
        // SAFETY: 由回调交出，按约定只消费一次。
        let b = unsafe { Box::from_raw(wake) };
        let result = if ok {
            Ok(())
        } else {
            // 字符串不合法时仍以失败完成（不能丢弃句柄的结果）。
            // SAFETY: 由调用方保证。
            let kind = parse_error_kind(unsafe { opt_str(error_kind, "error_kind") }.ok().flatten());
            // SAFETY: 同上。
            let msg = unsafe { opt_str(message, "message") }
                .ok()
                .flatten()
                .filter(|m| !m.is_empty())
                .unwrap_or("唤醒失败。")
                .to_owned();
            Err(ToolError::new(kind, msg))
        };
        b.tx.send(result).map_err(|_| {
            FfiError::new(AmHubStatus::AlreadyCompleted, "唤醒已超时或 Hub 已停止，结果被忽略")
        })
    })
}

#[cfg(test)]
mod tests;
