//! 端到端单元测试（续）：撤销（spec/hub-api.md 3.23）经绑定层的 `HubTool.undoable`、`CallOutcome.undo` / `undo_of` 与 `status().undo`。

use super::*;

/// 开关工具：把参数 `on` 原样返回，并给出逆操作（以相反值再调用自身）。
struct Toggle;

impl native::ToolHandler for Toggle {
    fn invoke(&self, call: native::CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or_default();
        let on = args["on"].as_bool().unwrap_or(false);
        let mut undo = native::UndoAction::new("toggle");
        undo.arguments = json!({ "on": !on });
        undo.label = Some(if on { "关掉开关" } else { "打开开关" }.into());
        let _ = call.complete_with(native::CallResult {
            data_json: Some(json!({ "on": on }).to_string()),
            undo: Some(undo),
            ..Default::default()
        });
    }
}

fn start_toggle_app(hub: &AppMcpHub) -> native::NativeClient {
    let mut cfg = native::NativeConfig::new("tg", "Toggle");
    cfg.host_url = format!("ws://{}/app", hub.listen_addr().expect("监听地址"));
    let app = native::NativeClient::new(cfg, None).expect("App");
    let options = native::ToolOptions { undoable: true, ..Default::default() };
    app.register_tool_with(native::ToolSpec::new("toggle", "开关"), options, Arc::new(Toggle)).expect("注册");
    app.start();
    eventually("tg.toggle 注册", || hub.tools(ToolFilter::default()).iter().any(|t| t.name == "tg.toggle"));
    app
}

/// App 声明 undoable 并返回 undo → `HubTool.undoable`、结果 `undo`；`apps.undo` 结果带 `undo_of` 并再次登记（重做）；
/// `status().undo` 报告上限与记录数。
#[test]
fn undo_offer_undo_of_and_status() {
    let hub = start_hub(None);
    let app = start_toggle_app(&hub);
    let tools = hub.tools(ToolFilter::default());
    let t = tools.iter().find(|t| t.name == "tg.toggle").expect("工具");
    assert!(t.undoable, "{t:?}");
    let builtin = tools.iter().find(|t| t.name == "apps.list").expect("内置工具");
    assert!(!builtin.undoable, "内置工具为 false");

    let first = wait(hub.call_tool(req("tg.toggle", json!({ "on": true })))).expect("调用");
    assert!(first.error.is_none(), "{first:?}");
    let offer = first.undo.clone().expect("已登记撤销");
    assert_eq!(offer.label.as_deref(), Some("关掉开关"));
    assert!(offer.expires_in_ms > 0 && offer.expires_in_ms <= 30 * 60 * 1000, "{offer:?}");
    assert_eq!(first.undo_of, None, "普通调用没有 undo_of");

    let st = hub.status().expect("status").undo.expect("undo 状态");
    assert_eq!((st.ttl_ms, st.max_per_task, st.records), (30 * 60 * 1000, 32, 1), "{st:?}");

    let undone = wait(hub.call_tool(req("apps.undo", json!({})))).expect("撤销");
    assert!(undone.error.is_none(), "{undone:?}");
    assert_eq!(undone.undo_of.as_deref(), Some(first.call_id.as_str()));
    assert_eq!(undone.data_json.as_deref(), Some(r#"{"on":false}"#), "逆调用以相反值执行");
    assert_eq!(undone.undo.and_then(|u| u.label).as_deref(), Some("打开开关"), "逆调用结果再次登记（重做）");

    let again = wait(hub.call_tool(req("apps.undo", json!({ "callId": first.call_id })))).expect("调用");
    assert_eq!(again.error.map(|e| e.kind).as_deref(), Some("TOOL_NOT_FOUND"), "只能撤销一次");
    app.stop();
    hub.shutdown();
}

/// `HubConfig.undo`：覆盖上限进状态；`max_per_task: 0` 关闭（`apps.undo` 不列出）；开启时 `ttl_ms: 0` 启动失败。
#[test]
fn undo_limits_config() {
    let start = |undo| AppMcpHub::start(HubConfig { enable_listen: false, enable_ipc: false, undo: Some(undo), ..Default::default() });
    let hub = start(UndoLimitOverrides { ttl_ms: Some(5000), ..Default::default() }).expect("启动");
    let st = hub.status().expect("status").undo.expect("undo 状态");
    assert_eq!((st.ttl_ms, st.max_per_task), (5000, 32));
    assert!(hub.tools(ToolFilter::default()).iter().any(|t| t.name == "apps.undo"), "开启时列出 apps.undo");
    hub.shutdown();
    let off = start(UndoLimitOverrides { max_per_task: Some(0), ..Default::default() }).expect("启动");
    assert_eq!(off.status().expect("status").undo.expect("undo 状态").max_per_task, 0);
    assert!(!off.tools(ToolFilter::default()).iter().any(|t| t.name == "apps.undo"), "关闭时不列出 apps.undo");
    off.shutdown();
    match start(UndoLimitOverrides { ttl_ms: Some(0), ..Default::default() }) {
        Err(e) => assert!(format!("{e:?}").contains("ttlMs"), "{e:?}"),
        Ok(_) => panic!("开启时 ttl_ms 为 0 应启动失败"),
    }
}
