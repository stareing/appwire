//! 单元测试：不依赖原生运行时的部分（字符串转换、错误码映射、空指针、头文件一致性）。

use std::cell::Cell;
use std::ffi::{CStr, CString};
use std::ptr;

use super::*;

fn last_error() -> String {
    // SAFETY: am_last_error_message 总是返回有效的 C 字符串。
    unsafe { CStr::from_ptr(am_last_error_message()) }
        .to_string_lossy()
        .into_owned()
}

thread_local! {
    static FREED: Cell<usize> = const { Cell::new(0) };
}

unsafe extern "C" fn count_free(_ud: *mut c_void) {
    FREED.with(|f| f.set(f.get() + 1));
}

unsafe extern "C" fn noop_tool(_ud: *mut c_void, _call: *mut AmCall) {}

fn freed() -> usize {
    FREED.with(Cell::get)
}

#[test]
fn version_is_crate_version() {
    let v = unsafe { CStr::from_ptr(am_version()) };
    assert_eq!(v.to_str().ok(), Some(env!("CARGO_PKG_VERSION")));
}

#[test]
fn cstring_lossy_strips_nul() {
    let c = strings::to_cstring_lossy("a\0b");
    assert_eq!(c.as_bytes(), b"ab");
    let raw = into_raw_cstring("héllo");
    let back = unsafe { CStr::from_ptr(raw) }.to_str().map(str::to_owned);
    assert_eq!(back.as_deref(), Ok("héllo"));
    unsafe { am_string_free(raw) };
    unsafe { am_string_free(ptr::null_mut()) };
}

#[test]
fn opt_and_req_str() {
    let good = CString::new("x").unwrap_or_default();
    assert_eq!(unsafe { opt_str(ptr::null(), "p") }, Ok(None));
    assert_eq!(unsafe { opt_str(good.as_ptr(), "p") }, Ok(Some("x")));
    let err = unsafe { req_str(ptr::null(), "p") }.err().map(|e| e.status);
    assert_eq!(err, Some(AmStatus::InvalidArgument));
    let bad = [0xffu8, 0xfe, 0];
    let err = unsafe { opt_str(bad.as_ptr().cast(), "p") }
        .err()
        .map(|e| e.status);
    assert_eq!(err, Some(AmStatus::InvalidArgument));
    assert_eq!(
        unsafe { lossy_str(bad.as_ptr().cast()) }.map(|s| s.chars().count()),
        Some(2)
    );
}

#[test]
fn native_error_mapping() {
    use app_mcp_native::NativeError as E;
    let cases = [
        (E::InvalidName("x".into()), AmStatus::InvalidName),
        (E::InvalidSchema("x".into()), AmStatus::InvalidSchema),
        (E::DuplicateName("x".into()), AmStatus::DuplicateName),
        (E::InvalidJson("x".into()), AmStatus::InvalidJson),
        (E::InvalidConfig("x".into()), AmStatus::InvalidConfig),
        (E::AlreadyCompleted, AmStatus::AlreadyCompleted),
        (E::Disposed, AmStatus::Disposed),
        (E::Stopped, AmStatus::Stopped),
        (E::Internal("x".into()), AmStatus::Internal),
    ];
    for (e, s) in cases {
        let msg = e.to_string();
        let f: FfiError = e.into();
        assert_eq!(f.status, s);
        assert_eq!(f.message, msg);
    }
}

#[test]
fn guard_sets_last_error_and_catches_panic() {
    let s = guard(|| Err(FfiError::new(AmStatus::DuplicateName, "重名")));
    assert_eq!(s, AmStatus::DuplicateName);
    assert_eq!(last_error(), "重名");
    // 成功不清除最近一次错误。
    assert_eq!(guard(|| Ok(())), AmStatus::Ok);
    assert_eq!(last_error(), "重名");

    let s = guard(|| panic!("boom"));
    assert_eq!(s, AmStatus::Panic);
    assert!(last_error().contains("boom"));
    assert_eq!(guard_value(7, || panic!("x")), 7);
}

#[test]
fn enum_conversions() {
    assert_eq!(risk_from(4).ok(), Some(Risk::OsSensitive));
    assert!(risk_from(5).is_err());
    assert_eq!(activation_from(-1).ok(), Some(None));
    assert_eq!(activation_from(2).ok(), Some(Some(Activation::Foreground)));
    assert!(activation_from(3).is_err());
    assert_eq!(visibility_from(2).ok(), Some(Visibility::Frozen));
    assert!(visibility_from(-1).is_err());
    assert_eq!(client_kind_from(1).ok(), Some(ClientKind::Hybrid));
    assert!(client_kind_from(2).is_err());
    assert_eq!(
        error_kind_from(Some("USER_REJECTED")),
        ErrorKind::UserRejected
    );
    assert_eq!(error_kind_from(Some("USER_ACTION_REQUIRED")), ErrorKind::UserActionRequired);
    assert_eq!(error_kind_from(Some("POLICY_DENIED")), ErrorKind::PolicyDenied);
    assert_eq!(error_kind_from(Some("nope")), ErrorKind::HandlerError);
    assert_eq!(error_kind_from(None), ErrorKind::HandlerError);
}

#[test]
fn config_conversion() {
    let id = CString::new("com.example").unwrap_or_default();
    let name = CString::new("Example").unwrap_or_default();
    let mut cfg = AmClientConfig {
        app_id: id.as_ptr(),
        app_name: name.as_ptr(),
        instance_id: ptr::null(),
        host_url: ptr::null(),
        app_version: ptr::null(),
        instance_title: ptr::null(),
        token: ptr::null(),
        launch_token: ptr::null(),
        client_kind: 1,
        max_concurrent_calls: 0,
        overview_summary: ptr::null(),
        overview_body: ptr::null(),
        overview_locale: ptr::null(),
    };
    let c = unsafe { convert_config(&cfg) };
    let c = c.as_ref().ok();
    assert_eq!(c.map(|c| c.max_concurrent_calls), Some(1));
    assert_eq!(c.map(|c| c.client_kind), Some(ClientKind::Hybrid));
    assert_eq!(
        c.map(|c| c.host_url.as_str()),
        Some(NativeConfig::new("a", "b").host_url.as_str())
    );
    assert_eq!(c.map(|c| c.overview.is_none()), Some(true));

    // overview：summary 为 NULL 时忽略 body / locale。
    let summary = CString::new("示例 App").unwrap_or_default();
    let body = CString::new("## 能力\n- 问好").unwrap_or_default();
    let locale = CString::new("zh-CN").unwrap_or_default();
    cfg.overview_body = body.as_ptr();
    cfg.overview_locale = locale.as_ptr();
    let c = unsafe { convert_config(&cfg) };
    assert_eq!(c.as_ref().ok().map(|c| c.overview.is_none()), Some(true));
    cfg.overview_summary = summary.as_ptr();
    let c = unsafe { convert_config(&cfg) };
    let ov = c.ok().and_then(|c| c.overview);
    assert_eq!(ov.as_ref().map(|o| o.summary.as_str()), Some("示例 App"));
    assert_eq!(
        ov.as_ref().and_then(|o| o.body.as_deref()),
        Some("## 能力\n- 问好")
    );
    assert_eq!(ov.as_ref().and_then(|o| o.locale.as_deref()), Some("zh-CN"));
    cfg.overview_body = ptr::null();
    let ov = unsafe { convert_config(&cfg) }
        .ok()
        .and_then(|c| c.overview);
    assert_eq!(ov.map(|o| o.body.is_none()), Some(true));

    cfg.app_name = ptr::null();
    let err = unsafe { convert_config(&cfg) }.err();
    assert_eq!(
        err.as_ref().map(|e| e.status),
        Some(AmStatus::InvalidArgument)
    );
    assert!(
        err.map(|e| e.message)
            .unwrap_or_default()
            .contains("app_name")
    );
}

