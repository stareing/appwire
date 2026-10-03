//! 异步操作入口：工具调用（含进度）、取消、读资源、订阅、实例选择、策略与 dispatch。

use std::ffi::{c_char, c_int, c_void};

use hub::{CallRequest, ErrorKind, ProgressUpdate, ToolError};
use serde_json::{Value, json};

use crate::dispatch::{self, AmHubResultFn, Dispatcher, ResultCb};
use crate::ffi_util::{self, AmHubStatus, FfiError, guard, opt_str, req_str, write_out_str};
use crate::handle::{AmHub, STOPPED_MESSAGE};
use crate::json::{dispatch_fallback, error_json, format_from, outcome_error_json};
use crate::query::hub_ref;

// ---------------------------------------------------------------------------
// 调用
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_call(
    hub: *mut AmHub,
    request_json: *const c_char,
    cb: Option<AmHubResultFn>,
    user_data: *mut c_void,
    out_call_id: *mut *mut c_char,
) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        unsafe { write_out_str(out_call_id, None) };
        // SAFETY: 同上。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let text = unsafe { req_str(request_json, "request_json") }?;
        let cb = cb.ok_or_else(|| FfiError::null("cb"))?;
        let mut req: CallRequest =
            serde_json::from_str(text).map_err(|e| FfiError::json("request_json", e))?;
        let call_id = req.call_id.get_or_insert_with(|| h.next_call_id()).clone();
        let fallback =
            outcome_error_json(&call_id, ToolError::new(ErrorKind::Cancelled, STOPPED_MESSAGE));
        let rc = ResultCb::new(cb, user_data, h.dispatcher.clone(), fallback);
        let id = call_id.clone();
        h.spawn_result(rc, move |hub| async move {
            match hub.call_tool(req).await {
                Ok(o) => serde_json::to_string(&o).unwrap_or_default(),
                Err(e) => outcome_error_json(&id, e.0),
            }
        })?;
        // SAFETY: 同上。
        unsafe { write_out_str(out_call_id, Some(&call_id)) };
        Ok(())
    })
}

/// v11：调用进度回调（spec/hub-api.md 3.12）。`progress_json` 归回调方所有。
pub type AmHubProgressFn = unsafe extern "C" fn(user_data: *mut c_void, progress_json: *mut c_char);

/// 一条进度 → `{"callId", "progress", "total"?, "message"?}`。
pub(crate) fn progress_json(call_id: &str, p: &ProgressUpdate) -> String {
    let mut v = json!({ "callId": call_id, "progress": p.progress });
    if let Some(t) = p.total {
        v["total"] = json!(t);
    }
    if let Some(m) = &p.message {
        v["message"] = json!(m);
    }
    v.to_string()
}

/// 把一条进度排到分发线程上回调（与结果回调同一队列，因此先于结果到达）。
pub(crate) fn post_progress(dispatcher: &Dispatcher, f: AmHubProgressFn, user_data: dispatch::SendPtr, json: String) {
    let job: Box<dyn FnOnce() + Send> = Box::new(move || {
        let ud = user_data;
        // SAFETY: 调用方提供的回调；字符串所有权转移给回调方。
        unsafe { f(ud.0, ffi_util::into_raw_cstring(&json)) };
    });
    // 分发线程已关闭（Hub 释放中）时丢弃进度：进度不保证送达。
    let _ = dispatcher.post(job);
}

