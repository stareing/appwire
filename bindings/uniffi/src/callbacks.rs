//! 由外部语言实现的回调接口，以及把它们适配到原生运行时 trait 的适配器（`catch_unwind` 兜底）。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use app_mcp_native as native;
use native::ErrorKind;

use crate::enums::{CancelReason, LogLevel};
use crate::handles::{Call, Navigate, Read};
use crate::records::StateInfo;

/// 工具 handler。在分发线程上同步调用，必须尽快返回；结果通过 `call` 提交。
#[uniffi::export(foreign)]
pub trait ToolHandler: Send + Sync {
    fn invoke(&self, call: Arc<Call>);
}

/// 资源读取。在分发线程上同步调用，结果通过 `read` 提交。
#[uniffi::export(foreign)]
pub trait ResourceReader: Send + Sync {
    fn read(&self, read: Arc<Read>);
}

/// 调用被取消时通知。在分发线程上调用（已取消时在设置监听的线程上立即调用）。
#[uniffi::export(foreign)]
pub trait CancelListener: Send + Sync {
    fn on_cancel(&self, reason: CancelReason);
}

/// 导航回调（Host 的 `app/navigate`，spec/protocol.md 3.4）。在分发线程上同步调用，必须尽快返回；
/// 切换界面后通过 `request` 提交结果。
#[uniffi::export(foreign)]
pub trait NavigationHandler: Send + Sync {
    fn navigate(&self, request: Arc<Navigate>);
}

/// 客户端事件。在分发线程上调用。
#[uniffi::export(foreign)]
pub trait ClientListener: Send + Sync {
    fn on_state_changed(&self, state: StateInfo);
    /// 配对成功并获得新 token，App 应持久化，下次放入 `ClientConfig.token`。
    fn on_paired(&self, token: String);
    fn on_log(&self, level: LogLevel, message: String);
    /// 已进入休眠，且驻留策略允许退出进程。App 自行决定是否退出。
    fn on_idle_exit(&self);
}

/// 执行外部回调，吞掉 panic（包括 uniffi 把外部未预期异常转换成的 panic）。返回是否正常结束。
pub(crate) fn guarded(f: impl FnOnce()) -> bool {
    catch_unwind(AssertUnwindSafe(f)).is_ok()
}

pub(crate) struct ToolHandlerAdapter(pub(crate) Arc<dyn ToolHandler>);

impl native::ToolHandler for ToolHandlerAdapter {
    fn invoke(&self, call: native::CallHandle) {
        let wrapped = Arc::new(Call {
            inner: call.clone(),
        });
        if !guarded(|| self.0.invoke(wrapped)) {
            // 外部 handler 在同步部分抛出异常：转为 HANDLER_ERROR（若已完成则忽略）。
            let _ = call.fail(ErrorKind::HandlerError, "handler 抛出了未处理的异常");
        }
    }
}

pub(crate) struct ResourceReaderAdapter(pub(crate) Arc<dyn ResourceReader>);

impl native::ResourceReader for ResourceReaderAdapter {
    fn read(&self, read: native::ReadHandle) {
        let wrapped = Arc::new(Read {
            inner: read.clone(),
        });
        if !guarded(|| self.0.read(wrapped)) {
            let _ = read.fail(ErrorKind::HandlerError, "资源读取抛出了未处理的异常");
        }
    }
}

pub(crate) struct NavigationHandlerAdapter(pub(crate) Arc<dyn NavigationHandler>);

impl native::NavigationHandler for NavigationHandlerAdapter {
    fn navigate(&self, request: native::NavigateHandle) {
        let wrapped = Arc::new(Navigate { inner: request.clone() });
        if !guarded(|| self.0.navigate(wrapped)) {
            let _ = request.fail("导航回调抛出了未处理的异常");
        }
    }
}

pub(crate) struct CancelListenerAdapter(pub(crate) Arc<dyn CancelListener>);

impl native::CancelListener for CancelListenerAdapter {
    fn on_cancel(&self, reason: native::CancelReason) {
        guarded(|| self.0.on_cancel(reason.into()));
    }
}

pub(crate) struct ClientListenerAdapter(pub(crate) Arc<dyn ClientListener>);

impl native::ClientListener for ClientListenerAdapter {
    fn on_state_changed(&self, state: native::StateInfo) {
        guarded(|| self.0.on_state_changed(state.into()));
    }
    fn on_paired(&self, token: String) {
        guarded(|| self.0.on_paired(token));
    }
    fn on_log(&self, level: native::LogLevel, message: String) {
        guarded(|| self.0.on_log(level.into(), message));
    }
    fn on_idle_exit(&self) {
        guarded(|| self.0.on_idle_exit());
    }
}
