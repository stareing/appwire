//! 来自 Host 的请求句柄：工具调用、资源读取与导航的参数读取和完成 / 失败回复。

use std::ffi::{c_char, c_void};
use std::sync::Arc;

use app_mcp_native::ErrorKind;

use crate::callbacks::{AmCancelFn, AmFreeFn, CCancelListener, UserData};
use crate::convert::{consume, error_kind_from, prepare_out, read_call_result, read_hints};
use crate::ffi_types::AmCallResult;
use crate::handles::{AmCall, AmHold, AmNavigate, AmRead};
use crate::status::{AmStatus, FfiError, guard, guard_value};
use crate::strings::{lossy_str, opt_str};

// ---------------------------------------------------------------------------
// 调用
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_id(call: *const AmCall) -> *const c_char {
    // SAFETY: call 为 NULL 或尚未消费的调用。
    unsafe { call.as_ref() }.map_or(std::ptr::null(), |c| c.id.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_tool_name(call: *const AmCall) -> *const c_char {
    unsafe { call.as_ref() }.map_or(std::ptr::null(), |c| c.tool_name.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_arguments_json(call: *const AmCall) -> *const c_char {
    unsafe { call.as_ref() }.map_or(std::ptr::null(), |c| c.arguments.as_ptr())
}

/// v16：Agent 幂等键（spec/protocol.md 3.3）；没有时返回 NULL。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_idempotency_key(call: *const AmCall) -> *const c_char {
    unsafe { call.as_ref() }
        .and_then(|c| c.idempotency_key.as_ref())
        .map_or(std::ptr::null(), |k| k.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_is_cancelled(call: *const AmCall) -> bool {
    guard_value(false, || {
        Ok(unsafe { call.as_ref() }.is_some_and(|c| c.handle.is_cancelled()))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_set_cancel_callback(
    call: *mut AmCall,
    on_cancel: Option<AmCancelFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
) -> AmStatus {
    guard(|| {
        let ud = UserData::new(user_data, free_user_data);
        let call = unsafe { call.as_ref() }.ok_or_else(|| FfiError::null("call"))?;
        let f = on_cancel.ok_or_else(|| FfiError::null("on_cancel"))?;
        call.handle
            .set_cancel_listener(Arc::new(CCancelListener { f, user_data: ud }));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_complete(
    call: *mut AmCall,
    data_json: *const c_char,
    state_hints: *const *const c_char,
    state_hints_len: usize,
) -> AmStatus {
    if call.is_null() {
        return guard(|| Err(FfiError::null("call")));
    }
    let status = guard(|| {
        // SAFETY: call 非空且尚未消费。
        let c = unsafe { &*call };
        let data = unsafe { opt_str(data_json, "data_json") }
            .map_err(|_| FfiError::new(AmStatus::InvalidJson, "data_json 不是合法的 UTF-8"))?;
        let hints = match unsafe { read_hints(state_hints, state_hints_len) } {
            Ok(h) => h,
            Err(e) => {
                // call 仍会被消费，因此以 HANDLER_ERROR 结束调用，避免调用悬挂到超时。
                let _ = c.handle.fail(
                    ErrorKind::HandlerError,
                    &format!("state_hints 非法：{}", e.message),
                );
                return Err(e);
            }
        };
        c.handle.complete(data, hints)?;
        Ok(())
    });
    if status != AmStatus::InvalidJson {
        unsafe { consume(call) };
    }
    status
}

/// v9：成功完成并消费 call，附带业务状态、摘要与内容注解（`result` 可为 NULL）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_complete_ex(call: *mut AmCall, result: *const AmCallResult) -> AmStatus {
    if call.is_null() {
        return guard(|| Err(FfiError::null("call")));
    }
    let status = guard(|| {
        // SAFETY: call 非空且尚未消费。
        let c = unsafe { &*call };
        // SAFETY: result 为 NULL 或指向至少 struct_size 字节。
        let result = match unsafe { read_call_result(result) } {
            Ok(r) => r,
            Err(e) if e.status == AmStatus::InvalidJson => return Err(e),
            Err(e) => {
                // call 仍会被消费，因此以 HANDLER_ERROR 结束调用，避免调用悬挂到超时。
                let _ = c.handle.fail(ErrorKind::HandlerError, &format!("调用结果非法：{}", e.message));
                return Err(e);
            }
        };
        c.handle.complete_with(result)?;
        Ok(())
    });
    if status != AmStatus::InvalidJson {
        unsafe { consume(call) };
    }
    status
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_fail(
    call: *mut AmCall,
    kind: *const c_char,
    message: *const c_char,
) -> AmStatus {
    if call.is_null() {
        return guard(|| Err(FfiError::null("call")));
    }
    let status = guard(|| {
        let c = unsafe { &*call };
        let kind = error_kind_from(unsafe { lossy_str(kind) }.as_deref());
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        c.handle.fail(kind, &message)?;
        Ok(())
    });
    unsafe { consume(call) };
    status
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_fail_with_details(
    call: *mut AmCall,
    kind: *const c_char,
    message: *const c_char,
    details_json: *const c_char,
) -> AmStatus {
    if call.is_null() {
        return guard(|| Err(FfiError::null("call")));
    }
    let status = guard(|| {
        let c = unsafe { &*call };
        let kind = error_kind_from(unsafe { lossy_str(kind) }.as_deref());
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        let details = unsafe { opt_str(details_json, "details_json") }
            .map_err(|_| FfiError::new(AmStatus::InvalidJson, "details_json 不是合法的 UTF-8"))?;
        c.handle.fail_with_details(kind, &message, details)?;
        Ok(())
    });
    if status != AmStatus::InvalidJson {
        unsafe { consume(call) };
    }
    status
}

/// v11：以 `USER_ACTION_REQUIRED` 失败完成并消费 call；reason / uri 为 NULL 时不出现在错误的 data 中。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_fail_user_action(
    call: *mut AmCall,
    message: *const c_char,
    reason: *const c_char,
    uri: *const c_char,
) -> AmStatus {
    if call.is_null() {
        return guard(|| Err(FfiError::null("call")));
    }
    let status = guard(|| {
        let c = unsafe { &*call };
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        let reason = unsafe { lossy_str(reason) };
        let uri = unsafe { lossy_str(uri) };
        c.handle.fail_user_action(&message, reason.as_deref(), uri.as_deref())?;
        Ok(())
    });
    unsafe { consume(call) };
    status
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_hold(call: *const AmCall, out: *mut *mut AmHold) -> AmStatus {
    guard(|| {
        let out = unsafe { prepare_out(out) }?;
        let c = unsafe { call.as_ref() }.ok_or_else(|| FfiError::null("call"))?;
        *out = Box::into_raw(Box::new(AmHold {
            handle: c.handle.hold()?,
        }));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_call_progress(
    call: *const AmCall,
    progress: f64,
    total: f64,
    message: *const c_char,
) -> AmStatus {
    guard(|| {
        let c = unsafe { call.as_ref() }.ok_or_else(|| FfiError::null("call"))?;
        let message = unsafe { opt_str(message, "message") }?;
        let total = (total >= 0.0).then_some(total);
        c.handle.report_progress(progress, total, message)?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// 资源读取
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_read_resource_name(read: *const AmRead) -> *const c_char {
    unsafe { read.as_ref() }.map_or(std::ptr::null(), |r| r.resource_name.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_read_complete(
    read: *mut AmRead,
    contents_json: *const c_char,
) -> AmStatus {
    if read.is_null() {
        return guard(|| Err(FfiError::null("read")));
    }
    let status = guard(|| {
        let r = unsafe { &*read };
        let contents = unsafe { opt_str(contents_json, "contents_json") }
            .map_err(|_| FfiError::new(AmStatus::InvalidJson, "contents_json 不是合法的 UTF-8"))?
            .ok_or_else(|| FfiError::new(AmStatus::InvalidJson, "contents_json 不能为 NULL"))?;
        r.handle.complete(contents)?;
        Ok(())
    });
    if status != AmStatus::InvalidJson {
        unsafe { consume(read) };
    }
    status
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_read_fail(
    read: *mut AmRead,
    kind: *const c_char,
    message: *const c_char,
) -> AmStatus {
    if read.is_null() {
        return guard(|| Err(FfiError::null("read")));
    }
    let status = guard(|| {
        let r = unsafe { &*read };
        let kind = error_kind_from(unsafe { lossy_str(kind) }.as_deref());
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        r.handle.fail(kind, &message)?;
        Ok(())
    });
    unsafe { consume(read) };
    status
}

/// v12：失败完成并消费 read，附带结构化详情；语义同 [`am_call_fail_with_details`]（`details_json` 为 NULL 等同
/// [`am_read_fail`]；非法时返回 `AM_ERR_INVALID_JSON` 且不消费 read）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_read_fail_with_details(
    read: *mut AmRead,
    kind: *const c_char,
    message: *const c_char,
    details_json: *const c_char,
) -> AmStatus {
    if read.is_null() {
        return guard(|| Err(FfiError::null("read")));
    }
    let status = guard(|| {
        let r = unsafe { &*read };
        let kind = error_kind_from(unsafe { lossy_str(kind) }.as_deref());
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        let details = unsafe { opt_str(details_json, "details_json") }
            .map_err(|_| FfiError::new(AmStatus::InvalidJson, "details_json 不是合法的 UTF-8"))?;
        r.handle.fail_with_details(kind, &message, details)?;
        Ok(())
    });
    if status != AmStatus::InvalidJson {
        unsafe { consume(read) };
    }
    status
}

/// v12：以 `USER_ACTION_REQUIRED` 失败完成并消费 read；语义同 [`am_call_fail_user_action`]。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_read_fail_user_action(
    read: *mut AmRead,
    message: *const c_char,
    reason: *const c_char,
    uri: *const c_char,
) -> AmStatus {
    if read.is_null() {
        return guard(|| Err(FfiError::null("read")));
    }
    let status = guard(|| {
        let r = unsafe { &*read };
        let message = unsafe { lossy_str(message) }.unwrap_or_default();
        let reason = unsafe { lossy_str(reason) };
        let uri = unsafe { lossy_str(uri) };
        r.handle.fail_user_action(&message, reason.as_deref(), uri.as_deref())?;
        Ok(())
    });
    unsafe { consume(read) };
    status
}

// ---------------------------------------------------------------------------
// 导航（v14）
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_page(navigate: *const AmNavigate) -> *const c_char {
    unsafe { navigate.as_ref() }.map_or(std::ptr::null(), |n| n.page.as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_params_json(navigate: *const AmNavigate) -> *const c_char {
    unsafe { navigate.as_ref() }
        .and_then(|n| n.params_json.as_ref())
        .map_or(std::ptr::null(), |p| p.as_ptr())
}

/// 完成并消费 `navigate`：`f` 对句柄提交结果。
pub(crate) unsafe fn finish_navigate(
    navigate: *mut AmNavigate,
    f: impl FnOnce(&AmNavigate) -> Result<(), app_mcp_native::NativeError>,
) -> AmStatus {
    if navigate.is_null() {
        return guard(|| Err(FfiError::null("navigate")));
    }
    let status = guard(|| {
        f(unsafe { &*navigate })?;
        Ok(())
    });
    unsafe { consume(navigate) };
    status
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_complete(navigate: *mut AmNavigate) -> AmStatus {
    unsafe { finish_navigate(navigate, |n| n.handle.complete()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_fail(navigate: *mut AmNavigate, message: *const c_char) -> AmStatus {
    let message = unsafe { lossy_str(message) }.unwrap_or_default();
    unsafe { finish_navigate(navigate, |n| n.handle.fail(&message)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_deny(navigate: *mut AmNavigate, message: *const c_char) -> AmStatus {
    let message = unsafe { lossy_str(message) }.unwrap_or_default();
    unsafe { finish_navigate(navigate, |n| n.handle.deny(&message)) }
}

/// v15：以 `USER_ACTION_REQUIRED` 结束导航并消费 `navigate`；reason / uri 为 NULL 时不出现在错误的 data 中。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_navigate_fail_user_action(
    navigate: *mut AmNavigate,
    message: *const c_char,
    reason: *const c_char,
    uri: *const c_char,
) -> AmStatus {
    let message = unsafe { lossy_str(message) }.unwrap_or_default();
    let reason = unsafe { lossy_str(reason) };
    let uri = unsafe { lossy_str(uri) };
    unsafe { finish_navigate(navigate, |n| n.handle.fail_user_action(&message, reason.as_deref(), uri.as_deref())) }
}
