//! 事件（第 16 项 N3，spec/protocol.md 3.5）：声明同步与发出。

use super::support::*;

pub(crate) fn event(name: &str) -> EventInfo {
    EventInfo { name: name.into(), description: format!("{name} 发生时"), payload_schema: None }
}

fn find<'a>(msgs: &'a [Value], method: &str) -> Vec<&'a Value> {
    msgs.iter().filter(|m| m["method"] == method).collect()
}

/// 断线后等到重连并完成握手，返回握手完成时的事件。
fn reconnect(h: &mut Harness) -> Vec<Event> {
    h.c.handle_disconnected(h.now);
    h.drain();
    let ev = h.advance(60_000);
    assert!(ev.contains(&Event::Connect), "{ev:?}");
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain()).remove(0);
    let ev = h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    assert_eq!(h.c.state(), &ConnectionState::Connected);
    ev
}

// ---------------------------------------------------------------------------
// 声明：events/sync
// ---------------------------------------------------------------------------

#[test]
fn handshake_sends_events_sync_after_resources_sync() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.c.declare_event(event("order.shipped")).unwrap();
    let mut with_schema = event("download.done");
    with_schema.payload_schema = Some(json!({"type": "object"}));
    h.c.declare_event(with_schema).unwrap();
    assert!(h.drain().is_empty(), "未连接时声明不产生事件");

    let msgs = sends(&h.connect());
    assert_eq!(methods(&msgs), vec!["tools/sync", "resources/sync", "events/sync", "app/visibility", "app/ready"]);
    assert_eq!(
        msgs[2]["params"],
        json!({"events": [
            {"name": "order.shipped", "description": "order.shipped 发生时"},
            {"name": "download.done", "description": "download.done 发生时", "payloadSchema": {"type": "object"}},
        ]})
    );
}

#[test]
fn no_declarations_no_events_sync() {
    let mut h = Harness::new();
    let msgs = sends(&h.connect());
    assert!(find(&msgs, "events/sync").is_empty(), "{msgs:?}");
}

#[test]
fn declarations_resent_on_every_handshake() {
    let mut h = Harness::new();
    h.c.declare_event(event("a")).unwrap();
    h.connect();
    let ev = reconnect(&mut h);
    let msgs = sends(&ev);
    assert_eq!(find(&msgs, "events/sync").len(), 1, "回连后再发全量：{msgs:?}");
}

#[test]
fn changes_while_connected_resend_full_list() {
    let mut h = Harness::new();
    h.c.declare_event(event("a")).unwrap();
    h.connect();

    h.c.declare_event(event("b")).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(methods(&msgs), vec!["events/sync"]);
    let names: Vec<&str> = msgs[0]["params"]["events"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["a", "b"]);

    h.c.declare_event(event("b")).unwrap();
    assert!(h.drain().is_empty(), "相同声明不重发");

    let mut changed = event("a");
    changed.description = "改了".into();
    h.c.declare_event(changed).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["params"]["events"][0]["description"], "改了", "同名替换保持位置");

    assert!(h.c.remove_event("a"));
    assert!(!h.c.remove_event("a"), "重复撤销返回 false");
    let msgs = sends(&h.drain());
    assert_eq!(methods(&msgs), vec!["events/sync"], "只有一次撤销生效");
    assert_eq!(msgs[0]["params"]["events"].as_array().unwrap().len(), 1);

    assert!(h.c.remove_event("b"));
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["params"], json!({"events": []}), "撤销全部也发全量");
}

#[test]
fn declare_rejects_invalid_name() {
    let mut h = Harness::new();
    assert_eq!(h.c.declare_event(event("bad name")), Err(CoreError::InvalidName("bad name".into())));
    assert_eq!(h.c.declare_event(event("")), Err(CoreError::InvalidName(String::new())));
    let msgs = sends(&h.connect());
    assert!(find(&msgs, "events/sync").is_empty(), "非法声明不入表");
}

// ---------------------------------------------------------------------------
// 发出：events/emit
// ---------------------------------------------------------------------------

