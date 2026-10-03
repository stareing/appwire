//! 单元测试（续）：对象锁（v20，spec/hub-api.md 3.6「对象锁」）。

use super::*;

const SHOP: &str = r#"{"manifestVersion":1,"appId":"shop","name":"商城",
    "tools":[{"name":"cart.add","description":"加购","inputSchema":{"type":"object"}}]}"#;

fn builtin_names(hub: *mut AmHub) -> Vec<String> {
    // SAFETY: 有效参数（filter 为 NULL）。
    let tools = query_json(|o| unsafe { am_hub_tools_json(hub, ptr::null(), o) });
    tools.as_array().into_iter().flatten().filter_map(|t| t["name"].as_str()).map(str::to_owned).collect()
}

fn start(extra: &str) -> *mut AmHub {
    start_hub(&format!(r#"{{"listen":null,"manifests":[{SHOP}]{extra}}}"#))
}

/// 缺省列出 apps.lock / apps.unlock；会话 s1 加锁后 s2 加同一把锁 → LOCKED（details.holder = "api"），
/// status.locks 列出这把锁；解锁后清空。maxLocks = 0 → 不列出，调用为 TOOL_NOT_FOUND。
#[test]
fn locks_config_call_and_status() {
    let hub = start("");
    let names = builtin_names(hub);
    assert!(names.iter().any(|n| n == "apps.lock") && names.iter().any(|n| n == "apps.unlock"), "{names:?}");

    let (tx, rx) = mpsc::channel::<String>();
    call(hub, json!({"name":"apps.lock","arguments":{"appId":"shop","ttlMs":30000},"session":"s1"}), &tx);
    let ok = recv(&rx);
    assert_eq!(ok["result"]["ok"]["renewed"], false, "{ok}");
    call(hub, json!({"name":"apps.lock","arguments":{"appId":"shop"},"session":"s2"}), &tx);
    let err = recv(&rx);
    assert_eq!(err["result"]["error"]["kind"], "LOCKED", "{err}");
    assert_eq!(err["result"]["error"]["details"]["holder"], "api", "{err}");
    assert_eq!(err["result"]["error"]["details"]["appId"], "shop", "{err}");

    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    let locks = st["locks"].as_array().cloned().unwrap_or_default();
    assert_eq!(locks.len(), 1, "{st}");
    assert_eq!((locks[0]["appId"].as_str(), locks[0]["caller"].as_str()), (Some("shop"), Some("api:s1")), "{st}");
    assert_eq!(locks[0]["holder"], "api");
    assert!(locks[0].get("key").is_none(), "{st}");
    assert!(locks[0]["expiresInMs"].as_u64().is_some_and(|ms| ms > 0 && ms <= 30_000), "{st}");

    call(hub, json!({"name":"apps.unlock","arguments":{"appId":"shop"},"session":"s1"}), &tx);
    assert_eq!(recv(&rx)["result"]["ok"]["released"], true);
    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    assert_eq!(st["locks"], json!([]), "{st}");
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };

    let hub = start(r#","maxLocks":0"#);
    let names = builtin_names(hub);
    assert!(!names.iter().any(|n| n.starts_with("apps.lock") || n == "apps.unlock"), "{names:?}");
    call(hub, json!({"name":"apps.lock","arguments":{"appId":"shop"},"session":"s1"}), &tx);
    assert_eq!(recv(&rx)["result"]["error"]["kind"], "TOOL_NOT_FOUND");
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}