/// v11：同 [`am_hub_call`]，并接收调用进度（`Hub::call_tool_with_progress`）。
///
/// @input on_progress 可为 NULL（等同 [`am_hub_call`]）；与 `cb` 共用 `user_data`。
/// @invariant 进度回调与结果回调在同一分发线程上串行执行，全部进度回调先于结果回调；结果回调之后不再有进度回调。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_call_with_progress(
    hub: *mut AmHub,
    request_json: *const c_char,
    cb: Option<AmHubResultFn>,
    on_progress: Option<AmHubProgressFn>,
    user_data: *mut c_void,
    out_call_id: *mut *mut c_char,
) -> AmHubStatus {
    let Some(on_progress) = on_progress else {
        // SAFETY: 由调用方保证。
        return unsafe { am_hub_call(hub, request_json, cb, user_data, out_call_id) };
    };
    guard(|| {
        // SAFETY: 由调用方保证。
        unsafe { write_out_str(out_call_id, None) };
        // SAFETY: 同上。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let text = unsafe { req_str(request_json, "request_json") }?;
        let cb = cb.ok_or_else(|| FfiError::null("cb"))?;
        let mut req: CallRequest =
            serde_json::from_str(text).map_err(|e| FfiError::json("request_json", e))?;
        let call_id = req.call_id.get_or_insert_with(|| h.next_call_id()).clone();
        let fallback =
            outcome_error_json(&call_id, ToolError::new(ErrorKind::Cancelled, STOPPED_MESSAGE));
        let rc = ResultCb::new(cb, user_data, h.dispatcher.clone(), fallback);
        let id = call_id.clone();
        let dispatcher = h.dispatcher.clone();
        let ud = dispatch::SendPtr(user_data);
        h.spawn_result(rc, move |hub| async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProgressUpdate>();
            let call = hub.call_tool_with_progress(req, tx);
            tokio::pin!(call);
            let res = loop {
                tokio::select! {
                    biased;
                    Some(p) = rx.recv() => post_progress(&dispatcher, on_progress, ud, progress_json(&id, &p)),
                    res = &mut call => break res,
                }
            };
            while let Ok(p) = rx.try_recv() {
                post_progress(&dispatcher, on_progress, ud, progress_json(&id, &p));
            }
            match res {
                Ok(o) => serde_json::to_string(&o).unwrap_or_default(),
                Err(e) => outcome_error_json(&id, e.0),
            }
        })?;
        // SAFETY: 同上。
        unsafe { write_out_str(out_call_id, Some(&call_id)) };
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_cancel_call(hub: *mut AmHub, call_id: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let id = unsafe { req_str(call_id, "call_id") }?;
        h.hub()?.cancel_call(id);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_read_resource(
    hub: *mut AmHub,
    uri: *const c_char,
    cb: Option<AmHubResultFn>,
    user_data: *mut c_void,
) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let uri = unsafe { req_str(uri, "uri") }?.to_owned();
        let cb = cb.ok_or_else(|| FfiError::null("cb"))?;
        let fallback = json!({
            "error": error_json(&ToolError::new(ErrorKind::Cancelled, STOPPED_MESSAGE))
        })
        .to_string();
        let rc = ResultCb::new(cb, user_data, h.dispatcher.clone(), fallback);
        h.spawn_result(rc, move |hub| async move {
            match hub.read_resource(&uri).await {
                Ok(c) => json!({ "ok": c }).to_string(),
                Err(e) => json!({ "error": error_json(&e.0) }).to_string(),
            }
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_subscribe(hub: *mut AmHub, uri: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let uri = unsafe { req_str(uri, "uri") }?;
        let inner = h.hub()?;
        let _rt = h.handle.enter();
        inner.subscribe(uri)?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_unsubscribe(hub: *mut AmHub, uri: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let uri = unsafe { req_str(uri, "uri") }?;
        let inner = h.hub()?;
        let _rt = h.handle.enter();
        inner.unsubscribe(uri);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_select_instance(
    hub: *mut AmHub,
    app_id: *const c_char,
    instance_id: *const c_char,
) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let app_id = unsafe { req_str(app_id, "app_id") }?;
        // SAFETY: 同上。
        let instance_id = unsafe { opt_str(instance_id, "instance_id") }?;
        h.hub()?.select_instance(app_id, instance_id);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_reset_session(hub: *mut AmHub, session: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let session = unsafe { opt_str(session, "session") }?;
        h.hub()?.reset_session(session);
        Ok(())
    })
}

/// 替换策略规则集（v10，spec/hub-api.md 3.13）。
///
/// @error JSON 不合法或有未知字段 → `InvalidJson`；规则不合法 → `InvalidConfig`（之前的规则继续生效，原因记入
/// `HubStatus.policy.lastError`）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_set_policy(hub: *mut AmHub, policy_json: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let text = unsafe { req_str(policy_json, "policy_json") }?;
        let policy: hub::PolicyConfig = serde_json::from_str(text).map_err(|e| FfiError::json("policy_json", e))?;
        h.hub()?
            .set_policy(policy)
            .map_err(|e| FfiError::new(AmHubStatus::InvalidConfig, e.0.message))
    })
}

/// 替换 Agent 登记（v19，spec/hub-api.md 3.6「Agent 身份」）：`[{"name","token"}]`，`[]` 清空。只影响之后到达的 MCP 请求。
///
/// @error 不是合法 JSON 或结构不符 → `InvalidJson`；登记不合法 → `InvalidConfig`（之前的登记继续生效；信息不含令牌）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_set_agents(hub: *mut AmHub, agents_json: *const c_char) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        // SAFETY: 同上。
        let text = unsafe { req_str(agents_json, "agents_json") }?;
        let agents: Vec<hub::AgentCredential> =
            serde_json::from_str(text).map_err(|e| FfiError::json("agents_json", e))?;
        let config = crate::config::parse_agents(agents)?;
        h.hub()?
            .set_agents(config)
            .map_err(|e| FfiError::new(AmHubStatus::InvalidConfig, e.0.message))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hub_dispatch(
    hub: *mut AmHub,
    format: c_int,
    tool_call_json: *const c_char,
    session: *const c_char,
    cb: Option<AmHubResultFn>,
    user_data: *mut c_void,
) -> AmHubStatus {
    guard(|| {
        // SAFETY: 由调用方保证。
        let h = unsafe { hub_ref(hub) }?;
        let format = format_from(format)?;
        // SAFETY: 同上。
        let text = unsafe { req_str(tool_call_json, "tool_call_json") }?;
        // SAFETY: 同上。
        let session = unsafe { opt_str(session, "session") }?.map(str::to_owned);
        let cb = cb.ok_or_else(|| FfiError::null("cb"))?;
        let call: Value =
            serde_json::from_str(text).map_err(|e| FfiError::json("tool_call_json", e))?;
        let fallback = dispatch_fallback(format, &call);
        let rc = ResultCb::new(cb, user_data, h.dispatcher.clone(), fallback);
        h.spawn_result(rc, move |hub| async move {
            hub.dispatch_in_session(format, call, session.as_deref())
                .await
                .to_string()
        })
    })
}
