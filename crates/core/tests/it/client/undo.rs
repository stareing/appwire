//! 第 15 项 X2：撤销（spec/protocol.md 3.8）——`undoable` 声明随定义同步与 `toolsHash`、结果 `undo` 的发送与发送前校验。

use super::support::*;

fn undoable(name: &str) -> ToolDef {
    ToolDef { undoable: true, ..tool(name) }
}

fn complete(h: &mut Harness, id: i64, call_id: &str, out: CallOutput) -> Vec<Event> {
    h.invoke(id, call_id, "a", None);
    h.c.complete_call(call_id, Ok(out), h.now).unwrap();
    h.drain()
}

fn result(ev: &[Event]) -> Value {
    sends(ev).into_iter().next().expect("调用结果")["result"].clone()
}

/// 同步带 `undoable: true`，未声明不序列化；更新发 `tools/changed`。
#[test]
fn undoable_syncs_and_updates() {
    let mut h = Harness::new();
    let a = h.c.register_tool(undoable("a")).unwrap();
    h.c.register_tool(tool("plain")).unwrap();
    let msgs = sends(&h.connect());
    let sync = msgs.iter().find(|m| m["method"] == "tools/sync").unwrap();
    let tools = sync["params"]["tools"].as_array().unwrap();
    let by_name = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap().clone();
    assert_eq!(by_name("a")["undoable"], json!(true));
    assert!(by_name("plain").get("undoable").is_none());

    h.c.update_tool(a, ToolUpdate { undoable: Some(false), ..Default::default() }).unwrap();
    let msgs = sends(&h.drain());
    let up = &msgs.iter().find(|m| m["method"] == "tools/changed").expect("tools/changed")["params"]["upserted"][0];
    assert_eq!(up["name"], "a");
    assert!(up.get("undoable").is_none(), "false 不序列化");
    assert!(!h.c.tool_def(a).unwrap().undoable);
}

/// `toolsHash`：声明后变化、取消后恢复。
#[test]
fn tools_hash_reflects_undoable() {
    let mut h = Harness::new();
    let id = h.c.register_tool(tool("a")).unwrap();
    let base = h.c.tools_hash();
    h.c.update_tool(id, ToolUpdate { undoable: Some(true), ..Default::default() }).unwrap();
    assert_ne!(h.c.tools_hash(), base);
    h.c.update_tool(id, ToolUpdate { undoable: Some(false), ..Default::default() }).unwrap();
    assert_eq!(h.c.tools_hash(), base);
}

/// 合法 `undo` 原样发送（完整与最小形式）；任何 `status` 都照发（忽略规则在 Hub）；无警告。
#[test]
fn valid_undo_is_sent_as_is() {
    let mut h = Harness::new();
    h.c.register_tool(undoable("a")).unwrap();
    h.connect();
    let full = UndoAction { tool: "todo.remove".into(), arguments: json!({"id": 3}), label: Some("删除刚添加的待办".into()) };
    let ev = complete(&mut h, 1, "c1", CallOutput { data: json!({"id": 3}), undo: Some(full), ..Default::default() });
    assert_eq!(warnings(&ev), 0);
    assert_eq!(
        result(&ev),
        json!({"data": {"id": 3}, "undo": {"tool": "todo.remove", "arguments": {"id": 3}, "label": "删除刚添加的待办"}})
    );
    let ev = complete(
        &mut h,
        2,
        "c2",
        CallOutput { status: ResultStatus::Pending, undo: Some(UndoAction::new("a")), ..Default::default() },
    );
    assert_eq!(warnings(&ev), 0);
    assert_eq!(result(&ev), json!({"data": null, "status": "pending", "undo": {"tool": "a", "arguments": {}}}));
}

/// 不合法 `undo`（每条规则一例）：去掉 `undo`、产生警告，结果其余部分照常发送；调用去重重放同样不带。
#[test]
fn invalid_undo_is_dropped_with_warning() {
    let mut h = Harness::new();
    h.c.register_tool(undoable("a")).unwrap();
    h.connect();
    let bad = [
        UndoAction::new("bad name"),
        UndoAction { arguments: json!([1]), ..UndoAction::new("t") },
        UndoAction { arguments: json!({"v": "x".repeat(MAX_UNDO_ARGUMENTS_BYTES)}), ..UndoAction::new("t") },
        UndoAction { label: Some(" ".into()), ..UndoAction::new("t") },
    ];
    for (i, undo) in bad.into_iter().enumerate() {
        let call_id = format!("c{i}");
        let out = CallOutput { data: json!({"ok": true}), summary: Some("已完成".into()), undo: Some(undo.clone()), ..Default::default() };
        let ev = complete(&mut h, i as i64, &call_id, out);
        assert_eq!(warnings(&ev), 1, "{undo:?}");
        assert!(ev.iter().any(|e| matches!(e, Event::Warning(m) if m.contains("\"a\"") && m.contains("undo"))), "{ev:?}");
        assert_eq!(result(&ev), json!({"data": {"ok": true}, "summary": "已完成"}), "{undo:?}");
    }
    // 重复到达的 callId 重放首次（已去掉 undo 的）结果
    let ev = h.invoke(9, "c0", "a", None);
    assert_eq!(result(&ev), json!({"data": {"ok": true}, "summary": "已完成"}));
}
