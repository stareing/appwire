//! 集成测试：事件声明与发出（第 16 项 N3，spec/protocol.md 3.5）。

use app_mcp_native::{EventInfo, MAX_EVENT_PAYLOAD_BYTES, NativeClient, NativeConfig, NativeError, StateStatus};
use app_mcp_protocol::method;
use crate::common::{MockHost, eventually};
use serde_json::json;

fn config(host: &MockHost) -> NativeConfig {
    let mut c = NativeConfig::new("test-app", "测试应用");
    c.host_url = host.url();
    c.instance_id = Some("inst-1".into());
    c
}

fn event(name: &str) -> EventInfo {
    EventInfo { name: name.into(), description: format!("{name} 发生时"), payload_schema: None }
}

#[test]
fn declare_sync_and_emit() {
    let host = MockHost::start();
    let client = NativeClient::new(config(&host), None).unwrap();
    client.declare_event(event("order.shipped")).unwrap();
    assert_eq!(client.emit_event("order.shipped", None), Ok(false), "未连接时丢弃");

    client.start();
    host.wait_connected();
    let sync = host.wait_notification(method::EVENTS_SYNC);
    assert_eq!(sync, json!({"events": [{"name": "order.shipped", "description": "order.shipped 发生时"}]}));
    host.wait_ready();
    eventually("Connected", || client.state().status == StateStatus::Connected);

    assert_eq!(client.emit_event("order.shipped", Some(r#"{"orderId":"o1"}"#)), Ok(true));
    let emit = host.wait_notification(method::EVENTS_EMIT);
    assert_eq!(emit, json!({"name": "order.shipped", "eventId": "e1", "payload": {"orderId": "o1"}}));

    // 连接中声明变化：重发全量
    client.declare_event(event("download.done")).unwrap();
    let sync = host.wait_notification(method::EVENTS_SYNC);
    assert_eq!(sync["events"].as_array().map(Vec::len), Some(2));
    assert!(client.remove_event("order.shipped"));
    assert!(!client.remove_event("order.shipped"));
    let sync = host.wait_notification(method::EVENTS_SYNC);
    assert_eq!(sync["events"][0]["name"], "download.done");

    assert_eq!(client.emit_event("download.done", None), Ok(true));
    assert_eq!(host.wait_notification(method::EVENTS_EMIT), json!({"name": "download.done", "eventId": "e2"}));

    client.stop();
    assert_eq!(client.emit_event("download.done", None), Err(NativeError::Stopped));
    assert_eq!(client.declare_event(event("x")), Err(NativeError::Stopped));
}

#[test]
fn local_errors() {
    let host = MockHost::start();
    let client = NativeClient::new(config(&host), None).unwrap();
    assert!(matches!(client.declare_event(event("bad name")), Err(NativeError::InvalidName(_))));
    client.declare_event(event("a")).unwrap();

    assert!(matches!(client.emit_event("b", None), Err(NativeError::InvalidName(m)) if m.contains("未声明")));
    assert!(matches!(client.emit_event("bad name", None), Err(NativeError::InvalidName(_))));
    assert!(matches!(client.emit_event("a", Some("{")), Err(NativeError::InvalidJson(m)) if m.contains("合法 JSON")));
    assert!(matches!(client.emit_event("a", Some("[1]")), Err(NativeError::InvalidJson(_))));
    let big = json!({"k": "x".repeat(MAX_EVENT_PAYLOAD_BYTES)}).to_string();
    assert!(matches!(client.emit_event("a", Some(&big)), Err(NativeError::InvalidJson(m)) if m.contains("bytes")));
    assert_eq!(client.emit_event("a", Some("{}")), Ok(false), "合法但未连接");
}

#[test]
fn emit_after_disconnect_returns_false() {
    let host = MockHost::start();
    let client = NativeClient::new(config(&host), None).unwrap();
    client.declare_event(event("a")).unwrap();
    client.start();
    host.wait_connected();
    host.wait_ready();
    eventually("Connected", || client.state().status == StateStatus::Connected);
    host.close();
    eventually("断开", || client.state().status != StateStatus::Connected);
    assert_eq!(client.emit_event("a", None), Ok(false));
}
