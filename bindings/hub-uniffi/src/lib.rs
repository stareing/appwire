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
//! - 进度：`call_tool_with_progress` 的 [`ProgressListener`] 在专用阻塞线程上按顺序**同步**回调，
//!   全部回调在调用结果返回之前完成。
//! - 事件：外部实现 [`HubEventListener`]，在专用分发线程上**同步**回调（须尽快返回）；
//!   接收方落后时回调 `on_lagged(skipped)`。
//! - 审批 / 配对 / 唤醒：外部实现**同步**回调接口 [`ApprovalHandler`] / [`PairingHandler`] /
//!   [`HubWaker`]，回调收到一个完成句柄（[`ApprovalResponder`] / [`PairingResponder`] /
//!   [`WakeResponder`]），在任意线程、任意时刻给出结果。回调本身在 Hub 的阻塞线程上调用、应尽快返回；
//!   外部语言不需要在回调线程上有事件循环 / 协程上下文——各语言封装把句柄适配为惯用的
//!   `suspend` / `async` / `asyncio`。句柄未完成即被释放、回调抛出异常 → 拒绝（唤醒为 `LAUNCH_FAILED`）。
//!
//! 外部同步回调抛出的未预期异常在 uniffi 里会变成 panic；适配器用 `catch_unwind` 兜底。

pub mod naming;
pub mod types;

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use app_mcp_hub as hub;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::oneshot;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

pub use naming::{DialOutcome, HubNameService, NamedApp};
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

/// 调用进度接收方（[`AppMcpHub::call_tool_with_progress`]）。在专用阻塞线程上按顺序同步调用，必须尽快返回；
/// 同一次调用的全部进度回调都在该调用的结果返回之前完成。
#[uniffi::export(foreign)]
pub trait ProgressListener: Send + Sync {
    fn on_progress(&self, update: ProgressUpdate);
}

/// 厂商 UI 接管调用确认。
///
/// 同步回调：实现应尽快返回（不要在回调线程上等待用户），在任意线程、任意时刻调用
/// `responder.complete(approved)` 给出结果。`complete(false)`、回调抛出异常、`responder` 未完成即被释放
/// → 调用以 `USER_REJECTED` 结束；超时（`HubConfig.approval_timeout_ms`）同样视为拒绝。
#[uniffi::export(foreign)]
pub trait ApprovalHandler: Send + Sync {
    fn on_request(&self, request: ApprovalRequest, responder: Arc<ApprovalResponder>);
}

/// 厂商 UI 接管 App 配对。约定同 [`ApprovalHandler`]：`responder.complete(approved)`，
/// 未完成即释放 / 异常 / 超时 → 拒绝配对。
#[uniffi::export(foreign)]
pub trait PairingHandler: Send + Sync {
    fn on_request(&self, request: PairingRequest, responder: Arc<PairingResponder>);
}

/// 自定义唤醒（spec/hub-api.md 3.5）。同步回调，尽快返回；发出激活后调用 `responder.succeed()`
/// （Hub 随后等待 App 回连，`HubConfig.wake_timeout_ms`），失败时 `responder.fail(kind, reason)`
/// 以该错误类别结束调用。回调抛出异常、`responder` 未完成即被释放 → `LAUNCH_FAILED`。
#[uniffi::export(foreign)]
pub trait HubWaker: Send + Sync {
    fn wake(&self, request: WakeRequest, responder: Arc<WakeResponder>);
}

/// 一次性结果通道：只有第一次 `complete` 生效；未完成即释放时接收方得到 `Err`。
struct Once<T>(Mutex<Option<oneshot::Sender<T>>>);

impl<T> Once<T> {
    fn new() -> (Self, oneshot::Receiver<T>) {
        let (tx, rx) = oneshot::channel();
        (Self(Mutex::new(Some(tx))), rx)
    }

    fn complete(&self, value: T) -> bool {
        lock(&self.0).take().is_some_and(|tx| tx.send(value).is_ok())
    }
}

