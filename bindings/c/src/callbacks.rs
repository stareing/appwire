//! 把 C 回调（函数指针 + user_data + free_user_data）包装成 `app-mcp-native` 的回调 trait。

use std::ffi::{c_char, c_void};

use app_mcp_native::{
    CallHandle, CancelListener, CancelReason, ClientListener, LogLevel, ReadHandle, ResourceReader,
    StateInfo, StateStatus, ToolHandler,
};

use crate::handles::{AmCall, AmRead};
use crate::strings::into_raw_cstring;

// ---------------------------------------------------------------------------
// 输出枚举（库 → 调用方）
// ---------------------------------------------------------------------------

/// 对应头文件 `AmStateStatus`。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmStateStatus {
    Idle = 0,
    Connecting = 1,
    Handshaking = 2,
    PendingPairing = 3,
    Connected = 4,
    Backoff = 5,
    Rejected = 6,
    Stopped = 7,
    Dormant = 8,
    Waking = 9,
}

impl From<StateStatus> for AmStateStatus {
    fn from(s: StateStatus) -> Self {
        match s {
            StateStatus::Idle => Self::Idle,
            StateStatus::Connecting => Self::Connecting,
            StateStatus::Handshaking => Self::Handshaking,
            StateStatus::PendingPairing => Self::PendingPairing,
            StateStatus::Connected => Self::Connected,
            StateStatus::Backoff => Self::Backoff,
            StateStatus::Rejected => Self::Rejected,
            StateStatus::Stopped => Self::Stopped,
            StateStatus::Dormant => Self::Dormant,
            StateStatus::Waking => Self::Waking,
        }
    }
}

/// 对应头文件 `AmLogLevel`。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmLogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl From<LogLevel> for AmLogLevel {
    fn from(l: LogLevel) -> Self {
        match l {
            LogLevel::Debug => Self::Debug,
            LogLevel::Info => Self::Info,
            LogLevel::Warn => Self::Warn,
            LogLevel::Error => Self::Error,
        }
    }
}

/// 对应头文件 `AmCancelReason`。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmCancelReason {
    Requested = 0,
    Timeout = 1,
    Disconnected = 2,
    Stopped = 3,
}

impl From<CancelReason> for AmCancelReason {
    fn from(r: CancelReason) -> Self {
        match r {
            CancelReason::Requested => Self::Requested,
            CancelReason::Timeout => Self::Timeout,
            CancelReason::Disconnected => Self::Disconnected,
            CancelReason::Stopped => Self::Stopped,
        }
    }
}

// ---------------------------------------------------------------------------
// 函数指针类型
// ---------------------------------------------------------------------------

pub type AmFreeFn = unsafe extern "C" fn(user_data: *mut c_void);
pub type AmStateFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    status: AmStateStatus,
    retry_in_ms: u64,
    reason: *mut c_char,
);
pub type AmPairedFn = unsafe extern "C" fn(user_data: *mut c_void, token: *mut c_char);
pub type AmLogFn =
    unsafe extern "C" fn(user_data: *mut c_void, level: AmLogLevel, message: *mut c_char);
pub type AmToolFn = unsafe extern "C" fn(user_data: *mut c_void, call: *mut AmCall);
pub type AmReadFn = unsafe extern "C" fn(user_data: *mut c_void, read: *mut AmRead);
pub type AmCancelFn = unsafe extern "C" fn(user_data: *mut c_void, reason: AmCancelReason);
pub type AmIdleExitFn = unsafe extern "C" fn(user_data: *mut c_void);

// ---------------------------------------------------------------------------
// user_data
// ---------------------------------------------------------------------------

/// 调用方的 `user_data`，丢弃时调用 `free_user_data`（若有）。
pub(crate) struct UserData {
    ptr: *mut c_void,
    free: Option<AmFreeFn>,
}

