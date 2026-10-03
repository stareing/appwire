use super::*;
use app_mcp_core::{ConnectionErrorCode, ReadId, ResourceId, ToolId};
use serde_json::json;

#[test]
fn config_defaults_and_overrides() {
    let c: JsConfig = JsConfig::from_json(json!({
        "appId": "shop", "appName": "商城", "instanceId": "i1",
        "sdkVersion": "9.9.9", "token": "t", "maxConcurrentCalls": 0, "maxQueuedCalls": 5,
        "heartbeat": { "timeoutMs": 5 }
    }))
    .unwrap();
    let c = c.into_core();
    assert_eq!(c.client_kind, ClientKind::Web);
    assert_eq!(c.sdk_version, "9.9.9");
    assert_eq!(c.token.as_deref(), Some("t"));
    assert_eq!(c.max_concurrent_calls, 1);
    assert_eq!(c.max_queued_calls, 5);
    assert_eq!(c.heartbeat.timeout_ms, 5);
    assert_eq!(c.heartbeat.interval_ms, HeartbeatPolicy::default().interval_ms);
    assert_eq!(c.reconnect, ReconnectPolicy::default());
    assert_eq!(c.resource_update_throttle_ms, 100);
    assert_eq!(c.overview, None);
    assert_eq!(c.call_dedup, CallDedupPolicy::default());
}

#[test]
fn config_call_dedup() {
    let base = || json!({ "appId": "shop", "appName": "商城", "instanceId": "i1" });
    let with = |d: Value| {
        let mut v = base();
        v["callDedup"] = d;
        JsConfig::from_json(v).map(JsConfig::into_core)
    };
    assert_eq!(with(json!({ "ttlMs": 0 })).unwrap().call_dedup, CallDedupPolicy { ttl_ms: 0, ..CallDedupPolicy::default() });
    assert!(!with(json!({ "maxEntries": 0 })).unwrap().call_dedup.enabled());
    assert_eq!(with(json!({ "ttlMs": 10, "maxEntries": 3 })).unwrap().call_dedup, CallDedupPolicy { ttl_ms: 10, max_entries: 3 });
    assert!(with(json!({ "ttlMs": -1 })).is_err());
    assert_eq!(with(json!({ "bogus": 1 })).unwrap().call_dedup, CallDedupPolicy::default(), "未知字段忽略");
}

#[test]
fn config_power_fields() {
    let c = JsConfig::from_json(json!({
        "appId": "shop", "appName": "商城", "instanceId": "i1", "transport": "loopback",
        "heartbeat": { "mode": "always" },
        "lifecycle": { "mode": "idle", "hostAbsentRetries": 5, "legacyTimers": true,
                       "mergeWindowMs": 500, "sleepOnBackground": true }
    }))
    .unwrap()
    .into_core();
    assert_eq!(c.transport, TransportKind::Loopback);
    assert_eq!(c.heartbeat.mode, HeartbeatMode::Always);
    assert_eq!((c.lifecycle.host_absent_retries, c.lifecycle.legacy_timers), (5, true));
    assert_eq!((c.lifecycle.merge_window_ms, c.lifecycle.sleep_on_background), (500, true));
    let d = JsConfig::from_json(json!({ "appId": "shop", "appName": "商城", "instanceId": "i1" })).unwrap().into_core();
    assert_eq!(d.transport, TransportKind::Unknown);
    assert_eq!((d.lifecycle.host_absent_retries, d.lifecycle.legacy_timers), (3, false));
    assert_eq!((d.lifecycle.merge_window_ms, d.lifecycle.sleep_on_background), (2_000, false));
    let r = JsResourceDef::from_json(json!({ "name": "order", "realtime": true })).unwrap().into_core().unwrap();
    assert!(r.realtime);
    let r = JsResourceDef::from_json(json!({ "name": "cart" })).unwrap().into_core().unwrap();
    assert!(!r.realtime);
    assert_eq!(r.annotations, None);
    let r = JsResourceDef::from_json(json!({ "name": "cart", "annotations": { "audience": ["user"], "priority": 0.5 } }))
        .unwrap()
        .into_core()
        .unwrap();
    let want = ContentAnnotations { audience: Some(vec![Audience::User]), priority: Some(0.5), last_modified: None };
    assert_eq!(r.annotations, Some(want));
    let bad = JsResourceDef::from_json(json!({ "name": "cart", "annotations": { "audience": ["bot"] } }));
    assert!(bad.is_err(), "非法内容标注报错");
    let bad = JsConfig::from_json(json!({ "appId": "shop", "appName": "商城", "instanceId": "i1", "transport": "x" }));
    assert!(bad.is_err_and(|e| e.contains("无效的传输类别")));
}

