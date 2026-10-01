//! 功耗回归测试（spec/lifecycle.md 第 11 节 A1–A3、O2）：用确定性时间模拟驱动层，统计定时器触发、
//! 连接发起与心跳次数的上限。新增唤醒 / 定时器会使这些上限断言失败。

use app_mcp_core::*;
use serde_json::{Value, json};

const MINUTE: Millis = 60_000;
const HOUR: Millis = 60 * MINUTE;

/// 驱动层模拟：自动回复 ping；连接请求按 `host_up` 决定成功（并完成握手）或以 `HOST_NOT_RUNNING` 失败。
struct Sim {
    c: Client,
    now: Millis,
    host_up: bool,
    /// handle_timeout 被调用（定时器触发）的次数。
    timer_fires: u32,
    /// Event::Connect 次数（连接发起）。
    connects: u32,
    /// SDK 发出的 ping 次数。
    pings: u32,
    sleeps: Vec<Value>,
    /// 首次发出 `app/sleep` 的时刻。
    sleep_at: Option<Millis>,
    hellos: Vec<Value>,
    /// 连接失败时报告的错误码（`host_up` 为假时）。
    fail_code: ConnectionErrorCode,
}

fn config(mode: LifecycleMode, transport: TransportKind) -> ClientConfig {
    let mut c = ClientConfig::new("shop", "示例商城", "inst-1", ClientKind::Native);
    c.lifecycle.mode = mode;
    c.transport = transport;
    c
}

impl Sim {
    fn new(cfg: ClientConfig, host_up: bool) -> Self {
        Self { c: Client::new(cfg), now: 1_000, host_up, timer_fires: 0, connects: 0, pings: 0, sleeps: vec![], sleep_at: None, hellos: vec![], fail_code: ConnectionErrorCode::HostNotRunning }
    }

    fn reply(&mut self, id: &Value, result: Value) {
        let msg = json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
        self.c.handle_message(&msg, self.now);
    }