#[test]
fn emit_when_connected_sends_with_monotonic_ids() {
    let mut h = Harness::new();
    h.c.declare_event(event("order.shipped")).unwrap();
    h.connect();

    assert_eq!(h.c.emit_event("order.shipped", Some(json!({"orderId": "o1"}))), Ok(true));
    assert_eq!(h.c.emit_event("order.shipped", None), Ok(true));
    let msgs = sends(&h.drain());
    assert_eq!(
        msgs[0],
        json!({"jsonrpc": "2.0", "method": "events/emit",
            "params": {"name": "order.shipped", "eventId": "e1", "payload": {"orderId": "o1"}}})
    );
    assert_eq!(msgs[1]["params"], json!({"name": "order.shipped", "eventId": "e2"}), "无载荷时省略 payload");
    assert!(msgs.iter().all(|m| m.get("id").is_none()), "通知，不等回复");

    // 跨重连继续递增
    reconnect(&mut h);
    assert_eq!(h.c.emit_event("order.shipped", None), Ok(true));
    assert_eq!(sends(&h.drain())[0]["params"]["eventId"], "e3");
}

#[test]
fn emit_local_errors_send_nothing() {
    let mut h = Harness::new();
    h.c.declare_event(event("a")).unwrap();
    h.connect();

    assert_eq!(h.c.emit_event("bad name", None), Err(CoreError::InvalidName("bad name".into())));
    assert_eq!(h.c.emit_event("b", None), Err(CoreError::UnknownEvent("b".into())));
    for bad in [json!(1), json!("s"), json!([1]), json!(null), json!(true)] {
        assert!(matches!(h.c.emit_event("a", Some(bad.clone())), Err(CoreError::InvalidEventPayload(_))), "{bad}");
    }
    assert!(h.drain().is_empty(), "出错时不发送");

    h.c.remove_event("a");
    h.drain();
    assert_eq!(h.c.emit_event("a", None), Err(CoreError::UnknownEvent("a".into())), "撤销后视为未声明");
    // 失败不消耗 eventId
    h.c.declare_event(event("a")).unwrap();
    h.drain();
    assert_eq!(h.c.emit_event("a", None), Ok(true));
    assert_eq!(sends(&h.drain())[0]["params"]["eventId"], "e1");
}

#[test]
fn emit_payload_size_limit() {
    let mut h = Harness::new();
    h.c.declare_event(event("a")).unwrap();
    h.connect();
    // {"k":"<n 个 x>"} 序列化长度 = n + 8
    let at_limit = json!({"k": "x".repeat(MAX_EVENT_PAYLOAD_BYTES - 8)});
    assert_eq!(serde_json::to_string(&at_limit).unwrap().len(), MAX_EVENT_PAYLOAD_BYTES);
    assert_eq!(h.c.emit_event("a", Some(at_limit)), Ok(true), "恰好等于上限可以发");
    h.drain();
    let over = json!({"k": "x".repeat(MAX_EVENT_PAYLOAD_BYTES - 7)});
    assert!(matches!(h.c.emit_event("a", Some(over)), Err(CoreError::InvalidEventPayload(_))));
    assert!(h.drain().is_empty());
}

#[test]
fn emit_when_not_connected_returns_false_without_side_effects() {
    let mut h = Harness::new();
    h.c.declare_event(event("a")).unwrap();

    // 尚未 start
    assert_eq!(h.c.emit_event("a", None), Ok(false));
    assert!(h.drain().is_empty());

    // 连接建立中、握手中
    h.c.start(h.now);
    h.drain();
    assert_eq!(h.c.emit_event("a", None), Ok(false));
    assert!(h.drain().is_empty(), "connecting");
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain()).remove(0);
    assert_eq!(h.c.emit_event("a", None), Ok(false));
    assert!(h.drain().is_empty(), "handshaking");

    // 握手完成后不补发之前丢弃的事件
    let msgs = sends(&h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"})));
    assert!(find(&msgs, "events/emit").is_empty(), "不缓存：{msgs:?}");

    // 断线（Backoff）：不触发连接，也不改变重连时刻
    h.c.handle_disconnected(h.now);
    h.drain();
    let timeout = h.c.poll_timeout();
    let state = h.c.state().clone();
    assert_eq!(h.c.emit_event("a", Some(json!({}))), Ok(false));
    assert!(h.drain().is_empty(), "backoff");
    assert_eq!(h.c.poll_timeout(), timeout);
    assert_eq!(h.c.state(), &state);

    // 停止后
    h.c.stop(h.now);
    h.drain();
    assert_eq!(h.c.emit_event("a", None), Ok(false));
    assert!(h.drain().is_empty());
}

#[test]
fn not_connected_still_validates() {
    let mut h = Harness::new();
    assert_eq!(h.c.emit_event("a", None), Err(CoreError::UnknownEvent("a".into())), "未声明的错误先于连接判断");
    h.c.declare_event(event("a")).unwrap();
    assert!(matches!(h.c.emit_event("a", Some(json!(1))), Err(CoreError::InvalidEventPayload(_))));
}