#[test]
fn config_overview() {
    let c: JsConfig = JsConfig::from_json(json!({
        "appId": "shop", "appName": "商城", "instanceId": "i1",
        "overview": { "summary": "演示商城", "body": "## 能力范围", "locale": "zh-CN" }
    }))
    .unwrap();
    let o = c.into_core().overview.unwrap();
    assert_eq!(o.summary, "演示商城");
    assert_eq!(o.body.as_deref(), Some("## 能力范围"));
    assert_eq!(o.locale.as_deref(), Some("zh-CN"));
}

#[test]
fn tool_surface_and_page() {
    let d = JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "surface": "view", "page": "cart" }))
        .unwrap()
        .into_core()
        .unwrap();
    assert_eq!((d.surface, d.page.as_deref()), (ToolSurface::View, Some("cart")));
    let d = JsToolDef::from_json(json!({ "name": "x", "inputSchema": {} })).unwrap().into_core().unwrap();
    assert_eq!((d.surface, d.page), (ToolSurface::App, None));
    assert!(JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "surface": "page" })).is_err());
    let u = JsToolUpdate::from_json(json!({ "page": null, "surface": "app" })).unwrap().into_core();
    assert_eq!((u.surface, u.page), (Some(ToolSurface::App), Some(None)));
    let u = JsToolUpdate::from_json(json!({ "page": "orders" })).unwrap().into_core();
    assert_eq!((u.surface, u.page), (None, Some(Some("orders".into()))));
    assert!(JsToolUpdate::from_json(json!({ "page": 1 })).is_err());
}

#[test]
fn tool_background_tool() {
    let d = JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "surface": "view", "backgroundTool": "x.bg" }))
        .unwrap()
        .into_core()
        .unwrap();
    assert_eq!(d.background_tool.as_deref(), Some("x.bg"));
    let d = JsToolDef::from_json(json!({ "name": "x", "inputSchema": {} })).unwrap().into_core().unwrap();
    assert_eq!(d.background_tool, None);
    assert!(JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "backgroundTool": 1 })).is_err());
    let u = JsToolUpdate::from_json(json!({ "backgroundTool": null })).unwrap().into_core();
    assert_eq!(u.background_tool, Some(None));
    let u = JsToolUpdate::from_json(json!({ "backgroundTool": "x.bg" })).unwrap().into_core();
    assert_eq!(u.background_tool, Some(Some("x.bg".into())));
    assert_eq!(JsToolUpdate::from_json(json!({})).unwrap().into_core().background_tool, None);
    assert!(JsToolUpdate::from_json(json!({ "backgroundTool": 1 })).is_err());
}

#[test]
fn tool_concurrency_declarations() {
    let d = JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "concurrency": 2, "exclusive": "doc" }))
        .unwrap()
        .into_core()
        .unwrap();
    assert_eq!((d.concurrency, d.exclusive.as_deref()), (2, Some("doc")));
    let d = JsToolDef::from_json(json!({ "name": "x", "inputSchema": {} })).unwrap().into_core().unwrap();
    assert_eq!((d.concurrency, d.exclusive), (0, None));
    assert!(JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "concurrency": -1 })).is_err());
    assert!(JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "concurrency": 4_294_967_296u64 })).is_err());
    assert!(JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "exclusive": 1 })).is_err());
    let u = JsToolUpdate::from_json(json!({ "concurrency": 3, "exclusive": null })).unwrap().into_core();
    assert_eq!((u.concurrency, u.exclusive), (Some(3), Some(None)));
    let u = JsToolUpdate::from_json(json!({})).unwrap().into_core();
    assert_eq!((u.concurrency, u.exclusive), (None, None));
}

