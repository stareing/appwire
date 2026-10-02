//! 回调适配：原生分发线程上的回调经 ThreadsafeFunction 非阻塞投递到 Node 事件循环。

use std::sync::{Arc, Mutex};

use app_mcp_native as native;
use napi::Status;
use napi::threadsafe_function::ThreadsafeFunctionCallMode;
use native::{CancelReason, ErrorKind, LogLevel, StateInfo};

use super::WeakTsfn;
use super::convert::{cancel_reason_str, log_level_str};
use super::handles::{Call, Navigate, Read};
use super::objects::ClientEvent;

pub(super) struct JsToolHandler {
    pub(super) tsfn: WeakTsfn<Call>,
}

impl native::ToolHandler for JsToolHandler {
    fn invoke(&self, call: native::CallHandle) {
        let status = self.tsfn.call(Call { inner: call.clone() }, ThreadsafeFunctionCallMode::NonBlocking);
        if status != Status::Ok {
            // 事件循环已关闭（进程正在退出）：直接失败，避免 Host 等到超时。
            let _ = call.fail(ErrorKind::AppNotResponding, "Node 事件循环不可用");
        }
    }
}

pub(super) struct JsResourceReader {
    pub(super) tsfn: WeakTsfn<Read>,
}

impl native::ResourceReader for JsResourceReader {
    fn read(&self, read: native::ReadHandle) {
        let status = self.tsfn.call(Read { inner: read.clone() }, ThreadsafeFunctionCallMode::NonBlocking);
        if status != Status::Ok {
            let _ = read.fail(ErrorKind::AppNotResponding, "Node 事件循环不可用");
        }
    }
}

pub(super) struct JsNavigationHandler {
    pub(super) tsfn: WeakTsfn<Navigate>,
}

impl native::NavigationHandler for JsNavigationHandler {
    fn navigate(&self, request: native::NavigateHandle) {
        let status = self.tsfn.call(Navigate { inner: request.clone() }, ThreadsafeFunctionCallMode::NonBlocking);
        if status != Status::Ok {
            let _ = request.fail("Node 事件循环不可用");
        }
    }
}

pub(super) struct JsCancelListener {
    pub(super) tsfn: WeakTsfn<String>,
}

impl native::CancelListener for JsCancelListener {
    fn on_cancel(&self, reason: CancelReason) {
        let _ = self.tsfn.call(cancel_reason_str(reason).to_string(), ThreadsafeFunctionCallMode::NonBlocking);
    }
}

/// 客户端监听器。`stop()` 后清空，以便尽早释放 ThreadsafeFunction。
pub(super) struct JsClientListener {
    pub(super) tsfn: Mutex<Option<Arc<WeakTsfn<ClientEvent>>>>,
}

impl JsClientListener {
    fn emit(&self, event: ClientEvent) {
        let tsfn = match self.tsfn.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        if let Some(tsfn) = tsfn {
            let _ = tsfn.call(event, ThreadsafeFunctionCallMode::NonBlocking);
        }
    }

    pub(super) fn release(&self) {
        let taken = match self.tsfn.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        drop(taken);
    }
}

impl native::ClientListener for JsClientListener {
    fn on_state_changed(&self, state: StateInfo) {
        self.emit(ClientEvent {
            kind: "state".to_string(),
            state: Some(state.into()),
            token: None,
            level: None,
            message: None,
        });
    }

    fn on_paired(&self, token: String) {
        self.emit(ClientEvent { kind: "paired".to_string(), state: None, token: Some(token), level: None, message: None });
    }

    fn on_log(&self, level: LogLevel, message: String) {
        self.emit(ClientEvent {
            kind: "log".to_string(),
            state: None,
            token: None,
            level: Some(log_level_str(level).to_string()),
            message: Some(message),
        });
    }

    fn on_idle_exit(&self) {
        self.emit(ClientEvent { kind: "idle-exit".to_string(), state: None, token: None, level: None, message: None });
    }
}
