//! 类型转换单元测试。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use app_mcp_hub as hub;
use serde_json::{Value, json};

use super::events::other_event;
use super::*;

#[test]
fn config_defaults_follow_hub() {
    let c = HubConfig::default().into_hub().unwrap();
    let d = hub::HubConfig::default();
    assert_eq!(c.listen, d.listen);
    assert_eq!(c.listen_alternates, d.listen_alternates);
    assert!(!c.mcp_http);
    assert_eq!(c.run_dir, None);
    assert_eq!(c.state_dir, None);
    assert_eq!(c.ipc_endpoint, d.ipc_endpoint);
    assert_eq!(c.response_timeout, d.response_timeout);
    assert_eq!(c.navigate_timeout, hub::DEFAULT_NAVIGATE_TIMEOUT);
    assert_eq!(
        (c.task_idle_ttl, c.stateless_tool_exposure, c.principal_select_ttl, c.stateless_list_ttl),
        (d.task_idle_ttl, d.stateless_tool_exposure, d.principal_select_ttl, d.stateless_list_ttl)
    );
    assert_eq!(
        (c.mcp_protocol_mode, c.max_listen_streams, c.max_listen_resources),
        (d.mcp_protocol_mode, d.max_listen_streams, d.max_listen_resources)
    );
    assert_eq!(c.max_task_handles, d.max_task_handles);
    assert_eq!(c.max_locks, d.max_locks);
    assert_eq!(c.approval, d.approval);
    assert!(c.manifests.is_empty() && c.upstreams.is_empty());
}