/// [`ApprovalHandler`] 的结果句柄。可在任意线程调用，可在回调返回后调用；只有第一次调用生效。
#[derive(uniffi::Object)]
pub struct ApprovalResponder(Once<bool>);

#[uniffi::export]
impl ApprovalResponder {
    /// 给出审批结果。返回本次是否生效（已完成、Hub 已不再等待时为 `false`）。
    pub fn complete(&self, approved: bool) -> bool {
        self.0.complete(approved)
    }
}

/// [`PairingHandler`] 的结果句柄。约定同 [`ApprovalResponder`]。
#[derive(uniffi::Object)]
pub struct PairingResponder(Once<bool>);

#[uniffi::export]
impl PairingResponder {
    /// 给出配对结果。返回本次是否生效。
    pub fn complete(&self, approved: bool) -> bool {
        self.0.complete(approved)
    }
}

/// [`HubWaker`] 的结果句柄。可在任意线程调用，可在回调返回后调用；只有第一次调用生效。
#[derive(uniffi::Object)]
pub struct WakeResponder(Once<Result<(), hub::HubError>>);

#[uniffi::export]
impl WakeResponder {
    /// 已发出激活，Hub 等待 App 回连。返回本次是否生效。
    pub fn succeed(&self) -> bool {
        self.0.complete(Ok(()))
    }

    /// 唤醒失败：`kind` 为协议错误类别（如 `"APP_NOT_INSTALLED"`；不认识的类别按 `LAUNCH_FAILED`）。
    /// 返回本次是否生效。
    pub fn fail(&self, kind: String, reason: String) -> bool {
        self.0.complete(Err(wake_error(&kind, reason)))
    }
}

/// 在阻塞线程上调用外部同步回调（外部实现即使阻塞也不占用运行时工作线程），吞掉 panic。
async fn call_foreign(what: &'static str, f: impl FnOnce() + Send + 'static) {
    let ok = tokio::task::spawn_blocking(move || guarded(f)).await.unwrap_or(false);
    if !ok {
        tracing::warn!("{what}回调抛出异常");
    }
}

struct ApprovalAdapter(Arc<dyn ApprovalHandler>);

#[async_trait::async_trait]
impl hub::ApprovalHandler for ApprovalAdapter {
    async fn approve(&self, req: hub::ApprovalRequest) -> bool {
        let (once, rx) = Once::new();
        let (handler, request) = (self.0.clone(), ApprovalRequest::from(req));
        call_foreign("审批", move || handler.on_request(request, Arc::new(ApprovalResponder(once)))).await;
        // 回调异常 / 句柄未完成即释放：发送端被丢弃，视为拒绝。
        rx.await.unwrap_or(false)
    }
}

struct PairingAdapter(Arc<dyn PairingHandler>);

#[async_trait::async_trait]
impl hub::PairingHandler for PairingAdapter {
    async fn pair(&self, req: hub::PairingRequest) -> bool {
        let (once, rx) = Once::new();
        let (handler, request) = (self.0.clone(), PairingRequest::from(req));
        call_foreign("配对", move || handler.on_request(request, Arc::new(PairingResponder(once)))).await;
        rx.await.unwrap_or(false)
    }
}

struct WakerAdapter(Arc<dyn HubWaker>);

#[async_trait::async_trait]
impl hub::Waker for WakerAdapter {
    async fn wake(&self, req: hub::WakeRequest) -> Result<(), hub::HubError> {
        let (once, rx) = Once::new();
        let (waker, request) = (self.0.clone(), WakeRequest::from(req));
        call_foreign("唤醒", move || waker.wake(request, Arc::new(WakeResponder(once)))).await;
        rx.await.unwrap_or_else(|_| {
            Err(hub::HubError::new(
                hub::ErrorKind::LaunchFailed,
                "唤醒回调没有给出结果（WakeResponder 未完成即被释放，或回调抛出异常）。",
            ))
        })
    }
}

