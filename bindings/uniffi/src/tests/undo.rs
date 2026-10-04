//! 撤销（spec/protocol.md 3.8）：`undoable` 声明透传与更新、结果 `undo` 的记录转换（非法参数交给核心去掉，不让调用失败）。

use std::sync::Arc;

use app_mcp_native as native;
use serde_json::json;

use crate::*;

struct NoopTool;

impl ToolHandler for NoopTool {
    fn invoke(&self, call: Arc<Call>) {
        let _ = call.complete(None, Vec::new());
    }
}

fn tool(name: &str, undoable: bool) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: "d".into(),
        input_schema_json: None,
        risk: None,
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
        cache: None,
        deprecated: None,
        undoable,
    }
}

fn result_with(undo: Option<UndoAction>) -> native::CallResult {
    CallResult { data_json: None, state_hints: Vec::new(), status: ResultStatus::Done, state_resource: None, summary: None, annotations: None, undo }
        .into()
}

#[test]
fn undoable_conversion() {
    let (_, options): (native::ToolSpec, native::ToolOptions) = tool("t.add", false).into();
    assert!(!options.undoable, "未声明时为 false");
    let (_, options): (native::ToolSpec, native::ToolOptions) = tool("t.add", true).into();
    assert!(options.undoable, "声明原样透传");
}

#[test]
fn undo_conversion() {
    assert_eq!(result_with(None).undo, None, "未给出时为空");
    let full = UndoAction { tool: "todo.remove".into(), arguments_json: Some(r#"{"id":3}"#.into()), label: Some("删除刚添加的待办".into()) };
    let u = result_with(Some(full)).undo.expect("undo");
    assert_eq!((u.tool.as_str(), &u.arguments, u.label.as_deref()), ("todo.remove", &json!({"id": 3}), Some("删除刚添加的待办")));
    let min = result_with(Some(UndoAction { tool: "t".into(), arguments_json: None, label: None })).undo.expect("最小形式");
    assert_eq!(min, native::UndoAction::new("t"), "参数缺省为 {{}}");
    // 非法 JSON 不在绑定层报错：原样成为字符串，由核心校验（arguments 须为对象）后去掉并记警告。
    let bad = result_with(Some(UndoAction { tool: "t".into(), arguments_json: Some("{".into()), label: None })).undo.expect("bad");
    assert_eq!(bad.arguments, json!("{"));
    assert!(bad.validate().is_err(), "核心按协议规则判为不合法");
}

/// toolsHash 含 undoable：注册、启停、更新替换 / 清除都按声明变化，不被丢掉。
#[test]
fn update_replaces_and_clears_undoable() {
    let hash_with = |undoable: bool| {
        let mut cfg = super::fake_host_config("uniffi-undoable", "127.0.0.1:9");
        cfg.connect_timeout_ms = Some(1000);
        let client = AppMcpClient::new(cfg, None).expect("client");
        let t = client.register_tool(tool("u.t", undoable), Arc::new(NoopTool)).expect("tool");
        let h = client.tools_hash();
        (client, t, h)
    };
    let (c0, _t0, none) = hash_with(false);
    c0.stop();
    let (client, t, yes) = hash_with(true);
    assert_ne!(none, yes, "undoable 进 toolsHash");
    t.set_enabled(false).expect("disable");
    t.set_enabled(true).expect("enable");
    assert_eq!(client.tools_hash(), yes, "启停不丢 undoable");
    t.update(tool("u.t", false)).expect("清除");
    assert_eq!(client.tools_hash(), none, "更新为 false 即清除");
    t.update(tool("u.t", true)).expect("恢复");
    assert_eq!(client.tools_hash(), yes);
    client.stop();
}
