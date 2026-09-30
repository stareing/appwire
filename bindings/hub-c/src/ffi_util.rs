//! 状态码、线程局部错误信息、FFI 入口保护（panic → `AM_HUB_ERR_PANIC`）与 C 字符串转换。

use std::any::Any;
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use hub::HubError;

/// 对应头文件 `AmHubStatus`。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmHubStatus {
    Ok = 0,
    InvalidArgument = 1,
    InvalidJson = 2,
    InvalidConfig = 3,
    Io = 4,
    Hub = 5,
    AlreadyCompleted = 6,
    Stopped = 7,
    Internal = 8,
    Panic = 9,
}

/// FFI 层内部错误：状态码 + 说明。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FfiError {
    pub status: AmHubStatus,
    pub message: String,
}

pub(crate) type FfiResult<T> = Result<T, FfiError>;

impl FfiError {
    pub fn new(status: AmHubStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(AmHubStatus::InvalidArgument, message)
    }

    pub fn null(what: &str) -> Self {
        Self::invalid_argument(format!("参数 {what} 不能为 NULL"))
    }

    pub fn stopped() -> Self {
        Self::new(AmHubStatus::Stopped, "Hub 已停止")
    }

    pub fn json(what: &str, e: impl std::fmt::Display) -> Self {
        Self::new(AmHubStatus::InvalidJson, format!("{what} 不是合法的 JSON：{e}"))
    }
}

impl From<HubError> for FfiError {
    fn from(e: HubError) -> Self {
        Self::new(AmHubStatus::Hub, e.0.to_string())
    }
}

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

/// 空字符串，TLS 已销毁时使用。
static EMPTY: &[u8] = b"\0";

pub(crate) fn set_last_error(message: &str) {
    let c = to_cstring_lossy(message);
    let _ = LAST_ERROR.try_with(|e| *e.borrow_mut() = c);
}

pub(crate) fn last_error_ptr() -> *const c_char {
    LAST_ERROR
        .try_with(|e| e.borrow().as_ptr())
        .unwrap_or(EMPTY.as_ptr().cast())
}

pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "未知 panic".to_owned()
    }
}

fn record(result: std::thread::Result<FfiResult<()>>) -> AmHubStatus {
    match result {
        Ok(Ok(())) => AmHubStatus::Ok,
        Ok(Err(e)) => {
            set_last_error(&e.message);
            e.status
        }
        Err(payload) => {
            set_last_error(&format!(
                "app_mcp_hub 内部 panic：{}",
                panic_message(payload.as_ref())
            ));
            AmHubStatus::Panic
        }
    }
}

/// 执行 FFI 入口主体：错误写入线程局部存储并返回对应状态码；panic 转为 `AM_HUB_ERR_PANIC`。
pub(crate) fn guard(f: impl FnOnce() -> FfiResult<()>) -> AmHubStatus {
    record(catch_unwind(AssertUnwindSafe(f)))
}

/// 返回值不是状态码的入口：出错或 panic 时返回 `fallback`（错误信息写入线程局部存储）。
pub(crate) fn guard_value<T>(fallback: T, f: impl FnOnce() -> FfiResult<T>) -> T {
    let mut out = None;
    let status = record(catch_unwind(AssertUnwindSafe(|| {
        out = Some(f()?);
        Ok(())
    })));
    match (status, out) {
        (AmHubStatus::Ok, Some(v)) => v,
        _ => fallback,
    }
}

// ---------------------------------------------------------------------------
// 字符串
// ---------------------------------------------------------------------------

/// 把 Rust 字符串转成 `CString`。内部的 NUL 字节会被去掉（C 字符串无法表示）。
pub(crate) fn to_cstring_lossy(s: &str) -> CString {
    match CString::new(s) {
        Ok(c) => c,
        Err(_) => {
            let bytes: Vec<u8> = s.bytes().filter(|b| *b != 0).collect();
            CString::new(bytes).unwrap_or_default()
        }
    }
}

/// 返回由接收方用 `am_hub_string_free` 释放的字符串。
pub(crate) fn into_raw_cstring(s: &str) -> *mut c_char {
    to_cstring_lossy(s).into_raw()
}

/// 可为 NULL 的输入字符串。非法 UTF-8 返回 `AM_HUB_ERR_INVALID_ARGUMENT`。
///
/// # Safety
/// `p` 为 NULL 或指向以 NUL 结尾、在返回的引用存活期间有效的字符串。
pub(crate) unsafe fn opt_str<'a>(p: *const c_char, what: &str) -> FfiResult<Option<&'a str>> {
    if p.is_null() {
        return Ok(None);
    }
    // SAFETY: 由调用方保证 p 指向以 NUL 结尾的有效字符串。
    let c = unsafe { CStr::from_ptr(p) };
    c.to_str()
        .map(Some)
        .map_err(|_| FfiError::invalid_argument(format!("参数 {what} 不是合法的 UTF-8")))
}

/// 必填的输入字符串。
///
/// # Safety
/// 同 [`opt_str`]。
pub(crate) unsafe fn req_str<'a>(p: *const c_char, what: &str) -> FfiResult<&'a str> {
    // SAFETY: 转交调用方的保证。
    unsafe { opt_str(p, what) }?.ok_or_else(|| FfiError::null(what))
}

/// 写 out 参数（字符串）。`out` 为 NULL 时忽略。
///
/// # Safety
/// `out` 为 NULL 或指向可写的 `*mut c_char`。
pub(crate) unsafe fn write_out_str(out: *mut *mut c_char, s: Option<&str>) {
    if !out.is_null() {
        // SAFETY: 由调用方保证 out 可写。
        unsafe { *out = s.map_or(std::ptr::null_mut(), into_raw_cstring) };
    }
}
