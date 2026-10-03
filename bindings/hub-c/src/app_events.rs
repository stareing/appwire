//! v22 App 事件的厂商回调（第 16 项 N3，spec/hub-api.md 3.17「厂商 / 机主」）：`Hub::set_event_handler` → C 回调。

use std::ffi::{c_char, c_void};
use std::sync::Arc;

use hub::{AppEvent, EventHandler};

use crate::callbacks::set_slot;
use crate::dispatch::{AmHubFreeFn, Dispatcher, Slot};
use crate::ffi_util::{AmHubStatus, into_raw_cstring};
use crate::handle::AmHub;

/// App 事件回调（头文件 `AmHubAppEventFn`）：`event_json` 为 `AppEvent` JSON，归回调方所有。
pub type AmHubAppEventFn = unsafe extern "C" fn(user_data: *mut c_void, event_json: *mut c_char);

/// 把 Hub 的同步厂商回调转到分发线程：Hub 的连接任务上只做序列化与入队。
/// @invariant 回调在分发线程上按事件到达顺序串行执行；执行时再读取回调（被清除后不再调用旧回调）。
pub(crate) struct CAppEventHandler {
    pub(crate) slot: Arc<Slot<AmHubAppEventFn>>,
    pub(crate) dispatcher: Dispatcher,
}

impl EventHandler for CAppEventHandler {
    fn on_event(&self, event: &AppEvent) {
        if self.slot.get().is_none() {
            return;
        }
        let json = serde_json::to_string(event).unwrap_or_default();
        let slot = self.slot.clone();
        // 分发线程已关闭（Hub 释放中）时丢弃。
        let _ = self.dispatcher.post(Box::new(move || {
            if let Some(entry) = slot.get() {
                // SAFETY: 调用方注册的回调；字符串所有权转移给回调方。
                unsafe { (entry.f)(entry.user_data.ptr(), into_raw_cstring(&json)) };
            }
        }));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_set_app_event_cb(
    hub: *mut AmHub,
    cb: Option<AmHubAppEventFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmHubFreeFn>,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe { set_slot(hub, cb, user_data, free_user_data, |h| &h.app_events) }
}
