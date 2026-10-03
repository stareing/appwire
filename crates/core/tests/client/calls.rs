//! 调用。

use super::support::*;

// ---------------------------------------------------------------------------
// 调用
// ---------------------------------------------------------------------------

#[test]
fn call_success_and_handler_error() {
    let mut h = Harness::new();
    let a = h.c.register_tool(tool("a")).unwrap();
    h.connect();

    let ev = h.invoke(10, "c1", "a", Some(5_000));
    assert_eq!(
        ev,
        vec![Event::InvokeTool { call_id: "c1".into(), tool: a, name: "a".into(), arguments: json!({"x": 1}), idempotency_key: None }]
    );
    h.c.complete_call(
        "c1",
        Ok(CallOutput { data: json!({"ok": true}), state_hints: vec!["cart.state".into()], ..CallOutput::default() }),
        h.now,
    )
    .unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs, vec![json!({"jsonrpc": "2.0", "id": 10, "result": {"data": {"ok": true}, "stateHints": ["cart.state"]}})]);
    assert_eq!(h.c.poll_timeout().map(|t| t > h.now + 5_000), Some(true), "已完成调用的超时不再计时");

    // 完成后重复完成
    assert_eq!(
        h.c.complete_call("c1", Ok(CallOutput::default()), h.now),
        Err(CoreError::UnknownCall("c1".into()))
    );

    h.invoke(11, "c2", "a", None);
    let err = ToolError::new(ErrorKind::HandlerError, "库存不足").with_details(json!({"sku": "x"}));
    h.c.complete_call("c2", Err(err), h.now).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["id"], 11);
    assert_eq!(msgs[0]["error"]["code"], -32006);
    assert_eq!(msgs[0]["error"]["message"], "库存不足");
    assert_eq!(msgs[0]["error"]["data"], json!({"kind": "HANDLER_ERROR", "sku": "x"}));

    // 无返回值
    h.invoke(12, "c3", "a", None);
    h.c.complete_call("c3", Ok(CallOutput::default()), h.now).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["result"], json!({"data": null}));
}

#[test]
fn tool_not_found_and_disabled() {
    let mut h = Harness::new();
    let mut t = tool("off");
    t.enabled = false;
    h.c.register_tool(t).unwrap();
    h.connect();

    let ev = h.invoke(1, "c1", "missing", None);
    assert!(invoked(&ev).is_empty());
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["id"], 1);
    assert_eq!(msgs[0]["error"]["code"], -32001);
    assert_eq!(error_kind(&msgs[0]), "TOOL_NOT_FOUND");

    let ev = h.invoke(2, "c2", "off", None);
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["error"]["code"], -32002);
    assert_eq!(error_kind(&msgs[0]), "TOOL_DISABLED");
}

#[test]
fn calls_are_serialized_by_default() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();

    assert_eq!(invoked(&h.invoke(1, "c1", "a", None)), vec!["c1"]);
    assert!(h.invoke(2, "c2", "a", None).is_empty());
    assert!(h.invoke(3, "c3", "a", None).is_empty());
    assert_eq!((h.c.running_call_count(), h.c.queued_call_count()), (1, 2));

    // 排队中的调用不能提前完成
    assert!(h.c.complete_call("c2", Ok(CallOutput::default()), h.now).is_err());

    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    let ev = h.drain();
    assert_eq!(sends(&ev)[0]["id"], 1);
    assert_eq!(invoked(&ev), vec!["c2"]);
    h.c.complete_call("c2", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c3"]);
}

#[test]
fn max_concurrent_calls() {
    let mut cfg = config();
    cfg.max_concurrent_calls = 2;
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.invoke(2, "c2", "a", None);
    assert!(h.invoke(3, "c3", "a", None).is_empty());
    assert_eq!(h.c.running_call_count(), 2);
    h.c.complete_call("c2", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c3"]);
}

#[test]
fn timeout_includes_queue_time() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    let start = h.now;
    h.invoke(1, "c1", "a", None);
    h.advance(400);
    h.invoke(2, "c2", "a", Some(1_000));
    assert_eq!(h.c.poll_timeout(), Some(start + 1_400));

    let ev = h.advance(1_000);
    assert!(cancelled(&ev).is_empty(), "排队中的调用从未启动，不需要中止 handler");
    let msgs = sends(&ev);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["id"], 2);
    assert_eq!(error_kind(&msgs[0]), "TIMEOUT");
    assert_eq!(h.c.queued_call_count(), 0);
    assert_eq!(h.c.running_call_count(), 1);

    // 第一个调用照常完成
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    assert!(invoked(&h.drain()).is_empty());
}

