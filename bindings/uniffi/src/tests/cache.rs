//! 结果缓存声明（spec/protocol.md 3.6）：记录转换、注册 / 更新时的格式校验，以及更新与启停不丢声明。

use std::sync::Arc;

use app_mcp_native as native;

use crate::*;

struct NoopTool;

impl ToolHandler for NoopTool {
    fn invoke(&self, call: Arc<Call>) {
        let _ = call.complete(None, Vec::new());
    }
}

struct NoopReader;

impl ResourceReader for NoopReader {
    fn read(&self, read: Arc<Read>) {
        let _ = read.complete("{}".into());
    }
}

fn read_tool(name: &str, cache: Option<CachePolicy>) -> ToolSpec {
    ToolSpec {
        name: name.into(),
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
        implements: Vec::new(),
        cache,
        deprecated: None,
        undoable: false,
    }
}

fn resource(name: &str, cache: Option<CachePolicy>) -> ResourceSpec {
    ResourceSpec { name: name.into(), description: "d".into(), mime_type: None, realtime: false, annotations: None, cache }
}

fn offline_client() -> Arc<AppMcpClient> {
    let mut cfg = super::fake_host_config("uniffi-cache", "127.0.0.1:9");
    cfg.connect_timeout_ms = Some(1000);
    AppMcpClient::new(cfg, None).expect("client")
}

fn private(ttl_ms: u64) -> CachePolicy {
    CachePolicy { ttl_ms, scope: None }
}

#[test]
fn cache_policy_conversion() {
    let (_, options): (native::ToolSpec, native::ToolOptions) = read_tool("t", None).into();
    assert_eq!(options.cache, None, "未声明时为空");
    let (_, options): (native::ToolSpec, native::ToolOptions) = read_tool("t", Some(private(5_000))).into();
    assert_eq!(options.cache, Some(native::CachePolicy { ttl_ms: 5_000, scope: native::CacheScope::Private }), "scope 缺省 private");
    let shared = CachePolicy { ttl_ms: 60_000, scope: Some(CacheScope::Shared) };
    let (_, options): (native::ToolSpec, native::ToolOptions) = read_tool("t", Some(shared)).into();
    assert_eq!(options.cache, Some(native::CachePolicy { ttl_ms: 60_000, scope: native::CacheScope::Shared }));
    let explicit = CachePolicy { ttl_ms: 1, scope: Some(CacheScope::Private) };
    let (_, options): (native::ToolSpec, native::ToolOptions) = read_tool("t", Some(explicit)).into();
    assert_eq!(options.cache.map(|c| c.scope), Some(native::CacheScope::Private));

    let (_, options): (native::ResourceSpec, native::ResourceOptions) = resource("r", None).into();
    assert_eq!(options.cache, None);
    let (_, options): (native::ResourceSpec, native::ResourceOptions) = resource("r", Some(shared)).into();
    assert_eq!(options.cache, Some(native::CachePolicy { ttl_ms: 60_000, scope: native::CacheScope::Shared }));
}

#[test]
fn out_of_range_ttl_is_rejected() {
    let client = offline_client();
    for ttl in [0, native::MAX_CACHE_TTL_MS + 1] {
        let err = client.register_tool(read_tool("c.bad", Some(private(ttl))), Arc::new(NoopTool)).expect_err("越界");
        assert!(matches!(err, AppMcpError::InvalidConfig { .. }), "{err:?}");
        let err = client.register_resource(resource("r.bad", Some(private(ttl))), Arc::new(NoopReader)).expect_err("越界");
        assert!(matches!(err, AppMcpError::InvalidConfig { .. }), "{err:?}");
    }
    let tool = client.register_tool(read_tool("c.ok", Some(private(native::MAX_CACHE_TTL_MS))), Arc::new(NoopTool));
    let tool = tool.expect("上限可用");
    let err = tool.update(read_tool("c.ok", Some(private(0)))).expect_err("更新越界");
    assert!(matches!(err, AppMcpError::InvalidConfig { .. }), "{err:?}");
    client.stop();
}

/// toolsHash 含 cache 声明：更新替换 / 清除、启停都按声明变化，不被丢掉。
#[test]
fn update_replaces_and_clears_cache() {
    let hash_with = |cache: Option<CachePolicy>| {
        let client = offline_client();
        let _tool = client.register_tool(read_tool("c.t", cache), Arc::new(NoopTool)).expect("tool");
        let h = client.tools_hash();
        client.stop();
        h
    };
    let none = hash_with(None);
    let five = hash_with(Some(private(5_000)));
    let shared = hash_with(Some(CachePolicy { ttl_ms: 1_000, scope: Some(CacheScope::Shared) }));
    assert_ne!(none, five, "cache 进 toolsHash");
    assert_ne!(five, shared);

    let client = offline_client();
    let tool = client.register_tool(read_tool("c.t", Some(private(5_000))), Arc::new(NoopTool)).expect("tool");
    assert_eq!(client.tools_hash(), five);
    tool.set_enabled(false).expect("disable");
    tool.set_enabled(true).expect("enable");
    assert_eq!(client.tools_hash(), five, "启停不丢 cache");
    tool.update(read_tool("c.t", Some(CachePolicy { ttl_ms: 1_000, scope: Some(CacheScope::Shared) }))).expect("替换");
    assert_eq!(client.tools_hash(), shared, "更新替换 cache");
    tool.update(read_tool("c.t", None)).expect("清除");
    assert_eq!(client.tools_hash(), none, "更新为空即清除");
    client.stop();
}
