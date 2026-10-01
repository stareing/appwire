//! 功耗回归测试（spec/lifecycle.md 第 11 节 A1–A3、第 13 节 B1 / B3 / B4、O2）：用确定性时间模拟驱动层，统计定时器触发、
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
    /// 累计在线（`Connected`，含休眠握手中）毫秒数与当前这段的起点。
    online_ms: Millis,
    online_since: Option<Millis>,
}

fn config(mode: LifecycleMode, transport: TransportKind) -> ClientConfig {
    let mut c = ClientConfig::new("shop", "示例商城", "inst-1", ClientKind::Native);
    c.lifecycle.mode = mode;
    c.transport = transport;
    c
}

impl Sim {
    fn new(cfg: ClientConfig, host_up: bool) -> Self {
        Self {
            c: Client::new(cfg),
            now: 1_000,
            host_up,
            timer_fires: 0,
            connects: 0,
            pings: 0,
            sleeps: vec![],
            sleep_at: None,
            hellos: vec![],
            fail_code: ConnectionErrorCode::HostNotRunning,
            online_ms: 0,
            online_since: None,
        }
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
                            // Host 接受休眠（之后的消息在本轮事件之后处理）
                            let id = m["id"].clone();
                            self.sleeps.push(m);
                            self.reply(&id, json!({"accepted": true, "resumeToken": "r"}));
                        }
                        _ => {}
                    }
                }
                Event::StateChanged(ConnectionState::Connected) => {
                    self.online_since.get_or_insert(self.now);
                }
                Event::StateChanged(_) => {
                    if let Some(since) = self.online_since.take() {
                        self.online_ms += self.now - since;
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

    /// Host 发来一条消息（请求 / 通知）。
    fn host_sends(&mut self, msg: Value) {
        self.c.handle_message(&msg.to_string(), self.now);
        self.pump();
    }

    /// 一次立即完成的调用（工具不存在 → 直接返回错误；对生命周期而言与成功的调用相同）。
    fn call(&mut self, id: i64) {
        self.host_sends(json!({"jsonrpc": "2.0", "id": id, "method": "tools/invoke",
            "params": {"callId": format!("c{id}"), "name": "missing", "arguments": {}}}));
    }

    fn lease(&mut self, ttl_ms: Millis) {
        self.host_sends(json!({"jsonrpc": "2.0", "method": "app/lease", "params": {"ttlMs": ttl_ms}}));
    }

    /// 截至当前的累计在线毫秒数。
    fn online(&self) -> Millis {
        self.online_ms + self.online_since.map_or(0, |t| self.now - t)
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

/// 一次调用（Host 随后发 `lease_ms` 租约，0 = 不给租约）完成后到发出 `app/sleep` 的时长，以及期间的定时器触发次数。
fn online_after_call_with_lease(cfg: ClientConfig, lease_ms: Millis) -> (Millis, usize) {
    let mut s = Sim::new(cfg, true);
    s.start();
    if s.c.state() == &ConnectionState::Dormant {
        // on-demand：被唤醒后连接
        assert!(s.c.handle_wake("app-mcp-wake:wk", s.now));
        s.pump();
    }
    s.call(7);
    s.lease(lease_ms);
    let done = s.now;
    let mut fires = 0;
    while s.sleep_at.is_none() {
        let t = s.c.poll_timeout().expect("应有休眠定时器");
        fires += 1;
        s.run_until(t);
    }
    (s.sleep_at.unwrap_or(0) - done, fires)
}

fn online_after_call(cfg: ClientConfig) -> (Millis, usize) {
    online_after_call_with_lease(cfg, 60_000)
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

// ---------------------------------------------------------------------------
// B1：调用后只留合并窗口
// ---------------------------------------------------------------------------

#[test]
fn online_after_call_is_merge_window_when_no_lease() {
    // 租约为 0（或 Hub 不给租约）：调用后在线 = 合并窗口 2 s，期间 1 次定时器（旧行为：空闲时长 60 s / 宽限 10 s）
    for mode in [LifecycleMode::Idle, LifecycleMode::OnDemand] {
        let cfg = config(mode, TransportKind::Ipc);
        assert_eq!(online_after_call_with_lease(cfg.clone(), 0), (2_000, 1), "{mode:?}");
        let mut legacy = cfg;
        legacy.lifecycle.legacy_timers = true;
        let expected = if mode == LifecycleMode::Idle { 60_000 } else { 10_000 };
        assert_eq!(online_after_call_with_lease(legacy, 0).0, expected, "{mode:?} 旧行为");
    }
    // 是否继续在线只由租约决定
    let cfg = config(LifecycleMode::Idle, TransportKind::Ipc);
    assert_eq!(online_after_call_with_lease(cfg.clone(), 5_000), (5_000, 1));
    // 合并窗口可配置
    let mut wide = cfg;
    wide.lifecycle.merge_window_ms = 1_000;
    assert_eq!(online_after_call_with_lease(wide, 0), (1_000, 1));
}

#[test]
fn burst_of_calls_costs_one_connection_per_burst() {
    // on-demand：每 10 分钟一组 3 次调用（组内间隔 1 s，Hub 不给租约），1 小时 6 组 → 6 次连接、在线 ≈ 6 × (2 + 2) s
    let mut s = Sim::new(config(LifecycleMode::OnDemand, TransportKind::Ipc), true);
    s.start();
    let mut id = 0;
    for burst in 0..6 {
        s.run_until(1_000 + burst * 10 * MINUTE);
        assert!(s.c.handle_wake("app-mcp-wake:wk", s.now));
        s.pump();
        for _ in 0..3 {
            id += 1;
            s.call(id);
            s.run_until(s.now + 1_000);
        }
    }
    s.run_until(s.now + HOUR);
    assert_eq!(s.connects, 6);
    assert_eq!(s.c.state(), &ConnectionState::Dormant);
    assert!(s.online() <= 6 * 4_000, "{}", s.online());
}

// ---------------------------------------------------------------------------
// B3：资源订阅不强制在线
// ---------------------------------------------------------------------------

fn subscribed_hour(realtime: bool, legacy: bool) -> Sim {
    let mut cfg = config(LifecycleMode::Idle, TransportKind::Ipc);
    cfg.lifecycle.legacy_timers = legacy;
    let mut s = Sim::new(cfg, true);
    let r = s
        .c
        .register_resource(ResourceDef {
            name: "cart".into(),
            description: "购物车".into(),
            mime_type: None,
            scope: None,
            realtime,
        })
        .unwrap();
    s.start();
    s.host_sends(json!({"jsonrpc": "2.0", "id": 1, "method": "resources/subscribe", "params": {"name": "cart"}}));
    // 每 5 分钟变化一次
    for i in 1..=12 {
        s.run_until(1_000 + i * 5 * MINUTE);
        s.c.notify_resource_changed(r, s.now).unwrap();
        s.pump();
    }
    s
}

#[test]
fn plain_subscription_does_not_keep_app_online() {
    // 普通资源：空闲 60 s 后照常休眠，变化不回连（下次连接时补发）
    let s = subscribed_hour(false, false);
    assert_eq!(s.connects, 1);
    assert_eq!(s.online(), 60_000);
    assert_eq!(s.c.state(), &ConnectionState::Dormant);
    assert_eq!(s.c.poll_timeout(), None);

    // 旧行为：任何订阅都保持在线整小时
    let s = subscribed_hour(false, true);
    assert_eq!(s.connects, 1);
    assert!(s.online() >= HOUR, "{}", s.online());
    assert!(s.sleeps.is_empty());
}

#[test]
fn realtime_subscription_keeps_app_online() {
    let s = subscribed_hour(true, false);
    assert_eq!(s.connects, 1);
    assert!(s.sleeps.is_empty());
    assert!(s.online() >= HOUR, "{}", s.online());
    // 本地传输：在线期间仍没有定时器（变化随即推送，无节流定时器）
    assert_eq!(s.timer_fires, 0);
}

// ---------------------------------------------------------------------------
// B4：后台立即休眠
// ---------------------------------------------------------------------------

/// 调用 + 60 s 租约后立即进入后台，到发出 `app/sleep` 的时长。
fn online_after_background(cfg: ClientConfig) -> Millis {
    let mut s = Sim::new(cfg, true);
    s.start();
    s.call(7);
    s.lease(60_000);
    let at = s.now;
    s.c.set_visibility(Visibility::Hidden, false, s.now);
    s.pump();
    while s.sleep_at.is_none() {
        let t = s.c.poll_timeout().expect("应有休眠定时器");
        s.run_until(t);
    }
    s.sleep_at.unwrap_or(0) - at
}

#[test]
fn background_sleeps_at_once() {
    let mut cfg = config(LifecycleMode::Idle, TransportKind::Remote);
    cfg.lifecycle.sleep_on_background = true;
    assert_eq!(online_after_background(cfg.clone()), 0, "进入后台立即休眠，不等租约");

    // 关闭（核心默认）或旧行为：按租约在线 60 s
    let mut off = cfg.clone();
    off.lifecycle.sleep_on_background = false;
    assert_eq!(online_after_background(off), 60_000);
    let mut legacy = cfg;
    legacy.lifecycle.legacy_timers = true;
    assert_eq!(online_after_background(legacy), 60_000 + 15_000, "旧行为：租约后再计隐藏空闲 15 s");
}
