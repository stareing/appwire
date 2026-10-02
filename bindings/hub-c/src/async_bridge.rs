//! 审批 / 配对 / 唤醒：把 Hub 的 async trait 映射为“回调 + 完成句柄”。

use std::ffi::{c_char, c_void};
use std::sync::Arc;

use hub::{
    ApprovalHandler, ApprovalRequest, ErrorKind, HubError, PairingHandler, PairingRequest,
    ToolError, WakeRequest, Waker, async_trait,
};
use tokio::sync::oneshot;

use crate::dispatch::{Dispatcher, Slot};
use crate::ffi_util::into_raw_cstring;
use crate::handle::{
    AmHubApproval, AmHubApprovalFn, AmHubPairing, AmHubPairingFn, AmHubWake, AmHubWakerFn,
};

// ---------------------------------------------------------------------------
// 审批 / 配对：async trait → 回调 + 完成句柄
// ---------------------------------------------------------------------------

/// 把请求投递到分发线程上的回调，等待完成句柄的结果。回调未设置、句柄被丢弃 → `false`。
pub(crate) async fn ask<F: Copy + Send + Sync + 'static>(
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

pub(crate) fn invoke_approval(f: AmHubApprovalFn, ud: *mut c_void, json: *mut c_char, tx: oneshot::Sender<bool>) {
    let handle = Box::into_raw(Box::new(AmHubApproval { tx }));
    // SAFETY: 调用方注册的回调；字符串与句柄的所有权转移给回调方。
    unsafe { f(ud, json, handle) };
}

pub(crate) fn invoke_pairing(f: AmHubPairingFn, ud: *mut c_void, json: *mut c_char, tx: oneshot::Sender<bool>) {
    let handle = Box::into_raw(Box::new(AmHubPairing { tx }));
    // SAFETY: 同上。
    unsafe { f(ud, json, handle) };
}

pub(crate) struct CApprovalHandler {
    pub(crate) slot: Arc<Slot<AmHubApprovalFn>>,
    pub(crate) dispatcher: Dispatcher,
}

#[async_trait]
impl ApprovalHandler for CApprovalHandler {
    async fn approve(&self, req: ApprovalRequest) -> bool {
        let json = serde_json::to_string(&req).unwrap_or_default();
        ask(&self.slot, &self.dispatcher, json, invoke_approval).await
    }
}

pub(crate) struct CPairingHandler {
    pub(crate) slot: Arc<Slot<AmHubPairingFn>>,
    pub(crate) dispatcher: Dispatcher,
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
pub(crate) struct CWaker {
    pub(crate) slot: Arc<Slot<AmHubWakerFn>>,
    pub(crate) dispatcher: Dispatcher,
}

pub(crate) fn waker_dropped() -> ToolError {
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
