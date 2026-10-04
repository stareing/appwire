//! 调用去重与进度。

use super::support::*;

// ---------------------------------------------------------------------------
// 调用去重（spec/protocol.md 3.3，第 16 项 U1 / N7a）
// ---------------------------------------------------------------------------

/// U1 复现：handler 已完成、结果还没发出连接就断了（结果随连接丢弃，Host 只能报"结果未知"）；
/// 同一 callId 在回连后再次到达时不能再执行一次，而是重放首次结果。
#[test]
fn u1_same_call_id_after_reconnect_runs_once() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    assert_eq!(invoked(&h.invoke(1, "c1", "a", None)), vec!["c1".to_owned()]);
    h.c.complete_call("c1", Ok(CallOutput { data: json!({"orderId": "o1"}), ..CallOutput::default() }), h.now).unwrap();
    // 驱动层还没取走响应，连接已断：响应被丢弃
    h.c.handle_disconnected(h.now);
    let ev = h.drain();
    assert!(sends(&ev).is_empty(), "断线时丢弃未发出的响应");

    h.advance(500);
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain())[0].clone();
    h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    let ev = h.invoke(7, "c1", "a", None);
    assert!(invoked(&ev).is_empty(), "同一 callId 不再执行 handler");
    let msgs = sends(&ev);
    assert_eq!(msgs, vec![json!({"jsonrpc": "2.0", "id": 7, "result": {"data": {"orderId": "o1"}}})]);
}

/// 执行中断线：首次执行被中断（handler 已收到取消），回连后同一 callId 得到"中断、结果未知"，不再执行。
#[test]
fn interrupted_call_replays_interruption() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.c.handle_disconnected(h.now);
    assert_eq!(cancelled(&h.drain()), vec![("c1".into(), CancelReason::Disconnected)]);
    h.advance(500);
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain())[0].clone();
    h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    let ev = h.invoke(2, "c1", "a", None);
    assert!(invoked(&ev).is_empty());
    let msgs = sends(&ev);
    assert_eq!(error_kind(&msgs[0]), "CANCELLED");
    assert_eq!(msgs[0]["error"]["data"]["interrupted"], true);
    assert_eq!(msgs[0]["error"]["data"]["callId"], "c1");
}

/// 已开始执行的超时 / 取消记为首次结果；排队中被取消、超时的调用 handler 没执行过，同一 callId 可以重新执行。
#[test]
fn dedup_records_only_started_calls() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "run", "a", Some(100));
    h.invoke(2, "queued", "a", None);
    h.recv(json!({"jsonrpc": "2.0", "method": "tools/cancel", "params": {"callId": "queued"}}));
    h.advance(100);
    assert_eq!(invoked(&h.drain()), Vec::<String>::new());
    let ev = h.invoke(3, "run", "a", None);
    assert!(invoked(&ev).is_empty());
    assert_eq!(error_kind(&sends(&ev)[0]), "TIMEOUT", "超时结果被重放");
    assert_eq!(warnings(&ev), 1, "重放记一条警告日志");
    assert_eq!(invoked(&h.invoke(4, "queued", "a", None)), vec!["queued".to_owned()], "排队中取消的可重新执行");
    h.recv(json!({"jsonrpc": "2.0", "method": "tools/cancel", "params": {"callId": "queued"}}));
    assert_eq!(error_kind(&sends(&h.invoke(5, "queued", "a", None))[0]), "CANCELLED", "执行中取消记为首次结果");
}

