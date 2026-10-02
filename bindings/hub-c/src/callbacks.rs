//! 常驻回调的设置（事件、审批、配对、唤醒）与完成句柄的回复入口。

use std::ffi::{c_char, c_void};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use hub::{ErrorKind, ToolError};
use serde_json::Value;
use tokio::sync::oneshot;

use crate::async_bridge::{CPairingHandler, CWaker};
use crate::dispatch::{AmHubFreeFn, CallbackEntry, Slot, UserData};
use crate::ffi_util::{AmHubStatus, FfiError, FfiResult, guard, opt_str};
use crate::handle::{
    AmHub, AmHubApproval, AmHubApprovalFn, AmHubEventFn, AmHubPairing, AmHubPairingFn, AmHubWake,
    AmHubWakerFn,
};
use crate::query::hub_ref;

// ---------------------------------------------------------------------------
// 事件与策略回调
// ---------------------------------------------------------------------------

/// 设置常驻回调的公共部分：user_data 无论成败都归库所有。
pub(crate) unsafe fn set_slot<F>(
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

pub(crate) fn complete(tx: oneshot::Sender<bool>, approved: bool) -> FfiResult<()> {
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
pub(crate) fn parse_error_kind(s: Option<&str>) -> ErrorKind {
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