#[test]
fn null_pointers_are_invalid_argument() {
    let mut client: *mut AmClient = ptr::null_mut();
    assert_eq!(
        unsafe { am_client_new(ptr::null(), ptr::null(), &mut client) },
        AmStatus::InvalidArgument
    );
    assert!(client.is_null());
    assert_eq!(
        unsafe { am_client_start(ptr::null_mut()) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_client_stop(ptr::null_mut()) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_client_set_visibility(ptr::null_mut(), 0, true) },
        AmStatus::InvalidArgument
    );
    let mut st = AmStateStatus::Idle;
    assert_eq!(
        unsafe { am_client_state(ptr::null(), &mut st, ptr::null_mut(), ptr::null_mut()) },
        AmStatus::InvalidArgument
    );
    assert!(unsafe { am_client_instance_id(ptr::null()) }.is_null());
    assert!(unsafe { am_client_token(ptr::null()) }.is_null());
    let mut text: *mut c_char = ptr::null_mut();
    assert_eq!(unsafe { am_client_state_code(ptr::null(), &mut text) }, AmStatus::InvalidArgument);
    assert_eq!(unsafe { am_client_connection_id(ptr::null(), &mut text) }, AmStatus::InvalidArgument);
    assert!(text.is_null());
    let mut scope: *mut AmScope = ptr::null_mut();
    assert_eq!(
        unsafe { am_client_root_scope(ptr::null_mut(), &mut scope) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_scope_create(ptr::null_mut(), ptr::null(), &mut scope) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_scope_dispose(ptr::null_mut()) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_tool_update(ptr::null_mut(), ptr::null()) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_tool_set_enabled(ptr::null_mut(), true) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_tool_dispose(ptr::null_mut()) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_resource_notify_changed(ptr::null_mut()) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_resource_dispose(ptr::null_mut()) },
        AmStatus::InvalidArgument
    );
    assert!(unsafe { am_call_id(ptr::null()) }.is_null());
    assert!(unsafe { am_call_tool_name(ptr::null()) }.is_null());
    assert!(unsafe { am_call_arguments_json(ptr::null()) }.is_null());
    assert!(!unsafe { am_call_is_cancelled(ptr::null()) });
    assert_eq!(
        unsafe { am_call_complete(ptr::null_mut(), ptr::null(), ptr::null(), 0) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_call_fail(ptr::null_mut(), ptr::null(), ptr::null()) },
        AmStatus::InvalidArgument
    );
    assert!(unsafe { am_read_resource_name(ptr::null()) }.is_null());
    assert_eq!(
        unsafe { am_read_complete(ptr::null_mut(), ptr::null()) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_read_fail(ptr::null_mut(), ptr::null(), ptr::null()) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_read_fail_with_details(ptr::null_mut(), ptr::null(), ptr::null(), ptr::null()) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_read_fail_user_action(ptr::null_mut(), ptr::null(), ptr::null(), ptr::null()) },
        AmStatus::InvalidArgument
    );
    assert!(last_error().contains("read"));
    // 释放函数接受 NULL。
    unsafe {
        am_client_free(ptr::null_mut());
        am_scope_free(ptr::null_mut());
        am_tool_free(ptr::null_mut());
        am_resource_free(ptr::null_mut());
    }
}

#[test]
fn user_data_freed_on_failure() {
    let before = freed();
    let cb = AmClientCallbacks {
        on_state: None,
        on_paired: None,
        on_log: None,
        user_data: ptr::null_mut(),
        free_user_data: Some(count_free),
    };
    let mut client: *mut AmClient = ptr::null_mut();
    assert_eq!(
        unsafe { am_client_new(ptr::null(), &cb, &mut client) },
        AmStatus::InvalidArgument
    );
    assert_eq!(freed(), before + 1);

    let mut tool: *mut AmTool = ptr::null_mut();
    let s = unsafe {
        am_tool_register(
            ptr::null_mut(),
            ptr::null(),
            Some(noop_tool),
            ptr::null_mut(),
            Some(count_free),
            &mut tool,
        )
    };
    assert_eq!(s, AmStatus::InvalidArgument);
    assert_eq!(freed(), before + 2);

    let s = unsafe {
        am_call_set_cancel_callback(ptr::null_mut(), None, ptr::null_mut(), Some(count_free))
    };
    assert_eq!(s, AmStatus::InvalidArgument);
    assert_eq!(freed(), before + 3);
}

#[test]
fn hints_parsing() {
    let a = CString::new("cart").unwrap_or_default();
    let arr = [a.as_ptr()];
    assert_eq!(
        unsafe { read_hints(arr.as_ptr(), 1) }.ok(),
        Some(vec!["cart".to_owned()])
    );
    assert_eq!(unsafe { read_hints(ptr::null(), 0) }.ok(), Some(vec![]));
    assert!(unsafe { read_hints(ptr::null(), 2) }.is_err());
    let arr = [ptr::null()];
    assert!(unsafe { read_hints(arr.as_ptr(), 1) }.is_err());
}