#[test]
fn running_call_timeout() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", Some(3_000));
    h.invoke(2, "c2", "a", None);
    assert!(h.advance(2_999).iter().all(|e| !matches!(e, Event::CancelTool { .. })));
    let ev = h.advance(1);
    assert_eq!(cancelled(&ev), vec![("c1".into(), CancelReason::Timeout)]);
    let msgs = sends(&ev);
    assert_eq!(error_kind(&msgs[0]), "TIMEOUT");
    assert_eq!(msgs[0]["error"]["code"], -32005);
    assert_eq!(invoked(&ev), vec!["c2"], "超时释放并发名额");

    assert_eq!(
        h.c.complete_call("c1", Ok(CallOutput::default()), h.now),
        Err(CoreError::UnknownCall("c1".into()))
    );
    assert!(h.drain().is_empty(), "超时后的完成结果不发送");
}

#[test]
fn cancel_running_and_queued() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.invoke(2, "c2", "a", None);
    h.invoke(3, "c3", "a", None);

    // 取消排队中的调用：直接移出
    let ev = h.recv(json!({"jsonrpc": "2.0", "method": "tools/cancel", "params": {"callId": "c2"}}));
    assert!(cancelled(&ev).is_empty());
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["id"], 2);
    assert_eq!(error_kind(&msgs[0]), "CANCELLED");

    // 取消进行中的调用
    let ev = h.recv(json!({"jsonrpc": "2.0", "method": "tools/cancel", "params": {"callId": "c1", "reason": "用户中止"}}));
    assert_eq!(cancelled(&ev), vec![("c1".into(), CancelReason::Requested)]);
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["id"], 1);
    assert_eq!(msgs[0]["error"]["code"], -32007);
    assert_eq!(invoked(&ev), vec!["c3"]);

    assert_eq!(
        h.c.complete_call("c1", Ok(CallOutput::default()), h.now),
        Err(CoreError::UnknownCall("c1".into()))
    );
    assert!(h.drain().is_empty());

    // 未知调用的取消：警告
    let ev = h.recv(json!({"jsonrpc": "2.0", "method": "tools/cancel", "params": {"callId": "nope"}}));
    assert_eq!(warnings(&ev), 1);
    assert!(sends(&ev).is_empty());
}

#[test]
fn unregistered_tool_calls_still_complete() {
    let mut h = Harness::new();
    let a = h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.invoke(2, "c2", "a", None);
    h.c.unregister_tool(a).unwrap();
    h.drain();
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    let ev = h.drain();
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["id"], 1);
    assert!(msgs[0].get("result").is_some());
    // 排队中的调用在启动时发现工具已注销
    assert_eq!(msgs[1]["id"], 2);
    assert_eq!(error_kind(&msgs[1]), "TOOL_NOT_FOUND");
    assert!(invoked(&ev).is_empty());
}

#[test]
fn disconnect_cancels_all_calls() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.c.register_resource(resource("r")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", Some(10_000));
    h.invoke(2, "c2", "a", None);
    let ev = h.request(3, "resources/read", json!({"name": "r"}));
    let read = match &ev[0] {
        Event::ReadResource { read, .. } => *read,
        other => panic!("unexpected {other:?}"),
    };

    h.c.handle_disconnected(h.now);
    let ev = h.drain();
    assert!(sends(&ev).is_empty(), "断线时不发送任何响应");
    assert_eq!(cancelled(&ev), vec![("c1".into(), CancelReason::Disconnected)]);
    assert_eq!(
        ev.last(),
        Some(&Event::StateChanged(ConnectionState::Backoff { retry_at: h.now + 500, reason: None, code: None }))
    );
    assert_eq!((h.c.running_call_count(), h.c.queued_call_count()), (0, 0));
    assert_eq!(
        h.c.complete_call("c1", Ok(CallOutput::default()), h.now),
        Err(CoreError::UnknownCall("c1".into()))
    );
    assert_eq!(h.c.complete_read(read, Ok(json!(1))), Err(CoreError::UnknownRead(read)));
    assert!(h.drain().is_empty());
    assert_eq!(h.c.poll_timeout(), Some(h.now + 500), "只剩重连定时器");
}

#[test]
fn stop_cancels_and_stays_stopped() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.c.stop(h.now);
    let ev = h.drain();
    assert_eq!(cancelled(&ev), vec![("c1".into(), CancelReason::Stopped)]);
    assert!(ev.contains(&Event::Disconnect));
    assert_eq!(ev.last(), Some(&Event::StateChanged(ConnectionState::Stopped)));
    assert_eq!(h.c.poll_timeout(), None);
    h.c.handle_disconnected(h.now);
    h.c.start(h.now);
    assert!(h.drain().is_empty());
    assert_eq!(h.c.state(), &ConnectionState::Stopped);

    // 未连接时停止不产生 Disconnect
    let mut h = Harness::new();
    h.c.stop(h.now);
    assert_eq!(h.drain(), vec![Event::StateChanged(ConnectionState::Stopped)]);
}

