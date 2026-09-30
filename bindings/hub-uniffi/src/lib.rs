//! Hub SDK 的 uniffi 绑定（proc-macro 方式）：把 `app-mcp-hub` 暴露给 Kotlin、Swift、Python
//! （spec/hub-api.md 第 2 节）。
//!
//! # 设计
//!
//! - 对象 [`AppMcpHub`]：自带 tokio 多线程运行时，包装 `app_mcp_hub::Hub`。
//! - 记录 / 枚举 / 错误见 [`types`]；任意 JSON 以文本传递。
//! - `call_tool` / `read_resource` / `dispatch` / `serve_http` 为 **uniffi async**
//!   （Kotlin `suspend`、Swift `async`、Python `async def`）。Hub 的 future 在自带运行时上执行，
//!   外部语言只轮询 `JoinHandle`，因此不依赖外部语言的执行器类型。
//!   `call_tool` 的 future 被外部取消（如 Kotlin 协程取消）时，自动 `cancel_call`。
//! - 事件：外部实现 [`HubEventListener`]，在专用分发线程上**同步**回调（须尽快返回）；
//!   接收方落后时回调 `on_lagged(skipped)`。
//! - 审批 / 配对：外部实现 **async** 回调接口 [`ApprovalHandler`] / [`PairingHandler`]
//!   （uniffi async foreign trait）。外部抛出异常（含未预期异常）或 Rust 侧 panic 均视为拒绝。
//! - 唤醒：外部实现 **async** 回调接口 [`HubWaker`]（如 Android 厂商发送显式广播），
//!   `set_waker` 替换默认的系统唤醒实现。抛出 `WakeError` 以指定错误类别结束调用，
//!   未预期异常 / panic 按 `LAUNCH_FAILED`。
//!
//! 外部同步回调抛出的未预期异常在 uniffi 里会变成 panic；适配器用 `catch_unwind` 兜底。

pub mod types;

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use app_mcp_hub as hub;
use futures::FutureExt;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

pub use types::*;

uniffi::setup_scaffolding!();

// ---------------------------------------------------------------------------
// 回调接口（由外部语言实现）
// ---------------------------------------------------------------------------

/// Hub 事件监听。在专用分发线程上同步调用，必须尽快返回（需要时自行切换线程）。
#[uniffi::export(foreign)]
pub trait HubEventListener: Send + Sync {
    fn on_event(&self, event: HubEvent);
    /// 监听方处理过慢，跳过了 `skipped` 个事件；应重新拉取 `apps()` / `tools()`。
    fn on_lagged(&self, skipped: u64);
}

/// 厂商 UI 接管调用确认。返回 `false` 或抛出异常 → 调用以 `USER_REJECTED` 结束；超时同样视为拒绝。
#[uniffi::export(foreign)]
#[async_trait::async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn approve(&self, request: ApprovalRequest) -> Result<bool, CallbackError>;
}

/// 厂商 UI 接管 App 配对。返回 `false` 或抛出异常 → 拒绝配对；超时同样视为拒绝。
#[uniffi::export(foreign)]
#[async_trait::async_trait]
pub trait PairingHandler: Send + Sync {
    async fn pair(&self, request: PairingRequest) -> Result<bool, CallbackError>;
}

/// 自定义唤醒（spec/hub-api.md 3.5）。正常返回表示已发出激活，Hub 随后等待 App 回连
/// （`HubConfig.wake_timeout_ms`）；抛出 `WakeError::Failed` 以该类别结束调用。
#[uniffi::export(foreign)]
#[async_trait::async_trait]
pub trait HubWaker: Send + Sync {
    async fn wake(&self, request: WakeRequest) -> Result<(), WakeError>;
}

struct WakerAdapter(Arc<dyn HubWaker>);

#[async_trait::async_trait]
impl hub::Waker for WakerAdapter {
    async fn wake(&self, req: hub::WakeRequest) -> Result<(), hub::HubError> {
        let req = WakeRequest::from(req);
        let launch_failed = |m: &str| hub::HubError::new(hub::ErrorKind::LaunchFailed, m);
        let fut = match catch_unwind(AssertUnwindSafe(|| self.0.wake(req))) {
            Ok(fut) => fut,
            Err(_) => return Err(launch_failed("唤醒回调 panic")),
        };
        match AssertUnwindSafe(fut).catch_unwind().await {
            Ok(r) => r.map_err(Into::into),
            Err(_) => Err(launch_failed("唤醒回调 panic")),
        }
    }
}

/// 执行外部同步回调，吞掉 panic。返回是否正常结束。
fn guarded(f: impl FnOnce()) -> bool {
    catch_unwind(AssertUnwindSafe(f)).is_ok()
}

