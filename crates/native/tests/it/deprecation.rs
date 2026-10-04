//! 工具弃用声明（spec/protocol.md 3.7，第 16 项 O4）：原生 App 的 `deprecated` 原样同步给 Host；清除后不带；格式不合法为
//! `InvalidConfig`；必填参数标弃用记警告。

use std::sync::Arc;

use app_mcp_native::{CallHandle, Deprecation, LogLevel, NativeClient, NativeConfig, NativeError, ToolHandler, ToolOptions, ToolSpec};
use app_mcp_protocol::method;
use serde_json::json;

use crate::common::{MockHost, Recorder, eventually};

struct Noop;
impl ToolHandler for Noop {
    fn invoke(&self, call: CallHandle) {
        call.complete(None, vec![]).unwrap();
    }
}

fn deprecated(message: &str, replacement: Option<&str>) -> ToolOptions {
    let d = Deprecation { message: message.into(), replacement: replacement.map(Into::into), until: None };
    ToolOptions { deprecated: Some(d), ..ToolOptions::default() }
}

#[test]
fn deprecation_reaches_host_and_invalid_is_config_error() {
    let host = MockHost::start();
    let rec = Arc::new(Recorder::default());
    let mut config = NativeConfig::new("test-app", "测试应用");
    config.host_url = host.url();
    let client = NativeClient::new(config, Some(rec.clone())).unwrap();

    let old = client.register_tool_with(ToolSpec::new("orders.list", "旧"), deprecated("改用 orders.query", Some("orders.query")), Arc::new(Noop)).unwrap();
    client.register_tool(ToolSpec::new("plain", "p"), Arc::new(Noop)).unwrap();
    // 格式不合法：注册失败，归入配置错误
    for bad in [deprecated("", None), deprecated("m", Some("bad.dep")), deprecated("m", Some("bad name"))] {
        let r = client.register_tool_with(ToolSpec::new("bad.dep", "d"), bad, Arc::new(Noop));
        assert!(matches!(r, Err(NativeError::InvalidConfig(_))), "{:?}", r.err());
    }
    // 必填参数标弃用：照常注册，记警告
    let mut spec = ToolSpec::new("search", "s");
    spec.input_schema_json = Some(json!({"type": "object", "properties": {"q": {"type": "string", "deprecated": true}}, "required": ["q"]}).to_string());
    client.register_tool(spec, Arc::new(Noop)).unwrap();
    eventually("必填参数弃用警告", || {
        rec.logs.lock().unwrap().iter().any(|(level, m)| *level == LogLevel::Warn && m.contains("search") && m.contains("deprecated"))
    });

    client.start();
    let tools = host.wait_notification(method::TOOLS_SYNC);
    let by_name = |name: &str| tools["tools"].as_array().unwrap().iter().find(|t| t["name"] == name).cloned().unwrap();
    assert_eq!(by_name("orders.list")["deprecated"], json!({ "message": "改用 orders.query", "replacement": "orders.query" }));
    assert!(by_name("plain").get("deprecated").is_none(), "未声明时不序列化");
    host.wait_ready();

    // 更新为不合法值（指向自身）：失败
    let r = old.update_with(ToolSpec::new("orders.list", "旧"), deprecated("m", Some("orders.list")));
    assert!(matches!(r, Err(NativeError::InvalidConfig(_))), "{:?}", r.err());
    // 更新声明 → tools/changed 带新值
    old.update_with(ToolSpec::new("orders.list", "旧"), deprecated("v2", None)).unwrap();
    let changed = host.wait_notification(method::TOOLS_CHANGED);
    assert_eq!(changed["upserted"][0]["deprecated"], json!({ "message": "v2" }));
    // 清除声明 → tools/changed 不再带
    old.update_with(ToolSpec::new("orders.list", "旧"), ToolOptions::default()).unwrap();
    let changed = host.wait_notification(method::TOOLS_CHANGED);
    assert_eq!(changed["upserted"][0]["name"], "orders.list");
    assert!(changed["upserted"][0].get("deprecated").is_none());
}