#[test]
fn tool_def_defaults() {
    let d: JsToolDef = JsToolDef::from_json(json!({
        "name": "cart.add", "description": "加入购物车",
        "inputSchema": { "type": "object" }, "risk": "os-sensitive", "scope": 3
    }))
    .unwrap();
    let d = d.into_core().unwrap();
    assert_eq!(d.risk, Risk::OsSensitive);
    assert!(d.enabled);
    assert_eq!(d.scope, Some(ScopeId(3)));
    assert_eq!(d.activation, None);

    let bad: JsToolDef =
        JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "scope": 1.5 })).unwrap();
    assert!(bad.into_core().is_err());
}

#[test]
fn tool_update_distinguishes_missing_and_null() {
    let u: JsToolUpdate = JsToolUpdate::from_json(json!({ "title": null, "enabled": false })).unwrap();
    let u = u.into_core();
    assert_eq!(u.title, Some(None));
    assert_eq!(u.activation, None);
    assert_eq!(u.enabled, Some(false));
    assert_eq!(u.description, None);
}

#[test]
fn outcome_conversion() {
    let ok: JsCallOutcome = JsCallOutcome::from_json(json!({ "data": { "a": 1 }, "stateHints": ["cart"] })).unwrap();
    let ok = ok.into_call().unwrap();
    assert_eq!(ok.data, json!({ "a": 1 }));
    assert_eq!(ok.state_hints, vec!["cart".to_owned()]);

    let empty: JsCallOutcome = JsCallOutcome::from_json(json!({})).unwrap();
    assert_eq!(empty.into_read().unwrap(), Value::Null);

    let err: JsCallOutcome = JsCallOutcome::from_json(json!({
        "error": { "kind": "INVALID_INPUT", "message": "bad", "details": { "path": "a" } }
    }))
    .unwrap();
    let err = err.into_call().unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput);
    assert_eq!(err.message, "bad");
    assert_eq!(err.details, Some(json!({ "path": "a" })));

    let bad_kind = JsCallOutcome::from_json(json!({ "error": { "kind": "NOPE" } }));
    assert!(bad_kind.is_err());

    // USER_ACTION_REQUIRED（@app-mcp/web 的 ToolCallError.userActionRequired）：详情原样进入核心错误
    let action = JsCallOutcome::from_json(json!({
        "error": { "kind": "USER_ACTION_REQUIRED", "message": "请先登录", "details": { "reason": "login", "uri": "shop://login" } }
    }))
    .unwrap()
    .into_call()
    .unwrap_err();
    assert_eq!(action, ToolError::user_action_required("请先登录", Some("login"), Some("shop://login")));
    let bare = JsCallOutcome::from_json(json!({ "error": { "kind": "USER_ACTION_REQUIRED", "message": "切到前台" } }))
        .unwrap()
        .into_call()
        .unwrap_err();
    assert_eq!(bare, ToolError::user_action_required("切到前台", None, None));
    let denied = JsCallOutcome::from_json(json!({ "error": { "kind": "POLICY_DENIED" } })).unwrap();
    assert_eq!(denied.into_call().unwrap_err().kind, ErrorKind::PolicyDenied);
}

/// 第 14 项 S1 / 第 19 项 R2：工具注解与输出 schema；更新时 `null` 清除。
#[test]
fn tool_annotations_and_output_schema() {
    let d = JsToolDef::from_json(json!({
        "name": "order.cancel", "inputSchema": { "type": "object" },
        "annotations": { "destructiveHint": true, "idempotentHint": true, "title": "取消" },
        "outputSchema": { "type": "array" }
    }))
    .unwrap()
    .into_core()
    .unwrap();
    let a = d.annotations.unwrap();
    assert_eq!((a.destructive_hint, a.idempotent_hint, a.read_only_hint), (Some(true), Some(true), None));
    assert_eq!(a.title.as_deref(), Some("取消"));
    assert_eq!(d.output_schema, Some(json!({ "type": "array" })));
    let e = JsToolDef::from_json(json!({ "name": "x", "inputSchema": {}, "annotations": { "readOnlyHint": 1 } }))
        .unwrap_err();
    assert_eq!(e, "annotations.字段 readOnlyHint 应为布尔值");
    let u = JsToolUpdate::from_json(json!({ "annotations": null, "outputSchema": null })).unwrap().into_core();
    assert_eq!((u.annotations, u.output_schema), (Some(None), Some(None)));
    let u = JsToolUpdate::from_json(json!({ "annotations": { "openWorldHint": false } })).unwrap().into_core();
    assert_eq!(u.annotations.unwrap().unwrap().open_world_hint, Some(false));
    assert_eq!(u.output_schema, None);
    assert!(JsToolUpdate::from_json(json!({ "annotations": { "title": 1 } })).is_err());
}