/// 执行外部 async 回调：异常、panic 都视为 `false`。
async fn guarded_bool<F>(f: impl FnOnce() -> F) -> bool
where
    F: Future<Output = Result<bool, CallbackError>>,
{
    let fut = match catch_unwind(AssertUnwindSafe(f)) {
        Ok(fut) => fut,
        Err(_) => return false,
    };
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::warn!("外部回调失败，视为拒绝：{e}");
            false
        }
        Err(_) => {
            tracing::warn!("外部回调 panic，视为拒绝");
            false
        }
    }
}

struct ApprovalAdapter(Arc<dyn ApprovalHandler>);

#[async_trait::async_trait]
impl hub::ApprovalHandler for ApprovalAdapter {
    async fn approve(&self, req: hub::ApprovalRequest) -> bool {
        let req = ApprovalRequest::from(req);
        guarded_bool(|| self.0.approve(req)).await
    }
}

struct PairingAdapter(Arc<dyn PairingHandler>);

#[async_trait::async_trait]
impl hub::PairingHandler for PairingAdapter {
    async fn pair(&self, req: hub::PairingRequest) -> bool {
        let req = PairingRequest::from(req);
        guarded_bool(|| self.0.pair(req)).await
    }
}

// ---------------------------------------------------------------------------
// 顶层函数
// ---------------------------------------------------------------------------

/// 解析格式名：`mcp`、`openai-chat`（或 `openai`）、`openai-responses`、`anthropic`、`gemini`，
/// 不区分大小写，`-` / `_` 可省略。
#[uniffi::export]
pub fn parse_tool_format(name: String) -> Result<ToolFormat, HubError> {
    name.parse::<hub::ToolFormat>()
        .map(Into::into)
        .map_err(|detail| HubError::InvalidConfig { detail })
}

/// 把 Hub 日志（tracing）输出到 stderr。`filter` 同 `RUST_LOG` 语法，为空时读 `RUST_LOG`，
/// 再为空时为 `info`。只有第一次调用生效；返回是否本次完成了初始化。
#[uniffi::export]
pub fn init_logging(filter: Option<String>) -> bool {
    use tracing_subscriber::EnvFilter;
    let filter = match filter {
        Some(f) => EnvFilter::try_new(f).unwrap_or_else(|_| EnvFilter::new("info")),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .is_ok()
}

// ---------------------------------------------------------------------------
// AppMcpHub
// ---------------------------------------------------------------------------

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 在运行时上执行 `fut` 并阻塞等待。若当前线程已在某个 tokio 运行时内（不能 `block_on`），
/// 改到临时线程上等待。
fn block_on<F>(handle: &Handle, fut: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    if Handle::try_current().is_err() {
        return handle.block_on(fut);
    }
    let h = handle.clone();
    std::thread::scope(|s| {
        s.spawn(move || h.block_on(fut))
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p))
    })
}

/// 生成 callId（`call_tool` 未指定时），用于外部取消时 `cancel_call`。
fn new_call_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    format!("ffi-{:x}-{nanos:08x}-{seq}", std::process::id())
}

/// `call_tool` 的 future 被外部丢弃（取消）而调用尚未结束时，取消该调用。
struct CancelOnDrop {
    hub: Arc<hub::Hub>,
    call_id: String,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.hub.cancel_call(&self.call_id);
        }
    }
}

/// 嵌入式 Hub。创建时启动自带的 tokio 运行时与 Hub 后台任务；
/// `shutdown` 或对象被释放（最后一个引用消失）时停止。
#[derive(uniffi::Object)]
pub struct AppMcpHub {
    hub: Mutex<Option<Arc<hub::Hub>>>,
    handle: Handle,
    runtime: Mutex<Option<Runtime>>,
    events_task: Mutex<Option<JoinHandle<()>>>,
    ws_addr: Option<String>,
}

impl std::fmt::Debug for AppMcpHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppMcpHub")
            .field("ws_addr", &self.ws_addr)
            .finish_non_exhaustive()
    }
}

impl AppMcpHub {
    fn hub(&self) -> Result<Arc<hub::Hub>, HubError> {
        lock(&self.hub).clone().ok_or(HubError::Shutdown)
    }

    /// 在自带运行时上执行，外部语言等待 `JoinHandle`（与执行器无关）。
    async fn run<F>(&self, fut: F) -> Result<F::Output, HubError>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.handle.spawn(fut).await.map_err(|_| HubError::Shutdown)
    }
}

#[uniffi::export]
impl AppMcpHub {
    /// 启动 Hub：绑定 App 连接服务（若开启）并启动后台任务。
    #[uniffi::constructor]
    pub fn start(config: HubConfig) -> Result<Arc<Self>, HubError> {
        let cfg = config.into_hub()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("app-mcp-hub")
            .enable_all()
            .build()?;
        let handle = runtime.handle().clone();
        let hub = block_on(&handle, hub::Hub::start(cfg))?;
        let ws_addr = hub.ws_addr().map(|a| a.to_string());
        Ok(Arc::new(AppMcpHub {
            hub: Mutex::new(Some(Arc::new(hub))),
            handle,
            runtime: Mutex::new(Some(runtime)),
            events_task: Mutex::new(None),
            ws_addr,
        }))
    }

