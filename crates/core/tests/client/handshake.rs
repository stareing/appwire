//! 握手。

use super::support::*;

// ---------------------------------------------------------------------------
// 握手
// ---------------------------------------------------------------------------

#[test]
fn handshake_message_order() {
    let mut h = Harness::new();
    h.c.register_tool(tool("cart.add")).unwrap();
    let mut hidden = tool("cart.hidden");
    hidden.enabled = false;
    h.c.register_tool(hidden).unwrap();
    h.c.register_resource(resource("cart.state")).unwrap();
    assert!(h.drain().is_empty(), "未连接时注册不产生事件");

    h.c.start(h.now);
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
    h.c.start(h.now);
    assert!(h.drain().is_empty(), "重复 start 无效果");

    h.c.handle_connected(h.now);
    let ev = h.drain();
    assert_eq!(ev.last(), Some(&Event::StateChanged(ConnectionState::Handshaking)));
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["app/hello"]);
    let hello = &msgs[0];
    assert_eq!(hello["params"]["appId"], "shop");
    assert_eq!(hello["params"]["protocolVersion"], "1");
    assert_eq!(hello["params"]["clientKind"], "web");
    assert_eq!(hello["params"]["instanceId"], "inst-1");
    assert!(hello["params"].get("token").is_none());
    assert!(hello["id"].is_number());

    // 握手期间的注册变更不单独发送
    h.c.register_tool(tool("late")).unwrap();
    assert!(h.drain().is_empty());

    let ev = h.hello_result(
        hello,
        json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "0.1.0"}),
    );
    assert_eq!(ev.first(), Some(&Event::Paired { token: "tk".into() }));
    assert_eq!(ev.last(), Some(&Event::StateChanged(ConnectionState::Connected)));
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["tools/sync", "resources/sync", "app/visibility", "app/ready"]);
    let names: Vec<&str> = msgs[0]["params"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["cart.add", "late"], "只同步已启用的工具，按注册顺序");
    assert_eq!(msgs[0]["params"]["tools"][0]["inputSchema"]["type"], "object");
    assert_eq!(msgs[0]["params"]["tools"][0]["risk"], "read");
    assert_eq!(msgs[1]["params"]["resources"][0]["name"], "cart.state");
    assert_eq!(msgs[2]["params"], json!({"visibility": "visible", "focused": true}));
    assert_eq!(msgs[3]["params"], json!({}));
    assert_eq!(h.c.token(), Some("tk"));
    // 已同步，无后续 changed
    assert!(h.drain().is_empty());
}

#[test]
fn existing_token_is_sent_and_not_repersisted() {
    let mut cfg = config();
    cfg.token = Some("tk".into());
    cfg.launch_token = Some("launch".into());
    let mut h = Harness::with(cfg);
    let hello = h.open();
    assert_eq!(hello["params"]["token"], "tk");
    assert_eq!(hello["params"]["launchToken"], "launch");
    let ev = h.hello_result(
        &hello,
        json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "0.1.0"}),
    );
    assert!(!ev.iter().any(|e| matches!(e, Event::Paired { .. })), "token 未变化时不通知持久化");

    // 重连时不再携带一次性 launchToken
    h.c.handle_disconnected(h.now);
    h.drain();
    h.advance(500);
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain())[0].clone();
    assert_eq!(hello["params"]["token"], "tk");
    assert!(hello["params"].get("launchToken").is_none());
}

