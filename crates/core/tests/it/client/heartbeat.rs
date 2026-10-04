//! 心跳与重连。

use super::support::*;

// ---------------------------------------------------------------------------
// 心跳
// ---------------------------------------------------------------------------

fn find_ping(events: &[Event]) -> Option<Value> {
    sends(events).into_iter().find(|m| m["method"] == "ping")
}

#[test]
fn heartbeat_ping_and_response() {
    let mut h = Harness::new();
    h.connect();
    let t0 = h.now;
    assert_eq!(h.c.poll_timeout(), Some(t0 + 15_000));
    assert!(find_ping(&h.advance(14_999)).is_none());
    let ping = find_ping(&h.advance(1)).expect("ping sent");
    assert!(ping["id"].is_number());
    assert!(ping.get("params").is_none());
    assert_eq!(h.c.poll_timeout(), Some(t0 + 25_000), "等待响应超时");

    // 其他消息不会重置心跳
    h.now += 5_000;
    h.recv(json!({"jsonrpc": "2.0", "id": "h1", "method": "ping"}));
    assert_eq!(h.c.poll_timeout(), Some(t0 + 25_000));

    let ev = h.recv(json!({"jsonrpc": "2.0", "id": ping["id"], "result": {}}));
    assert!(ev.is_empty());
    assert_eq!(h.c.poll_timeout(), Some(t0 + 30_000));
    assert!(find_ping(&h.advance(10_000)).is_some());
    assert_eq!(h.c.state(), &ConnectionState::Connected);
}

#[test]
fn heartbeat_timeout_visible() {
    let mut h = Harness::new();
    h.connect();
    h.advance(15_000);
    assert!(h.advance(9_999).iter().all(|e| *e != Event::Disconnect));
    let ev = h.advance(1);
    assert_eq!(warnings(&ev), 1);
    assert!(ev.contains(&Event::Disconnect));
    let Some(Event::StateChanged(ConnectionState::Backoff { retry_at, code, .. })) = ev.last() else {
        panic!("{ev:?}")
    };
    assert_eq!((*retry_at, *code), (h.now + 500, Some(ConnectionErrorCode::HeartbeatTimeout)));

    // 核心自行完成断开处理，随后按退避重连
    let ev = h.advance(500);
    assert_eq!(ev, vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
}

#[test]
fn heartbeat_timeout_hidden_is_longer() {
    let mut h = Harness::new();
    h.connect();
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["method"], "app/visibility");
    assert_eq!(msgs[0]["params"], json!({"visibility": "hidden", "focused": false}));
    // 值未变化：不发送
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    assert!(h.drain().is_empty());

    h.advance(15_000);
    assert!(!h.advance(10_000).contains(&Event::Disconnect));
    assert!(!h.advance(109_999).contains(&Event::Disconnect));
    assert!(h.advance(1).contains(&Event::Disconnect));
}

#[test]
fn visibility_before_connect_is_reported_in_handshake() {
    let mut h = Harness::new();
    h.c.set_visibility(Visibility::Frozen, false, h.now);
    assert!(h.drain().is_empty());
    let ev = h.connect();
    let vis = sends(&ev).into_iter().find(|m| m["method"] == "app/visibility").unwrap();
    assert_eq!(vis["params"], json!({"visibility": "frozen", "focused": false}));
}

#[test]
fn host_ping_and_activate() {
    let mut h = Harness::new();
    let hello = h.open();
    // 握手期间也响应 ping
    let msgs = sends(&h.request(1, "ping", Value::Null));
    assert_eq!(msgs[0], json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
    h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));

    let ev = h.request(2, "app/activate", json!({"mode": "foreground"}));
    assert_eq!(sends(&ev)[0], json!({"jsonrpc": "2.0", "id": 2, "result": {}}));
    assert_eq!(warnings(&ev), 1);
    let msgs = sends(&h.request(3, "app/activate", json!({"mode": "sideways"})));
    assert_eq!(msgs[0]["error"]["code"], -32602);
}

// ---------------------------------------------------------------------------
// 重连
// ---------------------------------------------------------------------------

fn retry_delay(h: &mut Harness) -> Millis {
    match h.c.state() {
        ConnectionState::Backoff { retry_at, .. } => retry_at - h.now,
        other => panic!("not in backoff: {other:?}"),
    }
}

#[test]
fn exponential_backoff_and_reset() {
    let mut h = Harness::new();
    h.c.start(h.now);
    h.drain();
    let mut delays = Vec::new();
    for _ in 0..9 {
        // Connecting 状态下连接失败
        h.c.handle_disconnected(h.now);
        h.drain();
        let d = retry_delay(&mut h);
        delays.push(d);
        assert_eq!(h.c.poll_timeout(), Some(h.now + d));
        let ev = h.advance(d);
        assert_eq!(ev, vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
    }
    assert_eq!(delays, vec![500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000, 30_000]);

    // 握手中断线也继续累加
    h.c.handle_connected(h.now);
    h.drain();
    h.c.handle_disconnected(h.now);
    h.drain();
    assert_eq!(retry_delay(&mut h), 30_000);
    h.advance(30_000);

    // 成功握手后清零
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain())[0].clone();
    h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    assert_eq!(h.c.state(), &ConnectionState::Connected);
    h.c.handle_disconnected(h.now);
    h.drain();
    assert_eq!(retry_delay(&mut h), 500);
}

#[test]
fn reconnect_resyncs_full_state() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.c.handle_disconnected(h.now);
    // 断线期间的变更
    h.c.register_tool(tool("b")).unwrap();
    let ev = h.drain();
    assert!(sends(&ev).is_empty());
    h.advance(500);
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain())[0].clone();
    let ev = h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["tools/sync", "resources/sync", "app/visibility", "app/ready"]);
    assert_eq!(msgs[0]["params"]["tools"].as_array().unwrap().len(), 2);
    assert!(h.drain().is_empty());
}
