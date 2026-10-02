//! Hub SDK 的 C ABI 实现，契约见 `include/app_mcp_hub.h`（spec/hub-api.md 第 6 节）。
//!
//! - 每个 [`AmHub`] 自带一个 tokio 多线程运行时与一个回调分发线程（[`dispatch::Dispatcher`]）；
//!   回调从不在 tokio 工作线程或调用方线程上执行。
//! - 所有入口用 [`ffi_util::guard`] 包住：错误写入线程局部存储（`am_hub_last_error_message`），
//!   panic 转为 `AM_HUB_ERR_PANIC`，不会跨越 FFI 边界。
//! - 异步操作（调用、读资源、dispatch）在运行时上以任务执行，结果经 [`dispatch::ResultCb`] 投递，
//!   保证恰好一次（Hub 停止时中止任务并以 `CANCELLED` 兜底结果回调）。
//! - 审批 / 配对 / 唤醒：Hub 的 async trait 映射为“回调 + 完成句柄”；句柄内部是 oneshot 发送端，
//!   未完成就被丢弃时接收端得到错误，按拒绝（唤醒：`LAUNCH_FAILED`）处理。
//!
//! 所有导出符号带 `am_hub_` 前缀，可与 `app_mcp`（App 端 C ABI）同时链接进一个进程。
//! 各函数的安全约定（指针有效性、所有权）统一写在头文件中，这里不再逐个重复。
#![allow(clippy::missing_safety_doc)]

mod async_bridge;
mod callbacks;
mod calls;
mod config;
mod dispatch;
mod ffi_util;
mod handle;
mod json;
mod lifecycle;
mod query;

use std::ffi::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};

pub use callbacks::*;
pub use calls::*;
pub use dispatch::{AmHubFreeFn, AmHubResultFn};
pub use ffi_util::AmHubStatus;
use ffi_util::last_error_ptr;
pub use handle::{
    AmHub, AmHubApproval, AmHubApprovalFn, AmHubEventFn, AmHubPairing, AmHubPairingFn, AmHubWake, AmHubWakerFn,
};
pub use lifecycle::*;
pub use query::*;

/// 与头文件 `AM_HUB_API_VERSION` 一致。3：合并端口（`am_hub_listen_addr`、配置 `listen`）。
pub const AM_HUB_API_VERSION: u32 = 3;

// ---------------------------------------------------------------------------
// 通用
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn am_hub_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn am_hub_last_error_message() -> *const c_char {
    last_error_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: s 由本库 CString::into_raw 分配，按约定只释放一次。
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { std::ffi::CString::from_raw(s) })));
    }
}

#[cfg(test)]
mod tests;
