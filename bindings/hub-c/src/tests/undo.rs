//! 单元测试（续）：撤销（v26，spec/hub-api.md 3.23）：HubTool 带 undoable（App 声明原样，未声明时省略）；App 结果带撤销信息时
//! CallOutcome 带 undo、经 am_hub_call 调用 apps.undo 得到逆调用结果与 undoOf；status.undo；配置 undo.maxPerTask: 0 关闭。

use app_mcp_native::{CallResult, UndoAction};

use super::cache::wait_tool;
use super::*;

/// `add` 的结果带撤销信息（逆工具 `remove`）；`remove` 返回删除的 id。
struct Todo;

impl ToolHandler for Todo {
    fn invoke(&self, call: CallHandle) {
        let result = if call.tool_name() == "add" {
            CallResult {
                data_json: Some(json!({"id": 3}).to_string()),
                undo: Some(UndoAction { tool: "remove".into(), arguments: json!({"id": 3}), label: Some("删除刚添加的待办".into()) }),
                ..CallResult::default()
            }
        } else {
            let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or_default();
            CallResult { data_json: Some(json!({"removed": args["id"]}).to_string()), ..CallResult::default() }
        };
        let _ = call.complete_with(result);
    }
}

fn start_todo_app(hub: *mut AmHub) -> App {
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("todo", "待办");
    cfg.host_url = format!("ws://{addr}/app");
    cfg.instance_id = Some("t1".into());
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let options = ToolOptions { undoable: true, ..ToolOptions::default() };
    let add = client.register_tool_with(ToolSpec::new("add", "添加待办"), options, Arc::new(Todo)).expect("注册");
    let remove = client.register_tool(ToolSpec::new("remove", "删除待办"), Arc::new(Todo)).expect("注册");
    client.start();
    App { client, _handles: vec![Box::new(add), Box::new(remove)] }
}

fn tool(hub: *mut AmHub, name: &str) -> Value {
    // SAFETY: 有效参数。
    let tools = query_json(|o| unsafe { am_hub_tools_json(hub, ptr::null(), o) });
    tools.as_array().and_then(|t| t.iter().find(|t| t["name"] == name).cloned()).unwrap_or(Value::Null)
}

fn undo_status(hub: *mut AmHub) -> Value {
    // SAFETY: 有效参数。
    query_json(|o| unsafe { am_hub_status_json(hub, o) })["undo"].clone()
}

#[test]
fn undoable_outcome_undo_apps_undo_and_status() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0","undo":{"ttlMs":60000,"maxPerTask":4}}"#);
    let _app = start_todo_app(hub);
    wait_tool(hub, "todo.add");
    wait_tool(hub, "todo.remove");
    assert_eq!(tool(hub, "todo.add")["undoable"], json!(true));
    let remove = tool(hub, "todo.remove");
    assert!(remove.get("undoable").is_none(), "{remove}");
    let builtin = tool(hub, "apps.undo");
    assert!(builtin.is_object() && builtin.get("undoable").is_none(), "{builtin}");
    assert_eq!(undo_status(hub), json!({"ttlMs": 60000, "maxPerTask": 4, "records": 0}));

    let (tx, rx) = mpsc::channel::<String>();
    call(hub, json!({"name": "todo.add", "arguments": {}, "callId": "add-1"}), &tx);
    let added = recv(&rx);
    assert_eq!(added["result"]["ok"], json!({"id": 3}), "{added}");
    assert_eq!(added["undo"]["label"], json!("删除刚添加的待办"), "{added}");
    assert!(added["undo"]["expiresInMs"].as_u64().is_some_and(|ms| ms > 0 && ms <= 60000), "{added}");
    assert!(added.get("undoOf").is_none(), "{added}");
    assert_eq!(undo_status(hub)["records"], json!(1));

    // apps.undo：逆调用结果原样、undoOf 指向原调用；之后同一 callId 不存在
    call(hub, json!({"name": "apps.undo", "arguments": {"callId": "add-1"}}), &tx);
    let undone = recv(&rx);
    assert_eq!(undone["result"]["ok"], json!({"removed": 3}), "{undone}");
    assert_eq!(undone["undoOf"], json!("add-1"), "{undone}");
    assert!(undone.get("undo").is_none(), "{undone}");
    assert_eq!(undo_status(hub)["records"], json!(0));
    call(hub, json!({"name": "apps.undo", "arguments": {"callId": "add-1"}}), &tx);
    let again = recv(&rx);
    assert_eq!(again["result"]["error"]["kind"], json!("TOOL_NOT_FOUND"), "{again}");
    // SAFETY: 测试结束。
    unsafe { am_hub_free(hub) };
}

#[test]
fn undo_max_per_task_zero_disables_undo() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0","undo":{"maxPerTask":0}}"#);
    let _app = start_todo_app(hub);
    wait_tool(hub, "todo.add");
    assert!(tool(hub, "apps.undo").is_null(), "关闭时不列出 apps.undo");
    let (tx, rx) = mpsc::channel::<String>();
    call(hub, json!({"name": "todo.add", "arguments": {}}), &tx);
    let added = recv(&rx);
    assert_eq!(added["result"]["ok"], json!({"id": 3}), "{added}");
    assert!(added.get("undo").is_none(), "关闭时不登记：{added}");
    assert_eq!(undo_status(hub)["maxPerTask"], json!(0));
    // SAFETY: 测试结束。
    unsafe { am_hub_free(hub) };

    let cfg = c(r#"{"undo":{"maxRecords":1}}"#);
    let mut out = ptr::null_mut();
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_start(cfg.as_ptr(), &mut out) }, AmHubStatus::InvalidJson);
    assert!(out.is_null());
}