    /// 处理全部待处理事件（含因此产生的新事件）。
    fn pump(&mut self) {
        while let Some(ev) = self.c.poll_event() {
            match ev {
                Event::Connect => {
                    self.connects += 1;
                    if self.host_up {
                        self.c.handle_connected(self.now);
                    } else {
                        let issue = ConnectionIssue::new(self.fail_code, "连接失败");
                        self.c.handle_connect_failed(issue, self.now);
                    }
                }
                Event::Send(text) => {
                    let m: Value = serde_json::from_str(&text).expect("json");
                    match m["method"].as_str() {
                        Some("ping") => {
                            self.pings += 1;
                            self.reply(&m["id"], json!({}));
                        }
                        Some("app/hello") => {
                            self.hellos.push(m["params"].clone());
                            self.reply(&m["id"], json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "h"}));
                        }
                        Some("app/sleep") => {
                            self.sleep_at.get_or_insert(self.now);
                            self.sleeps.push(m);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// 推进到 `until`，期间按 poll_timeout 触发定时器。
    fn run_until(&mut self, until: Millis) {
        self.pump();
        while let Some(t) = self.c.poll_timeout() {
            if t > until {
                break;
            }
            self.now = self.now.max(t);
            self.timer_fires += 1;
            self.c.handle_timeout(self.now);
            self.pump();
            assert!(self.timer_fires < 100_000, "定时器失控");
        }
        self.now = until;
    }

    fn start(&mut self) {
        self.c.start(self.now);
        self.pump();
    }
}

// ---------------------------------------------------------------------------
// A3：心跳
// ---------------------------------------------------------------------------

#[test]
fn local_transport_idle_hour_has_no_timers_or_pings() {
    for transport in [TransportKind::Ipc, TransportKind::Loopback] {
        let mut s = Sim::new(config(LifecycleMode::Persistent, transport), true);
        s.start();
        assert_eq!(s.c.state(), &ConnectionState::Connected);
        assert_eq!(s.hellos[0]["heartbeatMs"], json!(0), "{transport:?}：声明不发心跳");
        assert_eq!(s.hellos[0]["lifecycleMode"], json!("persistent"));
        assert_eq!(s.c.poll_timeout(), None, "{transport:?}：空闲时没有任何定时器");
        s.run_until(s.now + HOUR);
        assert_eq!((s.pings, s.timer_fires, s.connects), (0, 0, 1), "{transport:?}");
    }
}

#[test]
fn remote_transport_pings_one_way_with_bounded_rate() {
    for transport in [TransportKind::Remote, TransportKind::Unknown] {
        let mut s = Sim::new(config(LifecycleMode::Persistent, transport), true);
        s.start();
        assert_eq!(s.hellos[0]["heartbeatMs"], json!(15_000));
        s.run_until(s.now + HOUR);
        // 每 15 s 一次：1 小时 240 次；每次 ping 对应一次定时器触发
        assert!(s.pings <= 240 && s.pings >= 239, "{transport:?}：{}", s.pings);
        assert!(s.timer_fires <= 241, "{transport:?}：{}", s.timer_fires);
        assert_eq!(s.connects, 1);
    }
}

#[test]
fn heartbeat_mode_overrides_transport() {
    let mut cfg = config(LifecycleMode::Persistent, TransportKind::Ipc);
    cfg.heartbeat.mode = HeartbeatMode::Always;
    let mut s = Sim::new(cfg, true);
    s.start();
    assert_eq!(s.c.heartbeat_interval(), Some(15_000));
    s.run_until(s.now + MINUTE);
    assert_eq!(s.pings, 4);

    let mut cfg = config(LifecycleMode::Persistent, TransportKind::Remote);
    cfg.heartbeat.mode = HeartbeatMode::Off;
    let mut s = Sim::new(cfg, true);
    s.start();
    assert_eq!(s.hellos[0]["heartbeatMs"], json!(0));
    s.run_until(s.now + HOUR);
    assert_eq!((s.pings, s.timer_fires), (0, 0));
}

#[test]
fn legacy_timers_restore_two_way_heartbeat() {
    let mut cfg = config(LifecycleMode::Persistent, TransportKind::Ipc);
    cfg.lifecycle.legacy_timers = true;
    let mut s = Sim::new(cfg, true);
    s.start();
    assert!(s.hellos[0].get("heartbeatMs").is_none(), "旧行为不声明，Host 照旧发 ping");
    s.run_until(s.now + MINUTE);
    assert_eq!(s.pings, 4);
}

#[test]
fn frozen_process_is_not_judged_dead_by_heartbeat() {
    let mut s = Sim::new(config(LifecycleMode::Persistent, TransportKind::Remote), true);
    s.start();
    // 到下一次 ping 时刻：发出 ping 但 Host 的响应还没到，进程随即被冻结
    let at = s.c.poll_timeout().expect("ping 定时器");
    s.now = at;
    s.c.handle_timeout(s.now);
    let ping = loop {
        match s.c.poll_event() {
            Some(Event::Send(t)) if t.contains("\"ping\"") => break serde_json::from_str::<Value>(&t).unwrap(),
            Some(_) => continue,
            None => panic!("应发出 ping"),
        }
    };
    // 冻结 5 分钟后解冻：先被调度的是定时器（响应仍在套接字缓冲区里）
    s.now += 5 * MINUTE;
    s.c.handle_timeout(s.now);
    let events: Vec<Event> = std::iter::from_fn(|| s.c.poll_event()).collect();
    assert!(!events.contains(&Event::Disconnect), "冻结不得判为心跳超时：{events:?}");
    assert_eq!(s.c.state(), &ConnectionState::Connected);
    // 旧 ping 的响应随后到达，新的 ping 正常往返
    s.reply(&ping["id"], json!({}));
    s.pump();
    s.run_until(s.now + MINUTE);
    assert_eq!(s.c.state(), &ConnectionState::Connected);

    // 进程正常运行而 Host 不响应：照常判心跳超时
    let mut s = Sim::new(config(LifecycleMode::Persistent, TransportKind::Remote), true);
    s.start();
    let at = s.c.poll_timeout().unwrap();
    s.now = at;
    s.c.handle_timeout(s.now);
    let _: Vec<Event> = std::iter::from_fn(|| s.c.poll_event()).collect();
    s.now = s.c.poll_timeout().unwrap();
    s.c.handle_timeout(s.now);
    assert!(matches!(s.c.state(), ConnectionState::Backoff { code: Some(ConnectionErrorCode::HeartbeatTimeout), .. }));
}

// ---------------------------------------------------------------------------
// A2：Host 不在
// ---------------------------------------------------------------------------

#[test]
fn host_absent_stops_retrying_after_limit() {
    for mode in [LifecycleMode::Idle, LifecycleMode::OnDemand] {
        let mut s = Sim::new(config(mode, TransportKind::Ipc), false);
        s.start();
        if mode == LifecycleMode::OnDemand {
            assert!(s.c.connect_now(s.now));
            s.pump();
        }
        s.run_until(s.now + HOUR);
        assert_eq!(s.connects, 3, "{mode:?}：连续 3 次 HOST_NOT_RUNNING 后停止");
        assert_eq!(s.c.state(), &ConnectionState::Dormant);
        assert_eq!(s.c.poll_timeout(), None);
        assert!(s.timer_fires <= 2, "{mode:?}：{}", s.timer_fires);

        // Host 起来后，可见 / App 主动唤醒即回连
        s.host_up = true;
        assert!(s.c.wake_with_reason(WakeReason::Visible, s.now));
        s.pump();
        assert_eq!(s.c.state(), &ConnectionState::Connected);
        assert_eq!(s.hellos[0]["wakeReason"], json!("visible"));
    }
}

#[test]
fn host_absent_counter_resets_and_persistent_keeps_retrying() {
    // persistent：一直退避重连（最长 30 s 一次）
    let mut s = Sim::new(config(LifecycleMode::Persistent, TransportKind::Ipc), false);
    s.start();
    s.run_until(s.now + HOUR);
    assert!(matches!(s.c.state(), ConnectionState::Backoff { .. }));
    assert!(s.connects > 100 && s.connects <= 3600 / 30 + 8, "{}", s.connects);

    // 0 = 不限（旧行为）；legacy_timers 同样
    for (limit, legacy) in [(0, false), (3, true)] {
        let mut cfg = config(LifecycleMode::Idle, TransportKind::Ipc);
        cfg.lifecycle.host_absent_retries = limit;
        cfg.lifecycle.legacy_timers = legacy;
        let mut s = Sim::new(cfg, false);
        s.start();
        s.run_until(s.now + 10 * MINUTE);
        assert!(s.connects > 10, "limit={limit} legacy={legacy}");
    }

    // 其他原因的失败打断"连续"：2 次不在 → 1 次超时 → 需要再连续 3 次不在才休眠
    let mut s = Sim::new(config(LifecycleMode::Idle, TransportKind::Ipc), false);
    s.start();
    let next = s.c.poll_timeout().unwrap();
    s.run_until(next);
    assert_eq!(s.connects, 2);
    s.fail_code = ConnectionErrorCode::ConnectTimeout;
    let next = s.c.poll_timeout().unwrap();
    s.run_until(next);
    assert_eq!(s.connects, 3);
    s.fail_code = ConnectionErrorCode::HostNotRunning;
    s.run_until(s.now + HOUR);
    assert_eq!(s.c.state(), &ConnectionState::Dormant);
    assert_eq!(s.connects, 6);
}

// ---------------------------------------------------------------------------
// A1：调用后在线时长
// ---------------------------------------------------------------------------

/// 一次调用（Host 随后发 60 s 租约）完成后到发出 `app/sleep` 的时长，以及期间的定时器触发次数。
fn online_after_call(cfg: ClientConfig) -> (Millis, usize) {
    let mut s = Sim::new(cfg, true);
    s.start();
    let invoke = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/invoke",
        "params": {"callId": "c1", "name": "missing", "arguments": {}}});
    s.c.handle_message(&invoke.to_string(), s.now);
    let lease = json!({"jsonrpc": "2.0", "method": "app/lease", "params": {"ttlMs": 60_000}});
    s.c.handle_message(&lease.to_string(), s.now);
    s.pump();
    let done = s.now;
    let mut fires = 0;
    while s.sleep_at.is_none() {
        let t = s.c.poll_timeout().expect("应有休眠定时器");
        fires += 1;
        s.run_until(t);
    }
    (s.sleep_at.unwrap_or(0) - done, fires)
}

#[test]
fn online_time_after_call_is_bounded() {
    // 前台 idle（空闲 60 s）+ 60 s 租约：并行后在线 60 s、期间只有 1 次定时器（旧行为 120 s、8 次心跳）
    let cfg = config(LifecycleMode::Idle, TransportKind::Ipc);
    assert_eq!(online_after_call(cfg.clone()), (60_000, 1));
    let mut legacy = cfg.clone();
    legacy.lifecycle.legacy_timers = true;
    assert_eq!(online_after_call(legacy).0, 120_000);

    // 空闲 15 s（如隐藏）：受租约约束为 60 s（旧行为 75 s）
    let mut short = cfg;
    short.lifecycle.idle_timeout_ms = 15_000;
    assert_eq!(online_after_call(short), (60_000, 1));
}
