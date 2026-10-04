//! 结果缓存声明（spec/protocol.md 3.6）：原生 App 的工具 / 资源 `cache` 原样同步给 Host；越界报错；写工具记警告。

use std::sync::Arc;

use app_mcp_native::{
    CachePolicy, CacheScope, CallHandle, LogLevel, NativeClient, NativeConfig, NativeError, ReadHandle, ResourceOptions,
    ResourceReader, ResourceSpec, Risk, ToolHandler, ToolOptions, ToolSpec,
};
use app_mcp_protocol::method;
use serde_json::json;

use crate::common::{MockHost, Recorder, eventually};

struct Noop;
impl ToolHandler for Noop {
    fn invoke(&self, call: CallHandle) {
        call.complete(None, vec![]).unwrap();
    }
}

struct Empty;
impl ResourceReader for Empty {
    fn read(&self, read: ReadHandle) {
        read.complete("{}").unwrap();
    }
}

fn read_tool(name: &str) -> ToolSpec {
    let mut spec = ToolSpec::new(name, "只读");
    spec.risk = Risk::Read;
    spec
}

fn cached(ttl_ms: u64, scope: CacheScope) -> ToolOptions {
    ToolOptions { cache: Some(CachePolicy { ttl_ms, scope }), ..ToolOptions::default() }
}

#[test]
fn cache_declarations_reach_host() {
    let host = MockHost::start();
    let rec = Arc::new(Recorder::default());
    let mut config = NativeConfig::new("test-app", "测试应用");
    config.host_url = host.url();
    let client = NativeClient::new(config, Some(rec.clone())).unwrap();

    let price = client.register_tool_with(read_tool("price.get"), cached(60_000, CacheScope::Shared), Arc::new(Noop)).unwrap();
    client.register_tool(read_tool("plain"), Arc::new(Noop)).unwrap();
    // 越界：注册失败，归入配置错误
    for ttl in [0, app_mcp_native::MAX_CACHE_TTL_MS + 1] {
        let r = client.register_tool_with(read_tool("bad.cache"), cached(ttl, CacheScope::Private), Arc::new(Noop));
        assert!(matches!(r, Err(NativeError::InvalidConfig(ref m)) if m.contains("ttlMs")), "{ttl}: {:?}", r.err());
        let res = ResourceOptions { cache: Some(CachePolicy { ttl_ms: ttl, scope: CacheScope::Private }), ..ResourceOptions::default() };
        let spec = ResourceSpec { name: "bad.res".into(), description: "d".into(), mime_type: None };
        let r = client.register_resource_with(spec, res, Arc::new(Empty));
        assert!(matches!(r, Err(NativeError::InvalidConfig(_))), "{ttl}: {:?}", r.err());
    }
    // 写工具带 cache：照常注册，记警告
    client.register_tool_with(ToolSpec::new("cart.add", "写"), cached(1000, CacheScope::Private), Arc::new(Noop)).unwrap();
    eventually("写工具 cache 警告", || {
        rec.logs.lock().unwrap().iter().any(|(level, m)| *level == LogLevel::Warn && m.contains("cart.add") && m.contains("cache"))
    });
    assert!(!rec.logs.lock().unwrap().iter().any(|(_, m)| m.contains("price.get")), "只读工具不应警告");
    let options = ResourceOptions { cache: Some(CachePolicy { ttl_ms: 5000, scope: CacheScope::Private }), ..ResourceOptions::default() };
    let spec = ResourceSpec { name: "cart.state".into(), description: "购物车".into(), mime_type: None };
    client.register_resource_with(spec, options, Arc::new(Empty)).unwrap();

    client.start();
    let tools = host.wait_notification(method::TOOLS_SYNC);
    let by_name = |name: &str| tools["tools"].as_array().unwrap().iter().find(|t| t["name"] == name).cloned().unwrap();
    assert_eq!(by_name("price.get")["cache"], json!({ "ttlMs": 60000, "scope": "shared" }));
    assert_eq!(by_name("cart.add")["cache"], json!({ "ttlMs": 1000 }), "写工具的声明同样原样同步（Hub 忽略）");
    assert!(by_name("plain").get("cache").is_none(), "未声明时不序列化");
    let resources = host.wait_notification(method::RESOURCES_SYNC);
    assert_eq!(resources["resources"][0]["cache"], json!({ "ttlMs": 5000 }));
    host.wait_ready();

    // 更新：清除声明 → tools/changed 不再带 cache
    price.update_with(read_tool("price.get"), ToolOptions::default()).unwrap();
    let changed = host.wait_notification(method::TOOLS_CHANGED);
    assert_eq!(changed["upserted"][0]["name"], "price.get");
    assert!(changed["upserted"][0].get("cache").is_none());
    // 更新为越界值：失败
    let r = price.update_with(read_tool("price.get"), cached(0, CacheScope::Private));
    assert!(matches!(r, Err(NativeError::InvalidConfig(_))), "{:?}", r.err());
}