// SAFETY: 头文件约定回调在库的分发线程上执行、free_user_data 可能在任意线程调用，
// 也就是说调用方传入 user_data 时已承诺它可以跨线程使用（与 C 库传 void* 上下文的
// 一般约定一致）。本结构体只保存与转交该指针，自身从不解引用。
unsafe impl Send for UserData {}
// SAFETY: 同上；共享引用只用于读取指针值并转交给调用方的回调。
unsafe impl Sync for UserData {}

impl UserData {
    pub fn new(ptr: *mut c_void, free: Option<AmFreeFn>) -> Self {
        Self { ptr, free }
    }

    pub fn ptr(&self) -> *mut c_void {
        self.ptr
    }
}

impl Drop for UserData {
    fn drop(&mut self) {
        if let Some(free) = self.free.take() {
            // SAFETY: 调用方提供的释放函数，按约定只调用一次。
            unsafe { free(self.ptr) };
        }
    }
}

// ---------------------------------------------------------------------------
// trait 实现
// ---------------------------------------------------------------------------

pub(crate) struct CToolHandler {
    pub f: AmToolFn,
    pub user_data: UserData,
}

impl ToolHandler for CToolHandler {
    fn invoke(&self, call: CallHandle) {
        let raw = Box::into_raw(Box::new(AmCall::new(call)));
        // SAFETY: 调用方注册的回调；call 的所有权转移给回调方。
        unsafe { (self.f)(self.user_data.ptr(), raw) };
    }
}

pub(crate) struct CResourceReader {
    pub f: AmReadFn,
    pub user_data: UserData,
}

impl ResourceReader for CResourceReader {
    fn read(&self, read: ReadHandle) {
        let raw = Box::into_raw(Box::new(AmRead::new(read)));
        // SAFETY: 调用方注册的回调；read 的所有权转移给回调方。
        unsafe { (self.f)(self.user_data.ptr(), raw) };
    }
}

pub(crate) struct CCancelListener {
    pub f: AmCancelFn,
    pub user_data: UserData,
}

impl CancelListener for CCancelListener {
    fn on_cancel(&self, reason: CancelReason) {
        // SAFETY: 调用方注册的回调。
        unsafe { (self.f)(self.user_data.ptr(), reason.into()) };
    }
}

pub(crate) struct CClientListener {
    pub on_state: Option<AmStateFn>,
    pub on_paired: Option<AmPairedFn>,
    pub on_log: Option<AmLogFn>,
    pub on_idle_exit: Option<AmIdleExitFn>,
    pub user_data: UserData,
}

impl CClientListener {
    pub fn has_any(&self) -> bool {
        self.on_state.is_some()
            || self.on_paired.is_some()
            || self.on_log.is_some()
            || self.on_idle_exit.is_some()
    }
}

impl ClientListener for CClientListener {
    fn on_state_changed(&self, state: StateInfo) {
        let Some(f) = self.on_state else { return };
        // 字符串所有权转移给回调方（由其调用 am_string_free）。
        let reason_ptr = state
            .reason
            .as_deref()
            .map_or(std::ptr::null_mut(), into_raw_cstring);
        let retry = if state.status == StateStatus::Backoff {
            state.retry_in_ms.unwrap_or(0)
        } else {
            0
        };
        // SAFETY: 调用方注册的回调；reason 的所有权转移给回调方。
        unsafe { f(self.user_data.ptr(), state.status.into(), retry, reason_ptr) };
    }

    fn on_paired(&self, token: String) {
        let Some(f) = self.on_paired else { return };
        let token = into_raw_cstring(&token);
        // SAFETY: 调用方注册的回调；token 的所有权转移给回调方。
        unsafe { f(self.user_data.ptr(), token) };
    }

    fn on_log(&self, level: LogLevel, message: String) {
        let Some(f) = self.on_log else { return };
        let message = into_raw_cstring(&message);
        // SAFETY: 调用方注册的回调；message 的所有权转移给回调方。
        unsafe { f(self.user_data.ptr(), level.into(), message) };
    }

    fn on_idle_exit(&self) {
        let Some(f) = self.on_idle_exit else { return };
        // SAFETY: 调用方注册的回调。
        unsafe { f(self.user_data.ptr()) };
    }
}
