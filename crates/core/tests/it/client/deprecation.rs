//! 第 16 项 O4：工具弃用声明（spec/protocol.md 3.7）——注册 / 更新校验、随定义同步、`toolsHash`、必填参数标弃用的警告。

use super::support::*;

fn dep(message: &str, replacement: Option<&str>) -> Deprecation {
    Deprecation { message: message.into(), replacement: replacement.map(Into::into), until: Some("2027-06-30".into()) }
}

fn deprecated(name: &str, d: Option<Deprecation>) -> ToolDef {
    ToolDef { deprecated: d, ..tool(name) }
}

fn upserted(ev: &[Event]) -> Value {
    sends(ev).into_iter().find(|m| m["method"] == "tools/changed").expect("tools/changed")["params"]["upserted"][0].clone()
}

/// 格式不合法（每条规则一例）：注册返回 `InvalidDeprecation`，未注册；`replacement` 指向未注册工具照常注册。
#[test]
fn register_rejects_invalid_and_allows_unknown_replacement() {
    let mut h = Harness::new();
    let bad = [
        dep("", None),
        dep(&"x".repeat(MAX_DEPRECATION_MESSAGE_CHARS + 1), None),
        dep("m", Some("bad name")),
        dep("m", Some("old")),
        Deprecation { until: Some("2027-02-30".into()), ..dep("m", None) },
    ];
    for d in bad {
        let r = h.c.register_tool(deprecated("old", Some(d.clone())));
        assert!(matches!(r, Err(CoreError::InvalidDeprecation(_))), "{d:?}: {r:?}");
    }
    let id = h.c.register_tool(deprecated("old", Some(dep("改用 new", Some("new"))))).unwrap();
    assert_eq!(warnings(&h.drain()), 0);
    assert_eq!(h.c.tool_def(id).unwrap().deprecated, Some(dep("改用 new", Some("new"))));
}

/// 同步带字段、未声明不序列化；更新非法时拒绝且不变、不发变更；合法更新与清除都发 `tools/changed`。
#[test]
fn sync_update_and_clear() {
    let mut h = Harness::new();
    let old = h.c.register_tool(deprecated("old", Some(dep("改用 new", Some("new"))))).unwrap();
    h.c.register_tool(tool("plain")).unwrap();
    let msgs = sends(&h.connect());
    let sync = msgs.iter().find(|m| m["method"] == "tools/sync").unwrap();
    let tools = sync["params"]["tools"].as_array().unwrap();
    let by_name = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap().clone();
    assert_eq!(by_name("old")["deprecated"], json!({"message": "改用 new", "replacement": "new", "until": "2027-06-30"}));
    assert!(by_name("plain").get("deprecated").is_none());

    let r = h.c.update_tool(old, ToolUpdate { deprecated: Some(Some(dep("m", Some("old")))), ..Default::default() });
    assert!(matches!(r, Err(CoreError::InvalidDeprecation(ref m)) if m.contains("自身")), "{r:?}");
    assert_eq!(h.c.tool_def(old).unwrap().deprecated, Some(dep("改用 new", Some("new"))));
    assert!(h.drain().is_empty());

    h.c.update_tool(old, ToolUpdate { deprecated: Some(Some(dep("v2", None))), ..Default::default() }).unwrap();
    assert_eq!(upserted(&h.drain())["deprecated"], json!({"message": "v2", "until": "2027-06-30"}));
    h.c.update_tool(old, ToolUpdate { deprecated: Some(None), ..Default::default() }).unwrap();
    let up = upserted(&h.drain());
    assert_eq!(up["name"], "old");
    assert!(up.get("deprecated").is_none(), "清除后不再序列化");
}

/// `toolsHash`：声明后变化、清除后恢复（未声明时与旧定义相同由 lifecycle::wake 固定向量保证）。
#[test]
fn tools_hash_reflects_declaration() {
    let mut h = Harness::new();
    let id = h.c.register_tool(tool("old")).unwrap();
    let base = h.c.tools_hash();
    h.c.update_tool(id, ToolUpdate { deprecated: Some(Some(dep("m", None))), ..Default::default() }).unwrap();
    let declared = h.c.tools_hash();
    assert_ne!(declared, base);
    h.c.update_tool(id, ToolUpdate { deprecated: Some(None), ..Default::default() }).unwrap();
    assert_eq!(h.c.tools_hash(), base);
}

/// 必填参数标 `deprecated: true`：照常注册并警告；改 `inputSchema` 时重新检查，可选参数标弃用不警告。
#[test]
fn deprecated_required_param_warns() {
    let mut h = Harness::new();
    let schema = |required: Value| {
        json!({"type": "object", "properties": {"q": {"type": "string", "deprecated": true}, "k": {"type": "string"}}, "required": required})
    };
    let id = h.c.register_tool(ToolDef { input_schema: schema(json!(["q", "k"])), ..tool("search") }).unwrap();
    let ev = h.drain();
    let texts: Vec<&String> = ev.iter().filter_map(|e| if let Event::Warning(w) = e { Some(w) } else { None }).collect();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(texts[0].contains("\"search\"") && texts[0].contains("\"q\""), "{texts:?}");
    h.c.update_tool(id, ToolUpdate { input_schema: Some(schema(json!(["k"]))), ..Default::default() }).unwrap();
    assert_eq!(warnings(&h.drain()), 0, "q 改为可选后不警告");
    h.c.update_tool(id, ToolUpdate { input_schema: Some(schema(json!(["q"]))), ..Default::default() }).unwrap();
    assert_eq!(warnings(&h.drain()), 1, "改回必填重新警告");
    h.c.update_tool(id, ToolUpdate { description: Some("d".into()), ..Default::default() }).unwrap();
    assert_eq!(warnings(&h.drain()), 0, "未改 inputSchema 不重复警告");
}