#[test]
fn pending_then_paired() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    let hello = h.open();
    let ev = h.hello_result(&hello, json!({"status": "pending", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    assert_eq!(ev, vec![Event::StateChanged(ConnectionState::PendingPairing)]);

    // 配对等待期间的调用 → UNAUTHORIZED
    let ev = h.invoke(7, "c1", "a", None);
    let msgs = sends(&ev);
    assert_eq!(error_kind(&msgs[0]), "UNAUTHORIZED");
    assert!(invoked(&ev).is_empty());

    let ev = h.recv(json!({"jsonrpc": "2.0", "method": "app/pairingResult", "params": {"status": "paired", "token": "new"}}));
    assert_eq!(ev.first(), Some(&Event::Paired { token: "new".into() }));
    assert_eq!(methods(&sends(&ev)), vec!["tools/sync", "resources/sync", "app/visibility", "app/ready"]);
    assert_eq!(h.c.state(), &ConnectionState::Connected);
}

#[test]
fn pending_then_rejected_does_not_reconnect() {
    let mut h = Harness::new();
    let hello = h.open();
    h.hello_result(&hello, json!({"status": "pending", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    let ev = h.recv(json!({"jsonrpc": "2.0", "method": "app/pairingResult", "params": {"status": "rejected", "reason": "用户拒绝"}}));
    assert_eq!(
        ev,
        vec![Event::Disconnect, Event::StateChanged(ConnectionState::Rejected { reason: "用户拒绝".into(), code: ConnectionErrorCode::Rejected })]
    );
    assert_eq!(h.c.poll_timeout(), None);
    h.c.handle_disconnected(h.now);
    assert!(h.drain().is_empty());
    assert!(h.advance(100_000).is_empty());
    assert!(matches!(h.c.state(), ConnectionState::Rejected { .. }));
}

#[test]
fn hello_rejected_or_error_is_final() {
    let mut h = Harness::new();
    let hello = h.open();
    let ev = h.hello_result(
        &hello,
        json!({"status": "rejected", "reason": "协议版本不兼容", "protocolVersion": "2", "hostVersion": "9"}),
    );
    assert!(ev.contains(&Event::Disconnect));
    assert_eq!(
        h.c.state(),
        &ConnectionState::Rejected { reason: "协议版本不兼容".into(), code: ConnectionErrorCode::Rejected }
    );

    let mut h = Harness::new();
    let hello = h.open();
    let ev = h.recv(json!({"jsonrpc": "2.0", "id": hello["id"], "error": {"code": -32015, "message": "bad version"}}));
    assert!(ev.contains(&Event::Disconnect));
    assert!(matches!(h.c.state(), ConnectionState::Rejected { .. }));
    assert_eq!(h.c.poll_timeout(), None);
}

/// 握手结果中的 Host 身份（spec/protocol.md 1.6）。
#[test]
fn host_identity_mismatch_stops_retrying() {
    let mismatch = |h: &Harness| matches!(h.c.state(), ConnectionState::HostMismatch { .. });
    let paired = |extra: Value| {
        let mut v = json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "0.1.0"});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        v
    };

    // service 不是 app-mcp
    let mut h = Harness::new();
    let hello = h.open();
    let ev = h.hello_result(&hello, paired(json!({"service": "other"})));
    assert!(ev.contains(&Event::Disconnect));
    assert!(mismatch(&h), "{:?}", h.c.state());
    assert_eq!(h.c.state().code(), Some(ConnectionErrorCode::HostNotAppMcp));
    assert_eq!(h.c.poll_timeout(), None, "不自动重试");
    assert!(sends(&ev).is_empty(), "不发送 tools/sync");

    // 不认识 app/hello（标准 -32601）、结果无法解析：同样判定不是 app-mcp
    let mut h = Harness::new();
    let hello = h.open();
    h.recv(json!({"jsonrpc": "2.0", "id": hello["id"], "error": {"code": -32601, "message": "method not found"}}));
    assert!(mismatch(&h));
    let mut h = Harness::new();
    let hello = h.open();
    h.hello_result(&hello, json!({"hello": "world"}));
    assert!(mismatch(&h));

    // 用户不同；wake / connect_now 再试一次
    let mut cfg = config();
    cfg.expected_host_user = Some("1000".into());
    let mut h = Harness::with(cfg);
    let hello = h.open();
    h.hello_result(&hello, paired(json!({"service": "app-mcp", "user": "1001", "pid": 9})));
    let ConnectionState::HostMismatch { reason, code } = h.c.state() else { panic!("{:?}", h.c.state()) };
    assert_eq!(*code, ConnectionErrorCode::HostOtherUser);
    assert!(reason.contains("1001") && reason.contains("pid 9"), "{reason}");
    assert!(h.c.wake(h.now));
    assert!(h.drain().contains(&Event::Connect));
    assert_eq!(h.c.state(), &ConnectionState::Connecting);
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain()).remove(0);
    h.hello_result(&hello, paired(json!({"service": "app-mcp", "user": "1000"})));
    assert_eq!(h.c.state(), &ConnectionState::Connected);

    // 旧 Host 不带身份字段：无法核对，照常连接
    let mut cfg = config();
    cfg.expected_host_user = Some("1000".into());
    let mut h = Harness::with(cfg);
    let hello = h.open();
    h.hello_result(&hello, paired(json!({})));
    assert_eq!(h.c.state(), &ConnectionState::Connected);
}
