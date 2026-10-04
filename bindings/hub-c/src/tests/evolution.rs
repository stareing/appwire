//! 单元测试（续）：工具演进（v25，spec/hub-api.md 3.21）：HubTool 带 schemaHash / deprecated（App 声明原样）、
//! 未声明时省略 deprecated；App 把工具改成不兼容定义后 status.schemaChanges 出现一条 breaking 记录。

use app_mcp_native::Deprecation;

use super::cache::wait_tool;
use super::*;

fn tools(hub: *mut AmHub) -> Vec<Value> {
    // SAFETY: 有效参数。
    let v = query_json(|o| unsafe { am_hub_tools_json(hub, ptr::null(), o) });
    v.as_array().cloned().unwrap_or_default()
}

fn tool(hub: *mut AmHub, name: &str) -> Value {
    tools(hub).into_iter().find(|t| t["name"] == name).unwrap_or(Value::Null)
}

fn schema(required: &[&str]) -> Option<String> {
    Some(json!({"type": "object", "properties": {"q": {"type": "string"}, "page": {"type": "integer"}}, "required": required}).to_string())
}

#[test]
fn tools_carry_schema_hash_and_deprecated_and_status_records_changes() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0"}"#);
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("orders", "订单");
    cfg.host_url = format!("ws://{addr}/app");
    cfg.instance_id = Some("o1".into());
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let mut old = ToolSpec::new("list", "旧版列表");
    old.input_schema_json = schema(&[]);
    let deprecated = Deprecation { message: "改用 orders.list2".into(), replacement: Some("list2".into()), until: Some("2027-06-30".into()) };
    let options = ToolOptions { deprecated: Some(deprecated), ..ToolOptions::default() };
    let old_handle = client.register_tool_with(old.clone(), options, Arc::new(Echo)).expect("注册");
    let _new_handle = client.register_tool(ToolSpec::new("list2", "新版列表"), Arc::new(Echo)).expect("注册");
    client.start();
    wait_tool(hub, "orders.list");
    wait_tool(hub, "orders.list2");

    let listed = tool(hub, "orders.list");
    assert_eq!(listed["deprecated"], json!({"message": "改用 orders.list2", "replacement": "list2", "until": "2027-06-30"}), "{listed}");
    let hash = listed["schemaHash"].as_str().unwrap_or_default().to_owned();
    assert!(hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()), "{listed}");
    let plain = tool(hub, "orders.list2");
    assert!(plain.get("deprecated").is_none(), "{plain}");
    assert!(plain["schemaHash"].is_string() && plain["schemaHash"] != listed["schemaHash"], "{plain}");
    // SAFETY: 有效参数。
    let status = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    assert_eq!(status["schemaChanges"], json!([]), "{status}");

    // 新增必填参数（不兼容）并清除弃用：schemaHash 变化、deprecated 省略、status.schemaChanges 一条 breaking
    let mut changed = old;
    changed.input_schema_json = schema(&["page"]);
    old_handle.update_with(changed, ToolOptions::default()).expect("更新");
    let deadline = Instant::now() + WAIT;
    let updated = loop {
        let t = tool(hub, "orders.list");
        if t["schemaHash"].as_str().is_some_and(|h| h != hash) {
            break t;
        }
        assert!(Instant::now() < deadline, "等待工具更新超时");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(updated.get("deprecated").is_none(), "{updated}");
    // SAFETY: 有效参数。
    let status = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    let records = status["schemaChanges"].as_array().cloned().unwrap_or_default();
    assert_eq!(records.len(), 1, "{status}");
    assert_eq!((&records[0]["appId"], &records[0]["tool"], &records[0]["level"]), (&json!("orders"), &json!("list"), &json!("breaking")), "{status}");
    assert!(records[0]["at"].is_u64() && records[0]["changes"].as_array().is_some_and(|c| !c.is_empty()), "{status}");

    drop(client);
    // SAFETY: 测试结束。
    unsafe { am_hub_free(hub) };
}
