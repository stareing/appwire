//! 模式与基本流程。

use super::support::*;

// ---------------------------------------------------------------------------
// 模式与基本流程
// ---------------------------------------------------------------------------

#[test]
fn persistent_never_sleeps_and_hello_is_unchanged() {
    let mut h = Harness::new(LifecycleMode::Persistent);
    let hello = h.connect();
    assert!(hello["params"].get("wakeReason").is_none());
    assert!(hello["params"].get("resumeToken").is_none());
    assert!(hello["params"].get("toolsHash").is_none());
    assert_eq!(h.wait_sleep(30 * IDLE), None);
    assert_eq!(h.c.state(), &ConnectionState::Connected);
}

#[test]
fn idle_sleeps_after_timeout_and_is_dormant_without_timers() {
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.wake = Some(WakeDescriptor { kind: WakeKind::Uri, target: Some("shop".into()), background: true });
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    let hello = h.connect();
    assert_eq!(hello["params"]["wakeReason"], "cold-start");
    let t0 = h.now;

    let (sleep, at) = h.wait_sleep(10 * IDLE).unwrap();
    assert_eq!(at, t0 + IDLE);
    assert_eq!(sleep["params"]["reason"], "idle");
    assert_eq!(sleep["params"]["toolsHash"], json!(h.c.tools_hash()));
    assert_eq!(sleep["params"]["wake"], json!({"kind": "uri", "target": "shop", "background": true}));
    assert!(h.c.is_sleep_pending());
    assert_eq!(h.c.state(), &ConnectionState::Connected, "sleeping 对外仍为 connected");

    let ev = h.accept_sleep(&sleep, "resume-1");
    assert_eq!(ev, vec![Event::Disconnect, Event::StateChanged(ConnectionState::Dormant)]);
    assert_eq!(h.c.poll_timeout(), None, "休眠态没有定时器");
    assert_eq!(h.c.resume_token(), Some("resume-1"));
    // 休眠中收到的消息与断线通知都被忽略
    h.c.handle_disconnected(h.now);
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    assert!(h.advance(10 * IDLE).is_empty());
    assert_eq!(h.c.poll_timeout(), None);
}

#[test]
fn on_demand_does_not_connect_until_woken() {
    let mut h = Harness::new(LifecycleMode::OnDemand);
    h.c.register_tool(tool("a")).unwrap();
    h.c.start(h.now);
    assert_eq!(h.drain(), vec![Event::StateChanged(ConnectionState::Dormant)]);
    assert_eq!(h.c.poll_timeout(), None);

    assert!(h.c.connect_now(h.now));
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Waking)]);
    let hello = h.link();
    assert_eq!(hello["params"]["wakeReason"], "app");
    assert!(hello["params"].get("resumeToken").is_none());
    h.paired(&hello, false);

    // 任务完成后经过合并窗口（min(graceMs 10 s, mergeWindowMs 2 s)）休眠，原因 grace（B1）
    h.invoke("c1", "a");
    h.advance(3_000);
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    let done = h.now;
    let (sleep, at) = h.wait_sleep(IDLE).unwrap();
    assert_eq!(at, done + MERGE);
    assert_eq!(sleep["params"]["reason"], "grace");
}

#[test]
fn connect_now_before_start_connects_in_on_demand() {
    let mut h = Harness::new(LifecycleMode::OnDemand);
    assert!(h.c.connect_now(h.now));
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
    assert_eq!(h.link()["params"]["wakeReason"], "app");
}
