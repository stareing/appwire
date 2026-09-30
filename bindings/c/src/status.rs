//! 状态码、线程局部错误信息与 FFI 入口保护（panic → `AM_ERR_PANIC`）。

use std::any::Any;
use std::cell::RefCell;
use std::ffi::{CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use app_mcp_native::NativeError;

use crate::strings::to_cstring_lossy;

/// 对应头文件 `AmStatus`。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmStatus {
    Ok = 0,
    InvalidArgument = 1,
    InvalidName = 2,
    InvalidSchema = 3,
    DuplicateName = 4,
    InvalidJson = 5,
    InvalidConfig = 6,
    AlreadyCompleted = 7,
    Disposed = 8,
    Stopped = 9,
    Internal = 10,
    Panic = 11,
}

/// FFI 层内部错误：状态码 + 说明。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FfiError {
    pub status: AmStatus,
    pub message: String,
}

pub(crate) type FfiResult<T> = Result<T, FfiError>;

impl FfiError {
    pub fn new(status: AmStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(AmStatus::InvalidArgument, message)
    }

    pub fn null(what: &str) -> Self {
        Self::invalid_argument(format!("参数 {what} 不能为 NULL"))
    }

    pub fn stopped() -> Self {
        Self::new(AmStatus::Stopped, "客户端已停止或已释放")
    }
}

impl From<NativeError> for FfiError {
    fn from(e: NativeError) -> Self {
        let status = match &e {
            NativeError::InvalidName(_) => AmStatus::InvalidName,
            NativeError::InvalidSchema(_) => AmStatus::InvalidSchema,
            NativeError::DuplicateName(_) => AmStatus::DuplicateName,
            NativeError::InvalidJson(_) => AmStatus::InvalidJson,
            NativeError::InvalidConfig(_) => AmStatus::InvalidConfig,
            NativeError::AlreadyCompleted => AmStatus::AlreadyCompleted,
            NativeError::Disposed => AmStatus::Disposed,
            NativeError::Stopped => AmStatus::Stopped,
            NativeError::Internal(_) => AmStatus::Internal,
        };
        Self::new(status, e.to_string())
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

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "未知 panic".to_owned()
    }
}

/// 执行 FFI 入口主体：错误写入线程局部存储并返回对应状态码；panic 转为 `AM_ERR_PANIC`。
pub(crate) fn guard(f: impl FnOnce() -> FfiResult<()>) -> AmStatus {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => AmStatus::Ok,
        Ok(Err(e)) => {
            set_last_error(&e.message);
            e.status
        }
        Err(payload) => {
            set_last_error(&format!(
                "app_mcp 内部 panic：{}",
                panic_message(payload.as_ref())
            ));
            AmStatus::Panic
        }
    }
}

/// 返回值不是 `AmStatus` 的入口：出错或 panic 时返回 `fallback`（错误信息同样写入线程局部存储）。
pub(crate) fn guard_value<T>(fallback: T, f: impl FnOnce() -> FfiResult<T>) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            set_last_error(&e.message);
            fallback
        }
        Err(payload) => {
            set_last_error(&format!(
                "app_mcp 内部 panic：{}",
                panic_message(payload.as_ref())
            ));
            fallback
        }
    }
}
