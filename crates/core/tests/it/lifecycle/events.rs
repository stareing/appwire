//! 事件与生命周期（第 16 项 N3，spec/protocol.md 3.5）：发事件不唤醒、不推迟休眠；恢复握手仍同步声明。

use super::support::*;

fn event(name: &str) -> EventInfo {
    EventInfo { name: name.into(), description: format!("{name} 发生时"), payload_schema: None }
}

#[test]
fn emit_does_not_postpone_idle_sleep() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.declare_event(event("a")).unwrap();
    h.connect();
    let start = h.now;
    h.advance(IDLE / 2);
    assert_eq!(h.c.emit_event("a", None), Ok(true));
    assert_eq!(find(&sends(&h.drain()), "events/emit").map(|m| &m["params"]["name"]), Some(&json!("a")));
    let (_, at) = h.wait_sleep(3 * IDLE).expect("应发出 app/sleep");
    assert_eq!(at, start + IDLE, "发事件不算调用活动，按连接建立时开始的空闲计时休眠");
}

#[test]
fn emit_while_dormant_returns_false_and_does_not_wake() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.declare_event(event("a")).unwrap();
    h.connect();
    h.go_dormant();
    assert_eq!(h.c.emit_event("a", Some(json!({"k": 1}))), Ok(false));
    assert!(h.drain().is_empty(), "不产生 Connect 或任何发送");
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    assert_eq!(h.c.poll_timeout(), None, "不新增定时器");
    // 休眠中声明变化也不触发连接
    h.c.declare_event(event("b")).unwrap();
    assert!(h.c.remove_event("a"));
    assert!(h.drain().is_empty());
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
}

#[test]
fn on_demand_before_connect_returns_false() {
    let mut h = Harness::new(LifecycleMode::OnDemand);
    h.c.declare_event(event("a")).unwrap();
    h.c.start(h.now);
    h.drain();
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    assert_eq!(h.c.emit_event("a", None), Ok(false));
    assert!(h.drain().is_empty());
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
}

#[test]
fn resumed_handshake_still_sends_events_sync() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.declare_event(event("a")).unwrap();
    h.connect();
    h.go_dormant();
    assert!(h.c.wake(h.now));
    h.drain();
    let hello = h.link();
    assert_eq!(hello["params"]["resumeToken"], "resume-1");
    let msgs = sends(&h.paired(&hello, true));
    assert_eq!(methods(&msgs), vec!["events/sync", "app/visibility", "app/ready"], "toolsCurrent 跳过工具同步，事件声明照发");
    assert_eq!(msgs[0]["params"]["events"][0]["name"], "a");
}
