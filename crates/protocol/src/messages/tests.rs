use super::*;

#[test]
fn app_id_prefix_detection() {
    assert!(has_app_id_prefix("shop.info", "shop"));
    assert!(!has_app_id_prefix("shopping.info", "shop"));
    assert!(!has_app_id_prefix("shop", "shop"));
    assert!(!has_app_id_prefix("shop.", "shop"));
    assert!(!has_app_id_prefix("cart.checkout", "shop"));
    assert!(!has_app_id_prefix("info", ""));
    let w = app_id_prefix_warning("shop.info", "shop");
    assert!(w.contains("shop.shop.info") && w.contains("\"info\""), "{w}");
}
use serde_json::json;

#[test]
fn hello_is_camel_case_and_skips_none() {
    let p = HelloParams {
        app_id: "shop".into(),
        app_name: "示例商城".into(),
        protocol_version: crate::PROTOCOL_VERSION.into(),
        sdk_version: "0.1.0".into(),
        client_kind: ClientKind::Web,
        instance_id: "i-1".into(),
        app_version: None,
        origin: Some("http://localhost:5173".into()),
        instance_title: None,
        instance_url: None,
        token: None,
        launch_token: None,
        overview: None,
        resume_token: None,
        tools_hash: None,
        wake_reason: None,
        heartbeat_ms: None,
        lifecycle_mode: None,
        capabilities: None,
        wake: None,
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(
        v,
        json!({
            "appId": "shop", "appName": "示例商城", "protocolVersion": "1",
            "sdkVersion": "0.1.0", "clientKind": "web", "instanceId": "i-1",
            "origin": "http://localhost:5173"
        })
    );
    assert_eq!(serde_json::from_value::<HelloParams>(v).unwrap(), p);
}

#[test]
fn hello_lifecycle_fields() {
    let p = HelloParams {
        app_id: "shop".into(),
        resume_token: Some("r1".into()),
        tools_hash: Some("0123456789abcdef".into()),
        wake_reason: Some(WakeReason::OsActivation),
        ..Default::default()
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["resumeToken"], "r1");
    assert_eq!(v["toolsHash"], "0123456789abcdef");
    assert_eq!(v["wakeReason"], "os-activation");
    assert_eq!(v["protocolVersion"], "1");
    assert_eq!(serde_json::from_value::<HelloParams>(v).unwrap(), p);

    // toolsCurrent 缺省 false，false 时不序列化
    let r: HelloResult = serde_json::from_value(json!({
        "status": "paired", "protocolVersion": "1", "hostVersion": "h"
    }))
    .unwrap();
    assert!(!r.tools_current);
    assert!(serde_json::to_value(&r).unwrap().get("toolsCurrent").is_none());
    let r = HelloResult { status: PairingStatus::Paired, tools_current: true, ..Default::default() };
    assert_eq!(serde_json::to_value(&r).unwrap()["toolsCurrent"], true);

    // 身份字段（1.6）：缺省不序列化；旧 Host 不带时解析为 None
    assert!(serde_json::to_value(&r).unwrap().get("service").is_none());
    let r = HelloResult {
        service: Some("app-mcp".into()),
        user: Some("1000".into()),
        pid: Some(42),
        ..Default::default()
    };
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!((v["service"].as_str(), v["user"].as_str(), v["pid"].as_u64()), (Some("app-mcp"), Some("1000"), Some(42)));
    assert_eq!(serde_json::from_value::<HelloResult>(v).unwrap(), r);

    // 连接 ID 与拒绝错误码（第 10 节）：缺省不序列化
    assert!(serde_json::to_value(&r).unwrap().get("connectionId").is_none());
    let r = HelloResult {
        connection_id: Some("a1b2c3-7".into()),
        code: Some("ORIGIN_NOT_ALLOWED".into()),
        ..Default::default()
    };
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!((v["connectionId"].as_str(), v["code"].as_str()), (Some("a1b2c3-7"), Some("ORIGIN_NOT_ALLOWED")));
    assert_eq!(serde_json::from_value::<HelloResult>(v).unwrap(), r);
}

#[test]
fn lifecycle_messages() {
    let p = SleepParams {
        reason: SleepReason::Grace,
        wake: Some(WakeDescriptor {
            kind: WakeKind::AndroidIntent,
            target: Some("dev.example/.WakeReceiver".into()),
            background: true,
        }),
        tools_hash: "abc".into(),
    };
    assert_eq!(
        serde_json::to_value(&p).unwrap(),
        json!({
            "reason": "grace",
            "wake": {"kind": "android-intent", "target": "dev.example/.WakeReceiver", "background": true},
            "toolsHash": "abc"
        })
    );
    let w: WakeDescriptor = serde_json::from_value(json!({"kind": "none"})).unwrap();
    assert_eq!(w, WakeDescriptor::default());
    for (k, s) in [
        (WakeKind::Uri, "uri"),
        (WakeKind::Aumid, "aumid"),
        (WakeKind::AppleEvent, "apple-event"),
        (WakeKind::Dbus, "dbus"),
        (WakeKind::WebUrl, "web-url"),
    ] {
        assert_eq!(serde_json::to_value(k).unwrap(), json!(s));
    }
    assert_eq!(serde_json::to_value(WakeReason::ColdStart).unwrap(), json!("cold-start"));

    let ok: SleepResult = serde_json::from_value(json!({"accepted": true, "resumeToken": "r"})).unwrap();
    assert_eq!(ok.resume_token.as_deref(), Some("r"));
    let no: SleepResult = serde_json::from_value(json!({"accepted": false, "retryAfterMs": 5000})).unwrap();
    assert_eq!(no.retry_after_ms, Some(5000));
    assert_eq!(serde_json::to_value(LeaseParams { ttl_ms: 0, adaptive: false }).unwrap(), json!({"ttlMs": 0}));
    assert_eq!(
        serde_json::to_value(LeaseParams { ttl_ms: 5, adaptive: true }).unwrap(),
        json!({"ttlMs": 5, "adaptive": true})
    );
    let old: LeaseParams = serde_json::from_value(json!({"ttlMs": 7})).unwrap();
    assert!(!old.adaptive, "旧 Host 不带 adaptive：按默认值租约");
}

#[test]
fn overview_roundtrip() {
    let v = json!({"summary": "示例商城", "body": "## 能力范围\n- 购物车"});
    let o: AppOverview = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(o.locale, None);
    assert_eq!(serde_json::to_value(&o).unwrap(), v);
}

#[test]
fn tool_info_defaults() {
    let t: ToolInfo = serde_json::from_value(json!({
        "name": "cart.checkout", "description": "结算", "inputSchema": {"type": "object"}
    }))
    .unwrap();
    assert_eq!(t.risk, Risk::Write);
    assert_eq!(t.activation, None);
    assert_eq!(serde_json::to_value(Risk::OsSensitive).unwrap(), json!("os-sensitive"));
    assert_eq!(t.annotations, None);
    assert!(serde_json::to_value(&t).unwrap().get("annotations").is_none());
}

#[test]
fn risk_annotation_mapping() {
    let a = |ro: bool, d: Option<bool>| ToolAnnotations {
        read_only_hint: Some(ro),
        destructive_hint: d,
        ..ToolAnnotations::default()
    };
    assert_eq!(Risk::Read.annotations(), a(true, None));
    assert_eq!(Risk::Write.annotations(), a(false, None));
    assert_eq!(Risk::OsSensitive.annotations(), a(false, None));
    assert_eq!(Risk::Destructive.annotations(), a(false, Some(true)));
    assert_eq!(Risk::Payment.annotations(), a(false, Some(true)));
}

#[test]
fn declared_annotations_win_per_field() {
    let mut t: ToolInfo = serde_json::from_value(json!({
        "name": "orders.cancel", "description": "取消", "inputSchema": {"type": "object"}, "risk": "destructive",
        "annotations": {"readOnlyHint": false, "idempotentHint": true, "openWorldHint": true, "title": "取消订单"}
    }))
    .unwrap();
    let e = t.effective_annotations();
    assert_eq!(e.destructive_hint, Some(true), "缺少的字段按 risk 推导");
    assert_eq!((e.idempotent_hint, e.open_world_hint), (Some(true), Some(true)));
    assert_eq!(e.title.as_deref(), Some("取消订单"));
    // 声明的字段优先于 risk
    t.annotations = Some(ToolAnnotations { destructive_hint: Some(false), ..ToolAnnotations::default() });
    assert_eq!(t.effective_annotations().destructive_hint, Some(false));
    assert_eq!(t.effective_annotations().read_only_hint, Some(false));
    // 未声明：与 risk 映射相同
    t.annotations = None;
    assert_eq!(t.effective_annotations(), Risk::Destructive.annotations());
    let v = serde_json::to_value(ToolAnnotations { read_only_hint: Some(true), ..ToolAnnotations::default() }).unwrap();
    assert_eq!(v, json!({"readOnlyHint": true}));
}

#[test]
fn content_annotations_roundtrip() {
    let v = json!({"audience": ["user", "assistant"], "priority": 0.5, "lastModified": "2026-10-02T00:00:00Z"});
    let a: ContentAnnotations = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(a.audience, Some(vec![Audience::User, Audience::Assistant]));
    assert_eq!(serde_json::to_value(&a).unwrap(), v);
    let r: ToolsInvokeResult = serde_json::from_value(json!({"data": 1})).unwrap();
    assert_eq!(r.annotations, None);
    let r = ToolsInvokeResult { annotations: Some(a), ..r };
    assert_eq!(serde_json::to_value(&r).unwrap()["annotations"]["priority"], json!(0.5));
    let res: ResourceInfo = serde_json::from_value(json!({"name": "n", "description": "d"})).unwrap();
    assert!(serde_json::to_value(&res).unwrap().get("annotations").is_none());
}

#[test]
fn progress_params_roundtrip() {
    let p: ToolsProgressParams = serde_json::from_value(json!({"callId": "c1", "progress": 3})).unwrap();
    assert_eq!((p.progress, p.total, p.message.as_deref()), (3.0, None, None));
    assert_eq!(serde_json::to_value(&p).unwrap(), json!({"callId": "c1", "progress": 3.0}));
    let p = ToolsProgressParams { total: Some(10.0), message: Some("导出中".into()), ..p };
    assert_eq!(
        serde_json::to_value(&p).unwrap(),
        json!({"callId": "c1", "progress": 3.0, "total": 10.0, "message": "导出中"})
    );
    assert!(serde_json::from_value::<ToolsProgressParams>(json!({"callId": "c1"})).is_err(), "progress 必填");
}

#[test]
fn result_status_and_summary() {
    let r: ToolsInvokeResult = serde_json::from_value(json!({"data": null})).unwrap();
    assert_eq!((r.status, r.summary.as_deref(), r.state_resource.as_deref()), (ResultStatus::Done, None, None));
    // 缺省值不序列化：旧 Host 看到的消息不变
    assert_eq!(serde_json::to_value(&r).unwrap(), json!({"data": null}));
    let r: ToolsInvokeResult = serde_json::from_value(json!({
        "data": {"orderId": "o1"}, "status": "pending", "stateResource": "order.state", "summary": "已提交，等待付款"
    }))
    .unwrap();
    assert_eq!(r.status, ResultStatus::Pending);
    assert_eq!(r.state_resource.as_deref(), Some("order.state"));
    for (st, s) in [(ResultStatus::Partial, "partial"), (ResultStatus::Noop, "noop"), (ResultStatus::Pending, "pending")] {
        assert_eq!(serde_json::to_value(st).unwrap(), json!(s));
    }
    let t: ToolInfo = serde_json::from_value(json!({
        "name": "n", "description": "d", "inputSchema": {"type": "object"}, "outputSchema": {"type": "array"}
    }))
    .unwrap();
    assert_eq!(t.output_schema, Some(json!({"type": "array"})));
    assert_eq!(serde_json::to_value(&t).unwrap()["outputSchema"], json!({"type": "array"}));
}

#[test]
fn name_validation() {
    assert!(is_valid_name("cart.checkout"));
    assert!(is_valid_name("a-b_c.D9"));
    assert!(!is_valid_name(""));
    assert!(!is_valid_name("has space"));
    assert!(!is_valid_name(&"x".repeat(65)));
    assert!(is_valid_app_id("shop"));
    assert!(is_valid_app_id("my-app2"));
    assert!(!is_valid_app_id("Shop"));
    assert!(!is_valid_app_id("2shop"));
    assert!(!is_valid_app_id(""));
}

#[test]
fn navigation_messages() {
    // 旧 SDK：没有 surface / page，序列化不变（toolsHash 不变）
    let t: ToolInfo = serde_json::from_value(json!({"name": "n", "description": "d", "inputSchema": {"type": "object"}})).unwrap();
    assert_eq!((t.surface, t.page.as_deref()), (ToolSurface::App, None));
    let v = serde_json::to_value(&t).unwrap();
    assert!(v.get("surface").is_none() && v.get("page").is_none());
    let t = ToolInfo { surface: ToolSurface::View, page: Some("cart".into()), ..t };
    let v = serde_json::to_value(&t).unwrap();
    assert_eq!((v["surface"].as_str(), v["page"].as_str()), (Some("view"), Some("cart")));
    assert_eq!(serde_json::from_value::<ToolInfo>(v).unwrap(), t);
    assert!(serde_json::to_value(&t).unwrap().get("backgroundTool").is_none());
    let t = ToolInfo { background_tool: Some("cart.add".into()), ..t };
    let v = serde_json::to_value(&t).unwrap();
    assert_eq!(v["backgroundTool"], "cart.add");
    assert_eq!(serde_json::from_value::<ToolInfo>(v).unwrap(), t);

    // 能力协商：缺省不序列化
    let h = HelloParams::default();
    assert!(serde_json::to_value(&h).unwrap().get("capabilities").is_none());
    let h = HelloParams { capabilities: Some(SdkCapabilities { navigate: true }), ..h };
    let v = serde_json::to_value(&h).unwrap();
    assert_eq!(v["capabilities"], json!({"navigate": true}));
    assert_eq!(serde_json::from_value::<HelloParams>(v).unwrap(), h);
    assert_eq!(serde_json::to_value(SdkCapabilities::default()).unwrap(), json!({}));

    let p: NavigateParams = serde_json::from_value(json!({"page": "orders.detail", "params": {"id": "o1"}})).unwrap();
    assert_eq!(p.params, Some(json!({"id": "o1"})));
    assert_eq!(serde_json::to_value(NavigateParams { page: "cart".into(), params: None, timeout_ms: None }).unwrap(), json!({"page": "cart"}));
    assert_eq!(serde_json::to_value(NavigateResult { ok: true }).unwrap(), json!({"ok": true}));
}

#[test]
fn hello_power_fields() {
    let p = HelloParams {
        heartbeat_ms: Some(0),
        lifecycle_mode: Some(LifecycleMode::OnDemand),
        ..HelloParams::default()
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["heartbeatMs"], json!(0));
    assert_eq!(v["lifecycleMode"], json!("on-demand"));
    assert_eq!(serde_json::from_value::<HelloParams>(v).unwrap(), p);
    // 旧 SDK 不带这两个字段
    let old = serde_json::to_value(HelloParams::default()).unwrap();
    assert!(old.get("heartbeatMs").is_none() && old.get("lifecycleMode").is_none());
}

#[test]
fn hello_wake_descriptor() {
    let wake = WakeDescriptor { kind: WakeKind::Uri, target: Some("appmcp-x".into()), background: true };
    let p = HelloParams { wake: Some(wake), ..HelloParams::default() };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["wake"], json!({ "kind": "uri", "target": "appmcp-x", "background": true }));
    assert_eq!(serde_json::from_value::<HelloParams>(v).unwrap(), p);
    assert!(serde_json::to_value(HelloParams::default()).unwrap().get("wake").is_none());
}

