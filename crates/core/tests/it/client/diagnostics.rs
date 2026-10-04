//! 异常输入与诊断。

use super::support::*;

// ---------------------------------------------------------------------------
// 异常输入
// ---------------------------------------------------------------------------

#[test]
fn malformed_messages_warn_without_panicking() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    for text in [
        "not json",
        "[]",
        "{}",
        r#"{"jsonrpc":"2.0"}"#,
        r#"{"jsonrpc":"2.0","id":1}"#,
        r#"{"jsonrpc":"2.0","method":5}"#,
        r#"{"jsonrpc":"2.0","id":999,"result":{}}"#,
        r#"{"jsonrpc":"2.0","id":"x","result":{}}"#,
        r#"{"jsonrpc":"2.0","method":"tools/cancel","params":42}"#,
        r#"{"jsonrpc":"2.0","method":"some/notification"}"#,
        r#"{"jsonrpc":"2.0","method":"app/pairingResult","params":{"status":"paired"}}"#,
        "\u{0}\u{ffff}",
    ] {
        h.c.handle_message(text, h.now);
        let ev = h.drain();
        assert_eq!(warnings(&ev), 1, "{text}");
        assert!(sends(&ev).is_empty(), "{text}");
    }
    assert_eq!(h.c.state(), &ConnectionState::Connected);

    // 参数错误的请求 → -32602
    for params in [json!(null), json!({"name": "a"}), json!({"callId": 1, "name": "a"}), json!([1, 2])] {
        let msgs = sends(&h.request(9, "tools/invoke", params));
        assert_eq!(msgs[0]["error"]["code"], -32602);
        assert_eq!(msgs[0]["id"], 9);
    }
    let msgs = sends(&h.request(10, "resources/read", json!({})));
    assert_eq!(msgs[0]["error"]["code"], -32602);
    assert_eq!(h.c.running_call_count(), 0);
}

#[test]
fn unknown_method_returns_method_not_found() {
    let mut h = Harness::new();
    h.connect();
    let ev = h.request(42, "tools/frobnicate", json!({}));
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["id"], 42);
    assert_eq!(msgs[0]["error"]["code"], -32601);

    // 握手期间同样
    let mut h = Harness::new();
    h.open();
    let msgs = sends(&h.recv(json!({"jsonrpc": "2.0", "id": "s", "method": "nope"})));
    assert_eq!(msgs[0], json!({"jsonrpc": "2.0", "id": "s", "error": {"code": -32601, "message": "method not found: nope"}}));
}

#[test]
fn messages_while_disconnected_are_ignored() {
    let mut h = Harness::new();
    h.c.handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, 0);
    let ev = h.drain();
    assert_eq!(warnings(&ev), 1);
    assert!(sends(&ev).is_empty());
    h.c.handle_connected(0);
    assert_eq!(warnings(&h.drain()), 1, "未请求连接时的 handle_connected 被忽略");
    h.c.handle_timeout(1_000_000);
    assert!(h.drain().is_empty());
    assert_eq!(h.c.state(), &ConnectionState::Idle);
}

#[test]
fn tool_name_with_app_id_prefix_registers_but_warns() {
    let mut h = Harness::new();
    h.c.register_tool(tool("cart.add")).unwrap();
    assert_eq!(warnings(&h.drain()), 0, "含 . 的局部名是正常的");
    h.c.register_tool(tool("shop.info")).unwrap();
    let ev = h.drain();
    let msg = ev
        .iter()
        .find_map(|e| match e {
            Event::Warning(w) => Some(w.clone()),
            _ => None,
        })
        .expect("以 appId. 开头应产生警告");
    assert!(msg.contains("shop.shop.info") && msg.contains("\"info\""), "{msg}");
    // 协议语义不变：按原样注册，同步时名称不改写。
    let ev = h.connect();
    let sync = sends(&ev).into_iter().find(|m| m["method"] == "tools/sync").unwrap();
    let names: Vec<&str> =
        sync["params"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"shop.info"), "{names:?}");
}

// ---------------------------------------------------------------------------
// 诊断：错误码、连接 ID、app/diagnostic（spec/protocol.md 第 10 节）
// ---------------------------------------------------------------------------

