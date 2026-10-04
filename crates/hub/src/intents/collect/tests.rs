//! 实现者收集的每条规则（T-09）：词表种子、查询匹配、兼容性分流、未知动词、默认标记与排序、格式不合法项跳过。

use serde_json::{Value, json};

use super::*;

fn tool(name: &str, implements: &[&str], schema: Value) -> HubTool {
    let (app, local) = name.split_once('.').unwrap();
    serde_json::from_value(json!({
        "name": name, "appId": app, "tool": local, "description": "d", "inputSchema": schema,
        "risk": "write", "activation": "background", "availability": "dormant", "implements": implements
    }))
    .unwrap()
}

fn send_schema() -> Value {
    json!({"type": "object", "properties": {"to": {"type": "array", "items": {"type": "string"}}, "text": {"type": "string"}}})
}

fn ids(entries: &[IntentEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.intent.as_str()).collect()
}

fn find<'a>(entries: &'a [IntentEntry], id: &str) -> &'a IntentEntry {
    entries.iter().find(|e| e.intent == id).unwrap()
}

fn tools_of(e: &IntentEntry) -> Vec<(&str, bool)> {
    e.implementations.iter().map(|i| (i.tool.as_str(), i.default)).collect()
}

#[test]
fn no_query_lists_whole_vocabulary_sorted() {
    let out = collect(&[], &BTreeMap::new(), None);
    assert_eq!(
        ids(&out),
        ["calendar.create@1", "file.share@1", "link.open@1", "media.play@1", "message.send@1", "navigation.start@1"]
    );
    assert!(out.iter().all(|e| e.known && e.description.is_some() && e.implementations.is_empty()));
}

#[test]
fn compatible_incompatible_and_unknown() {
    let tools = [
        tool("mail.send", &["message.send@1"], send_schema()),
        tool("chat.post", &["message.send@1"], json!({"type": "object", "properties": {"to": {"type": "string"}}})),
        tool("chat.custom", &["chat.ping@1", "message.send@2"], json!({"type": "object"})),
    ];
    let out = collect(&tools, &BTreeMap::new(), None);
    let send = find(&out, "message.send@1");
    assert_eq!(tools_of(send), [("mail.send", false)]);
    assert_eq!(send.implementations[0].app_id, "mail");
    assert_eq!(send.implementations[0].availability, Availability::Dormant);
    assert_eq!(send.incompatible.len(), 1);
    assert_eq!(send.incompatible[0].tool, "chat.post");
    assert!(send.incompatible[0].reason.contains("参数 to 的类型应为 array") && send.incompatible[0].reason.contains("缺少必填参数 text"));
    // 未知动词 / 未知版本：known false、无说明、不做兼容性检查
    for id in ["chat.ping@1", "message.send@2"] {
        let e = find(&out, id);
        assert!(!e.known && e.description.is_none() && e.incompatible.is_empty(), "{id}");
        assert_eq!(tools_of(e), [("chat.custom", false)]);
    }
    assert_eq!(out.len(), 8, "词表 6 个 + 2 个未知");
}

#[test]
fn query_with_and_without_version() {
    let tools = [
        tool("a.v1", &["message.send@1"], send_schema()),
        tool("a.v2", &["message.send@2"], json!({"type": "object"})),
        tool("a.other", &["link.open@1"], json!({"type": "object", "properties": {"url": {}}})),
    ];
    let any = collect(&tools, &BTreeMap::new(), Some(IntentQuery { verb: "message.send", version: None }));
    assert_eq!(ids(&any), ["message.send@1", "message.send@2"]);
    let v1 = collect(&tools, &BTreeMap::new(), Some(IntentQuery { verb: "message.send", version: Some(1) }));
    assert_eq!(ids(&v1), ["message.send@1"]);
    assert_eq!(tools_of(&v1[0]), [("a.v1", false)]);
    // 带版本、无人实现、不在词表：仍返回一条 known false
    let v9 = collect(&tools, &BTreeMap::new(), Some(IntentQuery { verb: "message.send", version: Some(9) }));
    assert_eq!(ids(&v9), ["message.send@9"]);
    assert!(!v9[0].known && v9[0].implementations.is_empty());
    // 不带版本、词表中没有、无人实现：空
    assert!(collect(&tools, &BTreeMap::new(), Some(IntentQuery { verb: "x.y", version: None })).is_empty());
}

#[test]
fn defaults_first_then_by_name() {
    let tools = [
        tool("zmail.send", &["message.send@1"], send_schema()),
        tool("bmail.send", &["message.send@1"], send_schema()),
        tool("amail.send", &["message.send@1"], send_schema()),
    ];
    let none = collect(&tools, &BTreeMap::new(), None);
    assert_eq!(tools_of(find(&none, "message.send@1")), [("amail.send", false), ("bmail.send", false), ("zmail.send", false)]);
    let plain: BTreeMap<String, String> = [("message.send".to_owned(), "zmail.send".to_owned())].into();
    let out = collect(&tools, &plain, None);
    assert_eq!(tools_of(find(&out, "message.send@1")), [("zmail.send", true), ("amail.send", false), ("bmail.send", false)]);
    // 带版本的键优先于不带版本的键
    let versioned: BTreeMap<String, String> =
        [("message.send".to_owned(), "zmail.send".to_owned()), ("message.send@1".to_owned(), "bmail.send".to_owned())].into();
    let out = collect(&tools, &versioned, None);
    assert_eq!(tools_of(find(&out, "message.send@1")), [("bmail.send", true), ("amail.send", false), ("zmail.send", false)]);
    // 默认指向不存在 / 不兼容的工具：没有任何 default 标记
    let missing: BTreeMap<String, String> = [("message.send".to_owned(), "gone.send".to_owned())].into();
    let out = collect(&tools, &missing, None);
    assert!(find(&out, "message.send@1").implementations.iter().all(|i| !i.default));
}

#[test]
fn malformed_items_skipped_and_serialization_shape() {
    let tools = [tool("a.t", &["bad", "link.open@1"], json!({"type": "object", "properties": {"url": {"type": "string"}}}))];
    let out = collect(&tools, &BTreeMap::new(), Some(IntentQuery { verb: "link.open", version: None }));
    assert_eq!(ids(&out), ["link.open@1"]);
    let v = serde_json::to_value(&out[0]).unwrap();
    assert_eq!(v["implementations"], json!([{"tool": "a.t", "appId": "a", "availability": "dormant"}]));
    assert!(v.get("incompatible").is_none(), "空时不序列化");
    assert_eq!(v["known"], true);
    let unknown = serde_json::to_value(IntentEntry::new("x.y", 1)).unwrap();
    assert!(unknown.get("description").is_none());
    let mut d = BTreeMap::new();
    d.insert("link.open".to_owned(), "a.t".to_owned());
    let v = serde_json::to_value(collect(&tools, &d, None)).unwrap();
    assert_eq!(v[2]["implementations"][0]["default"], true);
}
