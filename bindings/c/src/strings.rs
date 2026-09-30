//! C 字符串与 Rust 字符串之间的转换。

use std::ffi::{CStr, CString, c_char};

use crate::status::{FfiError, FfiResult};

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

/// 返回由调用方用 `am_string_free` 释放的字符串。
pub(crate) fn into_raw_cstring(s: &str) -> *mut c_char {
    to_cstring_lossy(s).into_raw()
}

/// 可为 NULL 的输入字符串。非法 UTF-8 返回 `AM_ERR_INVALID_ARGUMENT`。
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

/// 必填的输入字符串。NULL 或非法 UTF-8 返回 `AM_ERR_INVALID_ARGUMENT`。
///
/// # Safety
/// 同 [`opt_str`]。
pub(crate) unsafe fn req_str<'a>(p: *const c_char, what: &str) -> FfiResult<&'a str> {
    // SAFETY: 转交调用方的保证。
    unsafe { opt_str(p, what) }?.ok_or_else(|| FfiError::null(what))
}

/// 宽松读取：NULL → `None`，非法 UTF-8 按替换字符处理。用于不应因编码失败而丢失的错误信息等。
///
/// # Safety
/// 同 [`opt_str`]。
pub(crate) unsafe fn lossy_str(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: 由调用方保证。
    let c = unsafe { CStr::from_ptr(p) };
    Some(c.to_string_lossy().into_owned())
}
