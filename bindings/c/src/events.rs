//! v20 事件（第 16 项 N3，spec/protocol.md 3.5）：声明、撤销与发出。

use std::ffi::c_char;

use app_mcp_native::EventInfo;
use serde_json::Value;

use crate::client::client_ref;
use crate::handles::AmClient;
use crate::status::{AmStatus, FfiError, FfiResult, guard};
use crate::strings::{opt_str, req_str};

/// 可为 NULL 的输出 bool。
///
/// # Safety
/// `out` 为 NULL 或指向可写的 bool。
unsafe fn write_opt_bool(out: *mut bool, value: bool) {
    // SAFETY: 由调用方保证。
    if let Some(slot) = unsafe { out.as_mut() } {
        *slot = value;
    }
}

/// `payload_schema_json` → JSON 对象；NULL = 不声明。
/// @error 不是合法 JSON 或不是对象 → `AM_ERR_INVALID_SCHEMA`。
pub(crate) fn parse_payload_schema(text: Option<&str>) -> FfiResult<Option<Value>> {
    let Some(text) = text else { return Ok(None) };
    let value: Value = serde_json::from_str(text)
        .map_err(|e| FfiError::new(AmStatus::InvalidSchema, format!("事件载荷 schema 不是合法 JSON：{e}")))?;
    if !value.is_object() {
        return Err(FfiError::new(AmStatus::InvalidSchema, "事件载荷 schema 必须是 JSON 对象"));
    }
    Ok(Some(value))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_declare_event(
    client: *mut AmClient,
    name: *const c_char,
    description: *const c_char,
    payload_schema_json: *const c_char,
) -> AmStatus {
    guard(|| {
        let c = unsafe { client_ref(client) }?;
        // SAFETY: 字符串为 NULL 或以 NUL 结尾（头文件约定）。
        let name = unsafe { req_str(name, "name") }?.to_owned();
        let description = unsafe { req_str(description, "description") }?.to_owned();
        let payload_schema = parse_payload_schema(unsafe { opt_str(payload_schema_json, "payload_schema_json") }?)?;
        c.shared.client()?.declare_event(EventInfo { name, description, payload_schema })?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_remove_event(client: *mut AmClient, name: *const c_char, removed: *mut bool) -> AmStatus {
    guard(|| {
        let c = unsafe { client_ref(client) }?;
        let name = unsafe { req_str(name, "name") }?;
        let done = c.shared.client()?.remove_event(name);
        unsafe { write_opt_bool(removed, done) };
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_emit_event(
    client: *mut AmClient,
    name: *const c_char,
    payload_json: *const c_char,
    sent: *mut bool,
) -> AmStatus {
    guard(|| {
        unsafe { write_opt_bool(sent, false) };
        let c = unsafe { client_ref(client) }?;
        let name = unsafe { req_str(name, "name") }?;
        // @why 非法 UTF-8 的载荷按 AM_ERR_INVALID_JSON（与 am_call_complete 的 data_json 一致），名称仍按 INVALID_ARGUMENT。
        let payload = unsafe { opt_str(payload_json, "payload_json") }
            .map_err(|e| FfiError::new(AmStatus::InvalidJson, e.message))?;
        let ok = c.shared.client()?.emit_event(name, payload)?;
        unsafe { write_opt_bool(sent, ok) };
        Ok(())
    })
}
