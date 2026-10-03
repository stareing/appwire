//! app-mcp-uniffi 单元测试。

use std::sync::Arc;

use app_mcp_native as native;

use super::*;
use crate::callbacks::guarded;
use crate::error::parse_error_kind;

#[test]
fn error_kinds_roundtrip() {
    let kinds = error_kinds();
    assert_eq!(kinds.len(), 22);
    assert!(kinds.contains(&"LOCKED".to_owned()));
    assert!(kinds.contains(&"NAVIGATION_FAILED".to_owned()) && kinds.contains(&"NAVIGATION_DENIED".to_owned()));
    assert!(kinds.contains(&"RATE_LIMITED".to_owned()));
    assert!(kinds.contains(&"PAYLOAD_TOO_LARGE".to_owned()));
    assert!(kinds.contains(&"POLICY_DENIED".to_owned()));
    assert!(kinds.contains(&"USER_ACTION_REQUIRED".to_owned()));
    for k in &kinds {
        assert_eq!(parse_error_kind(k).map(|e| e.as_str()), Ok(k.as_str()));
    }
    assert!(kinds.contains(&"HANDLER_ERROR".to_owned()));
    assert_eq!(
        parse_error_kind("NOPE"),
        Err(AppMcpError::UnknownErrorKind {
            kind: "NOPE".into()
        })
    );
}

#[test]
fn config_defaults_follow_native() {
    let cfg = ClientConfig {
        app_id: "shop".into(),
        app_name: "Shop".into(),
        instance_id: None,
        client_kind: None,
        host_url: None,
        app_version: None,
        instance_title: None,
        token: None,
        launch_token: None,
        max_concurrent_calls: 1,
        overview: None,
        lifecycle: None,
        connect_timeout_ms: None,
        heartbeat: None,
        call_dedup: None,
        register_name: false,
        name_instance: None,
        max_queued_calls: None,
        busy_policy: None,
    };
    let n: native::NativeConfig = cfg.clone().into();
    assert_eq!(n, native::NativeConfig::new("shop", "Shop"));
    let d = CallDedupPolicy { ttl_ms: 300_000, max_entries: 64 };
    assert_eq!(native::CallDedupPolicy::from(d), native::CallDedupPolicy::default(), "记录默认值与原生一致");
    let n: native::NativeConfig =
        ClientConfig { call_dedup: Some(CallDedupPolicy { ttl_ms: 0, max_entries: 8 }), ..cfg.clone() }.into();
    assert_eq!(n.call_dedup, native::CallDedupPolicy { ttl_ms: 0, max_entries: 8 });
    assert!(!n.call_dedup.enabled());

    let n: native::NativeConfig = ClientConfig {
        client_kind: Some(ClientKind::Hybrid),
        host_url: Some("ws://127.0.0.1:1".into()),
        max_concurrent_calls: 4,
        overview: Some(AppOverview {
            summary: "网店".into(),
            body: None,
            locale: Some("zh-CN".into()),
        }),
        ..cfg
    }
    .into();
    assert_eq!(
        n.overview.as_ref().map(|o| o.summary.as_str()),
        Some("网店")
    );
    assert_eq!(n.client_kind, native::ClientKind::Hybrid);
    assert_eq!(n.host_url, "ws://127.0.0.1:1");
    assert_eq!(n.max_concurrent_calls, 4);
}

#[test]
fn register_name_passes_through_and_instance_is_validated() {
    let cfg = ClientConfig {
        app_id: "shop".into(),
        app_name: "Shop".into(),
        instance_id: None,
        client_kind: None,
        host_url: None,
        app_version: None,
        instance_title: None,
        token: None,
        launch_token: None,
        max_concurrent_calls: 1,
        overview: None,
        lifecycle: None,
        connect_timeout_ms: None,
        heartbeat: None,
        call_dedup: None,
        register_name: true,
        name_instance: Some("w2".into()),
        max_queued_calls: None,
        busy_policy: None,
    };
    let n: native::NativeConfig = cfg.clone().into();
    assert!(n.register_name);
    assert_eq!(n.name_instance.as_deref(), Some("w2"));
    assert_eq!(n.name_service_address, None, "名字服务地址按环境取");

    for bad in ["default", "W2", "2w", ""] {
        let err = AppMcpClient::new(ClientConfig { name_instance: Some(bad.into()), ..cfg.clone() }, None)
            .err()
            .unwrap_or_else(|| panic!("实例名 {bad:?} 应被拒绝"));
        assert!(matches!(err, AppMcpError::InvalidConfig { .. }), "{bad:?}：{err:?}");
    }
}

