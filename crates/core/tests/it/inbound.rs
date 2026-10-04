//! 名字服务通道（"被连接方"，spec/naming.md 第 3 节）：`Client::accept_channel` 的状态机行为，确定性时间驱动。

use app_mcp_core::*;
use serde_json::{Value, json};

fn config(mode: LifecycleMode) -> ClientConfig {
    let mut c = ClientConfig::new("shop", "示例商城", "inst-1", ClientKind::Native);
    c.lifecycle.mode = mode;
    c
}

fn drain(c: &mut Client) -> Vec<Event> {
    std::iter::from_fn(|| c.poll_event()).collect()
}

fn sends(events: &[Event]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Send(s) => serde_json::from_str(s).ok(),
            _ => None,
        })
        .collect()
}

/// 接受通道 → 连接建立 → 握手成功，返回 hello。
fn accept_and_pair(c: &mut Client, now: Millis) -> Value {
    assert!(c.accept_channel(now));
    assert_eq!(drain(c), vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
    c.handle_connected(now);
    let hello = sends(&drain(c)).remove(0);
    assert_eq!(hello["method"], "app/hello");
    let reply = json!({"jsonrpc": "2.0", "id": hello["id"],
        "result": {"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "h"}});
    c.handle_message(&reply.to_string(), now);
    drain(c);
    assert_eq!(c.state(), &ConnectionState::Connected);
    hello
}

#[test]
fn accepted_channel_says_os_activation_and_has_no_heartbeat_or_idle_timer() {
    let mut cfg = config(LifecycleMode::OnDemand);
    cfg.transport = TransportKind::Remote;
    let remote_hb = Client::new(cfg.clone()).heartbeat_interval();
    assert!(remote_hb.is_some(), "前提：远程传输本来要发心跳");
    let mut c = Client::new(cfg);
    c.start(0);
    drain(&mut c);
    assert_eq!(c.state(), &ConnectionState::Dormant);
    let hello = accept_and_pair(&mut c, 10);
    assert_eq!(hello["params"]["wakeReason"], "os-activation");
    assert_eq!(hello["params"]["heartbeatMs"], 0, "名字服务通道不发心跳（即使端点是远程传输）");
    assert_eq!(c.heartbeat_interval(), None);
    // 关闭时机只由 Hub 决定：没有任何定时器，再久也不发 app/sleep。
    assert_eq!(c.poll_timeout(), None);
    c.handle_timeout(10_000_000);
    assert!(sends(&drain(&mut c)).iter().all(|m| m["method"] != "app/sleep"));
    assert_eq!(c.state(), &ConnectionState::Connected);
}

#[test]
fn hub_closing_channel_returns_to_dormant_without_reconnect() {
    let mut c = Client::new(config(LifecycleMode::OnDemand));
    c.start(0);
    drain(&mut c);
    accept_and_pair(&mut c, 10);
    c.handle_disconnected(20);
    let ev = drain(&mut c);
    assert!(ev.contains(&Event::StateChanged(ConnectionState::Dormant)), "{ev:?}");
    assert!(!ev.contains(&Event::Connect));
    assert_eq!(c.poll_timeout(), None, "回到休眠后没有定时器");
    // 下一次拨入照常接受；之后 App 拨出的连接恢复普通规则（有心跳判定）。
    assert!(c.accept_channel(30));
}

#[test]
fn channel_failing_before_handshake_also_goes_dormant() {
    let mut c = Client::new(config(LifecycleMode::Idle));
    c.start(0);
    drain(&mut c);
    c.sleep(0);
    drain(&mut c);
    assert_eq!(c.state(), &ConnectionState::Dormant);
    assert!(c.accept_channel(5));
    drain(&mut c);
    c.handle_connect_failed(ConnectionIssue::new(ConnectionErrorCode::ConnectionLost, "x"), 6);
    assert_eq!(c.state(), &ConnectionState::Dormant);
    assert_eq!(c.poll_timeout(), None);
}

#[test]
fn persistent_returns_to_normal_reconnect_after_channel() {
    let mut c = Client::new(config(LifecycleMode::Persistent));
    c.start(0);
    drain(&mut c);
    c.handle_connect_failed(ConnectionIssue::new(ConnectionErrorCode::HostNotRunning, "x"), 1);
    drain(&mut c);
    assert!(matches!(c.state(), ConnectionState::Backoff { .. }));
    accept_and_pair(&mut c, 10);
    c.handle_disconnected(20);
    assert!(matches!(c.state(), ConnectionState::Backoff { .. }), "{:?}", c.state());
    assert!(c.poll_timeout().is_some(), "persistent 照常按退避重连 Host 端点");
}

#[test]
fn rejects_when_not_started_connected_or_stopped() {
    let mut c = Client::new(config(LifecycleMode::OnDemand));
    assert!(!c.accept_channel(0), "尚未 start");
    c.start(0);
    drain(&mut c);
    accept_and_pair(&mut c, 1);
    assert!(!c.accept_channel(2), "已有连接（上限 1，spec/naming.md U-16）");
    c.stop(3);
    drain(&mut c);
    assert!(!c.accept_channel(4), "已停止");
}

#[test]
fn residency_exit_after_activation_launch() {
    let mut cfg = config(LifecycleMode::OnDemand);
    cfg.lifecycle.residency = Residency::ExitWhenIdle;
    cfg.launched_by_activation = true;
    let mut c = Client::new(cfg);
    c.start(0);
    drain(&mut c);
    accept_and_pair(&mut c, 1);
    c.handle_disconnected(2);
    assert!(drain(&mut c).contains(&Event::IdleExit));

    // 用户自己打开的进程（非激活启动）不因通道关闭而退出。
    let mut cfg = config(LifecycleMode::OnDemand);
    cfg.lifecycle.residency = Residency::ExitWhenIdle;
    let mut c = Client::new(cfg);
    c.start(0);
    drain(&mut c);
    accept_and_pair(&mut c, 1);
    c.handle_disconnected(2);
    assert!(!drain(&mut c).contains(&Event::IdleExit));
}

#[test]
fn app_requested_sleep_on_channel_still_handshakes() {
    let mut c = Client::new(config(LifecycleMode::OnDemand));
    c.start(0);
    drain(&mut c);
    accept_and_pair(&mut c, 1);
    assert!(c.sleep(2));
    let sleep = sends(&drain(&mut c)).into_iter().find(|m| m["method"] == "app/sleep").expect("app/sleep");
    assert_eq!(sleep["params"]["reason"], "app");
    let reply = json!({"jsonrpc": "2.0", "id": sleep["id"], "result": {"accepted": true, "resumeToken": "r"}});
    c.handle_message(&reply.to_string(), 3);
    assert!(drain(&mut c).contains(&Event::Disconnect));
    assert_eq!(c.state(), &ConnectionState::Dormant);
    assert_eq!(c.resume_token(), Some("r"));
}