/// 执行外部同步回调，吞掉 panic。返回是否正常结束。
fn guarded(f: impl FnOnce()) -> bool {
    catch_unwind(AssertUnwindSafe(f)).is_ok()
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
    listen_addr: Option<String>,
    ipc_endpoint: Option<String>,
    /// [`AppMcpHub::start_with_name_service`] 的连接器：宿主推送安装 / 卸载事件经它送达 Hub。
    #[cfg(unix)]
    name_service: Option<Arc<hub::connector::HostedConnector>>,
}

impl std::fmt::Debug for AppMcpHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppMcpHub")
            .field("listen_addr", &self.listen_addr)
            .field("ipc_endpoint", &self.ipc_endpoint)
            .finish_non_exhaustive()
    }
}

impl AppMcpHub {
    #[cfg(unix)]
    fn launch(cfg: hub::HubConfig, name_service: Option<Arc<hub::connector::HostedConnector>>) -> Result<Arc<Self>, HubError> {
        let (hub, handle, runtime) = Self::boot(cfg)?;
        Ok(Arc::new(Self::assemble(hub, handle, runtime, name_service)))
    }

    #[cfg(not(unix))]
    fn launch(cfg: hub::HubConfig, _name_service: Option<()>) -> Result<Arc<Self>, HubError> {
        let (hub, handle, runtime) = Self::boot(cfg)?;
        Ok(Arc::new(Self::assemble(hub, handle, runtime)))
    }

