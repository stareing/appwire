//! 端到端单元测试（续）：对象锁（spec/hub-api.md 3.6「对象锁」）。

use super::*;

const SHOP: &str = r#"{"manifestVersion":1,"appId":"shop","name":"商城",
    "tools":[{"name":"cart.add","description":"加购","inputSchema":{"type":"object"}}]}"#;

fn start(max_locks: Option<u32>) -> Arc<AppMcpHub> {
    AppMcpHub::start(HubConfig {
        enable_listen: false,
        enable_ipc: false,
        manifests_json: vec![SHOP.into()],
        max_locks,
        ..HubConfig::default()
    })
    .expect("启动 Hub")
}

fn session_req(name: &str, args: Value, session: &str) -> CallRequest {
    CallRequest { session: Some(session.into()), ..req(name, args) }
}

fn builtin_names(hub: &AppMcpHub) -> Vec<String> {
    hub.tools(ToolFilter::default()).into_iter().map(|t| t.name).filter(|n| n.starts_with("apps.")).collect()
}

/// 缺省列出 apps.lock / apps.unlock；会话 s1 加锁后 s2 加同一把锁 → LOCKED（holder = "api"）；`status().locks` 列出；
/// 解锁后清空。`max_locks = Some(0)` → 不列出，调用为 TOOL_NOT_FOUND。
#[test]
fn locks_config_call_and_status() {
    let hub = start(None);
    let names = builtin_names(&hub);
    assert!(names.contains(&"apps.lock".into()) && names.contains(&"apps.unlock".into()), "{names:?}");

    let out = wait(hub.call_tool(session_req("apps.lock", json!({"appId": "shop", "ttlMs": 30000}), "s1"))).expect("加锁");
    assert!(out.error.is_none(), "{out:?}");
    let out = wait(hub.call_tool(session_req("apps.lock", json!({"appId": "shop"}), "s2"))).expect("调用");
    let err = out.error.expect("应为 LOCKED");
    assert_eq!(err.kind, "LOCKED");
    let details: Value = serde_json::from_str(err.details_json.as_deref().unwrap_or("null")).unwrap_or_default();
    assert_eq!((details["appId"].as_str(), details["holder"].as_str()), (Some("shop"), Some("api")), "{details}");

    let locks = hub.status().expect("status").locks.unwrap_or_default();
    assert_eq!(locks.len(), 1, "{locks:?}");
    let l = &locks[0];
    assert_eq!((l.app_id.as_str(), l.key.as_deref(), l.caller.as_str(), l.holder.as_str()), ("shop", None, "api:s1", "api"));
    assert!(l.expires_in_ms > 0 && l.expires_in_ms <= 30_000, "{l:?}");

    let out = wait(hub.call_tool(session_req("apps.unlock", json!({"appId": "shop"}), "s1"))).expect("解锁");
    let data: Value = serde_json::from_str(out.data_json.as_deref().unwrap_or("null")).unwrap_or_default();
    assert_eq!(data["released"], true, "{out:?}");
    assert_eq!(hub.status().expect("status").locks, Some(Vec::new()));
    hub.shutdown();

    let hub = start(Some(0));
    let names = builtin_names(&hub);
    assert!(!names.iter().any(|n| n == "apps.lock" || n == "apps.unlock"), "{names:?}");
    let kind = match wait(hub.call_tool(session_req("apps.lock", json!({"appId": "shop"}), "s1"))) {
        Ok(out) => out.error.map(|e| e.kind),
        Err(HubError::Tool { kind, .. }) => Some(kind),
        Err(e) => panic!("意外错误：{e:?}"),
    };
    assert_eq!(kind.as_deref(), Some("TOOL_NOT_FOUND"));
    hub.shutdown();
}