#[test]
fn tool_spec_conversion() {
    let spec = ToolSpec {
        name: "cart.add".into(),
        description: "加入购物车".into(),
        input_schema_json: Some(r#"{"type":"object"}"#.into()),
        risk: None,
        activation: Some(Activation::Background),
        title: None,
        enabled: false,
        annotations: None,
        output_schema_json: None,
        surface: None,
        page: None,
        background_tool: None,
        concurrency: 0,
        exclusive: None,
    };
    let (n, options): (native::ToolSpec, native::ToolOptions) = spec.clone().into();
    assert_eq!(n.risk, native::Risk::Write);
    assert_eq!(n.activation, Some(native::Activation::Background));
    assert!(!n.enabled);
    assert_eq!(options, native::ToolOptions::default());
    let (n, options): (native::ToolSpec, native::ToolOptions) = ToolSpec {
        risk: Some(Risk::OsSensitive),
        annotations: Some(ToolAnnotations {
            read_only_hint: Some(false),
            idempotent_hint: Some(true),
            ..ToolAnnotations::default()
        }),
        output_schema_json: Some(r#"{"type":"object"}"#.into()),
        surface: Some(ToolSurface::View),
        page: Some("cart".into()),
        background_tool: Some("cart.add_bg".into()),
        ..spec
    }
    .into();
    assert_eq!(n.risk, native::Risk::OsSensitive);
    assert_eq!((options.surface, options.page.as_deref()), (native::ToolSurface::View, Some("cart")));
    assert_eq!(options.background_tool.as_deref(), Some("cart.add_bg"));
    let a = options.annotations.expect("annotations");
    assert_eq!((a.read_only_hint, a.idempotent_hint, a.destructive_hint), (Some(false), Some(true), None));
    assert_eq!(options.output_schema_json.as_deref(), Some(r#"{"type":"object"}"#));
}

#[test]
fn call_result_conversion() {
    let r: native::CallResult = CallResult {
        data_json: Some(r#"{"id":1}"#.into()),
        state_hints: vec!["cart".into()],
        status: ResultStatus::Pending,
        state_resource: Some("order.status".into()),
        summary: Some("等待确认".into()),
        annotations: Some(ContentAnnotations {
            audience: Some(vec![Audience::User]),
            priority: Some(0.5),
            last_modified: None,
        }),
    }
    .into();
    assert_eq!(r.status, native::ResultStatus::Pending);
    assert_eq!(r.state_resource.as_deref(), Some("order.status"));
    assert_eq!(r.summary.as_deref(), Some("等待确认"));
    assert_eq!(r.state_hints, vec!["cart".to_owned()]);
    let a = r.annotations.expect("annotations");
    assert_eq!(a.audience, Some(vec![native::Audience::User]));
    assert_eq!(a.priority, Some(0.5));
}

#[test]
fn native_error_mapping() {
    assert_eq!(
        AppMcpError::from(native::NativeError::DuplicateName("a".into())),
        AppMcpError::DuplicateName { name: "a".into() }
    );
    assert_eq!(
        AppMcpError::from(native::NativeError::Stopped),
        AppMcpError::Stopped
    );
}

#[test]
fn state_conversion() {
    let s = StateInfo::from(native::StateInfo {
        status: native::StateStatus::Backoff,
        retry_in_ms: Some(500),
        reason: Some("r".into()),
        code: Some("HOST_NOT_RUNNING".into()),
    });
    assert_eq!(s.status, StateStatus::Backoff);
    assert_eq!(s.code.as_deref(), Some("HOST_NOT_RUNNING"));
    assert_eq!(s.retry_in_ms, Some(500));
}

#[test]
fn lifecycle_conversion() {
    let p = LifecyclePolicy {
        mode: None,
        idle_timeout_ms: 60_000,
        hidden_idle_timeout_ms: 15_000,
        grace_ms: 10_000,
        residency: None,
        wake: None,
        host_absent_retries: 3,
        legacy_timers: false,
        merge_window_ms: 2_000,
        sleep_on_background: false,
    };
    assert_eq!(native::LifecyclePolicy::from(p.clone()), native::LifecyclePolicy::default());
    let n: native::LifecyclePolicy = LifecyclePolicy { host_absent_retries: 0, legacy_timers: true, ..p.clone() }.into();
    assert_eq!((n.host_absent_retries, n.legacy_timers), (0, true));
    let n: native::LifecyclePolicy =
        LifecyclePolicy { merge_window_ms: 500, sleep_on_background: true, ..p.clone() }.into();
    assert_eq!((n.merge_window_ms, n.sleep_on_background), (500, true));
    let (_, options): (native::ResourceSpec, native::ResourceOptions) =
        ResourceSpec { name: "r".into(), description: "d".into(), mime_type: None, realtime: true, annotations: None }
            .into();
    assert!(options.realtime);
    assert_eq!(options.annotations, None);
    let (_, options): (native::ResourceSpec, native::ResourceOptions) = ResourceSpec {
        name: "r".into(),
        description: "d".into(),
        mime_type: None,
        realtime: false,
        annotations: Some(ContentAnnotations {
            audience: Some(vec![Audience::Assistant]),
            priority: Some(0.2),
            last_modified: Some("2026-10-02T00:00:00Z".into()),
        }),
    }
    .into();
    let a = options.annotations.expect("annotations");
    assert_eq!(a.audience, Some(vec![native::Audience::Assistant]));
    assert_eq!((a.priority, a.last_modified.as_deref()), (Some(0.2), Some("2026-10-02T00:00:00Z")));
    let n: native::LifecyclePolicy = LifecyclePolicy {
        mode: Some(LifecycleMode::Idle),
        hidden_idle_timeout_ms: 0,
        residency: Some(Residency::ExitWhenIdle),
        wake: Some(WakeDescriptor {
            kind: WakeKind::AndroidIntent,
            target: Some("pkg/dev.appmcp.android.WakeReceiver".into()),
            background: true,
        }),
        ..p
    }
    .into();
    assert_eq!(n.mode, native::LifecycleMode::Idle);
    assert_eq!(n.hidden_idle_timeout_ms, 0);
    assert_eq!(n.residency, native::Residency::ExitWhenIdle);
    let wake = n.wake.expect("wake");
    assert_eq!(wake.kind, native::WakeKind::AndroidIntent);
    assert!(wake.background);
}

#[test]
fn wake_token_parsing() {
    assert_eq!(parse_wake_token("app-mcp-wake:abc".into()), Some("abc".into()));
    assert_eq!(
        parse_wake_token("shop://app-mcp/wake?token=x1".into()),
        Some("x1".into())
    );
    assert_eq!(parse_wake_token("--foo".into()), None);
}

#[test]
fn client_lifecycle_api() {
    let cfg = ClientConfig {
        app_id: "shop".into(),
        app_name: "Shop".into(),
        instance_id: None,
        client_kind: None,
        host_url: Some("ws://127.0.0.1:9".into()),
        app_version: None,
        instance_title: None,
        token: None,
        launch_token: None,
        max_concurrent_calls: 1,
        overview: None,
        lifecycle: Some(LifecyclePolicy {
            mode: Some(LifecycleMode::OnDemand),
            idle_timeout_ms: 60_000,
            hidden_idle_timeout_ms: 15_000,
            grace_ms: 10_000,
            residency: None,
            wake: None,
            host_absent_retries: 3,
            legacy_timers: false,
            merge_window_ms: 2_000,
            sleep_on_background: false,
        }),
        connect_timeout_ms: Some(1000),
        heartbeat: Some(HeartbeatMode::Off),
        call_dedup: Some(CallDedupPolicy { ttl_ms: 1_000, max_entries: 4 }),
        register_name: false,
        name_instance: None,
        max_queued_calls: None,
        busy_policy: None,
    };
    let client = AppMcpClient::new(cfg, None).expect("client");
    assert!(!client.handle_wake("not-a-wake".into()));
    assert_eq!(client.tools_hash().len(), 16);
    let hold = client.hold();
    hold.release();
    hold.release();
    client.stop();
}

/// `session`：USER_ACTION_REQUIRED（reason / uri）；`quota`：带详情失败（非法详情不消费 read，可重试）。
struct FailingReader;

impl ResourceReader for FailingReader {
    fn read(&self, read: Arc<Read>) {
        if read.resource_name() == "session" {
            let _ = read.fail_user_action(
                "登录已过期，请在 App 内重新登录".into(),
                Some("login".into()),
                Some("shop://login".into()),
            );
            return;
        }
        let bad = read.fail_with_details("USER_REJECTED".into(), "额度不足".into(), Some("{bad".into()));
        assert!(matches!(bad, Err(AppMcpError::InvalidJson { .. })), "{bad:?}");
        let unknown = read.fail_with_details("NOPE".into(), "x".into(), None);
        assert!(matches!(unknown, Err(AppMcpError::UnknownErrorKind { .. })), "{unknown:?}");
        let _ = read.fail_with_details("USER_REJECTED".into(), "额度不足".into(), Some(r#"{"quota":0}"#.into()));
    }
}

/// 需要用户操作的工具：`login` 带 reason / uri，`front` 只带说明。
struct UserActionTool;

impl ToolHandler for UserActionTool {
    fn invoke(&self, call: Arc<Call>) {
        let (reason, uri) = if call.tool_name() == "login" {
            (Some("login".to_owned()), Some("shop://login".to_owned()))
        } else {
            (None, None)
        };
        let _ = call.fail_user_action("请处理后重试".into(), reason, uri);
    }
}

/// 端到端：资源读取失败的详情与 USER_ACTION_REQUIRED（reason / uri）经 uniffi 层到达 Host（fake_host）。
#[test]
fn read_and_call_failures_reach_host() {
    let (mut child, lines, addr) = spawn_fake_host(&[
        "--invoke", "login", "--invoke", "front", "--read", "session", "--read", "quota", "--timeout-ms", "8000",
    ]);

    let client = AppMcpClient::new(fake_host_config("uniffi-read", &addr), None).expect("client");
    let mut keep = Vec::new();
    for n in ["login", "front"] {
        let spec = ToolSpec {
            name: n.into(),
            description: "d".into(),
            input_schema_json: None,
            risk: Some(Risk::Read),
            activation: None,
            title: None,
            enabled: true,
            annotations: None,
            output_schema_json: None,
            surface: None,
            page: None,
            background_tool: None,
            concurrency: 0,
            exclusive: None,
        };
        keep.push(client.register_tool(spec, Arc::new(UserActionTool)).expect("tool"));
    }
    let mut res = Vec::new();
    for n in ["session", "quota"] {
        let spec = ResourceSpec {
            name: n.into(),
            description: "d".into(),
            mime_type: None,
            realtime: false,
            annotations: Some(ContentAnnotations { priority: Some(1.0), ..ContentAnnotations::default() }),
        };
        res.push(client.register_resource(spec, Arc::new(FailingReader)).expect("resource"));
    }
    client.start();
    let out: Vec<serde_json::Value> =
        lines.map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
    let ok = child.wait().is_ok_and(|s| s.success());
    client.stop();
    assert!(ok, "fake_host 退出码非 0：{out:?}");
    let ua = serde_json::json!({
        "code": -32019, "message": "请处理后重试",
        "data": { "kind": "USER_ACTION_REQUIRED", "reason": "login", "uri": "shop://login" }
    });
    assert_eq!(out[1]["error"], ua);
    assert_eq!(out[2]["error"]["data"], serde_json::json!({ "kind": "USER_ACTION_REQUIRED" }));
    assert_eq!(
        out[3]["error"],
        serde_json::json!({
            "code": -32019, "message": "登录已过期，请在 App 内重新登录",
            "data": { "kind": "USER_ACTION_REQUIRED", "reason": "login", "uri": "shop://login" }
        })
    );
    assert_eq!(out[4]["error"]["data"], serde_json::json!({ "kind": "USER_REJECTED", "quota": 0 }));
}

/// 返回收到的幂等键：`{"key": <键或 null>}`。
struct KeyTool;

impl ToolHandler for KeyTool {
    fn invoke(&self, call: Arc<Call>) {
        let data = serde_json::json!({ "key": call.idempotency_key() }).to_string();
        let _ = call.complete(Some(data), Vec::new());
    }
}

/// 端到端：`Call::idempotency_key` 原样给出 Host 的 `idempotencyKey`；没有时为 None。
#[test]
fn idempotency_key_reaches_handler() {
    let (mut child, lines, addr) = spawn_fake_host(&[
        "--invoke", "k.key", "--idempotency-key", "Agent 键 / 1", "--invoke", "k.key", "--timeout-ms", "8000",
    ]);
    let client = AppMcpClient::new(fake_host_config("uniffi-key", &addr), None).expect("client");
    let spec = ToolSpec {
        name: "k.key".into(),
        description: "d".into(),
        input_schema_json: None,
        risk: Some(Risk::Read),
        activation: None,
        title: None,
        enabled: true,
        annotations: None,
        output_schema_json: None,
        surface: None,
        page: None,
        background_tool: None,
        concurrency: 0,
        exclusive: None,
    };
    let _tool = client.register_tool(spec, Arc::new(KeyTool)).expect("tool");
    client.start();
    let out: Vec<serde_json::Value> =
        lines.map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
    let ok = child.wait().is_ok_and(|s| s.success());
    client.stop();
    assert!(ok, "fake_host 退出码非 0：{out:?}");
    assert_eq!(out[1]["result"], serde_json::json!({ "data": { "key": "Agent 键 / 1" } }));
    assert_eq!(out[2]["result"], serde_json::json!({ "data": { "key": null } }));
}

/// 连接 fake_host（`addr`）的最小配置。
fn fake_host_config(app_id: &str, addr: &str) -> ClientConfig {
    ClientConfig {
        app_id: app_id.into(),
        app_name: app_id.into(),
        instance_id: None,
        client_kind: None,
        host_url: Some(format!("ws://{addr}")),
        app_version: None,
        instance_title: None,
        token: None,
        launch_token: None,
        max_concurrent_calls: 1,
        overview: None,
        lifecycle: None,
        connect_timeout_ms: None,
        heartbeat: None,
        call_dedup: None,
        register_name: false,
        name_instance: None,
        max_queued_calls: None,
        busy_policy: None,
    }
}

/// 启动 fake_host，返回进程、其 stdout 行迭代器与监听地址。
fn spawn_fake_host(
    args: &[&str],
) -> (std::process::Child, std::io::Lines<std::io::BufReader<std::process::ChildStdout>>, String) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let bin = native::test_support::fake_host_path().unwrap_or_else(|e| panic!("{e}"));
    let mut child = Command::new(bin).args(args).stdout(Stdio::piped()).spawn().expect("启动 fake_host");
    let stdout = child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let first = lines.next().and_then(Result::ok).unwrap_or_default();
    let addr = first.strip_prefix("LISTENING ").unwrap_or_default().to_owned();
    assert!(!addr.is_empty(), "LISTENING 行：{first}");
    (child, lines, addr)
}

/// 后台导航：`front` 页发通知后以 USER_ACTION_REQUIRED（foreground + uri）回复，其他页正常完成。
struct BackgroundNavigator;

impl NavigationHandler for BackgroundNavigator {
    fn navigate(&self, request: Arc<Navigate>) {
        if request.page() == "front" {
            let _ = request.fail_user_action(
                "已发通知，请点开 App 继续".into(),
                Some("foreground".into()),
                Some("shop://front".into()),
            );
            return;
        }
        let _ = request.complete();
    }
}

/// 端到端：后台可见性下，`navigate_in_background` 决定导航是由 SDK 立即回复还是交给回调。
#[test]
fn background_navigation_reaches_host() {
    for enabled in [false, true] {
        let (mut child, lines, addr) =
            spawn_fake_host(&["--navigate", "front", "--navigate", "home", "--timeout-ms", "8000"]);
        let client = AppMcpClient::new(fake_host_config("uniffi-nav", &addr), None).expect("client");
        client.set_navigation_handler(Some(Arc::new(BackgroundNavigator)));
        client.set_visibility(Visibility::Hidden, false);
        client.set_navigate_in_background(enabled);
        client.start();
        let out: Vec<serde_json::Value> =
            lines.map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
        let ok = child.wait().is_ok_and(|s| s.success());
        client.stop();
        assert!(ok, "fake_host 退出码非 0：{out:?}");
        let errors: Vec<&serde_json::Value> = out.iter().map(|v| &v["error"]["data"]).skip(1).collect();
        if enabled {
            assert_eq!(
                *errors[0],
                serde_json::json!({ "kind": "USER_ACTION_REQUIRED", "reason": "foreground", "uri": "shop://front" }),
                "{out:?}"
            );
            assert!(errors[1].is_null(), "{out:?}");
        } else {
            let fg = serde_json::json!({ "kind": "USER_ACTION_REQUIRED", "reason": "foreground" });
            assert_eq!((errors[0], errors[1]), (&fg, &fg), "{out:?}");
        }
    }
}

/// `accept_channel_fd`：fd 所有权转移、各拒绝原因与接受后 SDK 在通道上先发 `app/hello`（spec/naming.md 4.2）。
#[cfg(unix)]
#[test]
fn accept_channel_fd_offers() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::fd::IntoRawFd;
    use std::os::unix::net::UnixStream;
    let mut config = fake_host_config("uniffi-channel", "127.0.0.1:1");
    config.lifecycle = Some(LifecyclePolicy {
        mode: Some(LifecycleMode::OnDemand),
        idle_timeout_ms: 60_000,
        hidden_idle_timeout_ms: 15_000,
        grace_ms: 10_000,
        residency: None,
        wake: None,
        host_absent_retries: 3,
        legacy_timers: false,
        merge_window_ms: 2_000,
        sleep_on_background: false,
    });
    let client = AppMcpClient::new(config, None).expect("client");
    assert!(matches!(client.accept_channel_fd(-1), ChannelOffer::Invalid { .. }));
    let (app_end, _hub_end) = UnixStream::pair().expect("socketpair");
    assert!(matches!(client.accept_channel_fd(app_end.into_raw_fd()), ChannelOffer::Stopped { .. }), "尚未 start");

    client.start();
    let (app_end, hub_end) = UnixStream::pair().expect("socketpair");
    assert_eq!(client.accept_channel_fd(app_end.into_raw_fd()), ChannelOffer::Accepted);
    // Hub 一端读到 SDK 的 WebSocket 升级请求（SDK 仍是 WebSocket 客户端）。
    hub_end.set_read_timeout(Some(std::time::Duration::from_secs(5))).expect("timeout");
    let mut first = String::new();
    BufReader::new(&hub_end).read_line(&mut first).expect("读升级请求");
    assert!(first.starts_with("GET /app"), "{first:?}");
    let (busy_end, _b) = UnixStream::pair().expect("socketpair");
    assert!(matches!(client.accept_channel_fd(busy_end.into_raw_fd()), ChannelOffer::Busy { .. }));
    let _ = (&hub_end).write_all(b"");
    client.stop();
}

#[test]
fn guarded_catches_panic() {
    assert!(guarded(|| {}));
    assert!(!guarded(|| panic!("boom")));
}