    fn boot(cfg: hub::HubConfig) -> Result<(hub::Hub, Handle, Runtime), HubError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("app-mcp-hub")
            .enable_all()
            .build()?;
        let handle = runtime.handle().clone();
        let hub = block_on(&handle, hub::Hub::start(cfg))?;
        Ok((hub, handle, runtime))
    }

    fn assemble(
        hub: hub::Hub,
        handle: Handle,
        runtime: Runtime,
        #[cfg(unix)] name_service: Option<Arc<hub::connector::HostedConnector>>,
    ) -> Self {
        let listen_addr = hub.listen_addr().map(|a| a.to_string());
        let ipc_endpoint = hub.ipc_endpoint().map(str::to_owned);
        AppMcpHub {
            hub: Mutex::new(Some(Arc::new(hub))),
            handle,
            runtime: Mutex::new(Some(runtime)),
            events_task: Mutex::new(None),
            listen_addr,
            ipc_endpoint,
            #[cfg(unix)]
            name_service,
        }
    }

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
        Self::launch(config.into_hub()?, None)
    }

    /// 启动 Hub，并以宿主实现的名字服务按名寻址（spec/naming.md 4.2 Android、spec/hub-api.md 3.16）：Hub 启动时调用一次
    /// `discover`（只读安装元数据，清单中的工具随即可列出）；调用到达而 App 没有活连接时 `dial`，通道宽限
    /// （`HubConfig.channel_grace_ms`）后关闭并 `release`。之后的安装 / 卸载由宿主经 [`AppMcpHub::name_service_installed`] /
    /// [`AppMcpHub::name_service_removed`] 推送。`kind` 为发现记录的来源名（`"android"`；其他值记为 `"host"`）。
    /// 非 Unix 平台返回 `Unsupported`。
    #[uniffi::constructor]
    pub fn start_with_name_service(
        config: HubConfig,
        kind: String,
        service: Arc<dyn HubNameService>,
    ) -> Result<Arc<Self>, HubError> {
        #[cfg(unix)]
        {
            let connector = Arc::new(hub::connector::HostedConnector::new(
                naming::source_kind(&kind),
                Arc::new(naming::NameServiceAdapter(service)),
            ));
            let mut cfg = config.into_hub()?;
            cfg.connectors.push(connector.clone());
            Self::launch(cfg, Some(connector))
        }
        #[cfg(not(unix))]
        {
            let _ = (config, kind, service);
            Err(HubError::Unsupported { detail: "本平台没有宿主名字服务（按名寻址仅支持 Unix 上以 fd 交换通道）".to_owned() })
        }
    }

    /// HTTP 服务（`/app`、`/healthz`）实际监听的地址（`127.0.0.1:12345`，App 端点为 `ws://<地址>/app`）；未开启时为空。
    pub fn listen_addr(&self) -> Option<String> {
        self.listen_addr.clone()
    }

    /// 本地 IPC 连接服务的端点（可直接作为原生 SDK 的 `host_url`）；未开启时为空。
    pub fn ipc_endpoint(&self) -> Option<String> {
        self.ipc_endpoint.clone()
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

    /// 运行状态（spec/hub-api.md 3.9）：身份、监听位置、令牌策略、各 App 与实例的状态、最近错误与 SDK 诊断上报。
    /// 与 `GET /status` 内容相同。已停止时返回 `Shutdown`。
    pub fn status(&self) -> Result<HubStatus, HubError> {
        let hub = self.hub()?;
        let _g = self.handle.enter();
        Ok(hub.status().into())
    }

    /// 生效的策略规则、各规则命中次数与最近的加载错误（spec/hub-api.md 3.13）。已停止时返回 `Shutdown`。
    pub fn policy(&self) -> Result<PolicyStatus, HubError> {
        let hub = self.hub()?;
        Ok(hub.policy().into())
    }

    /// 替换策略规则集（命中计数清零）。规则不合法时返回 `HubError::Tool`（`kind = "INVALID_INPUT"`），
    /// 之前的规则继续生效，错误记入 `policy().last_error`。
    pub fn set_policy(&self, policy: PolicyConfig) -> Result<(), HubError> {
        let hub = self.hub()?;
        hub.set_policy(policy.into()).map_err(Into::into)
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

    /// 同 [`AppMcpHub::call_tool`]，并接收调用进度（spec/hub-api.md 3.12）：App 报告的进度经 Hub 合并
    /// （`HubConfig.progress_interval_ms`）、丢弃不递增的值后逐条回调 `listener`；调用结束后不再回调。
    ///
    /// @side-effect 回调在专用阻塞线程上按顺序执行；回调抛出异常时忽略该条、继续接收。
    /// @invariant 返回（或被外部取消而结束）之前，已收到的进度都已回调完毕。
    pub async fn call_tool_with_progress(
        &self,
        request: CallRequest,
        listener: Arc<dyn ProgressListener>,
    ) -> Result<CallOutcome, HubError> {
        let hub = self.hub()?;
        let mut req = request.into_hub()?;
        let call_id = req.call_id.get_or_insert_with(new_call_id).clone();
        let mut guard = CancelOnDrop {
            hub: hub.clone(),
            call_id,
            armed: true,
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<hub::ProgressUpdate>();
        let forward = self.handle.spawn_blocking(move || {
            while let Some(update) = rx.blocking_recv() {
                guarded(|| listener.on_progress(update.into()));
            }
        });
        let res = self
            .run(async move {
                let out = hub.call_tool_with_progress(req, tx).await;
                // 调用结束时 Hub 已释放进度出口，转发线程收完剩余进度后退出。
                let _ = forward.await;
                out
            })
            .await;
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

    /// 设置自定义唤醒（替换之前的）；为空时恢复配置 `waker` 决定的实现（默认系统唤醒，按平台执行系统命令；
    /// Android 上默认实现不支持 `android-intent`，厂商需要提供）。
    pub fn set_waker(&self, waker: Option<Arc<dyn HubWaker>>) {
        if let Ok(hub) = self.hub() {
            match waker {
                Some(w) => hub.set_waker(Arc::new(WakerAdapter(w))),
                None => hub.reset_waker(),
            }
        }
    }

    // ---- 按名寻址（spec/hub-api.md 3.16）----

    /// 宿主推送：App 安装或更新（Android `PACKAGE_ADDED` / `PACKAGE_REPLACED` 后重新读到的元数据）。
    /// 不是以 `start_with_name_service` 启动的 Hub 时无效果。
    pub fn name_service_installed(&self, app: NamedApp) {
        #[cfg(unix)]
        if let (Some(connector), Some(name)) = (&self.name_service, naming::hosted_name(app)) {
            connector.installed(name);
        }
        #[cfg(not(unix))]
        let _ = app;
    }

    /// 宿主推送：App 卸载（Android `PACKAGE_REMOVED` 且非替换）。移除发现记录与来自安装元数据的清单。
    pub fn name_service_removed(&self, app_id: String) {
        #[cfg(unix)]
        if let Some(connector) = &self.name_service {
            connector.removed(&app_id);
        }
        #[cfg(not(unix))]
        let _ = app_id;
    }

    // ---- 系统 IPC 上的 MCP 出口（TASKS 4g d）----

    /// 在交来的 fd（Unix 流式套接字，如 Android `bindService` 换得的 socketpair 一端）上提供 MCP，直到对端关闭；
    /// 帧与 stdio 相同（每行一条 JSON-RPC 消息），每次调用是一个独立会话。
    ///
    /// @input `fd` 的所有权随调用转移给本库（负数不接管）。
    /// @error 本构建不含 `mcp-server`（Android 精简库，spec/hub-api.md 3.10）或非 Unix 平台 → `Unsupported`；
    /// fd 不是套接字 → `Io`。
    pub async fn serve_mcp_fd(&self, fd: i32) -> Result<(), HubError> {
        let hub = self.hub()?;
        let stream = mcp_stream(fd)?;
        self.run(async move { serve_mcp(hub, stream).await }).await?
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

#[cfg(unix)]
fn mcp_stream(fd: i32) -> Result<std::os::unix::net::UnixStream, HubError> {
    use std::os::fd::{FromRawFd, OwnedFd};
    if fd < 0 {
        return Err(HubError::Io { detail: format!("fd 无效：{fd}") });
    }
    // @security 调用方按契约转移一个打开的 fd 的所有权（Kotlin `ParcelFileDescriptor.detachFd()`），之后不再使用它。
    let stream = std::os::unix::net::UnixStream::from(unsafe { OwnedFd::from_raw_fd(fd) });
    // 不是 Unix 套接字的 fd 在这里失败（随 `stream` 丢弃而关闭）。
    stream.local_addr()?;
    stream.set_nonblocking(true)?;
    Ok(stream)
}

#[cfg(not(unix))]
fn mcp_stream(fd: i32) -> Result<std::convert::Infallible, HubError> {
    Err(HubError::Unsupported { detail: format!("本平台不支持以 fd 交来 MCP 通道（fd {fd}）") })
}

#[cfg(all(unix, feature = "mcp-server"))]
async fn serve_mcp(hub: Arc<hub::Hub>, stream: std::os::unix::net::UnixStream) -> Result<(), HubError> {
    let stream = tokio::net::UnixStream::from_std(stream)?;
    hub.serve_mcp_stream(stream).await.map_err(|e| HubError::Io { detail: e.to_string() })
}

#[cfg(all(unix, not(feature = "mcp-server")))]
async fn serve_mcp(_hub: Arc<hub::Hub>, _stream: std::os::unix::net::UnixStream) -> Result<(), HubError> {
    Err(HubError::Unsupported { detail: "本构建未包含 MCP 出口（cargo feature `mcp-server`）".to_owned() })
}

#[cfg(not(unix))]
async fn serve_mcp(_hub: Arc<hub::Hub>, stream: std::convert::Infallible) -> Result<(), HubError> {
    match stream {}
}

fn empty_export(format: ToolFormat) -> String {
    match format {
        ToolFormat::Gemini => r#"{"functionDeclarations":[]}"#.to_owned(),
        _ => "[]".to_owned(),
    }
}

#[cfg(test)]
mod tests;
