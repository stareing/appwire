//! 休眠握手。

use super::support::*;

// ---------------------------------------------------------------------------
// 休眠握手
// ---------------------------------------------------------------------------

#[test]
fn rejected_sleep_retries_after_retry_after_ms() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    let ev = h.respond(&sleep, json!({"accepted": false, "retryAfterMs": 5_000}));
    assert!(ev.iter().all(|e| !matches!(e, Event::Disconnect | Event::StateChanged(_))));
    assert_eq!(h.c.state(), &ConnectionState::Connected);
    let t = h.now;
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t + 5_000);

    // 没有 retryAfterMs：重置空闲计时
    let ev = h.respond(&sleep, json!({"accepted": false}));
    assert!(sends(&ev).is_empty());
    let t = h.now;
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t + IDLE);

    // 被拒后有调用到达：重试取消，调用完成后按空闲时间重新计时
    h.respond(&sleep, json!({"accepted": false, "retryAfterMs": 1_000}));
    h.c.register_tool(tool("a")).unwrap();
    h.invoke("c1", "a");
    h.advance(2_000);
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + MERGE);
}

#[test]
fn sleep_error_disables_auto_sleep_for_connection() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    let ev = h.recv(json!({"jsonrpc": "2.0", "id": sleep["id"], "error": {"code": -32601, "message": "method not found"}}));
    assert!(ev.iter().any(|e| matches!(e, Event::Warning(_))));
    assert_eq!(h.wait_sleep(10 * IDLE), None);
    assert_eq!(h.c.state(), &ConnectionState::Connected);
}

#[test]
fn explicit_sleep_ignores_idle_conditions_and_retries() {
    let mut h = Harness::new(LifecycleMode::Persistent);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    let _hold = h.c.hold(h.now);
    assert!(h.c.sleep(h.now));
    let msgs = sends(&h.drain());
    let sleep = find(&msgs, "app/sleep").unwrap().clone();
    assert_eq!(sleep["params"]["reason"], "app");
    assert!(!h.c.sleep(h.now), "已在休眠握手中");
    h.respond(&sleep, json!({"accepted": false, "retryAfterMs": 2_000}));
    let t = h.now;
    let (sleep, at) = h.wait_sleep(IDLE).unwrap();
    assert_eq!(at, t + 2_000);
    assert_eq!(sleep["params"]["reason"], "app");
    h.accept_sleep(&sleep, "r");
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    // 休眠后 wake 回连
    assert!(h.c.wake(h.now));
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Waking)]);
}

#[test]
fn explicit_sleep_when_not_connected() {
    // Backoff：停止重连
    let mut h = Harness::new(LifecycleMode::Persistent);
    h.connect();
    h.c.handle_disconnected(h.now);
    h.drain();
    assert!(h.c.sleep(h.now));
    assert_eq!(h.drain(), vec![Event::StateChanged(ConnectionState::Dormant)]);
    assert_eq!(h.c.poll_timeout(), None);

    // 握手中：断开
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.start(h.now);
    h.link();
    assert!(h.c.sleep(h.now));
    let ev = h.drain();
    assert!(ev.contains(&Event::Disconnect));
    assert_eq!(h.c.state(), &ConnectionState::Dormant);

    // 未启动：无效果
    let mut h = Harness::new(LifecycleMode::Idle);
    assert!(!h.c.sleep(h.now));
}

#[test]
fn results_and_changes_are_flushed_before_sleep() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.c.register_tool(tool("b")).unwrap();
    // 不取事件，直接请求休眠
    h.c.sleep(h.now);
    let msgs = sends(&h.drain());
    assert_eq!(methods(&msgs), vec!["<response>", "tools/changed", "app/sleep"]);
}

#[test]
fn stop_flushes_queued_sends_before_disconnect() {
    let mut h = Harness::new(LifecycleMode::Persistent);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput { data: json!(7), state_hints: vec![], annotations: None, state_resource: None, status: Default::default(), summary: None }), h.now).unwrap();
    h.c.stop(h.now);
    let ev = h.drain();
    let send_idx = ev.iter().position(|e| matches!(e, Event::Send(_))).expect("结果仍会发出");
    let disc_idx = ev.iter().position(|e| *e == Event::Disconnect).unwrap();
    assert!(send_idx < disc_idx);
    assert_eq!(sends(&ev)[0]["result"]["data"], 7);
}

#[test]
fn disconnect_still_drops_unsent_messages() {
    let mut h = Harness::new(LifecycleMode::Persistent);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.c.handle_disconnected(h.now);
    assert!(sends(&h.drain()).is_empty());
}