/// 头文件与实现的一致性：声明的每个函数都已实现，枚举值一致。
#[test]
fn header_consistency() {
    let header = include_str!("../include/app_mcp.h");
    let implemented = [
        "am_version",
        "am_last_error_message",
        "am_string_free",
        "am_client_new",
        "am_client_free",
        "am_client_start",
        "am_client_stop",
        "am_client_set_visibility",
        "am_client_state",
        "am_client_instance_id",
        "am_client_token",
        "am_client_state_code",
        "am_client_connection_id",
        "am_client_root_scope",
        "am_scope_create",
        "am_scope_dispose",
        "am_scope_free",
        "am_tool_register",
        "am_tool_update",
        "am_tool_set_enabled",
        "am_tool_dispose",
        "am_tool_free",
        "am_resource_register",
        "am_resource_notify_changed",
        "am_resource_dispose",
        "am_resource_free",
        "am_call_id",
        "am_call_tool_name",
        "am_call_arguments_json",
        "am_call_is_cancelled",
        "am_call_set_cancel_callback",
        "am_call_complete",
        "am_call_fail",
        "am_read_resource_name",
        "am_read_complete",
        "am_read_fail",
        // v3
        "am_lifecycle_init",
        "am_client_new_ex",
        "am_client_handle_wake",
        "am_client_wake",
        "am_client_wake_with_reason",
        "am_client_connect_now",
        "am_client_sleep",
        "am_client_sleep_with_reason",
        "am_client_hold",
        "am_hold_release",
        "am_client_tools_hash",
        "am_parse_wake_token",
        "am_call_fail_with_details",
        "am_call_hold",
        // v8
        "am_resource_register_ex",
        // v9
        "am_tool_register_ex",
        "am_tool_update_ex",
        "am_call_complete_ex",
        // v10
        "am_call_progress",
        // v11
        "am_call_fail_user_action",
        // v12
        "am_read_fail_with_details",
        "am_read_fail_user_action",
    ];
    // 收集头文件中形如 `am_xxx(` 的声明。
    let mut declared = Vec::new();
    for line in header.lines() {
        let line = line.trim();
        if line.starts_with("/*") || line.starts_with('*') || line.starts_with("typedef") {
            continue;
        }
        if let Some(pos) = line.find("am_") {
            let rest = &line[pos..];
            if let Some(end) = rest.find('(') {
                let name = &rest[..end];
                if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    declared.push(name.to_owned());
                }
            }
        }
    }
    assert!(
        declared.len() >= implemented.len(),
        "解析到的声明过少：{declared:?}"
    );
    for d in &declared {
        assert!(implemented.contains(&d.as_str()), "头文件声明的 {d} 未实现");
    }
    for i in implemented {
        assert!(declared.iter().any(|d| d == i), "{i} 不在头文件中");
    }

    let expect = [
        ("AM_ERR_PANIC = 11", AmStatus::Panic as i32 == 11),
        ("AM_ERR_INVALID_JSON = 5", AmStatus::InvalidJson as i32 == 5),
        ("AM_STATE_STOPPED = 7", AmStateStatus::Stopped as i32 == 7),
        ("AM_CANCEL_STOPPED = 3", AmCancelReason::Stopped as i32 == 3),
        ("AM_LOG_ERROR = 3", AmLogLevel::Error as i32 == 3),
        (
            "AM_ACTIVATION_NONE = -1",
            activation_from(-1).ok() == Some(None),
        ),
        ("AM_STATE_DORMANT = 8", AmStateStatus::Dormant as i32 == 8),
        ("AM_STATE_WAKING = 9", AmStateStatus::Waking as i32 == 9),
        ("AM_STATE_HOST_MISMATCH = 10", AmStateStatus::HostMismatch as i32 == 10),
        (
            "AM_LIFECYCLE_ON_DEMAND = 2",
            lifecycle_mode_from(2).ok() == Some(LifecycleMode::OnDemand),
        ),
        (
            "AM_RESIDENCY_EXIT_ALWAYS = 2",
            residency_from(2).ok() == Some(Residency::ExitAlways),
        ),
        ("AM_WAKE_UNSET = -1", wake_kind_from(-1).ok() == Some(None)),
        (
            "AM_WAKE_WEB_URL = 6",
            wake_kind_from(6).ok() == Some(Some(WakeKind::WebUrl)),
        ),
        (
            "AM_WAKE_REASON_COLD_START = 3",
            wake_reason_from(3).ok() == Some(WakeReason::ColdStart),
        ),
        ("AM_RESULT_DONE = 0", result_status_from(0).ok() == Some(ResultStatus::Done)),
        ("AM_RESULT_PENDING = 1", result_status_from(1).ok() == Some(ResultStatus::Pending)),
        ("AM_RESULT_PARTIAL = 2", result_status_from(2).ok() == Some(ResultStatus::Partial)),
        ("AM_RESULT_NOOP = 3", result_status_from(3).ok() == Some(ResultStatus::Noop)),
        (
            "AM_SLEEP_REASON_APP = 3",
            sleep_reason_from(3).ok() == Some(SleepReason::App),
        ),
    ];
    for (text, ok) in expect {
        assert!(header.contains(text), "头文件缺少 {text}");
        assert!(ok, "{text} 与实现不一致");
    }
    assert!(header.contains(&format!("#define AM_API_VERSION {AM_API_VERSION}")));
}

/// 通过 C 接口驱动真实的原生运行时（不连接 Host）。
#[test]
fn runtime_through_c_abi() {
    let id = CString::new("c-abi-test").unwrap_or_default();
    let name = CString::new("C ABI Test").unwrap_or_default();
    let url = CString::new("ws://127.0.0.1:1").unwrap_or_default();
    let summary = CString::new("测试").unwrap_or_default();
    let cfg = AmClientConfig {
        app_id: id.as_ptr(),
        app_name: name.as_ptr(),
        instance_id: ptr::null(),
        host_url: url.as_ptr(),
        app_version: ptr::null(),
        instance_title: ptr::null(),
        token: ptr::null(),
        launch_token: ptr::null(),
        client_kind: 0,
        max_concurrent_calls: 0,
        overview_summary: summary.as_ptr(),
        overview_body: ptr::null(),
        overview_locale: ptr::null(),
    };
    let before = freed();
    let cb = AmClientCallbacks {
        on_state: None,
        on_paired: None,
        on_log: None,
        user_data: ptr::null_mut(),
        free_user_data: Some(count_free),
    };
    let mut client: *mut AmClient = ptr::null_mut();
    assert_eq!(
        unsafe { am_client_new(&cfg, &cb, &mut client) },
        AmStatus::Ok
    );
    assert!(!client.is_null());
    // 没有任何回调函数时，user_data 立即释放。
    assert_eq!(freed(), before + 1);

    let mut st = AmStateStatus::Stopped;
    let mut reason: *mut c_char = ptr::null_mut();
    assert_eq!(
        unsafe { am_client_state(client, &mut st, ptr::null_mut(), &mut reason) },
        AmStatus::Ok
    );
    assert_eq!(st, AmStateStatus::Idle);
    assert!(reason.is_null());
    let iid = unsafe { am_client_instance_id(client) };
    assert!(!iid.is_null());
    unsafe { am_string_free(iid) };
    assert!(unsafe { am_client_token(client) }.is_null());
    let mut text: *mut c_char = ptr::null_mut();
    assert_eq!(unsafe { am_client_state_code(client, &mut text) }, AmStatus::Ok);
    assert!(text.is_null(), "Idle 没有错误码");
    assert_eq!(unsafe { am_client_connection_id(client, &mut text) }, AmStatus::Ok);
    assert!(text.is_null(), "未连接没有连接 ID");
    assert_eq!(unsafe { am_client_state_code(client, ptr::null_mut()) }, AmStatus::InvalidArgument);
    assert_eq!(
        unsafe { am_client_set_visibility(client, 9, true) },
        AmStatus::InvalidArgument
    );

    let mut root: *mut AmScope = ptr::null_mut();
    assert_eq!(
        unsafe { am_client_root_scope(client, &mut root) },
        AmStatus::Ok
    );

    let tname = CString::new("echo").unwrap_or_default();
    let desc = CString::new("回显").unwrap_or_default();
    let spec = AmToolSpec {
        name: tname.as_ptr(),
        description: desc.as_ptr(),
        input_schema_json: ptr::null(),
        risk: 0,
        activation: -1,
        title: ptr::null(),
        enabled: true,
    };
    let mut tool: *mut AmTool = ptr::null_mut();
    let s = unsafe {
        am_tool_register(
            root,
            &spec,
            Some(noop_tool),
            ptr::null_mut(),
            None,
            &mut tool,
        )
    };
    assert_eq!(s, AmStatus::Ok);
    let mut dup: *mut AmTool = ptr::null_mut();
    let s = unsafe {
        am_tool_register(
            root,
            &spec,
            Some(noop_tool),
            ptr::null_mut(),
            Some(count_free),
            &mut dup,
        )
    };
    assert_eq!(s, AmStatus::DuplicateName);
    assert!(dup.is_null());
    assert_eq!(freed(), before + 2);

    // 根作用域 dispose 后可重新注册同名工具。
    assert_eq!(unsafe { am_scope_dispose(root) }, AmStatus::Ok);
    let mut again: *mut AmTool = ptr::null_mut();
    assert_eq!(
        unsafe {
            am_tool_register(
                root,
                &spec,
                Some(noop_tool),
                ptr::null_mut(),
                None,
                &mut again,
            )
        },
        AmStatus::Ok
    );

    // 释放客户端后，其他句柄的操作返回 AM_ERR_STOPPED。
    unsafe { am_client_free(client) };
    assert_eq!(
        unsafe { am_tool_set_enabled(again, false) },
        AmStatus::Stopped
    );
    assert_eq!(unsafe { am_tool_dispose(tool) }, AmStatus::Stopped);
    assert_eq!(unsafe { am_scope_dispose(root) }, AmStatus::Stopped);
    unsafe {
        am_tool_free(tool);
        am_tool_free(again);
        am_scope_free(root);
    }
}