    /// App 连接服务实际监听的地址（`127.0.0.1:12345`）；未开启时为空。
    pub fn ws_addr(&self) -> Option<String> {
        self.ws_addr.clone()
    }

    /// 停止：关闭所有 App 连接、中止后台任务（含上游子进程）与运行时。幂等；之后的操作返回 `Shutdown`。
    pub fn shutdown(&self) {
        if let Some(t) = lock(&self.events_task).take() {
            t.abort();
        }
        let hub = lock(&self.hub).take();
        if let Some(hub) = hub {
            match Arc::try_unwrap(hub) {
                Ok(hub) => block_on(&self.handle, hub.shutdown()),
                // 仍有进行中的调用持有引用：运行时关闭时随任务一起释放（Drop 中止后台任务）。
                Err(shared) => drop(shared),
            }
        }
        if let Some(rt) = lock(&self.runtime).take() {
            rt.shutdown_background();
        }
    }

    // ---- 查询 ----

    /// 所有已知 App（静态清单、已连接实例）与上游（`kind = Upstream`）。
    pub fn apps(&self) -> Vec<AppInfo> {
        let Ok(hub) = self.hub() else { return Vec::new() };
        let _g = self.handle.enter();
        hub.apps().into_iter().map(Into::into).collect()
    }

    pub fn tools(&self, filter: ToolFilter) -> Vec<HubTool> {
        let Ok(hub) = self.hub() else { return Vec::new() };
        let _g = self.handle.enter();
        hub.tools(&filter.into())
            .into_iter()
            .map(Into::into)
            .collect()
    }

    pub fn resources(&self) -> Vec<HubResource> {
        let Ok(hub) = self.hub() else { return Vec::new() };
        let _g = self.handle.enter();
        hub.resources().into_iter().map(Into::into).collect()
    }

    pub fn overview(&self, app_id: String) -> Option<AppOverviewInfo> {
        let hub = self.hub().ok()?;
        let _g = self.handle.enter();
        hub.overview(&app_id).map(Into::into)
    }

    // ---- 操作 ----

    /// 调用工具。工具层面的失败（参数不合法、用户拒绝、超时、App 报错…）放在 `CallOutcome.error`；
    /// 只有名称无法解析（appId 未知 / 格式不对）时返回 `HubError::Tool`。
    pub async fn call_tool(&self, request: CallRequest) -> Result<CallOutcome, HubError> {
        let hub = self.hub()?;
        let mut req = request.into_hub()?;
        let call_id = req.call_id.get_or_insert_with(new_call_id).clone();
        let mut guard = CancelOnDrop {
            hub: hub.clone(),
            call_id,
            armed: true,
        };
        let res = self.run(async move { hub.call_tool(req).await }).await;
        guard.armed = false;
        Ok(res??.into())
    }

    /// 取消进行中的调用（结果为 `CANCELLED`）。未知 callId 忽略。
    pub fn cancel_call(&self, call_id: String) {
        if let Ok(hub) = self.hub() {
            let _g = self.handle.enter();
            hub.cancel_call(&call_id);
        }
    }

    /// 读取资源（`app-mcp://<appId>/<name>`）。
    pub async fn read_resource(&self, uri: String) -> Result<ResourceContent, HubError> {
        let hub = self.hub()?;
        let res = self.run(async move { hub.read_resource(&uri).await }).await?;
        Ok(res?.into())
    }

    /// 订阅资源变化，之后收到 `HubEvent.ResourceUpdated`。
    pub fn subscribe(&self, uri: String) -> Result<(), HubError> {
        let hub = self.hub()?;
        let _g = self.handle.enter();
        Ok(hub.subscribe(&uri)?)
    }

    pub fn unsubscribe(&self, uri: String) {
        if let Ok(hub) = self.hub() {
            let _g = self.handle.enter();
            hub.unsubscribe(&uri);
        }
    }

    /// 指定某 App 的目标实例（所有会话共用）；`instance_id` 为空时清除。
    pub fn select_instance(&self, app_id: String, instance_id: Option<String>) {
        if let Ok(hub) = self.hub() {
            hub.select_instance(&app_id, instance_id.as_deref());
        }
    }

    /// 清除某会话的状态（已附带的总览、`apps.select`）。开始新对话时调用。
    pub fn reset_session(&self, session: Option<String>) {
        if let Ok(hub) = self.hub() {
            hub.reset_session(session.as_deref());
        }
    }

    // ---- 事件与策略回调 ----

