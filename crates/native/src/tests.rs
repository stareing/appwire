//! 不需要 Host 的单元测试。需要 Host 的集成测试见 `tests/`。

use super::*;

fn config() -> NativeConfig {
    NativeConfig::new("unit-app", "单元测试")
}

#[test]
fn rejects_bad_host_url() {
    for url in ["http://127.0.0.1:1", "127.0.0.1:1", "", "ws://", "unix:relative.sock", "pipe:x"] {
        let mut c = config();
        c.host_url = url.to_owned();
        assert!(
            matches!(
                NativeClient::new(c, None),
                Err(NativeError::InvalidConfig(_))
            ),
            "{url}"
        );
    }
    let mut c = config();
    c.host_url = "WSS://example.invalid".to_owned();
    assert!(NativeClient::new(c, None).is_ok());
    // 本平台不支持的 IPC 形式在创建时报错（不换用其他传输）。
    let foreign = if cfg!(windows) { "unix:/tmp/hub.sock" } else { r"pipe:\\.\pipe\app-mcp-x" };
    let mut c = config();
    c.host_url = foreign.to_owned();
    assert!(matches!(NativeClient::new(c, None), Err(NativeError::InvalidConfig(_))));
    let native = if cfg!(windows) { r"pipe:\\.\pipe\app-mcp-x" } else { "unix:/tmp/hub.sock" };
    let mut c = config();
    c.host_url = native.to_owned();
    assert!(NativeClient::new(c, None).is_ok());
}

#[test]
fn default_host_url_is_platform_endpoint() {
    // 解析顺序（spec/protocol.md 1.3）：APP_MCP_ENDPOINT → 登记文件 → 平台默认 IPC 端点。
    // 不依赖本机是否有运行中的 Host：登记文件存在时以它为准。
    let expected = app_mcp_protocol::endpoint::default_endpoint_without_env();
    if std::env::var_os(app_mcp_protocol::endpoint::ENDPOINT_ENV).is_none() {
        let registered = app_mcp_protocol::registry::registered_app_endpoint();
        assert_eq!(config().host_url, registered.unwrap_or_else(|| expected.clone()));
    }
    // 以 protocol 的平台分类为唯一依据（含 ohos：target_os 为 linux 但属沙箱平台）。
    use app_mcp_protocol::platform::{IpcKind, Target};
    let prefix = match Target::CURRENT.default_ipc_kind() {
        Some(IpcKind::Unix) => "unix:/",
        Some(IpcKind::Pipe) => r"pipe:\\.\pipe\app-mcp-",
        None => app_mcp_protocol::DEFAULT_WS_URL,
    };
    assert!(expected.starts_with(prefix), "{expected}");
}

#[test]
fn rejects_bad_app_id_and_concurrency() {
    let mut c = config();
    c.app_id = "Bad App".to_owned();
    assert!(matches!(
        NativeClient::new(c, None),
        Err(NativeError::InvalidConfig(_))
    ));
    let mut c = config();
    c.max_concurrent_calls = 0;
    assert!(matches!(
        NativeClient::new(c, None),
        Err(NativeError::InvalidConfig(_))
    ));
}

#[test]
fn instance_id_generated_once_per_process() {
    let a = NativeClient::new(config(), None).unwrap();
    let b = NativeClient::new(config(), None).unwrap();
    assert!(!a.instance_id().is_empty());
    assert_eq!(a.instance_id(), b.instance_id());
    let mut c = config();
    c.instance_id = Some("fixed".to_owned());
    assert_eq!(NativeClient::new(c, None).unwrap().instance_id(), "fixed");
}

#[test]
fn explicit_launch_token_is_used() {
    let mut c = config();
    c.launch_token = Some("lt".to_owned());
    c.host_url = "ws://127.0.0.1:7717".to_owned();
    let (core, endpoint) = build_core_config(c).unwrap();
    assert_eq!(core.launch_token.as_deref(), Some("lt"));
    assert_eq!(endpoint.to_string(), "ws://127.0.0.1:7717");
    assert_eq!(core.max_concurrent_calls, 1);
}

#[test]
fn maps_core_errors() {
    assert_eq!(
        core_error(CoreError::InvalidName("x y".into())),
        NativeError::InvalidName("x y".into())
    );
    assert!(matches!(
        core_error(CoreError::InvalidSchema),
        NativeError::InvalidSchema(_)
    ));
    assert_eq!(
        core_error(CoreError::DuplicateName("a".into())),
        NativeError::DuplicateName("a".into())
    );
    assert_eq!(
        core_error(CoreError::UnknownTool(ToolId(1))),
        NativeError::Disposed
    );
    assert_eq!(
        core_error(CoreError::UnknownResource(ResourceId(1))),
        NativeError::Disposed
    );
    assert_eq!(
        core_error(CoreError::UnknownScope(ScopeId(1))),
        NativeError::Disposed
    );
    assert_eq!(
        core_error(CoreError::UnknownCall("c".into())),
        NativeError::AlreadyCompleted
    );
    assert_eq!(
        core_error(CoreError::UnknownRead(ReadId(1))),
        NativeError::AlreadyCompleted
    );
}