#[test]
fn invoke_priority_is_lenient_and_omitted_when_normal() {
    let parse = |v: Value| serde_json::from_value::<ToolsInvokeParams>(json!({"callId": "c", "name": "t", "priority": v})).unwrap().priority;
    assert_eq!(parse(json!("interactive")), CallPriority::Interactive);
    assert_eq!(parse(json!("background")), CallPriority::Background);
    assert_eq!(parse(json!("urgent")), CallPriority::Normal, "不认识的取值按 normal");
    assert_eq!(parse(json!(3)), CallPriority::Normal);
    let p = ToolsInvokeParams {
        call_id: "c".into(),
        name: "t".into(),
        arguments: Value::Null,
        timeout_ms: None,
        idempotency_key: None,
        priority: CallPriority::Normal,
    };
    assert!(serde_json::to_value(&p).unwrap().get("priority").is_none(), "normal 不写出");
    let p = ToolsInvokeParams { priority: CallPriority::Background, ..p };
    assert_eq!(serde_json::to_value(&p).unwrap()["priority"], json!("background"));
    assert!(CallPriority::Interactive > CallPriority::Normal && CallPriority::Normal > CallPriority::Background);
    assert_eq!(CallPriority::parse("urgent"), None);
}

#[test]
fn event_messages_roundtrip() {
    let info = EventInfo { name: "order.shipped".into(), description: "订单已发货".into(), payload_schema: None };
    assert_eq!(serde_json::to_value(&info).unwrap(), json!({"name": "order.shipped", "description": "订单已发货"}));
    let emit: EventEmitParams =
        serde_json::from_value(json!({"name": "order.shipped", "eventId": "e1", "payload": {"orderId": "o1"}})).unwrap();
    assert_eq!((emit.event_id.as_str(), emit.payload), ("e1", Some(json!({"orderId": "o1"}))));
    let sync: EventsSyncParams = serde_json::from_value(json!({"events": [{"name": "a", "description": "b"}]})).unwrap();
    assert_eq!(sync.events[0].name, "a");
}