#[test]
fn duplicate_call_id_attaches_or_is_rejected_when_dedup_off() {
    // 去重开启（默认）：执行中的重复请求挂到同一次执行上，完成时两者得到同一结果
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    let ev = h.invoke(2, "c1", "a", None);
    assert!(sends(&ev).is_empty() && invoked(&ev).is_empty(), "不执行、不立即回复：{ev:?}");
    assert_eq!(warnings(&ev), 1, "命中记一条警告日志");
    h.c.complete_call("c1", Ok(CallOutput { data: json!(5), ..CallOutput::default() }), h.now).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs.iter().map(|m| m["id"].clone()).collect::<Vec<_>>(), vec![json!(2), json!(1)]);
    assert!(msgs.iter().all(|m| m["result"] == json!({"data": 5})));

    // 排队中的重复请求同样挂接
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.invoke(2, "c2", "a", None);
    let ev = h.invoke(3, "c2", "a", None);
    assert!(sends(&ev).is_empty() && invoked(&ev).is_empty(), "{ev:?}");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c2".to_owned()], "c2 只执行一次");

    // 关闭去重：旧行为
    let mut cfg = config();
    cfg.call_dedup = CallDedupPolicy::OFF;
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    let msgs = sends(&h.invoke(2, "c1", "a", None));
    assert_eq!(msgs[0]["id"], 2);
    assert_eq!(msgs[0]["error"]["code"], -32602);
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(invoked(&h.invoke(3, "c1", "a", None)), vec!["c1".to_owned()], "关闭时完成后再次执行");
}

/// spec/protocol.md 5.3：工具的 `concurrency` 限制本工具同时执行的调用数；忙的工具不阻塞队列中其后的其他工具。
#[test]
fn per_tool_concurrency_does_not_block_other_tools() {
    let mut cfg = config();
    cfg.max_concurrent_calls = 3;
    let mut h = Harness::with(cfg);
    h.c.register_tool(ToolDef { concurrency: 1, ..tool("slow") }).unwrap();
    h.c.register_tool(tool("fast")).unwrap();
    h.connect();
    assert_eq!(invoked(&h.invoke(1, "c1", "slow", None)), vec!["c1"]);
    assert!(invoked(&h.invoke(2, "c2", "slow", None)).is_empty(), "slow 已有一个在执行");
    assert_eq!(invoked(&h.invoke(3, "c3", "fast", None)), vec!["c3"], "其后的其他工具照常开始");
    assert_eq!((h.c.running_call_count(), h.c.queued_call_count()), (2, 1));
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c2"]);
}