/// 第 19 项 R1 / R3 与第 14 项 S2：结果状态、状态资源、摘要、内容标注。
#[test]
fn outcome_status_summary_annotations() {
    let o = JsCallOutcome::from_json(json!({
        "data": null, "status": "pending", "stateResource": "order.state", "summary": "已提交",
        "annotations": { "audience": ["user", "assistant"], "priority": 0.5, "lastModified": "2026-10-02T00:00:00Z" }
    }))
    .unwrap()
    .into_call()
    .unwrap();
    assert_eq!(o.status, ResultStatus::Pending);
    assert_eq!((o.state_resource.as_deref(), o.summary.as_deref()), (Some("order.state"), Some("已提交")));
    let a = o.annotations.unwrap();
    assert_eq!(a.audience, Some(vec![Audience::User, Audience::Assistant]));
    assert_eq!((a.priority, a.last_modified.as_deref()), (Some(0.5), Some("2026-10-02T00:00:00Z")));
    for (s, st) in [("done", ResultStatus::Done), ("partial", ResultStatus::Partial), ("noop", ResultStatus::Noop)] {
        let o = JsCallOutcome::from_json(json!({ "status": s })).unwrap().into_call().unwrap();
        assert_eq!(o.status, st);
    }
    assert_eq!(JsCallOutcome::from_json(json!({ "status": "maybe" })).unwrap_err(), "无效的 status：\"maybe\"");
    assert!(JsCallOutcome::from_json(json!({ "annotations": { "audience": ["robot"] } })).is_err());
    let plain = JsCallOutcome::from_json(json!({ "data": 1 })).unwrap().into_call().unwrap();
    assert_eq!((plain.status, plain.summary, plain.annotations), (ResultStatus::Done, None, None));
}

#[test]
fn state_shape() {
    let cases = [
        (ConnectionState::Idle, json!({ "status": "idle" })),
        (ConnectionState::Connecting, json!({ "status": "connecting" })),
        (ConnectionState::Handshaking, json!({ "status": "handshaking" })),
        (ConnectionState::PendingPairing, json!({ "status": "pending-pairing" })),
        (ConnectionState::Connected, json!({ "status": "connected" })),
        (
            ConnectionState::Backoff { retry_at: 42, reason: None, code: None },
            json!({ "status": "backoff", "retryAt": 42 }),
        ),
        (
            ConnectionState::Backoff {
                retry_at: 42,
                reason: Some("无法连接".into()),
                code: Some(ConnectionErrorCode::ConnectFailed),
            },
            json!({ "status": "backoff", "retryAt": 42, "reason": "无法连接", "code": "CONNECT_FAILED" }),
        ),
        (
            ConnectionState::Rejected { reason: "r".into(), code: ConnectionErrorCode::OriginNotAllowed },
            json!({ "status": "rejected", "reason": "r", "code": "ORIGIN_NOT_ALLOWED" }),
        ),
        (ConnectionState::Stopped, json!({ "status": "stopped" })),
        (ConnectionState::Dormant, json!({ "status": "dormant" })),
        (ConnectionState::Waking, json!({ "status": "waking" })),
        (
            ConnectionState::HostMismatch { reason: "m".into(), code: ConnectionErrorCode::HostNotAppMcp },
            json!({ "status": "host-mismatch", "reason": "m", "code": "HOST_NOT_APP_MCP" }),
        ),
    ];
    for (state, expected) in cases {
        assert_eq!(JsState::from_core(&state).to_value(), expected);
    }
}