#[test]
fn config_overrides() {
    let c = HubConfig {
        listen: Some("127.0.0.1:0".into()),
        mcp_http: true,
        run_dir: Some("/tmp/r".into()),
        state_dir: Some("/tmp/s".into()),
        approval_min_risk: Some(Risk::Destructive),
        approval_timeout_ms: Some(500),
        response_timeout_ms: Some(1234),
        navigate_timeout_ms: Some(800),
        task_idle_ttl_ms: Some(0),
        stateless_tool_exposure: Some(ToolExposure::Progressive),
        principal_select_ttl_ms: Some(1500),
        stateless_list_ttl_ms: Some(750),
        mcp_protocol_mode: Some(McpProtocolMode::LegacyOnly),
        max_listen_streams: Some(0),
        max_listen_resources: Some(8),
        max_task_handles: Some(0),
        max_locks: Some(0),
        upstreams: vec![UpstreamSpec {
            name: "fs".into(),
            command: "npx".into(),
            args: vec!["x".into()],
            env: HashMap::from([("A".into(), "1".into())]),
        }],
        manifests_json: vec![r#"{"manifestVersion":1,"appId":"shop","name":"Shop"}"#.into()],
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    assert_eq!(c.listen.as_deref(), Some("127.0.0.1:0"));
    assert!(c.listen_alternates.is_empty(), "显式地址不尝试备选端口");
    assert!(c.mcp_http);
    assert_eq!(c.run_dir, Some(PathBuf::from("/tmp/r")));
    assert_eq!(c.state_dir, Some(PathBuf::from("/tmp/s")));
    assert_eq!(c.approval.require_at_or_above, Some(hub::Risk::Destructive));
    assert_eq!(c.approval.timeout, Some(Duration::from_millis(500)));
    assert_eq!(c.response_timeout, Duration::from_millis(1234));
    assert_eq!(c.navigate_timeout, Duration::from_millis(800));
    assert_eq!(c.task_idle_ttl, Duration::ZERO);
    assert_eq!(c.stateless_tool_exposure, hub::ToolExposure::Progressive);
    assert_eq!(c.principal_select_ttl, Duration::from_millis(1500));
    assert_eq!(c.stateless_list_ttl, Duration::from_millis(750));
    assert_eq!(c.mcp_protocol_mode, hub::McpProtocolMode::LegacyOnly);
    assert_eq!((c.max_listen_streams, c.max_listen_resources), (0, 8));
    assert_eq!(c.max_task_handles, 0);
    assert_eq!(c.max_locks, 0);
    assert_eq!(c.upstreams["fs"].env["A"], "1");
    assert_eq!(c.manifests[0].app_id, "shop");

    let off = HubConfig {
        enable_listen: false,
        listen: Some("127.0.0.1:1".into()),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    assert_eq!(off.listen, None);

    let ipc = HubConfig {
        ipc_endpoint: Some("unix:/run/x/hub.sock".into()),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    assert_eq!(ipc.ipc_endpoint.as_deref(), Some("unix:/run/x/hub.sock"));
    let no_ipc = HubConfig {
        enable_ipc: false,
        ipc_endpoint: Some("unix:/run/x/hub.sock".into()),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    assert_eq!(no_ipc.ipc_endpoint, None);

    let bad = HubConfig {
        manifests_json: vec!["{".into()],
        ..Default::default()
    }
    .into_hub();
    assert!(matches!(bad, Err(HubError::InvalidConfig { .. })));
}

#[test]
fn call_request_arguments() {
    let r = CallRequest {
        name: "a.b".into(),
        arguments_json: None,
        instance_id: None,
        timeout_ms: Some(10),
        call_id: None,
        session: Some("s".into()),
        idempotency_key: Some("order-7".into()),
        priority: None,
        cache_bypass: false,
    };
    let h = r.clone().into_hub().unwrap();
    assert_eq!(h.arguments, json!({}));
    assert!(!h.cache_bypass);
    assert!(CallRequest { cache_bypass: true, ..r.clone() }.into_hub().unwrap().cache_bypass, "cache_bypass 透传");
    assert_eq!(h.priority, hub::CallPriority::Normal);
    let p = CallRequest { priority: Some(CallPriority::Background), ..r.clone() }.into_hub().unwrap();
    assert_eq!(p.priority, hub::CallPriority::Background);
    assert_eq!(h.idempotency_key.as_deref(), Some("order-7"));
    assert_eq!(h.timeout, Some(Duration::from_millis(10)));
    let h = CallRequest {
        arguments_json: Some(r#"{"x":1}"#.into()),
        ..r.clone()
    }
    .into_hub()
    .unwrap();
    assert_eq!(h.arguments, json!({"x": 1}));
    assert!(matches!(
        CallRequest {
            arguments_json: Some("nope".into()),
            ..r
        }
        .into_hub(),
        Err(HubError::InvalidJson { .. })
    ));
}

#[test]
fn outcome_and_error_conversion() {
    let o: CallOutcome = hub::CallOutcome {
        call_id: "c".into(),
        result: Err(hub::ToolError::new(hub::ErrorKind::UserRejected, "不")
            .with_details(json!({"a": 1}))),
        state_hints: vec![],
        instance_id: None,
        overview: None,
        status: hub::ResultStatus::Done,
        state_resource: None,
        summary: None,
        annotations: None,
        routed_to: None,
        duration_ms: 0,
        woke: false,
        cached_age_ms: None,
        undo: None,
        undo_of: None,
    }
    .into();
    assert_eq!(o.status, ResultStatus::Done);
    assert_eq!((o.duration_ms, o.woke, o.cached_age_ms), (0, false, None));
    let e = o.error.unwrap();
    assert_eq!(e.kind, "USER_REJECTED");
    assert_eq!(e.details_json.as_deref(), Some(r#"{"a":1}"#));
    assert!(o.data_json.is_none());

    let unsupported: HubError =
        std::io::Error::new(std::io::ErrorKind::Unsupported, "缺少 `mcp-server`").into();
    assert_eq!(unsupported, HubError::Unsupported { detail: "缺少 `mcp-server`".into() });
    let io: HubError = std::io::Error::new(std::io::ErrorKind::AddrInUse, "占用").into();
    assert_eq!(io, HubError::Io { detail: "占用".into() });

    let err: HubError = hub::HubError::new(hub::ErrorKind::ToolNotFound, "x").into();
    assert_eq!(
        err,
        HubError::Tool {
            kind: "TOOL_NOT_FOUND".into(),
            reason: "x".into(),
            details_json: None
        }
    );
}

#[test]
fn structured_outcome_conversion() {
    let o: CallOutcome = hub::CallOutcome {
        call_id: "c".into(),
        result: Ok(json!({"orderId": "o1"})),
        state_hints: vec!["cart".into()],
        instance_id: Some("i".into()),
        overview: None,
        status: hub::ResultStatus::Pending,
        state_resource: Some("app-mcp://shop/order.state".into()),
        summary: Some("等待付款".into()),
        annotations: Some(hub::ContentAnnotations {
            audience: Some(vec![hub::Audience::User]),
            priority: Some(0.5),
            last_modified: None,
        }),
        routed_to: Some("shop.cart.addItem".into()),
        duration_ms: 1234,
        woke: true,
        cached_age_ms: Some(1500),
        undo: Some(hub::UndoOffer { label: Some("删除刚添加的待办".into()), expires_in_ms: 1_800_000 }),
        undo_of: Some("c0".into()),
    }
    .into();
    assert_eq!(o.undo, Some(UndoOffer { label: Some("删除刚添加的待办".into()), expires_in_ms: 1_800_000 }));
    assert_eq!(o.undo_of.as_deref(), Some("c0"));
    assert_eq!(o.status, ResultStatus::Pending);
    assert_eq!((o.duration_ms, o.woke, o.cached_age_ms), (1234, true, Some(1500)));
    assert_eq!(o.routed_to.as_deref(), Some("shop.cart.addItem"));
    assert_eq!(o.state_resource.as_deref(), Some("app-mcp://shop/order.state"));
    assert_eq!(o.summary.as_deref(), Some("等待付款"));
    assert_eq!(
        o.annotations,
        Some(ContentAnnotations { audience: Some(vec![Audience::User]), priority: Some(0.5), last_modified: None })
    );
    let d = ToolDeclaration::from(hub::ToolDeclaration {
        name: "order.submit".into(),
        risk: hub::Risk::Write,
        annotations: Some(hub::ToolAnnotations { idempotent_hint: Some(false), ..Default::default() }),
        effective: hub::ToolAnnotations {
            read_only_hint: Some(false),
            idempotent_hint: Some(false),
            ..Default::default()
        },
        output_schema: true,
    });
    assert_eq!(d.risk, Risk::Write);
    assert_eq!(d.annotations.and_then(|a| a.idempotent_hint), Some(false));
    assert_eq!(d.effective.read_only_hint, Some(false));
    assert!(d.output_schema);
}

#[test]
fn limits_config() {
    let d = HubConfig::default().into_hub().unwrap();
    assert_eq!(d.limits, hub::LimitPolicy::default());
    assert_eq!(d.output_validation, hub::OutputValidation::Log);
    let c = HubConfig {
        limits: Some(LimitsConfig {
            tool_rate_per_minute: Some(10),
            tool_rate_burst: Some(2),
            max_result_bytes: Some(0),
            ..Default::default()
        }),
        output_validation: Some(OutputValidation::Reject),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    assert_eq!((c.limits.tool_rate.per_minute, c.limits.tool_rate.burst), (10, 2));
    assert_eq!(c.limits.max_result_bytes, 0);
    assert_eq!(c.limits.app_rate, hub::LimitPolicy::default().app_rate);
    assert_eq!(c.output_validation, hub::OutputValidation::Reject);
    // 限流时 burst 须 ≥ 1；per_minute = 0（不限）时 burst 不检查
    let e = HubConfig {
        limits: Some(LimitsConfig { app_rate_burst: Some(0), ..Default::default() }),
        ..Default::default()
    }
    .into_hub()
    .unwrap_err();
    assert!(matches!(e, HubError::InvalidConfig { .. }), "{e:?}");
    assert!(
        HubConfig {
            limits: Some(LimitsConfig { app_rate_per_minute: Some(0), app_rate_burst: Some(0), ..Default::default() }),
            ..Default::default()
        }
        .into_hub()
        .is_ok()
    );
    // 状态中的完整形式往返
    let full = LimitsConfig::from(hub::LimitOverrides::from_policy(&hub::LimitPolicy::default()));
    assert_eq!(full.tool_rate_per_minute, Some(120));
    assert_eq!(full.max_arguments_bytes, Some(1024 * 1024));
    let mut p = hub::LimitPolicy::unlimited();
    hub::LimitOverrides::from(full).apply(&mut p);
    assert_eq!(p, hub::LimitPolicy::default());
}

#[test]
fn event_limits_config() {
    let d = HubConfig::default().into_hub().unwrap();
    assert_eq!(d.event_limits, hub::EventLimits::default());
    let c = HubConfig {
        event_limits: Some(EventLimitOverrides { max_inbox_events: Some(5), inbox_ttl_ms: Some(1500), ..Default::default() }),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    let expected =
        hub::EventLimits { max_inbox_events: 5, inbox_ttl: Duration::from_millis(1500), ..hub::EventLimits::default() };
    assert_eq!(c.event_limits, expected, "只覆盖给出的字段");
    let c = HubConfig {
        event_limits: Some(EventLimitOverrides {
            max_subscriptions: Some(2),
            max_inbox_events: Some(3),
            inbox_ttl_ms: Some(0),
            per_subscription_per_minute: Some(0),
        }),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    let expected =
        hub::EventLimits { max_subscriptions: 2, max_inbox_events: 3, inbox_ttl: Duration::ZERO, per_subscription_per_minute: 0 };
    assert_eq!(c.event_limits, expected);
}

#[test]
fn result_cache_config_and_status() {
    assert_eq!(HubConfig::default().into_hub().unwrap().result_cache, hub::CacheLimits::default());
    let c = HubConfig {
        result_cache: Some(CacheLimitOverrides { max_entries: Some(0), ..Default::default() }),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    assert_eq!(c.result_cache, hub::CacheLimits { max_entries: 0, ..hub::CacheLimits::default() }, "只覆盖给出的字段");
    let c = HubConfig {
        result_cache: Some(CacheLimitOverrides { max_entries: Some(3), max_bytes: Some(4096), max_entry_bytes: Some(512) }),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    assert_eq!(c.result_cache, hub::CacheLimits { max_entries: 3, max_bytes: 4096, max_entry_bytes: 512 });
    let limits = Some(hub::CacheLimits { max_entries: 7, max_bytes: 4096, max_entry_bytes: 512 });
    let s = CacheStatus::from(hub::CacheStatus { entries: 2, bytes: 300, hits: 5, misses: 4, evictions: 1, limits });
    let expected = CacheStatus {
        entries: 2,
        bytes: 300,
        hits: 5,
        misses: 4,
        evictions: 1,
        max_entries: 7,
        max_bytes: 4096,
        max_entry_bytes: 512,
    };
    assert_eq!(s, expected, "状态带生效上限");
}

#[test]
fn schema_change_record_converts() {
    let r: hub::SchemaChangeRecord = serde_json::from_value(serde_json::json!({
        "appId": "chat", "tool": "send", "level": "breaking", "at": 9,
        "changes": [{"level": "warning", "path": "/outputSchema", "message": "m"}, {"level": "breaking", "path": "/inputSchema", "message": "n"}]
    }))
    .unwrap();
    let r = SchemaChangeRecord::from(r);
    assert_eq!((r.app_id.as_str(), r.tool.as_str(), r.level, r.at), ("chat", "send", ChangeLevel::Breaking, 9));
    assert_eq!(r.changes[0], SchemaChange { level: ChangeLevel::Warning, path: "/outputSchema".into(), message: "m".into() });
}

#[test]
fn filter_and_event_conversion() {
    let f: hub::ToolFilter = ToolFilter {
        max_risk: Some(Risk::Write),
        ..Default::default()
    }
    .into();
    assert_eq!(f.max_risk, Some(hub::Risk::Write));
    assert!(f.include_builtin);
    assert_eq!(f.session, None);
    let f: hub::ToolFilter = ToolFilter { session: Some("s".into()), ..Default::default() }.into();
    assert_eq!(f.session.as_deref(), Some("s"));
    let e: HubEvent = hub::HubEvent::VisibilityChanged {
        app_id: "a".into(),
        instance_id: "i".into(),
        visibility: hub::Visibility::Hidden,
    }
    .into();
    assert_eq!(
        e,
        HubEvent::VisibilityChanged {
            app_id: "a".into(),
            instance_id: "i".into(),
            visibility: Visibility::Hidden
        }
    );
    assert_eq!(ToolFormat::from(hub::ToolFormat::Gemini), ToolFormat::Gemini);
}

#[test]
fn lifecycle_mapping() {
    let c = HubConfig {
        lease_ttl_ms: Some(0),
        wake_timeout_ms: Some(2000),
        wake_token_ttl_ms: Some(3000),
        dormant_ttl_ms: Some(4000),
        dormant_replaced_by_new_instance: Some(false),
        wake_from_launch: Some(true),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    assert_eq!(c.lease_ttl, Duration::ZERO);
    assert_eq!(c.wake_timeout, Duration::from_millis(2000));
    assert_eq!(c.wake_token_ttl, Duration::from_millis(3000));
    assert_eq!(c.dormant_ttl, Duration::from_millis(4000));
    assert!(!c.dormant_replaced_by_new_instance && c.wake_from_launch);
    let d = HubConfig::default().into_hub().unwrap();
    assert_eq!(d.lease_ttl, hub::HubConfig::default().lease_ttl);
    assert!(d.dormant_replaced_by_new_instance && !d.wake_from_launch);
    assert_eq!(d.waker, hub::WakerConfig::System);
    assert_eq!(d.tool_exposure, hub::ToolExposure::Auto);
    assert_eq!(d.tool_exposure_threshold, hub::DEFAULT_TOOL_EXPOSURE_THRESHOLD);
    assert_eq!(d.channel_grace, hub::DEFAULT_CHANNEL_GRACE);
    let g = HubConfig { channel_grace_ms: Some(250), ..Default::default() }.into_hub().unwrap();
    assert_eq!(g.channel_grace, Duration::from_millis(250));
    let c = HubConfig {
        waker: Some(WakerConfig::Disabled),
        tool_exposure: Some(ToolExposure::Progressive),
        tool_exposure_threshold: Some(5),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    assert_eq!(c.waker, hub::WakerConfig::None);
    assert_eq!(c.tool_exposure, hub::ToolExposure::Progressive);
    assert_eq!(c.tool_exposure_threshold, 5);
    // 功耗（4e）
    assert_eq!((d.wake_rate_limit, d.legacy_heartbeat), (hub::DEFAULT_WAKE_RATE_LIMIT, false));
    assert_eq!(d.lease, hub::LeasePolicy::default());
    let c = HubConfig {
        wake_rate_limit: Some(0),
        legacy_heartbeat: Some(true),
        lease: Some(LeaseConfig { adaptive: Some(false), window: Some(4), max_ms: Some(30_000), ..Default::default() }),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    assert_eq!((c.wake_rate_limit, c.legacy_heartbeat), (0, true));
    assert_eq!((c.lease.adaptive, c.lease.window, c.lease.max), (false, 4, Duration::from_secs(30)));
    let e = HubConfig { lease: Some(LeaseConfig { window: Some(0), ..Default::default() }), ..Default::default() }
        .into_hub()
        .unwrap_err();
    assert!(matches!(e, HubError::InvalidConfig { .. }), "{e:?}");
    let c = HubConfig {
        waker: Some(WakerConfig::Exec { argv: vec!["node".into(), "w.mjs".into()] }),
        ..Default::default()
    }
    .into_hub()
    .unwrap();
    assert_eq!(c.waker, hub::WakerConfig::Exec(vec!["node".into(), "w.mjs".into()]));

    assert_eq!(Availability::from(hub::Availability::Dormant), Availability::Dormant);
    assert_eq!(
        HubEvent::from(hub::HubEvent::AppDormant { app_id: "a".into(), instance_id: "i".into() }),
        HubEvent::AppDormant { app_id: "a".into(), instance_id: "i".into() }
    );
    assert_eq!(
        HubEvent::from(hub::HubEvent::AppWaking { app_id: "a".into(), instance_id: None }),
        HubEvent::AppWaking { app_id: "a".into(), instance_id: None }
    );
    let e = wake_error("APP_NOT_INSTALLED", "没装".into());
    assert_eq!((e.kind(), e.message()), (hub::ErrorKind::AppNotInstalled, "没装"));
    let e = wake_error("NOPE", String::new());
    assert_eq!(e.kind(), hub::ErrorKind::LaunchFailed);
    let r = WakeRequest::from(hub::WakeRequest {
        app_id: "a".into(),
        instance_id: Some("i".into()),
        descriptor: hub::WakeDescriptor {
            kind: hub::WakeKind::AndroidIntent,
            target: Some("p/.R".into()),
            background: true,
        },
        token: "t".into(),
        activation_arg: "app-mcp-wake:t".into(),
    });
    assert_eq!(r.descriptor.kind, WakeKind::AndroidIntent);
    assert_eq!(r.descriptor.target.as_deref(), Some("p/.R"));
}

#[test]
fn diagnostic_mapping() {
    assert_eq!(
        HubEvent::from(hub::HubEvent::AppDiagnostic {
            app_id: "a".into(),
            instance_id: "i".into(),
            code: "BLOCKED_LOCAL_NETWORK_ACCESS".into(),
            message: "m".into(),
            count: 3,
        }),
        HubEvent::AppDiagnostic {
            app_id: "a".into(),
            instance_id: "i".into(),
            code: "BLOCKED_LOCAL_NETWORK_ACCESS".into(),
            message: "m".into(),
            count: 3,
        }
    );
    // 与 `GET /status` 同构：从 JSON 解析 Hub 的 HubStatus 再转换，覆盖每个字段。
    let st: hub::HubStatus = serde_json::from_value(serde_json::json!({
        "service": "app-mcp", "version": "9.9.9", "user": "u", "pid": 42,
        "listen": "127.0.0.1:7717", "ipcEndpoint": "unix:/x.sock", "startedAtMs": 5,
        "mcpHttp": true, "auth": {"tokenConfigured": true, "tokenRequiredWithoutOrigin": false},
        "mcpSessions": 2, "mcpListenStreams": 3,
        "apps": [
            {"appId": "a", "name": "A", "kind": "app", "state": "dormant",
             "instances": [
                {"instanceId": "i1", "clientKind": "native", "visibility": "visible", "focused": true,
                 "lastActiveMs": 7, "title": null, "pid": 9, "connectionId": "abc123-1", "state": "connected"},
                {"instanceId": "i2", "clientKind": "web", "visibility": "hidden", "focused": false,
                 "lastActiveMs": 8, "title": "t", "state": "waking"}],
             "lastError": {"code": "APP_NOT_RESPONDING", "message": "超时", "atMs": 11}},
            {"appId": "u", "name": "U", "kind": "upstream", "state": "disconnected", "instances": []}
        ],
        "reports": [{"appId": "a", "instanceId": "i1", "connectionId": "abc123-1",
                     "code": "BLOCKED_MIXED_CONTENT", "message": "m", "count": 1, "receivedAtMs": 12}],
        "dormantStore": {"dir": "/s/dormant", "loadedInstances": 3, "expiredInstances": 1, "writes": 4,
                         "issues": [{"file": "x.json", "reason": "损坏"}], "lastError": "磁盘满"},
        "tasks": [
            {"id": "task-1", "caller": "mcp:1", "kind": "mcpSession", "selections": [], "leases": [], "inflight": 0},
            {"id": "task-2", "caller": "principal:local", "kind": "principal",
             "selections": [{"appId": "a", "instanceId": "i1", "expiresInMs": 900}],
             "leases": [{"connectionId": "abc123-1", "expiresInMs": 500}], "inflight": 2, "idleMs": 3},
            {"id": "task-3", "caller": "api", "kind": "api",
             "selections": [{"appId": "a", "instanceId": "i2"}], "leases": [], "inflight": 0}
        ]
    }))
    .unwrap();
    let s = HubStatus::from(st);
    assert_eq!((s.service.as_str(), s.version.as_str(), s.user.as_deref(), s.pid), ("app-mcp", "9.9.9", Some("u"), 42));
    assert_eq!(s.listen.as_deref(), Some("127.0.0.1:7717"));
    assert_eq!(s.ipc_endpoint.as_deref(), Some("unix:/x.sock"));
    assert_eq!((s.started_at_ms, s.mcp_http, s.mcp_sessions, s.mcp_listen_streams), (5, true, 2, Some(3)));
    assert_eq!(
        s.auth,
        AuthStatus {
            token_configured: true,
            token_required_without_origin: false
        }
    );
    let a = &s.apps[0];
    assert_eq!((a.kind, a.state), (AppKind::App, AppState::Dormant));
    assert_eq!(a.instances[0].state, InstanceState::Connected);
    assert_eq!(a.instances[0].info.connection_id.as_deref(), Some("abc123-1"));
    assert_eq!(a.instances[0].info.pid, Some(9));
    assert_eq!(a.instances[1].state, InstanceState::Waking);
    assert_eq!(a.instances[1].info.connection_id, None);
    assert_eq!(
        a.last_error,
        Some(LastError {
            code: Some("APP_NOT_RESPONDING".into()),
            message: "超时".into(),
            at_ms: 11
        })
    );
    assert_eq!((s.apps[1].kind, s.apps[1].state, &s.apps[1].last_error), (AppKind::Upstream, AppState::Disconnected, &None));
    assert_eq!(
        s.reports,
        vec![DiagnosticReport {
            app_id: "a".into(),
            instance_id: "i1".into(),
            connection_id: "abc123-1".into(),
            code: "BLOCKED_MIXED_CONTENT".into(),
            message: "m".into(),
            count: 1,
            received_at_ms: 12
        }]
    );
    assert_eq!(
        s.dormant_store,
        Some(DormantStoreStatus {
            dir: "/s/dormant".into(),
            loaded_instances: 3,
            expired_instances: 1,
            writes: 4,
            issues: vec![StoreIssue { file: "x.json".into(), reason: "损坏".into() }],
            last_error: Some("磁盘满".into()),
        })
    );
    let tasks = s.tasks.expect("tasks");
    assert_eq!(
        tasks.iter().map(|t| (t.id.as_str(), t.caller.as_str(), t.kind)).collect::<Vec<_>>(),
        vec![("task-1", "mcp:1", CallerKind::McpSession), ("task-2", "principal:local", CallerKind::Principal), ("task-3", "api", CallerKind::Api)]
    );
    assert_eq!((tasks[0].inflight, tasks[0].idle_ms), (0, None));
    assert_eq!(
        tasks[1],
        AgentTaskStatus {
            id: "task-2".into(),
            caller: "principal:local".into(),
            kind: CallerKind::Principal,
            selections: vec![TaskSelectionStatus { app_id: "a".into(), instance_id: "i1".into(), expires_in_ms: Some(900) }],
            leases: vec![TaskLeaseStatus { connection_id: "abc123-1".into(), expires_in_ms: 500 }],
            inflight: 2,
            idle_ms: Some(3),
            agent: None,
        }
    );
    assert_eq!(tasks[2].selections[0].expires_in_ms, None);
    assert_eq!(InstanceState::from(hub::InstanceState::Dormant), InstanceState::Dormant);
    assert_eq!(AppState::from(hub::AppState::Waking), AppState::Waking);
    assert_eq!(AppState::from(hub::AppState::Connected), AppState::Connected);
}

#[test]
fn old_host_status_has_no_listen_streams() {
    let st: hub::HubStatus = serde_json::from_value(serde_json::json!({
        "service": "app-mcp", "version": "0", "pid": 1, "startedAtMs": 0, "mcpHttp": true,
        "auth": {"tokenConfigured": false, "tokenRequiredWithoutOrigin": false}, "mcpSessions": 1,
        "apps": [], "reports": []
    }))
    .unwrap();
    let s = HubStatus::from(st);
    assert_eq!((s.mcp_sessions, s.mcp_listen_streams, s.tasks), (1, None, None));
    assert_eq!((s.usage, s.locks), (None, None));
}

#[test]
fn approval_request_carries_principal_and_client_name() {
    let base = serde_json::json!({
        "callId": "c", "appId": "a", "appName": "A", "tool": "t", "title": null, "description": "d",
        "risk": "write", "arguments": {}, "session": "principal:local"
    });
    let mut mcp = base.clone();
    mcp["principal"] = "local".into();
    mcp["clientName"] = "claude-code".into();
    let r = ApprovalRequest::from(serde_json::from_value::<hub::ApprovalRequest>(mcp).unwrap());
    assert_eq!((r.principal.as_deref(), r.client_name.as_deref()), (Some("local"), Some("claude-code")));
    let r = ApprovalRequest::from(serde_json::from_value::<hub::ApprovalRequest>(base).unwrap());
    assert_eq!((r.principal, r.client_name), (None, None));
}

#[test]
fn other_event_fallback_keeps_type_and_json() {
    let e = other_event(&hub::HubEvent::ResourceUpdated { uri: "u://x".into() });
    let HubEvent::Other { kind, json } = e else {
        panic!("应为 Other");
    };
    assert_eq!(kind, "resourceUpdated");
    let v: Value = serde_json::from_str(&json).expect("json");
    assert_eq!(v["uri"], "u://x");
}

/// 第 16 项 N5 / P3：`agents`、`tasks[].agent` 与 `usage` 原样转换；旧 Host 不报告时为空。
#[test]
fn status_agents_and_usage() {
    let st: hub::HubStatus = serde_json::from_value(serde_json::json!({
        "service": "app-mcp", "version": "0", "pid": 1, "startedAtMs": 0, "mcpHttp": true,
        "auth": {"tokenConfigured": false, "tokenRequiredWithoutOrigin": false}, "mcpSessions": 0,
        "apps": [], "reports": [], "agents": ["claude"],
        "tasks": [{"id": "task-1", "caller": "principal:agent:claude", "kind": "principal", "agent": "claude",
                   "selections": [], "leases": [], "inflight": 0}],
        "usage": [{"subject": "agent:claude", "agent": "claude", "calls": 3, "wakes": 1, "rateLimited": 2,
                   "argumentsBytes": 10, "resultBytes": 20, "appsTruncated": true,
                   "apps": [{"appId": "shop", "calls": 3, "wakes": 1, "rateLimited": 2, "argumentsBytes": 10, "resultBytes": 20}]}]
    }))
    .unwrap();
    let s = HubStatus::from(st);
    assert_eq!(s.agents, Some(vec!["claude".to_owned()]));
    assert_eq!(s.tasks.unwrap()[0].agent.as_deref(), Some("claude"));
    let u = &s.usage.unwrap()[0];
    let counts = UsageCounts { calls: 3, wakes: 1, rate_limited: 2, arguments_bytes: 10, result_bytes: 20 };
    assert_eq!((u.subject.as_str(), u.agent.as_deref(), u.total, u.apps_truncated), ("agent:claude", Some("claude"), counts, true));
    assert_eq!(u.apps, vec![AppUsageStatus { app_id: "shop".into(), counts }]);
}

/// 第 16 项 N6：`locks` 原样转换（App 锁无 key）。
#[test]
fn status_locks() {
    let st: hub::HubStatus = serde_json::from_value(serde_json::json!({
        "service": "app-mcp", "version": "0", "pid": 1, "startedAtMs": 0, "mcpHttp": true,
        "auth": {"tokenConfigured": false, "tokenRequiredWithoutOrigin": false}, "mcpSessions": 0,
        "apps": [], "reports": [],
        "locks": [{"appId": "shop", "caller": "api:s1", "holder": "api", "expiresInMs": 1500},
                  {"appId": "shop", "key": "doc-1", "caller": "principal:agent:claude", "holder": "agent:claude", "expiresInMs": 9}]
    }))
    .unwrap();
    let locks = HubStatus::from(st).locks.unwrap();
    assert_eq!(
        locks,
        vec![
            LockStatus { app_id: "shop".into(), key: None, caller: "api:s1".into(), holder: "api".into(), expires_in_ms: 1500 },
            LockStatus {
                app_id: "shop".into(),
                key: Some("doc-1".into()),
                caller: "principal:agent:claude".into(),
                holder: "agent:claude".into(),
                expires_in_ms: 9,
            },
        ]
    );
}

#[test]
fn call_status_conversion() {
    let c: hub::call_objects::CallStatus = serde_json::from_value(json!({
        "callId": "c1", "name": "shop.job.run", "caller": "api", "subject": "api", "state": "approving",
        "elapsedMs": 12, "instanceId": "i1", "progress": {"progress": 2.0, "total": 4.0, "message": "半"},
        "platformState": "hidden"
    }))
    .unwrap();
    let u = CallStatus::from(c);
    assert_eq!((u.state, u.elapsed_ms, u.instance_id.as_deref()), (CallState::Approving, 12, Some("i1")));
    assert_eq!((u.progress, u.progress_total, u.progress_message.as_deref()), (Some(2.0), Some(4.0), Some("半")));
    assert_eq!(u.platform_state, Some(Visibility::Hidden));
}