/// 同一互斥组的工具同一时刻至多一个在执行，不同组 / 不互斥的工具不受影响；组内按到达顺序。
#[test]
fn exclusive_group_serializes_across_tools() {
    let mut cfg = config();
    cfg.max_concurrent_calls = 4;
    let mut h = Harness::with(cfg);
    let group = |name: &str, g: &str| ToolDef { exclusive: Some(g.into()), ..tool(name) };
    h.c.register_tool(group("doc.edit", "doc")).unwrap();
    h.c.register_tool(group("doc.rename", "doc")).unwrap();
    h.c.register_tool(group("sheet.edit", "sheet")).unwrap();
    h.c.register_tool(tool("other")).unwrap();
    h.connect();
    assert_eq!(invoked(&h.invoke(1, "c1", "doc.edit", None)), vec!["c1"]);
    assert!(invoked(&h.invoke(2, "c2", "doc.rename", None)).is_empty(), "同组互斥");
    assert!(invoked(&h.invoke(3, "c3", "doc.edit", None)).is_empty());
    assert_eq!(invoked(&h.invoke(4, "c4", "sheet.edit", None)), vec!["c4"]);
    assert_eq!(invoked(&h.invoke(5, "c5", "other", None)), vec!["c5"]);
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c2"], "组内按到达顺序，一次只放一个");
    h.c.complete_call("c2", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c3"]);
}

/// 新到的调用需要排队而队列已满 → `RATE_LIMITED`（`details.scope = "queue"`），不进去重表（同一 callId 稍后可重试）。
#[test]
fn queue_overflow_is_rate_limited() {
    let mut cfg = config();
    cfg.max_queued_calls = 1;
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    assert_eq!(invoked(&h.invoke(1, "c1", "a", None)), vec!["c1"]);
    assert!(sends(&h.invoke(2, "c2", "a", None)).is_empty(), "排进队列");
    let msgs = sends(&h.invoke(3, "c3", "a", None));
    assert_eq!((msgs.len(), msgs[0]["id"].as_i64(), error_kind(&msgs[0])), (1, Some(3), "RATE_LIMITED"));
    let details = &msgs[0]["error"]["data"];
    assert_eq!((details["scope"].as_str(), details["limit"].as_u64()), (Some("queue"), Some(1)), "{}", msgs[0]);
    assert_eq!(h.c.queued_call_count(), 1);
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c2"]);
    assert_eq!(invoked(&h.invoke(4, "c3", "a", None)), Vec::<String>::new(), "被拒绝的 callId 未记入去重表：重试时照常排队");
    assert_eq!(h.c.queued_call_count(), 1);
}

/// `max_queued_calls = 0` 不限；放宽工具的并发声明（`update_tool`）后排队中的调用随即开始，且不发 `tools/changed`。
#[test]
fn unlimited_queue_and_relaxing_declaration_starts_queued_calls() {
    let mut cfg = config();
    cfg.max_concurrent_calls = 2;
    cfg.max_queued_calls = 0;
    let mut h = Harness::with(cfg);
    let id = h.c.register_tool(ToolDef { concurrency: 1, ..tool("a") }).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    for i in 2..=100 {
        let msgs = sends(&h.invoke(i, &format!("c{i}"), "a", None));
        assert!(msgs.is_empty(), "不限时不拒绝：{msgs:?}");
    }
    h.c.update_tool(id, ToolUpdate { concurrency: Some(2), ..Default::default() }).unwrap();
    let ev = h.drain();
    assert_eq!(invoked(&ev), vec!["c2"]);
    assert!(!methods(&sends(&ev)).contains(&"tools/changed"), "调度声明只在 SDK 内");
}

fn write_tool(name: &str) -> ToolDef {
    ToolDef { risk: Risk::Write, ..tool(name) }
}

/// 用户正在操作（`set_busy`）且策略为拒绝（默认）：新到与排队中的写调用以 `RATE_LIMITED`（`scope = "busy"`）拒绝、不进去重表；
/// 只读调用照常执行；取消后同一 callId 照常执行。
#[test]
fn busy_rejects_write_calls_by_default() {
    let mut h = Harness::new();
    h.c.register_tool(write_tool("w")).unwrap();
    h.c.register_tool(tool("r")).unwrap();
    h.connect();
    assert_eq!(invoked(&h.invoke(1, "c1", "r", None)), vec!["c1"]);
    assert!(sends(&h.invoke(2, "c2", "w", None)).is_empty(), "串行：排在只读调用之后");
    h.c.set_busy(true);
    assert!(h.c.is_busy());
    let msgs = sends(&h.drain());
    assert_eq!((msgs.len(), msgs[0]["id"].as_i64(), error_kind(&msgs[0])), (1, Some(2), "RATE_LIMITED"), "排队中的写调用随即被拒绝");
    assert_eq!(msgs[0]["error"]["data"]["scope"].as_str(), Some("busy"), "{}", msgs[0]);
    let msgs = sends(&h.invoke(3, "c3", "w", None));
    assert_eq!((msgs.len(), error_kind(&msgs[0])), (1, "RATE_LIMITED"), "新到的写调用");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(invoked(&h.invoke(4, "c4", "r", None)), vec!["c4"], "只读调用不受影响");
    h.c.complete_call("c4", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    h.c.set_busy(false);
    assert_eq!(invoked(&h.invoke(5, "c3", "w", None)), vec!["c3"], "被拒绝的 callId 未记入去重表");
}

/// 策略为排队：写调用等到用户操作结束后按到达顺序开始，只读调用不被其阻塞；改回拒绝时排队中的写调用随即被拒绝。
#[test]
fn busy_queue_policy_defers_write_calls() {
    let mut cfg = config();
    cfg.busy_policy = BusyPolicy::Queue;
    cfg.max_concurrent_calls = 4;
    let mut h = Harness::with(cfg);
    h.c.register_tool(write_tool("w")).unwrap();
    h.c.register_tool(tool("r")).unwrap();
    h.connect();
    h.c.set_busy(true);
    assert!(sends(&h.invoke(1, "c1", "w", None)).is_empty(), "排队，不回复");
    assert!(invoked(&h.invoke(2, "c2", "w", None)).is_empty());
    assert_eq!(invoked(&h.invoke(3, "c3", "r", None)), vec!["c3"], "只读调用不被排队的写调用阻塞");
    h.c.set_busy(false);
    assert_eq!(invoked(&h.drain()), vec!["c1", "c2"]);

    h.c.set_busy(true);
    assert!(invoked(&h.invoke(4, "c4", "w", None)).is_empty());
    h.c.set_busy_policy(BusyPolicy::Reject);
    let msgs = sends(&h.drain());
    assert_eq!((msgs.len(), msgs[0]["id"].as_i64(), error_kind(&msgs[0])), (1, Some(4), "RATE_LIMITED"));
}
