//! 端到端单元测试（续）：工具演进（spec/hub-api.md 3.21）经绑定层的 `HubTool.schema_hash` / `deprecated` 与 `status().schema_changes`。

use super::*;

struct Noop;

impl native::ToolHandler for Noop {
    fn invoke(&self, call: native::CallHandle) {
        let _ = call.complete(Some("{}"), vec![]);
    }
}

fn find_tool(hub: &AppMcpHub, name: &str) -> Option<HubTool> {
    hub.tools(ToolFilter::default()).into_iter().find(|t| t.name == name)
}

fn eventually_tool(hub: &AppMcpHub, name: &str, pred: impl Fn(&HubTool) -> bool) -> HubTool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(t) = find_tool(hub, name).filter(|t| pred(t)) {
            return t;
        }
        assert!(Instant::now() < deadline, "等待 {name} 超时：{:?}", find_tool(hub, name));
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn schema(required: &[&str]) -> String {
    json!({"type":"object","properties":{"q":{"type":"string"}},"required":required}).to_string()
}

/// 真实 App 声明弃用工具 → `HubTool.deprecated` 原样、`schema_hash` 有值；未弃用工具与内置工具为空；
/// 新增必填参数后 `schema_hash` 变化并记入 `status().schema_changes`（破坏性）。
#[test]
fn deprecated_tool_schema_hash_and_schema_changes() {
    let hub = start_hub(None);
    let mut cfg = native::NativeConfig::new("lib", "Lib");
    cfg.host_url = format!("ws://{}/app", hub.listen_addr().expect("监听地址"));
    let app = native::NativeClient::new(cfg, None).expect("App");
    let mut old = native::ToolSpec::new("q.old", "旧版查询");
    old.input_schema_json = Some(schema(&[]));
    old.risk = native::Risk::Read;
    let dep = native::Deprecation { message: "改用 q.new".into(), replacement: Some("q.new".into()), until: Some("2027-06-30".into()) };
    let options = native::ToolOptions { deprecated: Some(dep), ..Default::default() };
    let old_handle = app.register_tool_with(old.clone(), options.clone(), Arc::new(Noop)).expect("注册");
    let _new = app.register_tool(native::ToolSpec::new("q.new", "新版查询"), Arc::new(Noop)).expect("注册");
    app.start();

    let t = eventually_tool(&hub, "lib.q.old", |_| true);
    let expected = Deprecation { message: "改用 q.new".into(), replacement: Some("q.new".into()), until: Some("2027-06-30".into()) };
    assert_eq!(t.deprecated, Some(expected));
    let hash = t.schema_hash.clone().expect("App 工具有 schema_hash");
    assert_eq!(hash.len(), 16, "{hash}");
    let n = eventually_tool(&hub, "lib.q.new", |_| true);
    assert_eq!(n.deprecated, None, "未弃用");
    assert!(n.schema_hash.is_some());
    let builtin = find_tool(&hub, "apps.list").expect("内置工具");
    assert_eq!((builtin.schema_hash, builtin.deprecated), (None, None), "内置工具不带");

    old.input_schema_json = Some(schema(&["q"]));
    old_handle.update_with(old, options).expect("更新");
    let t = eventually_tool(&hub, "lib.q.old", |t| t.schema_hash.as_deref() != Some(hash.as_str()));
    assert!(t.deprecated.is_some(), "更新 schema 不丢弃用声明");
    let changes = hub.status().expect("status").schema_changes.expect("schema_changes");
    let rec = changes.iter().find(|r| r.app_id == "lib" && r.tool == "q.old").expect("记入 schema_changes");
    assert_eq!(rec.level, ChangeLevel::Breaking, "{rec:?}");
    assert!(rec.at > 0 && !rec.changes.is_empty(), "{rec:?}");
    app.stop();
    hub.shutdown();
}
