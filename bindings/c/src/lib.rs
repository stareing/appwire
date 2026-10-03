//! C ABI 实现，契约见 `include/app_mcp.h`。
//!
//! - 所有入口用 [`status::guard`] 包住：错误写入线程局部存储（`am_last_error_message`），
//!   panic 转为 `AM_ERR_PANIC`，不会跨越 FFI 边界。
//! - 回调包装见 [`callbacks`]；句柄定义见 [`handles`]。
//! - `user_data` 的所有权：传入 `am_client_new` / `am_tool_register` / `am_resource_register` /
//!   `am_call_set_cancel_callback` 后，无论成功与否都归库所有，库在不再使用时调用 `free_user_data`
//!   （失败时在函数返回前调用）。
//!
//! 各函数的安全约定（指针有效性、所有权）统一写在头文件中，这里不再逐个重复。
#![allow(clippy::missing_safety_doc)]

mod callbacks;
mod client;
mod convert;
mod events;
mod ffi_types;
mod handles;
mod registration;
mod requests;
mod status;
mod strings;

use std::ffi::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};

pub use callbacks::{
    AmCancelFn, AmCancelReason, AmFreeFn, AmIdleExitFn, AmLogFn, AmLogLevel, AmNavigateFn, AmPairedFn, AmReadFn,
    AmStateFn, AmStateStatus, AmToolFn,
};
pub use client::*;
pub use events::*;
pub use ffi_types::{
    AmCallResult, AmClientCallbacks, AmClientConfig, AmClientOptions, AmLifecycle, AmResourceOptions, AmResourceSpec,
    AmToolOptions, AmToolSpec,
};
pub use handles::{AmCall, AmClient, AmHold, AmNavigate, AmRead, AmResource, AmScope, AmTool};
pub use registration::*;
pub use requests::*;
pub use status::AmStatus;
use status::last_error_ptr;

/// 与头文件 `AM_API_VERSION` 一致。
pub const AM_API_VERSION: u32 = 3;

// ---------------------------------------------------------------------------
// 通用
// ---------------------------------------------------------------------------

static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

#[unsafe(no_mangle)]
pub extern "C" fn am_version() -> *const c_char {
    VERSION.as_ptr().cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn am_last_error_message() -> *const c_char {
    last_error_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    // SAFETY: s 由本库通过 CString::into_raw 返回。
    unsafe { consume_cstring(s) };
}

pub(crate) unsafe fn consume_cstring(s: *mut c_char) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(unsafe { std::ffi::CString::from_raw(s) })
    }));
}

#[cfg(test)]
mod tests;
