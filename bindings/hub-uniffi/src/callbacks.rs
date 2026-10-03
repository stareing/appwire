//! 回调接口（由外部语言实现）、一次性完成句柄，以及把它们接到 Hub 的适配器。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use app_mcp_hub as hub;
use tokio::sync::oneshot;

use crate::lock;
use crate::types::{AppEvent, ApprovalRequest, HubEvent, PairingRequest, ProgressUpdate, WakeRequest, wake_error};

/// Hub 事件监听。在专用分发线程上同步调用，必须尽快返回（需要时自行切换线程）。
#[uniffi::export(foreign)]
pub trait HubEventListener: Send + Sync {
    fn on_event(&self, event: HubEvent);
    /// 监听方处理过慢，跳过了 `skipped` 个事件；应重新拉取 `apps()` / `tools()`。
    fn on_lagged(&self, skipped: u64);
}

/// App 事件回调（spec/hub-api.md 3.17，[`crate::AppMcpHub::set_event_handler`]）：每个通过去重与校验的事件（不论有无订阅）
/// 同步回调一次。
///
/// @invariant 在 Hub 的 App 连接任务上执行，必须很快返回（耗时工作请转交其他线程），否则拖慢该 App 的消息处理。
#[uniffi::export(foreign)]
pub trait AppEventHandler: Send + Sync {
    fn on_app_event(&self, event: AppEvent);
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
pub(crate) struct Once<T>(Mutex<Option<oneshot::Sender<T>>>);

impl<T> Once<T> {
    pub(crate) fn new() -> (Self, oneshot::Receiver<T>) {
        let (tx, rx) = oneshot::channel();
        (Self(Mutex::new(Some(tx))), rx)
    }

    fn complete(&self, value: T) -> bool {
        lock(&self.0).take().is_some_and(|tx| tx.send(value).is_ok())
    }
}

/// [`ApprovalHandler`] 的结果句柄。可在任意线程调用，可在回调返回后调用；只有第一次调用生效。
#[derive(uniffi::Object)]
pub struct ApprovalResponder(pub(crate) Once<bool>);

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

pub(crate) struct ApprovalAdapter(pub(crate) Arc<dyn ApprovalHandler>);

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

pub(crate) struct PairingAdapter(pub(crate) Arc<dyn PairingHandler>);

#[async_trait::async_trait]
impl hub::PairingHandler for PairingAdapter {
    async fn pair(&self, req: hub::PairingRequest) -> bool {
        let (once, rx) = Once::new();
        let (handler, request) = (self.0.clone(), PairingRequest::from(req));
        call_foreign("配对", move || handler.on_request(request, Arc::new(PairingResponder(once)))).await;
        rx.await.unwrap_or(false)
    }
}

pub(crate) struct WakerAdapter(pub(crate) Arc<dyn HubWaker>);

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

/// 接到 Hub 的事件回调；`None` 表示已清除（Hub 只能替换回调，不能移除）。
pub(crate) struct AppEventAdapter(pub(crate) Option<Arc<dyn AppEventHandler>>);

impl hub::EventHandler for AppEventAdapter {
    fn on_event(&self, event: &hub::AppEvent) {
        let Some(handler) = &self.0 else { return };
        let event = AppEvent::from(event.clone());
        if !guarded(|| handler.on_app_event(event)) {
            tracing::warn!("事件回调抛出异常");
        }
    }
}

/// 执行外部同步回调，吞掉 panic。返回是否正常结束。
pub(crate) fn guarded(f: impl FnOnce()) -> bool {
    catch_unwind(AssertUnwindSafe(f)).is_ok()
}