#[test]
fn event_shape() {
    let cases = [
        (Event::Connect, json!({ "type": "connect" })),
        (Event::Disconnect, json!({ "type": "disconnect" })),
        (Event::Send("x".into()), json!({ "type": "send", "text": "x" })),
        (
            Event::InvokeTool {
                call_id: "c1".into(),
                tool: ToolId(7),
                name: "t".into(),
                arguments: json!({ "a": 1 }),
                idempotency_key: None,
            },
            json!({ "type": "invokeTool", "callId": "c1", "tool": 7, "name": "t", "arguments": { "a": 1 } }),
        ),
        (
            Event::InvokeTool {
                call_id: "c2".into(),
                tool: ToolId(7),
                name: "t".into(),
                arguments: json!({}),
                idempotency_key: Some("k-1".into()),
            },
            json!({ "type": "invokeTool", "callId": "c2", "tool": 7, "name": "t", "arguments": {}, "idempotencyKey": "k-1" }),
        ),
        (
            Event::CancelTool { call_id: "c1".into(), reason: CancelReason::Timeout },
            json!({ "type": "cancelTool", "callId": "c1", "reason": "timeout" }),
        ),
        (
            Event::ReadResource { read: ReadId(2), resource: ResourceId(3), name: "r".into() },
            json!({ "type": "readResource", "read": 2, "resource": 3, "name": "r" }),
        ),
        (
            Event::StateChanged(ConnectionState::Connected),
            json!({ "type": "stateChanged", "state": { "status": "connected" } }),
        ),
        (Event::Paired { token: "tk".into() }, json!({ "type": "paired", "token": "tk" })),
        (Event::Warning("w".into()), json!({ "type": "warning", "message": "w" })),
        (Event::IdleExit, json!({ "type": "idleExit" })),
        (
            Event::Navigate { navigate: app_mcp_core::NavigateId(4), page: "cart".into(), params: Value::Null },
            json!({ "type": "navigate", "navigate": 4, "page": "cart", "params": null }),
        ),
        (
            Event::StateChanged(ConnectionState::Dormant),
            json!({ "type": "stateChanged", "state": { "status": "dormant" } }),
        ),
        (
            Event::StateChanged(ConnectionState::Waking),
            json!({ "type": "stateChanged", "state": { "status": "waking" } }),
        ),
    ];
    for (ev, expected) in cases {
        assert_eq!(JsEvent::from_core(ev).to_value(), expected);
    }
}

#[test]
fn config_lifecycle() {
    let c: JsConfig = JsConfig::from_json(json!({
        "appId": "shop", "appName": "商城", "instanceId": "i1", "handshakeTimeoutMs": 0,
        "lifecycle": {
            "mode": "on-demand", "graceMs": 3000, "residency": "exit-when-idle",
            "wake": { "kind": "web-url", "target": "http://localhost:5173/" }
        }
    }))
    .unwrap();
    let c = c.into_core();
    assert_eq!(c.handshake_timeout_ms, 0);
    let l = c.lifecycle;
    assert_eq!(l.mode, LifecycleMode::OnDemand);
    assert_eq!(l.grace_ms, 3000);
    assert_eq!(l.idle_timeout_ms, LifecyclePolicy::default().idle_timeout_ms);
    assert_eq!(l.residency, Residency::ExitWhenIdle);
    let w = l.wake.unwrap();
    assert_eq!(w.kind, app_mcp_core::WakeKind::WebUrl);
    assert!(!w.background);

    let c: JsConfig = JsConfig::from_json(json!({ "appId": "shop", "appName": "商城", "instanceId": "i1" })).unwrap();
    assert_eq!(c.into_core().lifecycle, LifecyclePolicy::default());
    let bad = JsConfig::from_json(json!({
        "appId": "shop", "appName": "商城", "instanceId": "i1", "lifecycle": { "mode": "sometimes" }
    }));
    assert!(bad.is_err());
}

#[test]
fn reason_parse() {
    assert_eq!(parse_wake_reason("visible").unwrap(), WakeReason::Visible);
    assert!(parse_wake_reason("x").is_err());
    assert_eq!(parse_sleep_reason("background").unwrap(), SleepReason::Background);
    assert!(parse_sleep_reason("x").is_err());
}