#[test]
fn converts_state() {
    let s = state_info(&ConnectionState::Backoff { retry_at: 1500, reason: None, code: None }, 1000);
    assert_eq!(
        s,
        StateInfo {
            status: StateStatus::Backoff,
            retry_in_ms: Some(500),
            reason: None,
            code: None,
        }
    );
    let s = state_info(
        &ConnectionState::Backoff {
            retry_at: 1500,
            reason: Some("连接失败".into()),
            code: Some(ConnectionErrorCode::HostNotRunning),
        },
        1000,
    );
    assert_eq!((s.reason.as_deref(), s.code.as_deref()), (Some("连接失败"), Some("HOST_NOT_RUNNING")));
    let s = state_info(
        &ConnectionState::Rejected {
            reason: "no".into(),
            code: ConnectionErrorCode::OriginNotAllowed,
        },
        0,
    );
    assert_eq!(s.status, StateStatus::Rejected);
    assert_eq!(s.reason.as_deref(), Some("no"));
    assert_eq!(s.code.as_deref(), Some("ORIGIN_NOT_ALLOWED"));
    assert_eq!(
        state_info(&ConnectionState::Idle, 0).status,
        StateStatus::Idle
    );
}

struct Nop;
impl ToolHandler for Nop {
    fn invoke(&self, _call: CallHandle) {}
}
impl ResourceReader for Nop {
    fn read(&self, _read: ReadHandle) {}
}

#[test]
fn registration_errors_are_synchronous() {
    let client = NativeClient::new(config(), None).unwrap();
    let t = client
        .register_tool(ToolSpec::new("a.b", "d"), Arc::new(Nop))
        .unwrap();
    assert_eq!(t.name(), "a.b");
    assert_eq!(
        client
            .register_tool(ToolSpec::new("a.b", "d"), Arc::new(Nop))
            .unwrap_err(),
        NativeError::DuplicateName("a.b".into())
    );
    assert!(matches!(
        client.register_tool(ToolSpec::new("bad name", "d"), Arc::new(Nop)),
        Err(NativeError::InvalidName(_))
    ));
    let mut spec = ToolSpec::new("s", "d");
    spec.input_schema_json = Some("{".into());
    assert!(matches!(
        client.register_tool(spec.clone(), Arc::new(Nop)),
        Err(NativeError::InvalidSchema(_))
    ));
    spec.input_schema_json = Some(r#"{"type":"string"}"#.into());
    assert!(matches!(
        client.register_tool(spec, Arc::new(Nop)),
        Err(NativeError::InvalidSchema(_))
    ));

    // 注销后同名可以再注册；dispose 幂等；注销后更新返回 Disposed。
    t.dispose();
    t.dispose();
    assert_eq!(t.set_enabled(false), Err(NativeError::Disposed));
    client
        .register_tool(ToolSpec::new("a.b", "d"), Arc::new(Nop))
        .unwrap();

    let r = client
        .register_resource(
            ResourceSpec {
                name: "r".into(),
                description: "d".into(),
                mime_type: None,
            },
            Arc::new(Nop),
        )
        .unwrap();
    r.notify_changed().unwrap();
    r.dispose();
    assert_eq!(r.notify_changed(), Err(NativeError::Disposed));
}

#[test]
fn scope_dispose_removes_children() {
    let client = NativeClient::new(config(), None).unwrap();
    let scope = client.create_scope("outer").unwrap();
    let inner = scope.create_scope("inner").unwrap();
    let t = inner
        .register_tool(ToolSpec::new("t", "d"), Arc::new(Nop))
        .unwrap();
    scope.dispose();
    scope.dispose();
    assert_eq!(t.set_enabled(false), Err(NativeError::Disposed));
    assert!(matches!(
        inner.register_tool(ToolSpec::new("u", "d"), Arc::new(Nop)),
        Err(NativeError::Disposed)
    ));
    assert!(matches!(
        scope.create_scope("x"),
        Err(NativeError::Disposed)
    ));
    // 名称已释放
    client
        .register_tool(ToolSpec::new("t", "d"), Arc::new(Nop))
        .unwrap();
    t.dispose();
}

#[test]
fn stop_makes_registration_fail() {
    let client = NativeClient::new(config(), None).unwrap();
    let t = client
        .register_tool(ToolSpec::new("t", "d"), Arc::new(Nop))
        .unwrap();
    let scope = client.create_scope("s").unwrap();
    client.stop();
    assert_eq!(client.state().status, StateStatus::Stopped);
    assert!(matches!(
        client.register_tool(ToolSpec::new("u", "d"), Arc::new(Nop)),
        Err(NativeError::Stopped)
    ));
    assert!(matches!(
        scope.register_tool(ToolSpec::new("u", "d"), Arc::new(Nop)),
        Err(NativeError::Stopped)
    ));
    assert!(matches!(
        client.create_scope("x"),
        Err(NativeError::Stopped)
    ));
    assert_eq!(t.set_enabled(false), Err(NativeError::Stopped));
    client.start();
    assert_eq!(client.state().status, StateStatus::Stopped);
}

#[test]
fn types_are_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<NativeClient>();
    check::<CallHandle>();
    check::<ReadHandle>();
    check::<ToolHandle>();
    check::<ResourceHandle>();
    check::<ScopeHandle>();
}

#[test]
fn host_mismatch_state_and_expected_user() {
    let info = state_info(
        &ConnectionState::HostMismatch { reason: "不是 app-mcp".into(), code: ConnectionErrorCode::HostNotAppMcp },
        0,
    );
    assert_eq!(info.status, StateStatus::HostMismatch);
    assert_eq!(info.code.as_deref(), Some("HOST_NOT_APP_MCP"));
    assert_eq!(info.reason.as_deref(), Some("不是 app-mcp"));
    // 桌面平台核对 Host 用户（spec/protocol.md 1.6）
    let (core, _) = build_core_config(config()).unwrap();
    assert_eq!(core.expected_host_user, app_mcp_protocol::identity::expected_host_user());
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    assert!(core.expected_host_user.is_some());
}