/// Agent 幂等键（spec/protocol.md 3.3，第 4f 项 j）：原样进入 `InvokeTool`；同一工具的同一幂等键以新 callId 到达时——
/// 执行中挂到同一次执行、完成后重放首次结果；其他工具或没有键的调用照常执行。去重关闭时只透传。
#[test]
fn idempotency_key_passthrough_and_dedup() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.c.register_tool(tool("b")).unwrap();
    h.connect();
    let invoke_keyed = |h: &mut Harness, id: i64, call: &str, name: &str, key: &str| {
        h.request(id, "tools/invoke", json!({"callId": call, "name": name, "arguments": {}, "idempotencyKey": key}))
    };
    let ev = invoke_keyed(&mut h, 1, "c1", "a", "k1");
    let keys: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            Event::InvokeTool { idempotency_key, .. } => Some(idempotency_key.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(keys, vec![Some("k1".to_owned())], "幂等键原样交给 handler");
    let ev = invoke_keyed(&mut h, 2, "c2", "a", "k1");
    assert!(invoked(&ev).is_empty(), "执行中：挂到同一次执行");
    assert_eq!(warnings(&ev), 1);
    h.c.complete_call("c1", Ok(CallOutput { data: json!({"n": 1}), ..CallOutput::default() }), h.now).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs.len(), 2, "两个请求都收到首次结果");
    assert!(msgs.iter().all(|m| m["result"]["data"] == json!({"n": 1})));
    let ev = invoke_keyed(&mut h, 3, "c3", "a", "k1");
    assert!(invoked(&ev).is_empty(), "完成后：重放首次结果");
    assert_eq!(sends(&ev)[0]["result"]["data"], json!({"n": 1}));
    assert_eq!(invoked(&invoke_keyed(&mut h, 4, "c4", "b", "k1")), vec!["c4".to_owned()], "其他工具不受影响");
    h.c.complete_call("c4", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(invoked(&h.invoke(5, "c5", "a", None)), vec!["c5".to_owned()], "没有键时只按 callId");

    let mut cfg = config();
    cfg.call_dedup = CallDedupPolicy::OFF;
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    assert_eq!(invoked(&invoke_keyed(&mut h, 1, "c1", "a", "k")), vec!["c1".to_owned()]);
    assert_eq!(invoked(&invoke_keyed(&mut h, 2, "c2", "a", "k")), Vec::<String>::new(), "并发上限 1：排队");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c2".to_owned()], "去重关闭时同一幂等键照常执行");
}

#[test]
fn dedup_expires_and_is_bounded() {
    let mut cfg = config();
    cfg.call_dedup = CallDedupPolicy { ttl_ms: 1_000, max_entries: 1 };
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    for (id, call) in [(1, "c1"), (2, "c2")] {
        h.invoke(id, call, "a", None);
        h.c.complete_call(call, Ok(CallOutput::default()), h.now).unwrap();
        h.drain();
    }
    assert_eq!(invoked(&h.invoke(3, "c1", "a", None)), vec!["c1".to_owned()], "超出条数上限的最早记录被淘汰");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert!(invoked(&h.invoke(4, "c1", "a", None)).is_empty());
    h.advance(1_000);
    assert_eq!(invoked(&h.invoke(5, "c1", "a", None)), vec!["c1".to_owned()], "过期后重新执行");
}

// ---------------------------------------------------------------------------
// 进度（spec/protocol.md 3.3，第 16 项 O2）
// ---------------------------------------------------------------------------

#[test]
fn progress_is_sent_for_running_calls_only() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.invoke(2, "c2", "a", None);
    h.c.report_progress("c1", 1.0, Some(4.0), Some("第 1 步".into()), h.now).unwrap();
    h.c.report_progress("c1", 2.0, Some(f64::NAN), None, h.now).unwrap();
    let ev = h.drain();
    assert_eq!(
        sends(&ev),
        vec![
            json!({"jsonrpc": "2.0", "method": "tools/progress", "params": {"callId": "c1", "progress": 1.0, "total": 4.0, "message": "第 1 步"}}),
            json!({"jsonrpc": "2.0", "method": "tools/progress", "params": {"callId": "c1", "progress": 2.0}}),
        ]
    );
    h.c.report_progress("c1", f64::INFINITY, None, None, h.now).unwrap();
    let ev = h.drain();
    assert!(sends(&ev).is_empty());
    assert_eq!(warnings(&ev), 1);
    assert_eq!(h.c.report_progress("c2", 1.0, None, None, h.now), Err(CoreError::UnknownCall("c2".into())), "排队中");
    assert_eq!(h.c.report_progress("x", 1.0, None, None, h.now), Err(CoreError::UnknownCall("x".into())));
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(h.c.report_progress("c1", 3.0, None, None, h.now), Err(CoreError::UnknownCall("c1".into())), "已完成");
}