#[test]
fn visibility_parse() {
    assert_eq!(parse_visibility("frozen").unwrap(), Visibility::Frozen);
    assert!(parse_visibility("gone").is_err());
}

#[test]
fn json_reader_errors_and_null_handling() {
    let err = |v: Value| JsConfig::from_json(v).unwrap_err();
    assert_eq!(err(json!([])), "应为对象");
    assert_eq!(err(json!({ "appName": "a", "instanceId": "i" })), "缺少字段 appId");
    assert_eq!(err(json!({ "appId": 1, "appName": "a", "instanceId": "i" })), "字段 appId 应为字符串");
    let base = json!({ "appId": "a", "appName": "a", "instanceId": "i" });
    let with = |k: &str, v: Value| {
        let mut o = base.clone();
        o[k] = v;
        o
    };
    assert_eq!(err(with("maxConcurrentCalls", json!(-1))), "字段 maxConcurrentCalls 应为非负整数");
    assert_eq!(err(with("heartbeat", json!({ "timeoutMs": "x" }))), "heartbeat.字段 timeoutMs 应为非负整数");
    assert_eq!(err(with("reconnect", json!({ "multiplier": true }))), "reconnect.字段 multiplier 应为数字");
    assert!(err(with("clientKind", json!("tv"))).starts_with("字段 clientKind："));
    assert_eq!(err(with("lifecycle", json!({ "residency": "x" }))), "lifecycle.无效的驻留策略：\"x\"");
    // 多处出错时只报读取顺序上的第一条
    assert_eq!(
        err(json!({ "appId": 1, "appName": 2, "instanceId": "i", "heartbeat": { "timeoutMs": "x" } })),
        "字段 appId 应为字符串"
    );
    assert_eq!(err(with("lifecycle", json!({ "mode": 1, "residency": "x" }))), "lifecycle.字段 mode 应为字符串");
    // null 等同缺省；未知字段忽略
    let c = JsConfig::from_json(with("token", Value::Null)).unwrap();
    assert_eq!(c.token, None);
    assert!(JsConfig::from_json(with("unknownField", json!(1))).is_ok());

    assert_eq!(JsToolDef::from_json(json!({ "name": "t" })).unwrap_err(), "缺少字段 inputSchema");
    assert_eq!(
        JsToolDef::from_json(json!({ "name": "t", "inputSchema": {}, "enabled": 1 })).unwrap_err(),
        "字段 enabled 应为布尔值"
    );
    assert_eq!(JsResourceDef::from_json(json!({ "name": 3 })).unwrap_err(), "字段 name 应为字符串");
    assert_eq!(
        JsToolUpdate::from_json(json!({ "title": 1 })).unwrap_err(),
        "字段 title 应为字符串"
    );
    let u = JsToolUpdate::from_json(json!({ "activation": null, "title": "x", "risk": "write" })).unwrap();
    assert_eq!(u.activation, Some(None));
    assert_eq!(u.title, Some(Some("x".into())));
    assert_eq!(u.risk, Some(Risk::Write));
    assert_eq!(
        JsCallOutcome::from_json(json!({ "stateHints": ["a", 1] })).unwrap_err(),
        "字段 stateHints 应为字符串数组"
    );
    assert_eq!(JsCallOutcome::from_json(json!({ "error": {} })).unwrap_err(), "error.缺少字段 kind");
}

#[test]
fn handle_validation() {
    assert_eq!(handle(7.0, "句柄"), Ok(7));
    assert_eq!(handle(1.5, "句柄").unwrap_err(), "无效的句柄：1.5");
    assert_eq!(handle(-1.0, "句柄").unwrap_err(), "无效的句柄：-1.0");
    assert_eq!(handle(f64::NAN, "句柄").unwrap_err(), "无效的句柄：NaN");
    assert_eq!(handle(f64::NEG_INFINITY, "句柄").unwrap_err(), "无效的句柄：-Infinity");
    assert!(handle(MAX_SAFE_INTEGER as f64 + 2.0, "句柄").is_err());
}