#[test]
fn reject_codes_come_from_host() {
    let mut h = Harness::new();
    let hello = h.open();
    h.hello_result(
        &hello,
        json!({"status": "rejected", "reason": "来源不允许", "code": "ORIGIN_NOT_ALLOWED", "protocolVersion": "1", "hostVersion": "0.1.0"}),
    );
    assert_eq!(
        h.c.state(),
        &ConnectionState::Rejected { reason: "来源不允许".into(), code: ConnectionErrorCode::OriginNotAllowed }
    );

    // 不认识的码（新版本）→ REJECTED
    let mut h = Harness::new();
    let hello = h.open();
    h.hello_result(&hello, json!({"status": "pending", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    h.recv(json!({"jsonrpc": "2.0", "method": "app/pairingResult", "params": {"status": "rejected", "code": "SOMETHING_NEW"}}));
    assert_eq!(h.c.state().code(), Some(ConnectionErrorCode::Rejected));

    let mut h = Harness::new();
    let hello = h.open();
    h.hello_result(&hello, json!({"status": "pending", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    h.recv(json!({"jsonrpc": "2.0", "method": "app/pairingResult", "params": {"status": "rejected", "reason": "拒绝", "code": "PAIRING_REJECTED"}}));
    assert_eq!(h.c.state().code(), Some(ConnectionErrorCode::PairingRejected));
    assert_eq!(h.c.state().reason(), Some("拒绝"));
}

#[test]
fn connect_failure_and_handshake_timeout_carry_codes() {
    let mut h = Harness::new();
    h.c.start(h.now);
    h.drain();
    h.c.handle_connect_failed(
        ConnectionIssue::new(ConnectionErrorCode::HostNotRunning, "连接 unix:/x 失败：No such file"),
        h.now,
    );
    let ev = h.drain();
    assert_eq!(
        ev.last(),
        Some(&Event::StateChanged(ConnectionState::Backoff {
            retry_at: h.now + 500,
            reason: Some("连接 unix:/x 失败：No such file".into()),
            code: Some(ConnectionErrorCode::HostNotRunning),
        }))
    );

    // 握手超时
    let mut h = Harness::new();
    h.open();
    let ev = h.advance(10_000);
    assert!(ev.contains(&Event::Disconnect));
    assert_eq!(h.c.state().code(), Some(ConnectionErrorCode::HandshakeTimeout));
}

#[test]
fn disconnect_with_issue_carries_code() {
    let mut h = Harness::new();
    h.connect();
    h.c.handle_disconnected_with(ConnectionIssue::new(ConnectionErrorCode::ConnectionClosed, "Host 关闭了连接"), h.now);
    let ev = h.drain();
    assert_eq!(
        ev.last(),
        Some(&Event::StateChanged(ConnectionState::Backoff {
            retry_at: h.now + 500,
            reason: Some("Host 关闭了连接".into()),
            code: Some(ConnectionErrorCode::ConnectionClosed),
        }))
    );
    // 已在 Backoff：再次报告断开不改变状态
    h.c.handle_disconnected_with(ConnectionIssue::new(ConnectionErrorCode::ConnectionLost, "x"), h.now);
    assert!(h.drain().is_empty());
    assert_eq!(h.c.state().code(), Some(ConnectionErrorCode::ConnectionClosed));

    // 无原因的断开仍不带码（向后兼容）
    let mut h = Harness::new();
    h.connect();
    h.c.handle_disconnected(h.now);
    assert_eq!(h.c.state().code(), None);
}

#[test]
fn connection_id_from_hello() {
    let mut h = Harness::new();
    let hello = h.open();
    assert_eq!(h.c.connection_id(), None);
    h.hello_result(
        &hello,
        json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "0.1.0", "connectionId": "3f9a1c-12"}),
    );
    assert_eq!(h.c.connection_id(), Some("3f9a1c-12"));
    h.c.handle_disconnected(h.now);
    assert_eq!(h.c.connection_id(), None, "断开后清除");
    // 旧 Host 不带
    let mut h = Harness::new();
    h.connect();
    assert_eq!(h.c.connection_id(), None);
}

#[test]
fn reported_issues_are_sent_after_handshake() {
    let mut h = Harness::new();
    h.c.report_issue("BLOCKED_CSP", "CSP 不允许");
    h.c.report_issue("BLOCKED_LOCAL_NETWORK_ACCESS", "LNA 1");
    h.c.report_issue("BLOCKED_LOCAL_NETWORK_ACCESS", "LNA 2");
    let ev = h.connect();
    let msgs = sends(&ev);
    assert_eq!(
        methods(&msgs),
        vec!["tools/sync", "resources/sync", "app/visibility", "app/ready", "app/diagnostic", "app/diagnostic"]
    );
    assert_eq!(msgs[4]["params"], json!({"code": "BLOCKED_CSP", "message": "CSP 不允许", "count": 1}));
    assert_eq!(msgs[5]["params"], json!({"code": "BLOCKED_LOCAL_NETWORK_ACCESS", "message": "LNA 2", "count": 2}));

    // 已上报的不再发送；已连接时直接发出
    h.c.handle_disconnected(h.now);
    h.drain();
    let mut h2 = Harness::new();
    h2.connect();
    h2.c.report_issue("BLOCKED_CSP", "x");
    assert_eq!(methods(&sends(&h2.drain())), vec!["app/diagnostic"]);

    // 不同的码最多积累 MAX_PENDING_DIAGNOSTICS 个
    let mut h3 = Harness::new();
    for i in 0..(MAX_PENDING_DIAGNOSTICS + 5) {
        h3.c.report_issue(&format!("CODE_{i}"), "m");
    }
    let ev = h3.connect();
    let n = methods(&sends(&ev)).iter().filter(|m| **m == "app/diagnostic").count();
    assert_eq!(n, MAX_PENDING_DIAGNOSTICS);
}