thread_local! {
    static RECEIVED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// 模拟异步回调方：先保存指针，稍后再读取并释放（字符串归回调方所有）。
unsafe extern "C" fn take_log(_ud: *mut c_void, _level: AmLogLevel, message: *mut c_char) {
    let s = unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned();
    unsafe { am_string_free(message) };
    RECEIVED.with(|r| r.borrow_mut().push(s));
}

unsafe extern "C" fn take_paired(_ud: *mut c_void, token: *mut c_char) {
    let s = unsafe { CStr::from_ptr(token) }
        .to_string_lossy()
        .into_owned();
    unsafe { am_string_free(token) };
    RECEIVED.with(|r| r.borrow_mut().push(s));
}

unsafe extern "C" fn take_state(
    _ud: *mut c_void,
    status: AmStateStatus,
    retry: u64,
    reason: *mut c_char,
) {
    let r = if reason.is_null() {
        "<null>".to_owned()
    } else {
        let s = unsafe { CStr::from_ptr(reason) }
            .to_string_lossy()
            .into_owned();
        unsafe { am_string_free(reason) };
        s
    };
    RECEIVED.with(|v| v.borrow_mut().push(format!("{status:?}/{retry}/{r}")));
}

#[test]
fn listener_strings_are_owned_by_callee() {
    use app_mcp_native::{ClientListener, LogLevel, StateInfo, StateStatus};
    let l = callbacks::CClientListener {
        on_state: Some(take_state),
        on_paired: Some(take_paired),
        on_log: Some(take_log),
        on_idle_exit: None,
        user_data: UserData::new(ptr::null_mut(), None),
    };
    l.on_log(LogLevel::Info, "日志".to_owned());
    l.on_paired("tok".to_owned());
    l.on_state_changed(StateInfo {
        status: StateStatus::Rejected,
        retry_in_ms: None,
        reason: Some("拒绝".to_owned()),
        code: Some("PAIRING_REJECTED".to_owned()),
    });
    l.on_state_changed(StateInfo {
        status: StateStatus::Backoff,
        retry_in_ms: Some(500),
        reason: None,
        code: None,
    });
    let got = RECEIVED.with(|r| r.borrow().clone());
    assert_eq!(
        got,
        vec!["日志", "tok", "Rejected/0/拒绝", "Backoff/500/<null>"]
    );
}

// ---------------------------------------------------------------------------
// v3：生命周期
// ---------------------------------------------------------------------------

#[test]
fn lifecycle_enum_and_default() {
    assert!(lifecycle_mode_from(3).is_err());
    assert!(residency_from(-1).is_err());
    assert!(wake_kind_from(7).is_err());
    assert!(wake_reason_from(4).is_err());
    assert!(sleep_reason_from(-1).is_err());
    assert_eq!(wake_kind_from(2).ok(), Some(Some(WakeKind::Aumid)));

    let mut l = AmLifecycle {
        mode: 9,
        idle_timeout_ms: 1,
        hidden_idle_timeout_ms: 1,
        grace_ms: 1,
        residency: 9,
        wake_kind: 9,
        wake_target: ptr::null(),
        wake_background: true,
    };
    unsafe { am_lifecycle_init(&mut l) };
    unsafe { am_lifecycle_init(ptr::null_mut()) };
    let policy = unsafe { convert_lifecycle(&l) }.ok();
    assert_eq!(policy, Some(LifecyclePolicy::default()));

    let target = CString::new("myapp").unwrap_or_default();
    l.mode = 2;
    l.hidden_idle_timeout_ms = 0;
    l.residency = 1;
    l.wake_kind = 1;
    l.wake_target = target.as_ptr();
    l.wake_background = true;
    let p = unsafe { convert_lifecycle(&l) }.ok();
    assert_eq!(
        p,
        Some(LifecyclePolicy {
            mode: LifecycleMode::OnDemand,
            hidden_idle_timeout_ms: 0,
            residency: Residency::ExitWhenIdle,
            wake: Some(WakeDescriptor {
                kind: WakeKind::Uri,
                target: Some("myapp".to_owned()),
                background: true,
            }),
            ..LifecyclePolicy::default()
        })
    );
    l.residency = 7;
    assert!(unsafe { convert_lifecycle(&l) }.is_err());
}

#[test]
fn options_are_read_up_to_struct_size() {
    let lc = AmLifecycle::default();
    let mut o = AmClientOptions {
        struct_size: std::mem::size_of::<AmClientOptions>() as u32,
        lifecycle: &lc,
        connect_timeout_ms: 1234,
        on_idle_exit: Some(count_free),
        heartbeat: 2,
        host_absent_retries: -1,
        legacy_timers: true,
        merge_window_ms: 500,
        sleep_on_background: true,
        call_dedup_ttl_ms: 1000,
        call_dedup_max_entries: -1,
    };
    let v = unsafe { read_options(&o) }.ok();
    assert!(v.as_ref().is_some_and(|v| std::ptr::eq(v.lifecycle, &lc)
        && v.connect_timeout_ms == 1234
        && v.on_idle_exit.is_some()
        && v.heartbeat == 2
        && v.host_absent_retries == -1
        && v.legacy_timers
        && v.merge_window_ms == 500
        && v.sleep_on_background
        && v.call_dedup_ttl_ms == 1000
        && v.call_dedup_max_entries == -1));
    // v8–v12 调用方（到 sleep_on_background 为止）：v13 去重字段取默认值。
    o.struct_size = std::mem::offset_of!(AmClientOptions, call_dedup_ttl_ms) as u32;
    let v = unsafe { read_options(&o) }.ok();
    assert!(v.as_ref().is_some_and(|v| v.sleep_on_background && v.call_dedup_ttl_ms == 0 && v.call_dedup_max_entries == 0));
    // v7 调用方（到 legacy_timers 为止）：v8 字段取默认值。
    o.struct_size = std::mem::offset_of!(AmClientOptions, merge_window_ms) as u32;
    let v = unsafe { read_options(&o) }.ok();
    assert!(v.as_ref().is_some_and(|v| v.legacy_timers && v.merge_window_ms == 0 && !v.sleep_on_background));
    // v3 调用方（到 on_idle_exit 为止）：4e 字段取默认值。
    o.struct_size = std::mem::offset_of!(AmClientOptions, heartbeat) as u32;
    let v = unsafe { read_options(&o) }.ok();
    assert!(v.as_ref().is_some_and(|v| v.on_idle_exit.is_some()
        && v.heartbeat == 0
        && v.host_absent_retries == 0
        && !v.legacy_timers));
    // 旧调用方（只含前两个字段）：后面的字段不读取。
    o.struct_size = std::mem::offset_of!(AmClientOptions, connect_timeout_ms) as u32;
    let v = unsafe { read_options(&o) }.ok();
    assert!(v.as_ref().is_some_and(|v| !v.lifecycle.is_null()
        && v.connect_timeout_ms == 0
        && v.on_idle_exit.is_none()));
    o.struct_size = 0;
    assert!(unsafe { read_options(&o) }.is_err());
    let v = unsafe { read_options(ptr::null()) }.ok();
    assert!(v.is_some_and(|v| v.lifecycle.is_null()));
}

#[test]
fn call_dedup_values() {
    let d = CallDedupPolicy::default();
    assert_eq!(call_dedup_from(d, 0, 0), d, "0 保留默认值");
    assert_eq!(call_dedup_from(d, 10, 3), CallDedupPolicy { ttl_ms: 10, max_entries: 3 });
    assert!(!call_dedup_from(d, -1, 0).enabled(), "负数关闭");
    assert!(!call_dedup_from(d, 0, -1).enabled());
    assert_eq!(call_dedup_from(d, 0, 5), CallDedupPolicy { ttl_ms: d.ttl_ms, max_entries: 5 });
}

#[test]
fn heartbeat_mode_values() {
    assert_eq!(heartbeat_mode_from(0).ok(), Some(HeartbeatMode::Auto));
    assert_eq!(heartbeat_mode_from(1).ok(), Some(HeartbeatMode::Always));
    assert_eq!(heartbeat_mode_from(2).ok(), Some(HeartbeatMode::Off));
    assert!(heartbeat_mode_from(3).is_err());
}

#[test]
fn parse_wake_token_through_c_abi() {
    let arg = CString::new("myapp://app-mcp/wake?token=abc-123").unwrap_or_default();
    let t = unsafe { am_parse_wake_token(arg.as_ptr()) };
    assert!(!t.is_null());
    assert_eq!(unsafe { CStr::from_ptr(t) }.to_str().ok(), Some("abc-123"));
    unsafe { am_string_free(t) };
    let other = CString::new("--flag file.txt").unwrap_or_default();
    assert!(unsafe { am_parse_wake_token(other.as_ptr()) }.is_null());
    assert!(unsafe { am_parse_wake_token(ptr::null()) }.is_null());
    assert!(last_error().contains("args"));
}

unsafe extern "C" fn idle_exit_counter(ud: *mut c_void) {
    // SAFETY: 测试传入的 AtomicUsize。
    let n = unsafe { &*(ud as *const std::sync::atomic::AtomicUsize) };
    n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

#[test]
fn idle_exit_callback_reaches_listener() {
    use app_mcp_native::ClientListener;
    let n = std::sync::atomic::AtomicUsize::new(0);
    let l = callbacks::CClientListener {
        on_state: None,
        on_paired: None,
        on_log: None,
        on_idle_exit: Some(idle_exit_counter),
        user_data: UserData::new(
            (&n as *const std::sync::atomic::AtomicUsize)
                .cast_mut()
                .cast(),
            None,
        ),
    };
    assert!(l.has_any());
    l.on_idle_exit();
    assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// on-demand 模式：start 后进入休眠；hold / tools_hash / handle_wake / sleep 经 C 接口可用。
#[test]
fn lifecycle_through_c_abi() {
    let id = CString::new("c-abi-lifecycle").unwrap_or_default();
    let name = CString::new("C ABI Lifecycle").unwrap_or_default();
    let url = CString::new("ws://127.0.0.1:1").unwrap_or_default();
    let cfg = AmClientConfig {
        app_id: id.as_ptr(),
        app_name: name.as_ptr(),
        instance_id: ptr::null(),
        host_url: url.as_ptr(),
        app_version: ptr::null(),
        instance_title: ptr::null(),
        token: ptr::null(),
        launch_token: ptr::null(),
        client_kind: 0,
        max_concurrent_calls: 0,
        overview_summary: ptr::null(),
        overview_body: ptr::null(),
        overview_locale: ptr::null(),
    };
    let mut lc = AmLifecycle::default();
    unsafe { am_lifecycle_init(&mut lc) };
    lc.mode = 2;
    let opts = AmClientOptions {
        struct_size: std::mem::size_of::<AmClientOptions>() as u32,
        lifecycle: &lc,
        connect_timeout_ms: 200,
        on_idle_exit: None,
        heartbeat: 0,
        host_absent_retries: 0,
        legacy_timers: false,
        merge_window_ms: 0,
        sleep_on_background: false,
        call_dedup_ttl_ms: 0,
        call_dedup_max_entries: 0,
    };
    let mut client: *mut AmClient = ptr::null_mut();
    assert_eq!(
        unsafe { am_client_new_ex(&cfg, ptr::null(), &opts, &mut client) },
        AmStatus::Ok
    );

    // 非法生命周期参数：配置错误，客户端不创建。
    let mut bad_lc = lc;
    bad_lc.wake_kind = 42;
    let bad = AmClientOptions {
        lifecycle: &bad_lc,
        ..opts
    };
    let mut none: *mut AmClient = ptr::null_mut();
    assert_eq!(
        unsafe { am_client_new_ex(&cfg, ptr::null(), &bad, &mut none) },
        AmStatus::InvalidArgument
    );
    assert!(none.is_null());

    assert_eq!(unsafe { am_client_start(client) }, AmStatus::Ok);
    let mut st = AmStateStatus::Idle;
    assert_eq!(
        unsafe { am_client_state(client, &mut st, ptr::null_mut(), ptr::null_mut()) },
        AmStatus::Ok
    );
    assert_eq!(st, AmStateStatus::Dormant);

    let h1 = unsafe { am_client_tools_hash(client) };
    assert!(!h1.is_null());
    let hash1 = unsafe { CStr::from_ptr(h1) }.to_string_lossy().into_owned();
    unsafe { am_string_free(h1) };
    assert_eq!(hash1.len(), 16);

    // 注册工具会改变摘要（休眠中也可以注册）。
    let mut root: *mut AmScope = ptr::null_mut();
    assert_eq!(
        unsafe { am_client_root_scope(client, &mut root) },
        AmStatus::Ok
    );
    let tname = CString::new("echo").unwrap_or_default();
    let desc = CString::new("回显").unwrap_or_default();
    let spec = AmToolSpec {
        name: tname.as_ptr(),
        description: desc.as_ptr(),
        input_schema_json: ptr::null(),
        risk: 0,
        activation: -1,
        title: ptr::null(),
        enabled: true,
    };
    let mut tool: *mut AmTool = ptr::null_mut();
    assert_eq!(
        unsafe {
            am_tool_register(
                root,
                &spec,
                Some(noop_tool),
                ptr::null_mut(),
                None,
                &mut tool,
            )
        },
        AmStatus::Ok
    );
    let h2 = unsafe { am_client_tools_hash(client) };
    let hash2 = unsafe { CStr::from_ptr(h2) }.to_string_lossy().into_owned();
    unsafe { am_string_free(h2) };
    assert_ne!(hash1, hash2);

    let mut hold: *mut AmHold = ptr::null_mut();
    assert_eq!(unsafe { am_client_hold(client, &mut hold) }, AmStatus::Ok);
    assert!(!hold.is_null());
    unsafe { am_hold_release(hold) };
    unsafe { am_hold_release(ptr::null_mut()) };

    let junk = CString::new("--not-a-wake").unwrap_or_default();
    assert!(!unsafe { am_client_handle_wake(client, junk.as_ptr()) });
    assert!(!unsafe { am_client_handle_wake(client, ptr::null()) });
    assert!(!unsafe { am_client_handle_wake(ptr::null_mut(), junk.as_ptr()) });

    let mut flag = true;
    assert_eq!(
        unsafe { am_client_wake_with_reason(client, 99, &mut flag) },
        AmStatus::InvalidArgument
    );
    assert!(!flag);
    assert_eq!(
        unsafe { am_client_sleep_with_reason(client, -3, ptr::null_mut()) },
        AmStatus::InvalidArgument
    );
    assert_eq!(
        unsafe { am_client_wake(ptr::null_mut(), &mut flag) },
        AmStatus::InvalidArgument
    );

    // 休眠中 sleep 无效果；connect_now 发起回连。
    assert_eq!(unsafe { am_client_sleep(client, &mut flag) }, AmStatus::Ok);
    assert!(!flag);
    assert_eq!(
        unsafe { am_client_connect_now(client, &mut flag) },
        AmStatus::Ok
    );
    assert!(flag);
    assert_eq!(unsafe { am_client_sleep(client, &mut flag) }, AmStatus::Ok);

    let wake = CString::new("app-mcp-wake:tok123").unwrap_or_default();
    assert!(unsafe { am_client_handle_wake(client, wake.as_ptr()) });

    // 客户端释放后，之前取得的 hold 仍可安全释放。
    let mut late: *mut AmHold = ptr::null_mut();
    assert_eq!(unsafe { am_client_hold(client, &mut late) }, AmStatus::Ok);
    unsafe { am_client_free(client) };
    unsafe {
        am_hold_release(late);
        am_tool_free(tool);
        am_scope_free(root);
    }
}

#[test]
fn call_fail_with_details_null_call() {
    assert_eq!(
        unsafe {
            am_call_fail_with_details(ptr::null_mut(), ptr::null(), ptr::null(), ptr::null())
        },
        AmStatus::InvalidArgument
    );
    let mut out: *mut AmHold = ptr::null_mut();
    assert_eq!(
        unsafe { am_call_hold(ptr::null(), &mut out) },
        AmStatus::InvalidArgument
    );
    assert!(out.is_null());
    assert_eq!(unsafe { am_call_progress(ptr::null(), 1.0, -1.0, ptr::null()) }, AmStatus::InvalidArgument);
    assert_eq!(
        unsafe { am_call_fail_user_action(ptr::null_mut(), ptr::null(), ptr::null(), ptr::null()) },
        AmStatus::InvalidArgument
    );
}

#[test]
fn resource_options_are_read_up_to_struct_size() {
    let ann = CString::new(r#"{"audience":["user"],"priority":0.5}"#).unwrap_or_default();
    let full = AmResourceOptions {
        struct_size: std::mem::size_of::<AmResourceOptions>() as u32,
        realtime: true,
        annotations_json: ann.as_ptr(),
    };
    let want = ContentAnnotations { audience: Some(vec![app_mcp_native::Audience::User]), priority: Some(0.5), last_modified: None };
    assert_eq!(
        unsafe { read_resource_options(&full) }.ok(),
        Some(ResourceOptions { realtime: true, annotations: Some(want) })
    );
    assert_eq!(unsafe { read_resource_options(ptr::null()) }.ok(), Some(ResourceOptions::default()));
    // v8–v12 调用方（到 realtime 为止）：annotations 不读取
    let v8 = AmResourceOptions {
        struct_size: std::mem::offset_of!(AmResourceOptions, annotations_json) as u32,
        ..full
    };
    assert_eq!(
        unsafe { read_resource_options(&v8) }.ok(),
        Some(ResourceOptions { realtime: true, annotations: None })
    );
    // 只含 struct_size 的调用方：realtime 取默认值
    let short = AmResourceOptions { struct_size: std::mem::size_of::<u32>() as u32, ..full };
    assert_eq!(unsafe { read_resource_options(&short) }.ok(), Some(ResourceOptions::default()));
    let bad = AmResourceOptions { struct_size: 0, ..full };
    assert!(unsafe { read_resource_options(&bad) }.is_err());
    let invalid = CString::new(r#"{"audience":["bot"]}"#).unwrap_or_default();
    let bad_json = AmResourceOptions { annotations_json: invalid.as_ptr(), ..full };
    assert!(unsafe { read_resource_options(&bad_json) }.is_err_and(|e| e.status == AmStatus::InvalidJson));
}

#[test]
fn tool_options_are_read_up_to_struct_size() {
    let ann = CString::new(r#"{"readOnlyHint":true,"title":"查询"}"#).unwrap_or_default();
    let schema = CString::new(r#"{"type":"object"}"#).unwrap_or_default();
    let full = AmToolOptions {
        struct_size: std::mem::size_of::<AmToolOptions>() as u32,
        annotations_json: ann.as_ptr(),
        output_schema_json: schema.as_ptr(),
    };
    let options = unsafe { read_tool_options(&full) }.ok();
    assert_eq!(
        options,
        Some(ToolOptions {
            annotations: Some(ToolAnnotations {
                title: Some("查询".into()),
                read_only_hint: Some(true),
                ..ToolAnnotations::default()
            }),
            output_schema_json: Some(r#"{"type":"object"}"#.into()),
        })
    );
    assert_eq!(unsafe { read_tool_options(ptr::null()) }.ok(), Some(ToolOptions::default()));
    // 只含到 annotations_json 的调用方：output_schema_json 按 NULL 处理
    let short = AmToolOptions {
        struct_size: (std::mem::offset_of!(AmToolOptions, annotations_json) + std::mem::size_of::<*const c_char>())
            as u32,
        ..full
    };
    let options = unsafe { read_tool_options(&short) }.ok();
    assert!(options.as_ref().is_some_and(|o| o.annotations.is_some() && o.output_schema_json.is_none()));
    let bad = AmToolOptions { struct_size: 0, ..full };
    assert_eq!(unsafe { read_tool_options(&bad) }.err().map(|e| e.status), Some(AmStatus::InvalidArgument));
    // 注解不是对象 / 字段类型不对：AM_ERR_INVALID_JSON；`null` 视为未声明
    for text in ["[1]", r#"{"readOnlyHint":"yes"}"#, "{"] {
        let t = CString::new(text).unwrap_or_default();
        let o = AmToolOptions { annotations_json: t.as_ptr(), ..full };
        assert_eq!(unsafe { read_tool_options(&o) }.err().map(|e| e.status), Some(AmStatus::InvalidJson), "{text}");
    }
    let null = CString::new("null").unwrap_or_default();
    let o = AmToolOptions { annotations_json: null.as_ptr(), ..full };
    assert!(unsafe { read_tool_options(&o) }.is_ok_and(|o| o.annotations.is_none()));
}

#[test]
fn call_result_is_read_up_to_struct_size() {
    let data = CString::new(r#"{"orderId":"o1"}"#).unwrap_or_default();
    let hint = CString::new("cart").unwrap_or_default();
    let hints = [hint.as_ptr()];
    let res = CString::new("order.state").unwrap_or_default();
    let summary = CString::new("已提交").unwrap_or_default();
    let ann = CString::new(r#"{"audience":["user"],"priority":0.5}"#).unwrap_or_default();
    let full = AmCallResult {
        struct_size: std::mem::size_of::<AmCallResult>() as u32,
        data_json: data.as_ptr(),
        state_hints: hints.as_ptr(),
        state_hints_len: 1,
        status: 1,
        state_resource: res.as_ptr(),
        summary: summary.as_ptr(),
        annotations_json: ann.as_ptr(),
    };
    assert_eq!(
        unsafe { read_call_result(&full) }.ok(),
        Some(CallResult {
            data_json: Some(r#"{"orderId":"o1"}"#.into()),
            state_hints: vec!["cart".into()],
            status: ResultStatus::Pending,
            state_resource: Some("order.state".into()),
            summary: Some("已提交".into()),
            annotations: Some(ContentAnnotations {
                audience: Some(vec![app_mcp_native::Audience::User]),
                priority: Some(0.5),
                last_modified: None,
            }),
        })
    );
    assert_eq!(unsafe { read_call_result(ptr::null()) }.ok(), Some(CallResult::default()));
    // 只含到 state_hints_len 的调用方：其余字段取默认值
    let short = AmCallResult {
        struct_size: (std::mem::offset_of!(AmCallResult, state_hints_len) + std::mem::size_of::<usize>()) as u32,
        ..full
    };
    let r = unsafe { read_call_result(&short) }.ok();
    assert!(r.as_ref().is_some_and(|r| r.status == ResultStatus::Done
        && r.summary.is_none()
        && r.annotations.is_none()
        && r.state_hints.len() == 1));
    let bad_status = AmCallResult { status: 9, ..full };
    assert_eq!(unsafe { read_call_result(&bad_status) }.err().map(|e| e.status), Some(AmStatus::InvalidArgument));
    let bad_ann = CString::new(r#"{"audience":["robot"]}"#).unwrap_or_default();
    let r = AmCallResult { annotations_json: bad_ann.as_ptr(), ..full };
    assert_eq!(unsafe { read_call_result(&r) }.err().map(|e| e.status), Some(AmStatus::InvalidJson));
    let r = AmCallResult { state_hints: ptr::null(), ..full };
    assert_eq!(unsafe { read_call_result(&r) }.err().map(|e| e.status), Some(AmStatus::InvalidArgument));
    assert_eq!(unsafe { am_call_complete_ex(ptr::null_mut(), &full) }, AmStatus::InvalidArgument);
}


/// `order.submit`：以 pending + stateResource + summary + 内容注解完成；`plain`：普通返回值（回归）。
unsafe extern "C" fn submit_tool(_ud: *mut c_void, call: *mut AmCall) {
    let name = unsafe { CStr::from_ptr(am_call_tool_name(call)) }.to_string_lossy().into_owned();
    if name == "login" {
        // USER_ACTION_REQUIRED：带 reason / uri
        let msg = CString::new("登录已过期，请在 App 内重新登录").unwrap_or_default();
        let reason = CString::new("login").unwrap_or_default();
        let uri = CString::new("shop://login").unwrap_or_default();
        let _ = unsafe { am_call_fail_user_action(call, msg.as_ptr(), reason.as_ptr(), uri.as_ptr()) };
        return;
    }
    if name == "front" {
        // reason / uri 为 NULL：data 中不出现
        let msg = CString::new("请把 App 切到前台").unwrap_or_default();
        let _ = unsafe { am_call_fail_user_action(call, msg.as_ptr(), ptr::null(), ptr::null()) };
        return;
    }
    if name == "plain" {
        // 进度：total 为负数表示未知；非法 UTF-8 的说明被拒绝（调用不受影响）
        let msg = CString::new("处理中").unwrap_or_default();
        let bad = [0xffu8, 0];
        if unsafe { am_call_progress(call, 1.0, -1.0, bad.as_ptr().cast()) } != AmStatus::InvalidArgument {
            let _ = unsafe { am_call_fail(call, ptr::null(), ptr::null()) };
            return;
        }
        let _ = unsafe { am_call_progress(call, 1.0, f64::NAN, msg.as_ptr()) };
        let _ = unsafe { am_call_progress(call, 2.0, 4.0, ptr::null()) };
        let data = CString::new(r#"{"ok":true}"#).unwrap_or_default();
        let _ = unsafe { am_call_complete(call, data.as_ptr(), ptr::null(), 0) };
        return;
    }
    let data = CString::new(r#"{"orderId":"o1"}"#).unwrap_or_default();
    let res = CString::new("order.state").unwrap_or_default();
    let summary = CString::new("已提交，等待用户在 App 内付款").unwrap_or_default();
    let ann = CString::new(r#"{"priority":0.5}"#).unwrap_or_default();
    let bad = CString::new("{").unwrap_or_default();
    let mut result = AmCallResult {
        struct_size: std::mem::size_of::<AmCallResult>() as u32,
        data_json: data.as_ptr(),
        state_hints: ptr::null(),
        state_hints_len: 0,
        status: 1,
        state_resource: res.as_ptr(),
        summary: summary.as_ptr(),
        annotations_json: bad.as_ptr(),
    };
    // 非法注解：不消费 call，可以重试
    if unsafe { am_call_complete_ex(call, &result) } != AmStatus::InvalidJson {
        let _ = unsafe { am_call_fail(call, ptr::null(), ptr::null()) };
        return;
    }
    result.annotations_json = ann.as_ptr();
    let _ = unsafe { am_call_complete_ex(call, &result) };
}

/// `session`：读取以 USER_ACTION_REQUIRED（reason / uri）失败；`quota`：带详情失败（非法详情不消费 read，可重试）。
unsafe extern "C" fn failing_reader(_ud: *mut c_void, read: *mut AmRead) {
    let name = unsafe { CStr::from_ptr(am_read_resource_name(read)) }.to_string_lossy().into_owned();
    if name == "session" {
        let msg = CString::new("登录已过期，请在 App 内重新登录").unwrap_or_default();
        let reason = CString::new("login").unwrap_or_default();
        let uri = CString::new("shop://login").unwrap_or_default();
        let _ = unsafe { am_read_fail_user_action(read, msg.as_ptr(), reason.as_ptr(), uri.as_ptr()) };
        return;
    }
    let kind = CString::new("USER_REJECTED").unwrap_or_default();
    let msg = CString::new("额度不足").unwrap_or_default();
    let bad = CString::new("{bad").unwrap_or_default();
    if unsafe { am_read_fail_with_details(read, kind.as_ptr(), msg.as_ptr(), bad.as_ptr()) } != AmStatus::InvalidJson {
        let _ = unsafe { am_read_fail(read, ptr::null(), ptr::null()) };
        return;
    }
    let details = CString::new(r#"{"quota":0}"#).unwrap_or_default();
    let _ = unsafe { am_read_fail_with_details(read, kind.as_ptr(), msg.as_ptr(), details.as_ptr()) };
}

/// 端到端：经 C 接口注册带注解 + outputSchema 的工具、以结构化结果完成调用，核对 fake_host 收到的内容。
#[test]
fn tool_options_and_call_result_reach_host() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    // 新构建的 fake_host（`cargo test` 不刷新 examples/fake_host，见 crates/native/src/test_support.rs）。
    let bin = app_mcp_native::test_support::fake_host_path().unwrap_or_else(|e| panic!("{e}"));
    let Ok(mut child) = Command::new(bin)
        .args([
            "--tool-info", "--invoke", "order.submit", "--invoke", "plain", "--invoke", "login", "--invoke", "front",
            "--read", "session", "--read", "quota", "--timeout-ms", "8000",
        ])
        .stdout(Stdio::piped())
        .spawn()
    else {
        panic!("无法启动 fake_host");
    };
    let Some(stdout) = child.stdout.take() else { panic!("fake_host 没有 stdout") };
    let mut lines = BufReader::new(stdout).lines();
    let first = lines.next().and_then(Result::ok).unwrap_or_default();
    let addr = first.strip_prefix("LISTENING ").unwrap_or_default().to_owned();
    assert!(!addr.is_empty(), "LISTENING 行：{first}");

    let id = CString::new("c-abi-result").unwrap_or_default();
    let name = CString::new("C ABI Result").unwrap_or_default();
    let url = CString::new(format!("ws://{addr}")).unwrap_or_default();
    let cfg = AmClientConfig {
        app_id: id.as_ptr(),
        app_name: name.as_ptr(),
        instance_id: ptr::null(),
        host_url: url.as_ptr(),
        app_version: ptr::null(),
        instance_title: ptr::null(),
        token: ptr::null(),
        launch_token: ptr::null(),
        client_kind: 0,
        max_concurrent_calls: 0,
        overview_summary: ptr::null(),
        overview_body: ptr::null(),
        overview_locale: ptr::null(),
    };
    let mut client: *mut AmClient = ptr::null_mut();
    assert_eq!(unsafe { am_client_new(&cfg, ptr::null(), &mut client) }, AmStatus::Ok);
    let mut root: *mut AmScope = ptr::null_mut();
    assert_eq!(unsafe { am_client_root_scope(client, &mut root) }, AmStatus::Ok);

    let tname = CString::new("order.submit").unwrap_or_default();
    let desc = CString::new("下单").unwrap_or_default();
    let spec = AmToolSpec {
        name: tname.as_ptr(),
        description: desc.as_ptr(),
        input_schema_json: ptr::null(),
        risk: 1,
        activation: -1,
        title: ptr::null(),
        enabled: true,
    };
    let ann = CString::new(r#"{"idempotentHint":false,"openWorldHint":true}"#).unwrap_or_default();
    let schema = CString::new(r#"{"type":"object","properties":{"orderId":{"type":"string"}}}"#).unwrap_or_default();
    let options = AmToolOptions {
        struct_size: std::mem::size_of::<AmToolOptions>() as u32,
        annotations_json: ann.as_ptr(),
        output_schema_json: schema.as_ptr(),
    };
    let mut tool: *mut AmTool = ptr::null_mut();
    assert_eq!(
        unsafe { am_tool_register_ex(root, &spec, &options, Some(submit_tool), ptr::null_mut(), None, &mut tool) },
        AmStatus::Ok
    );
    // 非法 outputSchema：更新失败，工具保持原定义
    let bad = CString::new("{").unwrap_or_default();
    let bad_options = AmToolOptions { output_schema_json: bad.as_ptr(), ..options };
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &bad_options) }, AmStatus::InvalidSchema);
    // am_tool_update 保留已声明的选项
    assert_eq!(unsafe { am_tool_update(tool, &spec) }, AmStatus::Ok);

    let pname = CString::new("plain").unwrap_or_default();
    let plain_spec = AmToolSpec { name: pname.as_ptr(), risk: 0, ..spec };
    let mut plain: *mut AmTool = ptr::null_mut();
    assert_eq!(
        unsafe { am_tool_register(root, &plain_spec, Some(submit_tool), ptr::null_mut(), None, &mut plain) },
        AmStatus::Ok
    );
    let mut ua_tools = Vec::new();
    for n in ["login", "front"] {
        let n = CString::new(n).unwrap_or_default();
        let ua_spec = AmToolSpec { name: n.as_ptr(), risk: 0, ..spec };
        let mut t: *mut AmTool = ptr::null_mut();
        assert_eq!(
            unsafe { am_tool_register(root, &ua_spec, Some(submit_tool), ptr::null_mut(), None, &mut t) },
            AmStatus::Ok
        );
        ua_tools.push(t);
    }
    let mut resources = Vec::new();
    for n in ["session", "quota"] {
        let n = CString::new(n).unwrap_or_default();
        let rspec = AmResourceSpec { name: n.as_ptr(), description: desc.as_ptr(), mime_type: ptr::null() };
        let mut r: *mut AmResource = ptr::null_mut();
        assert_eq!(
            unsafe { am_resource_register(root, &rspec, Some(failing_reader), ptr::null_mut(), None, &mut r) },
            AmStatus::Ok
        );
        resources.push(r);
    }
    assert_eq!(unsafe { am_client_start(client) }, AmStatus::Ok);

    let out: Vec<serde_json::Value> =
        lines.map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
    let ok = child.wait().is_ok_and(|s| s.success());
    unsafe {
        am_tool_free(tool);
        am_tool_free(plain);
        for t in ua_tools {
            am_tool_free(t);
        }
        for r in resources {
            am_resource_free(r);
        }
        am_scope_free(root);
        am_client_free(client);
    }
    assert!(ok, "fake_host 退出码非 0：{out:?}");
    assert_eq!(
        out[0]["toolInfo"]["order.submit"],
        serde_json::json!({
            "risk": "write",
            "annotations": { "idempotentHint": false, "openWorldHint": true },
            "outputSchema": { "type": "object", "properties": { "orderId": { "type": "string" } } }
        })
    );
    assert_eq!(out[0]["toolInfo"]["plain"], serde_json::json!({ "risk": "read" }));
    assert_eq!(
        out[1]["result"],
        serde_json::json!({
            "data": { "orderId": "o1" }, "status": "pending", "stateResource": "order.state",
            "summary": "已提交，等待用户在 App 内付款", "annotations": { "priority": 0.5 }
        })
    );
    assert_eq!(out[2], serde_json::json!({ "type": "progress", "callId": out[2]["callId"], "progress": 1.0, "message": "处理中" }));
    assert_eq!(out[3], serde_json::json!({ "type": "progress", "callId": out[2]["callId"], "progress": 2.0, "total": 4.0 }));
    assert_eq!(out[4]["result"], serde_json::json!({ "data": { "ok": true } }));
    assert_eq!(
        out[5]["error"],
        serde_json::json!({
            "code": -32019, "message": "登录已过期，请在 App 内重新登录",
            "data": { "kind": "USER_ACTION_REQUIRED", "reason": "login", "uri": "shop://login" }
        })
    );
    assert_eq!(
        out[6]["error"],
        serde_json::json!({ "code": -32019, "message": "请把 App 切到前台", "data": { "kind": "USER_ACTION_REQUIRED" } })
    );
    // v12：资源读取失败也带 reason / uri 与详情
    assert_eq!(
        out[7]["error"],
        serde_json::json!({
            "code": -32019, "message": "登录已过期，请在 App 内重新登录",
            "data": { "kind": "USER_ACTION_REQUIRED", "reason": "login", "uri": "shop://login" }
        })
    );
    assert_eq!(out[8]["error"]["data"], serde_json::json!({ "kind": "USER_REJECTED", "quota": 0 }));
}