    /// 设置事件监听（替换之前的；为空时移除）。事件在专用分发线程上按顺序回调。
    pub fn set_event_listener(&self, listener: Option<Arc<dyn HubEventListener>>) {
        let mut slot = lock(&self.events_task);
        if let Some(t) = slot.take() {
            t.abort();
        }
        let (Some(listener), Ok(hub)) = (listener, self.hub()) else {
            return;
        };
        // 同步订阅：设置之后发生的事件不会丢。
        let mut rx = hub.events();
        drop(hub);
        let (tx, deliveries) = std::sync::mpsc::channel::<Delivery>();
        let spawned = std::thread::Builder::new()
            .name("app-mcp-hub-events".into())
            .spawn(move || {
                // 发送端（转发任务）结束时退出。
                while let Ok(d) = deliveries.recv() {
                    match d {
                        Delivery::Event(e) => guarded(|| listener.on_event(e)),
                        Delivery::Lagged(n) => guarded(|| listener.on_lagged(n)),
                    };
                }
            });
        if let Err(e) = spawned {
            tracing::error!("无法创建事件分发线程：{e}");
            return;
        }
        *slot = Some(self.handle.spawn(async move {
            loop {
                let d = match rx.recv().await {
                    Ok(e) => Delivery::Event(e.into()),
                    Err(RecvError::Lagged(n)) => Delivery::Lagged(n),
                    Err(RecvError::Closed) => break,
                };
                if tx.send(d).is_err() {
                    break;
                }
            }
        }));
    }

    /// 设置审批处理器（替换之前的）。审批阈值由 `HubConfig.approval_min_risk` 决定；
    /// 策略要求审批但未设置处理器 → `USER_REJECTED`。
    pub fn set_approval_handler(&self, handler: Arc<dyn ApprovalHandler>) {
        if let Ok(hub) = self.hub() {
            hub.set_approval_handler(Arc::new(ApprovalAdapter(handler)));
        }
    }

    /// 设置配对处理器（替换之前的）。设置后，无静态清单或 Origin 不在白名单的 App
    /// 首次连接时先询问处理器；未设置时白名单内直接配对、白名单外拒绝。
    pub fn set_pairing_handler(&self, handler: Arc<dyn PairingHandler>) {
        if let Ok(hub) = self.hub() {
            hub.set_pairing_handler(Arc::new(PairingAdapter(handler)));
        }
    }

    /// 设置自定义唤醒（替换之前的）；为空时恢复默认的系统唤醒实现（按平台执行系统命令；
    /// Android 上默认实现不支持 `android-intent`，厂商需要提供）。
    pub fn set_waker(&self, waker: Option<Arc<dyn HubWaker>>) {
        if let Ok(hub) = self.hub() {
            match waker {
                Some(w) => hub.set_waker(Arc::new(WakerAdapter(w))),
                None => hub.set_waker(Arc::new(hub::SystemWaker::new())),
            }
        }
    }

    // ---- 格式导出与分派 ----

    /// 按格式导出工具定义（JSON 文本；名称已编码为 `[a-zA-Z0-9_-]{1,64}`）。
    pub fn export_tools(&self, format: ToolFormat, filter: ToolFilter) -> String {
        let Ok(hub) = self.hub() else {
            return empty_export(format);
        };
        let _g = self.handle.enter();
        hub.export_tools(format.into(), &filter.into()).to_string()
    }

    /// 执行模型返回的一个工具调用（该格式的 JSON 文本），返回该格式的“工具结果”消息（JSON 文本）。
    /// 工具失败以该格式的错误结果返回；只有输入不是合法 JSON 时返回 `InvalidJson`。
    pub async fn dispatch(
        &self,
        format: ToolFormat,
        tool_call_json: String,
        session: Option<String>,
    ) -> Result<String, HubError> {
        let hub = self.hub()?;
        let call = parse_json(&tool_call_json)?;
        let out = self
            .run(async move {
                hub.dispatch_in_session(format.into(), call, session.as_deref())
                    .await
            })
            .await?;
        Ok(out.to_string())
    }

    // ---- MCP 出口 ----

    /// 启动 Streamable HTTP MCP 服务（`http://<addr>/mcp`），返回实际监听地址。
    /// 非回环地址需要 `allow_remote`。
    pub async fn serve_http(&self, addr: String, allow_remote: bool) -> Result<String, HubError> {
        let hub = self.hub()?;
        let local = self
            .run(async move { hub.serve_http(&addr, allow_remote).await })
            .await??;
        Ok(local.to_string())
    }
}

impl Drop for AppMcpHub {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum Delivery {
    Event(HubEvent),
    Lagged(u64),
}

fn empty_export(format: ToolFormat) -> String {
    match format {
        ToolFormat::Gemini => r#"{"functionDeclarations":[]}"#.to_owned(),
        _ => "[]".to_owned(),
    }
}

#[cfg(test)]
mod tests;
