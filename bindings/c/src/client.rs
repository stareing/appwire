//! 客户端入口：创建、启停、状态查询与生命周期（唤醒、休眠、保持、根 scope）。

use std::ffi::{c_char, c_int, c_void};
use std::sync::Arc;

use app_mcp_native::{ClientListener, NativeClient};

use crate::callbacks::{AmFreeFn, AmNavigateFn, AmStateStatus, CClientListener, CNavigationHandler, UserData};
use crate::convert::{
    call_dedup_from, convert_config, convert_lifecycle, heartbeat_mode_from, prepare_out, read_options,
    sleep_reason_from, visibility_from, wake_reason_from,
};
use crate::ffi_types::{AmClientCallbacks, AmClientConfig, AmClientOptions, AmLifecycle};
use crate::handles::{AmClient, AmHold, AmScope, ScopeKind};
use crate::status::{AmStatus, FfiError, FfiResult, guard, guard_value};
use crate::strings::{into_raw_cstring, opt_str, req_str};

// ---------------------------------------------------------------------------
// 客户端
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_new(
    config: *const AmClientConfig,
    callbacks: *const AmClientCallbacks,
    out: *mut *mut AmClient,
) -> AmStatus {
    unsafe { am_client_new_ex(config, callbacks, std::ptr::null(), out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_new_ex(
    config: *const AmClientConfig,
    callbacks: *const AmClientCallbacks,
    options: *const AmClientOptions,
    out: *mut *mut AmClient,
) -> AmStatus {
    guard(|| {
        // 先接管 user_data，保证任何失败路径都会调用 free_user_data。
        // SAFETY: callbacks 为 NULL 或指向有效结构体。
        let mut listener = unsafe { callbacks.as_ref() }.map(|cb| CClientListener {
            on_state: cb.on_state,
            on_paired: cb.on_paired,
            on_log: cb.on_log,
            on_idle_exit: None,
            user_data: UserData::new(cb.user_data, cb.free_user_data),
        });
        let out = unsafe { prepare_out(out) }?;
        // SAFETY: options 为 NULL 或指向至少 struct_size 字节。
        let opts = unsafe { read_options(options) }?;
        if let Some(f) = opts.on_idle_exit {
            listener
                .get_or_insert_with(|| CClientListener {
                    on_state: None,
                    on_paired: None,
                    on_log: None,
                    on_idle_exit: None,
                    user_data: UserData::new(std::ptr::null_mut(), None),
                })
                .on_idle_exit = Some(f);
        }
        // SAFETY: config 为 NULL 或指向有效结构体。
        let config = unsafe { config.as_ref() }.ok_or_else(|| FfiError::null("config"))?;
        let mut cfg = unsafe { convert_config(config) }?;
        // SAFETY: lifecycle 为 NULL 或指向有效结构体。
        if let Some(l) = unsafe { opts.lifecycle.as_ref() } {
            cfg.lifecycle = unsafe { convert_lifecycle(l) }?;
        }
        if opts.connect_timeout_ms != 0 {
            cfg.connect_timeout_ms = opts.connect_timeout_ms;
        }
        cfg.heartbeat = heartbeat_mode_from(opts.heartbeat)?;
        match opts.host_absent_retries {
            0 => {}
            n if n < 0 => cfg.lifecycle.host_absent_retries = 0,
            n => cfg.lifecycle.host_absent_retries = n.unsigned_abs(),
        }
        cfg.lifecycle.legacy_timers = opts.legacy_timers;
        match opts.merge_window_ms {
            0 => {}
            n if n < 0 => cfg.lifecycle.merge_window_ms = 0,
            n => cfg.lifecycle.merge_window_ms = n.unsigned_abs(),
        }
        cfg.lifecycle.sleep_on_background = opts.sleep_on_background;
        cfg.call_dedup = call_dedup_from(cfg.call_dedup, opts.call_dedup_ttl_ms, opts.call_dedup_max_entries);
        cfg.register_name = opts.register_name;
        // SAFETY: name_instance 为 NULL 或指向以 NUL 结尾的字符串。
        cfg.name_instance = unsafe { opt_str(opts.name_instance, "options->name_instance") }?.map(str::to_owned);
        let listener: Option<Arc<dyn ClientListener>> = match listener {
            Some(l) if l.has_any() => Some(Arc::new(l)),
            _ => None,
        };
        let client = NativeClient::new(cfg, listener)?;
        *out = Box::into_raw(Box::new(AmClient::new(client)));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_free(client: *mut AmClient) {
    if client.is_null() {
        return;
    }
    let _ = guard(|| {
        // SAFETY: client 来自 am_client_new，调用方保证不再使用。
        let client = unsafe { Box::from_raw(client) };
        client.shared.shutdown();
        Ok(())
    });
}

pub(crate) unsafe fn client_ref<'a>(client: *const AmClient) -> FfiResult<&'a AmClient> {
    // SAFETY: client 为 NULL 或来自 am_client_new 且尚未释放。
    unsafe { client.as_ref() }.ok_or_else(|| FfiError::null("client"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_start(client: *mut AmClient) -> AmStatus {
    guard(|| {
        unsafe { client_ref(client) }?.shared.client()?.start();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_stop(client: *mut AmClient) -> AmStatus {
    guard(|| {
        unsafe { client_ref(client) }?.shared.client()?.stop();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_set_visibility(
    client: *mut AmClient,
    visibility: c_int,
    focused: bool,
) -> AmStatus {
    guard(|| {
        let c = unsafe { client_ref(client) }?;
        let v = visibility_from(visibility)?;
        c.shared.client()?.set_visibility(v, focused);
        Ok(())
    })
}

/// v14：设置导航回调；`handler` 为 NULL 时清除。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_set_navigation_handler(
    client: *mut AmClient,
    handler: Option<AmNavigateFn>,
    user_data: *mut c_void,
    free_user_data: Option<AmFreeFn>,
) -> AmStatus {
    guard(|| {
        let ud = UserData::new(user_data, free_user_data);
        let c = unsafe { client_ref(client) }?;
        let handler = handler.map(|f| Arc::new(CNavigationHandler { f, user_data: ud }) as Arc<dyn app_mcp_native::NavigationHandler>);
        c.shared.client()?.set_navigation_handler(handler);
        Ok(())
    })
}

/// v15：App 在后台（Hidden / Frozen）时是否仍把导航请求交给导航回调；false 时直接以
/// `USER_ACTION_REQUIRED`（reason "foreground"）回复。默认值随平台（桌面 true，移动端 false）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_set_navigate_in_background(client: *mut AmClient, enabled: bool) -> AmStatus {
    guard(|| {
        unsafe { client_ref(client) }?.shared.client()?.set_navigate_in_background(enabled);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_state(
    client: *const AmClient,
    status: *mut AmStateStatus,
    retry_in_ms: *mut u64,
    reason: *mut *mut c_char,
) -> AmStatus {
    guard(|| {
        // SAFETY: 输出指针为 NULL 或可写。
        if !reason.is_null() {
            unsafe { *reason = std::ptr::null_mut() };
        }
        let c = unsafe { client_ref(client) }?;
        let status = unsafe { status.as_mut() }.ok_or_else(|| FfiError::null("status"))?;
        let st = c.shared.client()?.state();
        *status = st.status.into();
        if let Some(r) = unsafe { retry_in_ms.as_mut() } {
            *r = if st.status == app_mcp_native::StateStatus::Backoff {
                st.retry_in_ms.unwrap_or(0)
            } else {
                0
            };
        }
        if !reason.is_null() {
            let text = match st.status {
                app_mcp_native::StateStatus::Rejected | app_mcp_native::StateStatus::HostMismatch => {
                    Some(st.reason.unwrap_or_default())
                }
                app_mcp_native::StateStatus::Backoff => st.reason,
                _ => None,
            };
            if let Some(text) = text {
                unsafe { *reason = into_raw_cstring(&text) };
            }
        }
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_instance_id(client: *const AmClient) -> *mut c_char {
    guard_value(std::ptr::null_mut(), || {
        let id = unsafe { client_ref(client) }?
            .shared
            .client()?
            .instance_id();
        Ok(into_raw_cstring(&id))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_token(client: *const AmClient) -> *mut c_char {
    guard_value(std::ptr::null_mut(), || {
        let token = unsafe { client_ref(client) }?.shared.client()?.token();
        Ok(token.map_or(std::ptr::null_mut(), |t| into_raw_cstring(&t)))
    })
}

/// v6：当前状态的错误码（spec/protocol.md 10.1）。
///
/// # Safety
/// `client` 为 NULL 或有效客户端；`code` 为 NULL 或可写。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_state_code(client: *const AmClient, code: *mut *mut c_char) -> AmStatus {
    guard(|| {
        // SAFETY: 输出指针为 NULL 或可写。
        let out = unsafe { code.as_mut() }.ok_or_else(|| FfiError::null("code"))?;
        *out = std::ptr::null_mut();
        let st = unsafe { client_ref(client) }?.shared.client()?.state();
        if let Some(c) = st.code {
            *out = into_raw_cstring(&c);
        }
        Ok(())
    })
}

/// v6：Host 为当前连接分配的连接 ID（spec/protocol.md 10.3）。
///
/// # Safety
/// `client` 为 NULL 或有效客户端；`id` 为 NULL 或可写。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_connection_id(client: *const AmClient, id: *mut *mut c_char) -> AmStatus {
    guard(|| {
        // SAFETY: 输出指针为 NULL 或可写。
        let out = unsafe { id.as_mut() }.ok_or_else(|| FfiError::null("id"))?;
        *out = std::ptr::null_mut();
        if let Some(cid) = unsafe { client_ref(client) }?.shared.client()?.connection_id() {
            *out = into_raw_cstring(&cid);
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// 客户端：生命周期（v3）
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_lifecycle_init(lifecycle: *mut AmLifecycle) {
    // SAFETY: lifecycle 为 NULL 或指向可写的结构体。
    if let Some(l) = unsafe { lifecycle.as_mut() } {
        *l = AmLifecycle::default();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_handle_wake(client: *mut AmClient, args: *const c_char) -> bool {
    guard_value(false, || {
        let c = unsafe { client_ref(client) }?;
        let args = unsafe { req_str(args, "args") }?;
        Ok(c.shared.client()?.handle_wake(args))
    })
}

/// 写入可选的 bool 输出参数。
pub(crate) unsafe fn write_flag(out: *mut bool, value: bool) {
    // SAFETY: out 为 NULL 或可写。
    if let Some(o) = unsafe { out.as_mut() } {
        *o = value;
    }
}

/// 执行一个返回 bool 的客户端操作，结果写入可选输出参数。
pub(crate) unsafe fn client_flag_op(
    client: *mut AmClient,
    out: *mut bool,
    f: impl FnOnce(&NativeClient) -> FfiResult<bool>,
) -> AmStatus {
    unsafe { write_flag(out, false) };
    guard(|| {
        let c = unsafe { client_ref(client) }?.shared.client()?;
        let v = f(&c)?;
        unsafe { write_flag(out, v) };
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_wake(client: *mut AmClient, started: *mut bool) -> AmStatus {
    unsafe { client_flag_op(client, started, |c| Ok(c.wake())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_wake_with_reason(
    client: *mut AmClient,
    reason: c_int,
    started: *mut bool,
) -> AmStatus {
    unsafe {
        client_flag_op(client, started, |c| {
            Ok(c.wake_with_reason(wake_reason_from(reason)?))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_connect_now(
    client: *mut AmClient,
    started: *mut bool,
) -> AmStatus {
    unsafe { client_flag_op(client, started, |c| Ok(c.connect_now())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_sleep(client: *mut AmClient, changed: *mut bool) -> AmStatus {
    unsafe { client_flag_op(client, changed, |c| Ok(c.sleep())) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_sleep_with_reason(
    client: *mut AmClient,
    reason: c_int,
    changed: *mut bool,
) -> AmStatus {
    unsafe {
        client_flag_op(client, changed, |c| {
            Ok(c.sleep_with_reason(sleep_reason_from(reason)?))
        })
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_hold(client: *mut AmClient, out: *mut *mut AmHold) -> AmStatus {
    guard(|| {
        let out = unsafe { prepare_out(out) }?;
        let c = unsafe { client_ref(client) }?.shared.client()?;
        *out = Box::into_raw(Box::new(AmHold { handle: c.hold() }));
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_hold_release(hold: *mut AmHold) {
    if hold.is_null() {
        return;
    }
    let _ = guard(|| {
        // SAFETY: hold 来自本库，调用方保证不再使用。
        let h = unsafe { Box::from_raw(hold) };
        h.handle.release();
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_tools_hash(client: *const AmClient) -> *mut c_char {
    guard_value(std::ptr::null_mut(), || {
        let hash = unsafe { client_ref(client) }?.shared.client()?.tools_hash();
        Ok(into_raw_cstring(&hash))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_parse_wake_token(args: *const c_char) -> *mut c_char {
    guard_value(std::ptr::null_mut(), || {
        let args = unsafe { req_str(args, "args") }?;
        Ok(app_mcp_native::parse_wake_token(args)
            .map_or(std::ptr::null_mut(), |t| into_raw_cstring(&t)))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn am_client_root_scope(
    client: *mut AmClient,
    out: *mut *mut AmScope,
) -> AmStatus {
    guard(|| {
        let out = unsafe { prepare_out(out) }?;
        let c = unsafe { client_ref(client) }?;
        c.shared.check_alive()?;
        *out = Box::into_raw(Box::new(AmScope {
            kind: ScopeKind::Root,
            shared: c.shared.clone(),
        }));
        Ok(())
    })
}
