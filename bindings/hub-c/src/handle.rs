//! Hub 句柄与回调函数类型：`AmHub` 持有运行时、分发线程与各回调槽；
//! 审批 / 配对 / 唤醒的完成句柄内部是 oneshot 发送端。

use std::ffi::{c_char, c_void};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use hub::{Hub, ToolError};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::oneshot;
use tokio::task::JoinSet;

use crate::dispatch::{Dispatcher, ResultCb, Slot, lock};
use crate::ffi_util::{FfiError, FfiResult};

/// Hub 停止时未完成的调用使用的错误说明。
pub(crate) const STOPPED_MESSAGE: &str = "Hub 已停止，调用未完成。";

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
    pub(crate) tx: oneshot::Sender<bool>,
}

/// 一次配对（头文件 `AmHubPairing`）。
pub struct AmHubPairing {
    pub(crate) tx: oneshot::Sender<bool>,
}

/// 一次唤醒（头文件 `AmHubWake`）。
pub struct AmHubWake {
    pub(crate) tx: oneshot::Sender<Result<(), ToolError>>,
}

/// 运行中的 Hub（头文件 `AmHub`）。
pub struct AmHub {
    pub(crate) rt: Mutex<Option<Runtime>>,
    pub(crate) handle: Handle,
    /// `None` = 已停止。异步操作在读锁下登记任务，停止时在写锁下取走，保证不遗漏。
    pub(crate) hub: RwLock<Option<Arc<Hub>>>,
    /// 进行中的异步操作；停止时全部中止（结果回调以兜底结果触发）。
    pub(crate) ops: Mutex<JoinSet<()>>,
    pub(crate) listen_addr: Option<SocketAddr>,
    pub(crate) ipc_endpoint: Option<String>,
    pub(crate) dispatcher: Dispatcher,
    pub(crate) events: Arc<Slot<AmHubEventFn>>,
    pub(crate) approval: Arc<Slot<AmHubApprovalFn>>,
    pub(crate) pairing: Arc<Slot<AmHubPairingFn>>,
    pub(crate) pairing_installed: AtomicBool,
    pub(crate) waker: Arc<Slot<AmHubWakerFn>>,
    pub(crate) seq: AtomicU64,
}

impl AmHub {
    pub(crate) fn hub(&self) -> FfiResult<Arc<Hub>> {
        self.hub
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(FfiError::stopped)
    }

    /// 在运行时上执行异步操作，结果经 `rc` 投递。Hub 已停止时不回调并返回 `STOPPED`。
    pub(crate) fn spawn_result<Fut>(&self, rc: ResultCb, f: impl FnOnce(Arc<Hub>) -> Fut) -> FfiResult<()>
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
    pub(crate) fn block_on<T: Send + 'static>(&self, fut: impl Future<Output = T> + Send + 'static) -> T {
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

    pub(crate) fn next_call_id(&self) -> String {
        format!("am-hub-{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// 停止 Hub：中止进行中的操作、关闭 App 连接与上游。幂等。
    pub(crate) fn shutdown(&self) {
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

    pub(crate) fn teardown(&self) {
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
