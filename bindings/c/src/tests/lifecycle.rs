//! 单元测试（续）：v3 生命周期、按 `struct_size` 读取扩展结构体，以及经 C ABI 与 Host 往返的集成用例。

use super::*;

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
        register_name: true,
        name_instance: c"w2".as_ptr(),
        max_queued_calls: 7,
    };
    let v = unsafe { read_options(&o) }.ok();
    assert!(v.as_ref().is_some_and(|v| v.register_name && !v.name_instance.is_null() && v.max_queued_calls == 7));
    // v17 调用方（到 name_instance 为止）：v18 排队上限取默认值。
    o.struct_size = std::mem::offset_of!(AmClientOptions, max_queued_calls) as u32;
    let v = unsafe { read_options(&o) }.ok();
    assert!(v.as_ref().is_some_and(|v| v.register_name && v.max_queued_calls == 0));
    // v13–v16 调用方（到 call_dedup_max_entries 为止）：v17 名字服务字段取默认值。
    o.struct_size = std::mem::offset_of!(AmClientOptions, register_name) as u32;
    let v = unsafe { read_options(&o) }.ok();
    assert!(v.as_ref().is_some_and(|v| v.call_dedup_max_entries == -1 && !v.register_name && v.name_instance.is_null()));
    o.struct_size = std::mem::size_of::<AmClientOptions>() as u32;
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
fn max_queued_values() {
    assert_eq!(max_queued_from(64, 0), 64, "0 保留默认值");
    assert_eq!(max_queued_from(64, -1), 0, "负数 = 不限");
    assert_eq!(max_queued_from(64, 3), 3);
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
        register_name: false,
        name_instance: ptr::null(),
        max_queued_calls: 0,
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
        cache_ttl_ms: 0,
        cache_scope: 0,
    };
    let want = ContentAnnotations { audience: Some(vec![app_mcp_native::Audience::User]), priority: Some(0.5), last_modified: None };
    assert_eq!(
        unsafe { read_resource_options(&full) }.ok(),
        Some(ResourceOptions { realtime: true, annotations: Some(want), cache: None })
    );
    assert_eq!(unsafe { read_resource_options(ptr::null()) }.ok(), Some(ResourceOptions::default()));
    // v8–v12 调用方（到 realtime 为止）：annotations 不读取
    let v8 = AmResourceOptions {
        struct_size: std::mem::offset_of!(AmResourceOptions, annotations_json) as u32,
        ..full
    };
    assert_eq!(
        unsafe { read_resource_options(&v8) }.ok(),
        Some(ResourceOptions { realtime: true, annotations: None, cache: None })
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
    let page = CString::new("cart").unwrap_or_default();
    let background = CString::new("cart.summary").unwrap_or_default();
    let group = CString::new("doc").unwrap_or_default();
    let verb = CString::new("message.send@1").unwrap_or_default();
    let verbs = [verb.as_ptr()];
    let full = AmToolOptions {
        struct_size: std::mem::size_of::<AmToolOptions>() as u32,
        annotations_json: ann.as_ptr(),
        output_schema_json: schema.as_ptr(),
        page: page.as_ptr(),
        surface: 1,
        background_tool: background.as_ptr(),
        concurrency: 2,
        exclusive: group.as_ptr(),
        implements: verbs.as_ptr(),
        implements_len: 1,
        cache_ttl_ms: 0,
        cache_scope: 0,
        deprecated_message: ptr::null(),
        deprecated_replacement: ptr::null(),
        deprecated_until: ptr::null(),
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
            surface: app_mcp_native::ToolSurface::View,
            page: Some("cart".into()),
            background_tool: Some("cart.summary".into()),
            concurrency: 2,
            exclusive: Some("doc".into()),
            implements: vec!["message.send@1".into()],
            cache: None,
            deprecated: None,
        })
    );
    // v20 调用方（不含 implements）：按未声明处理，其余字段照读
    let v20 = AmToolOptions { struct_size: std::mem::offset_of!(AmToolOptions, implements) as u32, ..full };
    let options = unsafe { read_tool_options(&v20) }.ok();
    assert!(options.as_ref().is_some_and(|o| o.implements.is_empty() && o.exclusive.as_deref() == Some("doc")));
    // implements_len 为 0 时 implements 可为 NULL；非 0 时 NULL / 元素为 NULL 均为 AM_ERR_INVALID_ARGUMENT
    let empty = AmToolOptions { implements: ptr::null(), implements_len: 0, ..full };
    assert!(unsafe { read_tool_options(&empty) }.is_ok_and(|o| o.implements.is_empty()));
    let null_items = AmToolOptions { implements: ptr::null(), implements_len: 1, ..full };
    assert_eq!(unsafe { read_tool_options(&null_items) }.err().map(|e| e.status), Some(AmStatus::InvalidArgument));
    let null_entry = [ptr::null::<c_char>()];
    let null_elem = AmToolOptions { implements: null_entry.as_ptr(), implements_len: 1, ..full };
    assert_eq!(unsafe { read_tool_options(&null_elem) }.err().map(|e| e.status), Some(AmStatus::InvalidArgument));
    // v17 调用方（不含 concurrency / exclusive）：不单独限制、不互斥
    let v17 = AmToolOptions { struct_size: std::mem::offset_of!(AmToolOptions, concurrency) as u32, ..full };
    let options = unsafe { read_tool_options(&v17) }.ok();
    assert!(options.as_ref().is_some_and(|o| o.concurrency == 0 && o.exclusive.is_none() && o.background_tool.is_some()));
    // v14 调用方（不含 background_tool）：按未声明处理
    let v14 = AmToolOptions { struct_size: std::mem::offset_of!(AmToolOptions, background_tool) as u32, ..full };
    let options = unsafe { read_tool_options(&v14) }.ok();
    assert!(options.as_ref().is_some_and(|o| o.background_tool.is_none() && o.surface == app_mcp_native::ToolSurface::View));
    // v13 调用方（不含 page / surface）：按未声明处理
    let v13 = AmToolOptions { struct_size: std::mem::offset_of!(AmToolOptions, page) as u32, ..full };
    let options = unsafe { read_tool_options(&v13) }.ok();
    assert!(options.as_ref().is_some_and(|o| o.page.is_none() && o.surface == app_mcp_native::ToolSurface::App && o.output_schema_json.is_some()));
    let bad_surface = AmToolOptions { surface: 7, ..full };
    assert_eq!(unsafe { read_tool_options(&bad_surface) }.err().map(|e| e.status), Some(AmStatus::InvalidArgument));
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
    if name == "key" {
        // v16：幂等键原样可读；没有时为 NULL
        let key = unsafe { am_call_idempotency_key(call) };
        let value = if key.is_null() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(unsafe { CStr::from_ptr(key) }.to_string_lossy().into_owned())
        };
        let data = CString::new(serde_json::json!({ "key": value }).to_string()).unwrap_or_default();
        let _ = unsafe { am_call_complete(call, data.as_ptr(), ptr::null(), 0) };
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
            "--read", "session", "--read", "quota", "--invoke", "key", "--idempotency-key", "Agent 键 / 1",
            "--invoke", "key", "--timeout-ms", "8000",
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
    let verb = CString::new("link.open@1").unwrap_or_default();
    let verbs = [verb.as_ptr()];
    let options = AmToolOptions {
        struct_size: std::mem::size_of::<AmToolOptions>() as u32,
        annotations_json: ann.as_ptr(),
        output_schema_json: schema.as_ptr(),
        page: ptr::null(),
        surface: 0,
        background_tool: ptr::null(),
        concurrency: 0,
        exclusive: ptr::null(),
        implements: verbs.as_ptr(),
        implements_len: verbs.len(),
        cache_ttl_ms: 0,
        cache_scope: 0,
        deprecated_message: ptr::null(),
        deprecated_replacement: ptr::null(),
        deprecated_until: ptr::null(),
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
    // v21：implements 格式不合法（缺主版本）：AM_ERR_INVALID_NAME，工具保持原定义
    let bad_verb = CString::new("link.open").unwrap_or_default();
    let bad_verbs = [bad_verb.as_ptr()];
    let bad_implements = AmToolOptions { implements: bad_verbs.as_ptr(), ..options };
    assert_eq!(unsafe { am_tool_update_ex(tool, &spec, &bad_implements) }, AmStatus::InvalidName);
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
    for n in ["login", "front", "key"] {
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
    // v16：am_call_idempotency_key
    assert_eq!(out[9]["result"], serde_json::json!({ "data": { "key": "Agent 键 / 1" } }));
    assert_eq!(out[10]["result"], serde_json::json!({ "data": { "key": null } }));
}

/// v15 导航回调：以 USER_ACTION_REQUIRED（foreground + uri）回复，随后关闭 navigateInBackground
/// （user_data 为 AmClient；之后的导航不再进入回调，由核心直接回复）。
unsafe extern "C" fn notify_navigate(ud: *mut c_void, navigate: *mut AmNavigate) {
    let msg = CString::new("已发通知，请点开后继续").unwrap_or_default();
    let reason = CString::new("foreground").unwrap_or_default();
    let uri = CString::new("conf://cart").unwrap_or_default();
    unsafe {
        am_navigate_fail_user_action(navigate, msg.as_ptr(), reason.as_ptr(), uri.as_ptr());
        am_client_set_navigate_in_background(ud.cast::<AmClient>(), false);
    }
}

/// 端到端（v15）：后台 + navigateInBackground 时回调以 USER_ACTION_REQUIRED 回复；关闭后核心直接回复 foreground。
#[test]
fn background_navigation_through_c_abi() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let bin = app_mcp_native::test_support::fake_host_path().unwrap_or_else(|e| panic!("{e}"));
    let Ok(mut child) = Command::new(bin)
        .args(["--navigate", "cart", "--navigate", "home", "--timeout-ms", "8000"])
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

    let id = CString::new("c-abi-bg-nav").unwrap_or_default();
    let name = CString::new("C ABI Background Navigation").unwrap_or_default();
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
    unsafe {
        assert_eq!(
            am_client_set_navigation_handler(client, Some(notify_navigate), client.cast(), None),
            AmStatus::Ok
        );
        assert_eq!(am_client_set_navigate_in_background(client, true), AmStatus::Ok);
        assert_eq!(am_client_set_visibility(client, 1, false), AmStatus::Ok);
        assert_eq!(am_client_start(client), AmStatus::Ok);
    }

    let out: Vec<serde_json::Value> =
        lines.map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
    let ok = child.wait().is_ok_and(|s| s.success());
    unsafe { am_client_free(client) };
    assert!(ok, "fake_host 退出码非 0：{out:?}");
    let navs: Vec<&serde_json::Value> = out.iter().filter(|l| l["type"] == "navigate").collect();
    assert_eq!(navs.len(), 2, "{out:?}");
    assert_eq!(
        navs[0]["error"],
        serde_json::json!({
            "code": -32019, "message": "已发通知，请点开后继续",
            "data": { "kind": "USER_ACTION_REQUIRED", "reason": "foreground", "uri": "conf://cart" }
        })
    );
    assert_eq!(navs[1]["error"]["code"], serde_json::json!(-32019));
    assert_eq!(navs[1]["error"]["data"]["reason"], serde_json::json!("foreground"));
    assert!(navs[1]["error"]["data"].get("uri").is_none(), "{:?}", navs[1]);
}
