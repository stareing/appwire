//! 不需要 Host 的单元测试。需要 Host 的集成测试见 `tests/`。

use super::*;

fn config() -> NativeConfig {
    NativeConfig::new("unit-app", "单元测试")
}

#[test]
fn rejects_bad_host_url() {
    for url in ["http://127.0.0.1:1", "127.0.0.1:1", "", "ws://"] {
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
    let (core, url) = build_core_config(c).unwrap();
    assert_eq!(core.launch_token.as_deref(), Some("lt"));
    assert_eq!(url, "ws://127.0.0.1:7717");
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
    let s = state_info(&ConnectionState::Backoff { retry_at: 1500 }, 1000);
    assert_eq!(
        s,
        StateInfo {
            status: StateStatus::Backoff,
            retry_in_ms: Some(500),
            reason: None
        }
    );
    let s = state_info(
        &ConnectionState::Rejected {
            reason: "no".into(),
        },
        0,
    );
    assert_eq!(s.status, StateStatus::Rejected);
    assert_eq!(s.reason.as_deref(), Some("no"));
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
