//! Hub 生命周期入口：启动、停止、释放、监听地址与 HTTP 出口。

use std::ffi::c_char;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, RwLock};

use hub::Hub;
use serde_json::json;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio::task::JoinSet;

use crate::async_bridge::CApprovalHandler;
use crate::config;
use crate::dispatch::{Dispatcher, Slot};
use crate::ffi_util::{
    AmHubStatus, FfiError, FfiResult, guard, guard_value, into_raw_cstring, opt_str, req_str,
    write_out_str,
};
use crate::handle::{AmHub, AmHubApprovalFn, AmHubEventFn};
use crate::query::hub_ref;

// ---------------------------------------------------------------------------
// 生命周期
// ---------------------------------------------------------------------------

pub(crate) fn start(config_json: Option<&str>) -> FfiResult<Box<AmHub>> {
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
