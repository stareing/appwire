//! 同步查询入口：App / 工具 / 资源列表、状态、总览与工具导出。

use std::ffi::{c_char, c_int};

use hub::Hub;

use crate::ffi_util::{AmHubStatus, FfiError, FfiResult, guard, opt_str, req_str, write_out_str};
use crate::handle::AmHub;
use crate::json::{format_from, parse_filter, to_json};

// ---------------------------------------------------------------------------
// 查询
// ---------------------------------------------------------------------------

/// # Safety
/// `p` 为 NULL 或 `am_hub_start` 返回且尚未释放的句柄。
pub(crate) unsafe fn hub_ref<'a>(p: *const AmHub) -> FfiResult<&'a AmHub> {
    // SAFETY: 由调用方保证。
    unsafe { p.as_ref() }.ok_or_else(|| FfiError::null("hub"))
}

/// 同步查询的公共部分：检查 out、执行、写出字符串。
pub(crate) unsafe fn query(
    hub: *const AmHub,
    out: *mut *mut c_char,
    f: impl FnOnce(&AmHub, &Hub) -> FfiResult<String>,
) -> AmHubStatus {
    guard(|| {
        if out.is_null() {
            return Err(FfiError::null("out_json"));
        }
        // SAFETY: out 非 NULL，由调用方保证可写。
        unsafe { write_out_str(out, None) };
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        let inner = h.hub()?;
        let _rt = h.handle.enter();
        let s = f(h, &inner)?;
        // SAFETY: 同上。
        unsafe { write_out_str(out, Some(&s)) };
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_apps_json(hub: *const AmHub, out_json: *mut *mut c_char) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe { query(hub, out_json, |_, h| to_json(&h.apps())) }
}

/// 运行状态（HubStatus，与 `GET /status` 相同，spec/hub-api.md 3.9）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_status_json(hub: *const AmHub, out_json: *mut *mut c_char) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe { query(hub, out_json, |_, h| to_json(&h.status())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_tools_json(
    hub: *const AmHub,
    filter_json: *const c_char,
    out_json: *mut *mut c_char,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe {
        query(hub, out_json, |_, h| {
            let filter = parse_filter(opt_str(filter_json, "filter_json")?)?;
            to_json(&h.tools(&filter))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_resources_json(
    hub: *const AmHub,
    out_json: *mut *mut c_char,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe { query(hub, out_json, |_, h| to_json(&h.resources())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_overview_json(
    hub: *const AmHub,
    app_id: *const c_char,
    out_json: *mut *mut c_char,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe {
        query(hub, out_json, |_, h| {
            let app_id = req_str(app_id, "app_id")?;
            to_json(&h.overview(app_id))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_export_tools(
    hub: *const AmHub,
    format: c_int,
    filter_json: *const c_char,
    out_json: *mut *mut c_char,
) -> AmHubStatus {
    // SAFETY: 转交调用方的保证。
    unsafe {
        query(hub, out_json, |_, h| {
            let format = format_from(format)?;
            let filter = parse_filter(opt_str(filter_json, "filter_json")?)?;
            to_json(&h.export_tools(format, &filter))
        })
    }
}
