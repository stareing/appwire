//! 生命周期行为测试（spec/lifecycle.md 第 2、3、6、7 节），全部用确定性时间驱动。

use app_mcp_core::*;
use serde_json::{Value, json};

struct Harness {
    c: Client,
    now: Millis,
    next_id: i64,
}

fn config(mode: LifecycleMode) -> ClientConfig {
    let mut c = ClientConfig::new("shop", "示例商城", "inst-1", ClientKind::Native);
    c.lifecycle.mode = mode;
    c
}

fn tool(name: &str) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: format!("{name} 的描述"),
        input_schema: json!({"type": "object", "properties": {}}),
        risk: Risk::Read,
        activation: None,
        title: None,
        enabled: true,
        scope: None,
        annotations: None,
        output_schema: None,
        surface: ToolSurface::App,
        page: None,
    }
}

fn resource(name: &str) -> ResourceDef {
    ResourceDef { name: name.into(), description: format!("{name} 资源"), mime_type: None, scope: None, realtime: false, annotations: None }
}

fn sends(events: &[Event]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Send(s) => Some(serde_json::from_str(s).expect("json")),
            _ => None,
        })
        .collect()
}

fn methods(msgs: &[Value]) -> Vec<&str> {
    msgs.iter().map(|m| m["method"].as_str().unwrap_or("<response>")).collect()
}

fn find<'a>(msgs: &'a [Value], method: &str) -> Option<&'a Value> {
    msgs.iter().find(|m| m["method"] == method)
}

const IDLE: Millis = 60_000;
/// 调用 / 读取后的合并窗口默认值（spec/lifecycle.md 第 13 节 B1）。
const MERGE: Millis = 2_000;

impl Harness {
    fn new(mode: LifecycleMode) -> Self {
        Self::with(config(mode))
    }

    fn with(cfg: ClientConfig) -> Self {
        Self { c: Client::new(cfg), now: 1_000, next_id: 0 }
    }

    fn drain(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Some(e) = self.c.poll_event() {
            out.push(e);
        }
        out
    }

    fn recv(&mut self, msg: Value) -> Vec<Event> {
        self.c.handle_message(&msg.to_string(), self.now);
        self.drain()
    }

    fn respond(&mut self, req: &Value, result: Value) -> Vec<Event> {
        self.recv(json!({"jsonrpc": "2.0", "id": req["id"], "result": result}))
    }

    /// 连接建立（驱动层已处理 Connect），返回 hello。
    fn link(&mut self) -> Value {
        self.c.handle_connected(self.now);
        let msgs = sends(&self.drain());
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["method"], "app/hello");
        msgs[0].clone()
    }

    fn paired(&mut self, hello: &Value, tools_current: bool) -> Vec<Event> {
        let mut r = json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "h"});
        if tools_current {
            r["toolsCurrent"] = json!(true);
        }
        let ev = self.respond(hello, r);
        assert_eq!(self.c.state(), &ConnectionState::Connected);
        ev
    }

    /// start + 连接 + 握手，返回 hello。
    fn connect(&mut self) -> Value {
        self.c.start(self.now);
        let ev = self.drain();
        assert!(ev.contains(&Event::Connect), "{ev:?}");
        let hello = self.link();
        self.paired(&hello, false);
        hello
    }

    fn advance(&mut self, ms: Millis) -> Vec<Event> {
        let target = self.now + ms;
        let mut out = Vec::new();
        while let Some(t) = self.c.poll_timeout() {
            if t > target {
                break;
            }
            self.now = self.now.max(t);
            self.c.handle_timeout(self.now);
            let ev = self.drain();
            // 自动回复心跳
            for m in sends(&ev) {
                if m["method"] == "ping" {
                    self.respond(&m, json!({}));
                }
            }
            out.extend(ev);
        }
        self.now = target;
        self.c.handle_timeout(self.now);
        out.extend(self.drain());
        out
    }

    /// 推进时间直到发出 app/sleep，返回该请求与发出时刻。
    fn wait_sleep(&mut self, limit: Millis) -> Option<(Value, Millis)> {
        let end = self.now + limit;
        while self.now < end {
            let step = self.c.poll_timeout().map_or(end, |t| t.min(end)).max(self.now + 1) - self.now;
            let ev = self.advance(step);
            if let Some(m) = find(&sends(&ev), "app/sleep") {
                return Some((m.clone(), self.now));
            }
        }
        None
    }

    fn invoke(&mut self, call_id: &str, name: &str) -> Vec<Event> {
        self.next_id += 1;
        self.recv(json!({"jsonrpc": "2.0", "id": 1000 + self.next_id, "method": "tools/invoke",
            "params": {"callId": call_id, "name": name, "arguments": {}}}))
    }

    fn request(&mut self, method: &str, params: Value) -> Vec<Event> {
        self.next_id += 1;
        self.recv(json!({"jsonrpc": "2.0", "id": 1000 + self.next_id, "method": method, "params": params}))
    }

    fn notify(&mut self, method: &str, params: Value) -> Vec<Event> {
        self.recv(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// 休眠握手被接受。
    fn accept_sleep(&mut self, sleep: &Value, resume: &str) -> Vec<Event> {
        self.respond(sleep, json!({"accepted": true, "resumeToken": resume}))
    }

    /// 从已连接进入 Dormant（idle 模式），返回 sleep 请求。
    fn go_dormant(&mut self) -> Value {
        let (sleep, _) = self.wait_sleep(10 * IDLE).expect("应发出 app/sleep");
        let ev = self.accept_sleep(&sleep, "resume-1");
        assert!(ev.contains(&Event::Disconnect));
        assert_eq!(self.c.state(), &ConnectionState::Dormant);
        sleep
    }
}

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

// ---------------------------------------------------------------------------
// 空闲条件：每个重置条件
// ---------------------------------------------------------------------------

#[test]
fn running_and_queued_calls_block_idle_and_reset_timer() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.advance(IDLE - 1_000);
    h.invoke("c1", "a");
    h.invoke("c2", "a"); // 排队
    assert_eq!(h.c.queued_call_count(), 1);
    assert_eq!(h.wait_sleep(3 * IDLE), None, "调用进行中不休眠");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(h.wait_sleep(3 * IDLE), None, "排队中的调用开始执行，仍不休眠");
    h.c.complete_call("c2", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    let done = h.now;
    let (_, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, done + MERGE, "从最后一个调用完成时重新计时（处理过调用后为合并窗口）");
}

#[test]
fn resource_read_blocks_idle_and_completion_rechecks() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_resource(resource("r")).unwrap();
    h.connect();
    let ev = h.request("resources/read", json!({"name": "r"}));
    let read = ev
        .iter()
        .find_map(|e| match e {
            Event::ReadResource { read, .. } => Some(*read),
            _ => None,
        })
        .unwrap();
    assert_eq!(h.wait_sleep(3 * IDLE), None, "读取进行中不休眠");
    h.c.complete_read(read, Ok(json!(1))).unwrap();
    h.drain();
    assert_eq!(h.c.poll_timeout(), Some(h.now), "没有时间参数时请求立即重新判定");
    h.advance(0);
    let done = h.now;
    let (_, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, done + MERGE, "读取也算处理过调用：合并窗口");
}

#[test]
fn navigation_blocks_idle_and_completion_rechecks() {
    let mut cfg = config(LifecycleMode::Idle);
    cfg.navigation = true;
    let mut h = Harness::with(cfg);
    h.connect();
    let ev = h.request("app/navigate", json!({"page": "cart"}));
    let nav = ev
        .iter()
        .find_map(|e| match e {
            Event::Navigate { navigate, .. } => Some(*navigate),
            _ => None,
        })
        .unwrap();
    assert_eq!(h.wait_sleep(3 * IDLE), None, "导航进行中不休眠");
    h.c.complete_navigate(nav, Ok(())).unwrap();
    h.drain();
    assert_eq!(h.c.poll_timeout(), Some(h.now), "完成后立即重新判定");
    h.advance(0);
    let done = h.now;
    let (_, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, done + MERGE, "导航也算处理过请求：合并窗口");
}

fn realtime(name: &str) -> ResourceDef {
    ResourceDef { realtime: true, ..resource(name) }
}

#[test]
fn realtime_subscription_blocks_idle() {
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(realtime("r")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "r"}));
    assert!(h.c.is_subscribed(r));
    assert_eq!(h.wait_sleep(3 * IDLE), None, "有实时资源订阅时不休眠");
    h.request("resources/unsubscribe", json!({"name": "r"}));
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);

    // 注销资源导致订阅被清除时同样重新判定
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(realtime("r")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "r"}));
    h.advance(IDLE);
    h.c.unregister_resource(r).unwrap();
    h.drain();
    assert_eq!(h.c.poll_timeout(), Some(h.now));
    h.advance(0);
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);
}

#[test]
fn hold_blocks_idle_until_released() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    let hold = h.c.hold(h.now);
    let hold2 = h.c.hold(h.now);
    assert_ne!(hold, hold2);
    assert_eq!(h.c.hold_count(), 2);
    assert_eq!(h.wait_sleep(3 * IDLE), None);
    assert!(h.c.release_hold(hold, h.now));
    assert!(!h.c.release_hold(hold, h.now), "重复释放返回 false");
    assert_eq!(h.wait_sleep(3 * IDLE), None, "仍有一个持有");
    assert!(h.c.release_hold(hold2, h.now));
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);
}

#[test]
fn hold_for_call_outlives_the_call() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    assert_eq!(h.c.hold_for_call("nope", h.now), Err(CoreError::UnknownCall("nope".into())));
    h.invoke("c1", "a");
    let hold = h.c.hold_for_call("c1", h.now).unwrap();
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(h.wait_sleep(3 * IDLE), None, "调用完成后持有仍然有效");
    h.c.release_hold(hold, h.now);
    assert!(h.wait_sleep(3 * IDLE).is_some());
}

#[test]
fn visibility_change_restarts_timer_and_hidden_is_shorter() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.advance(50_000);
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    h.drain();
    let t = h.now;
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t + 15_000, "隐藏后使用 hiddenIdleTimeoutMs，并从变化时重新计时");
    assert_eq!(sleep["params"]["reason"], "idle", "空闲计时到期一律为 idle，隐藏只缩短计时");

    // 重新可见也重新计时
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    h.connect();
    h.advance(10_000);
    h.c.set_visibility(Visibility::Visible, true, h.now);
    h.drain();
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);
}

#[test]
fn lease_defers_sleep_and_zero_cancels() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.advance(10_000);
    h.notify("app/lease", json!({"ttlMs": 100_000}));
    let lease_end = h.now + 100_000;
    // 更短的新租约不缩短截止时刻
    h.advance(1_000);
    h.notify("app/lease", json!({"ttlMs": 5_000}));
    let (_, at) = h.wait_sleep(10 * IDLE).unwrap();
    assert_eq!(at, lease_end, "A1：租约与空闲计时并行，休眠时刻 = max(空闲起点 + 空闲时长, 租约到期)");

    // 租约比空闲时长短：按空闲计时
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.notify("app/lease", json!({"ttlMs": 10_000}));
    let t = h.now;
    assert_eq!(h.wait_sleep(10 * IDLE).unwrap().1, t + IDLE);

    // 回退开关：租约到期后才开始计空闲（旧行为）
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.legacy_timers = true;
    let mut h = Harness::with(cfg);
    h.connect();
    h.notify("app/lease", json!({"ttlMs": 100_000}));
    let lease_end = h.now + 100_000;
    assert_eq!(h.wait_sleep(10 * IDLE).unwrap().1, lease_end + IDLE, "legacy_timers：串行");

    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.notify("app/lease", json!({"ttlMs": 1_000_000}));
    h.advance(IDLE * 2);
    assert!(!h.c.is_sleep_pending());
    h.notify("app/lease", json!({"ttlMs": 0}));
    let t = h.now;
    assert_eq!(h.wait_sleep(10 * IDLE).unwrap().1, t + IDLE, "取消租约后重新计时");

    // 参数无效只告警
    let ev = h.notify("app/lease", json!({"ttl": 1}));
    assert!(ev.iter().any(|e| matches!(e, Event::Warning(_))));
}

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

// ---------------------------------------------------------------------------
// 唤醒与快速恢复
// ---------------------------------------------------------------------------

#[test]
fn wake_with_tools_current_skips_sync() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    let sleep = h.go_dormant();

    assert!(!h.c.handle_wake("--unrelated", h.now));
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    assert!(h.c.handle_wake("myapp://app-mcp/wake?token=wk-1", h.now));
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Waking)]);
    let hello = h.link();
    let p = &hello["params"];
    assert_eq!(p["launchToken"], "wk-1");
    assert_eq!(p["resumeToken"], "resume-1");
    assert_eq!(p["toolsHash"], sleep["params"]["toolsHash"]);
    assert_eq!(p["wakeReason"], "os-activation");
    assert_eq!(p["token"], "tk");

    let ev = h.paired(&hello, true);
    assert_eq!(methods(&sends(&ev)), vec!["app/visibility", "app/ready"], "toolsCurrent 跳过全量同步");
    assert_eq!(h.c.resume_token(), None, "恢复令牌只用一次");

    // 恢复后增量追踪照常工作，调用照常处理
    h.c.register_tool(tool("b")).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(methods(&msgs), vec!["tools/changed"]);
    let ev = h.invoke("c1", "a");
    assert!(ev.iter().any(|e| matches!(e, Event::InvokeTool { .. })));
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    // 再次休眠（处理过调用：合并窗口）
    let t = h.now;
    let (sleep2, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t + MERGE);
    h.accept_sleep(&sleep2, "resume-2");
    assert_eq!(h.c.state(), &ConnectionState::Dormant);

    // 之后的普通回连不再带 launchToken / wakeReason os-activation
    assert!(h.c.wake(h.now));
    h.drain();
    let hello = h.link();
    assert!(hello["params"].get("launchToken").is_none());
    assert_eq!(hello["params"]["wakeReason"], "app");
    assert_eq!(hello["params"]["resumeToken"], "resume-2");
}

#[test]
fn registration_change_while_dormant_forces_full_sync() {
    let mut h = Harness::new(LifecycleMode::Idle);
    let a = h.c.register_tool(tool("a")).unwrap();
    h.connect();
    let sleep = h.go_dormant();
    // 休眠期间的注册变更不唤醒、不产生事件
    h.c.register_tool(tool("b")).unwrap();
    h.c.update_tool(a, ToolUpdate { description: Some("新描述".into()), ..Default::default() }).unwrap();
    assert!(h.drain().is_empty());
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    assert_eq!(h.c.poll_timeout(), None);

    assert!(h.c.handle_wake("app-mcp-wake:wk", h.now));
    h.drain();
    let hello = h.link();
    assert_ne!(hello["params"]["toolsHash"], sleep["params"]["toolsHash"], "摘要反映休眠期间的变更");
    assert_eq!(hello["params"]["toolsHash"], json!(h.c.tools_hash()));
    // Host 发现摘要不一致 → toolsCurrent: false → 完整同步
    let ev = h.paired(&hello, false);
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["tools/sync", "resources/sync", "app/visibility", "app/ready"]);
    assert_eq!(msgs[0]["params"]["tools"].as_array().unwrap().len(), 2);
}

#[test]
fn tools_current_without_resume_token_still_syncs() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.start(h.now);
    h.drain();
    let hello = h.link();
    let ev = h.paired(&hello, true);
    assert_eq!(methods(&sends(&ev)), vec!["tools/sync", "resources/sync", "app/visibility", "app/ready"]);
    assert!(ev.iter().any(|e| matches!(e, Event::Warning(_))));
}

#[test]
fn wake_in_backoff_reconnects_immediately() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.c.handle_disconnected(h.now);
    h.drain();
    assert!(matches!(h.c.state(), ConnectionState::Backoff { .. }));
    assert!(h.c.handle_wake("app-mcp-wake:abc", h.now));
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
    let hello = h.link();
    assert_eq!(hello["params"]["launchToken"], "abc");
    assert_eq!(hello["params"]["wakeReason"], "os-activation");
}

#[test]
fn wake_during_sleep_handshake_rewakes_after_accept() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    assert!(h.c.handle_wake("app-mcp-wake:late", h.now));
    let ev = h.accept_sleep(&sleep, "r1");
    assert_eq!(
        ev,
        vec![
            Event::Disconnect,
            Event::StateChanged(ConnectionState::Dormant),
            Event::Connect,
            Event::StateChanged(ConnectionState::Waking),
        ]
    );
    let hello = h.link();
    assert_eq!(hello["params"]["launchToken"], "late");
    assert_eq!(hello["params"]["resumeToken"], "r1");

    // hold 同理
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    let _hold = h.c.hold(h.now);
    let ev = h.accept_sleep(&sleep, "r1");
    assert!(ev.contains(&Event::StateChanged(ConnectionState::Waking)));
    assert_eq!(h.link()["params"]["wakeReason"], "app");
}

/// 回归：已连接时收到唤醒令牌（Android WakeWorker 重排后迟到、前台广播）不应残留，
/// 否则下一次空闲休眠被接受后会立即回连（2026-10-01 真机：DORMANT 后 11 ms 回连）。
#[test]
fn wake_token_while_connected_does_not_rewake_after_next_sleep() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.advance(IDLE / 2);
    assert!(h.c.handle_wake("app-mcp-wake:stale", h.now), "是本 SDK 的唤醒参数");
    assert!(h.drain().is_empty(), "已连接：不断开、不回连");
    // 重新开始空闲计时：从收到唤醒起满 IDLE 才休眠
    let woke_at = h.now;
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, woke_at + IDLE);
    let ev = h.accept_sleep(&sleep, "r1");
    assert_eq!(ev, vec![Event::Disconnect, Event::StateChanged(ConnectionState::Dormant)]);
    assert_eq!(h.c.poll_timeout(), None);

    // 之后的正常唤醒不携带过期令牌
    assert!(h.c.wake_with_reason(WakeReason::Visible, h.now));
    h.drain();
    let hello = h.link();
    assert!(hello["params"].get("launchToken").is_none_or(Value::is_null), "{hello}");
    assert_eq!(hello["params"]["wakeReason"], "visible");
}

#[test]
fn handle_wake_before_start_connects_even_on_demand() {
    let mut cfg = config(LifecycleMode::OnDemand);
    cfg.lifecycle.residency = Residency::ExitWhenIdle;
    let mut h = Harness::with(cfg);
    assert!(h.c.handle_wake("shop://app-mcp/wake?token=cold", h.now));
    h.c.start(h.now);
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
    let hello = h.link();
    assert_eq!(hello["params"]["launchToken"], "cold");
    assert_eq!(hello["params"]["wakeReason"], "os-activation");
    h.paired(&hello, false);
    // 由唤醒冷启动 + exit-when-idle → 休眠后 IdleExit
    let (sleep, _) = h.wait_sleep(IDLE).unwrap();
    let ev = h.accept_sleep(&sleep, "r");
    assert_eq!(ev.last(), Some(&Event::IdleExit));
}

#[test]
fn residency_controls_idle_exit() {
    // exit-when-idle 但不是唤醒启动：不发 IdleExit
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.residency = Residency::ExitWhenIdle;
    let mut h = Harness::with(cfg);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    assert!(!h.accept_sleep(&sleep, "r").contains(&Event::IdleExit));

    // 配置带 launch token（Host 冷启动）→ 视为唤醒启动
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.residency = Residency::ExitWhenIdle;
    cfg.launch_token = Some("lt".into());
    let mut h = Harness::with(cfg);
    let hello = h.connect();
    assert_eq!(hello["params"]["wakeReason"], "os-activation");
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    assert!(h.accept_sleep(&sleep, "r").contains(&Event::IdleExit));

    // exit-always
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.residency = Residency::ExitAlways;
    let mut h = Harness::with(cfg);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(h.accept_sleep(&sleep, "r").last(), Some(&Event::IdleExit));

    // keep
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    assert!(!h.accept_sleep(&sleep, "r").contains(&Event::IdleExit));
}

#[test]
fn accepted_sleep_with_running_call_cancels_it() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    // 竞态：休眠握手中到达的调用照常执行
    let ev = h.invoke("c1", "a");
    assert!(ev.iter().any(|e| matches!(e, Event::InvokeTool { .. })));
    let ev = h.accept_sleep(&sleep, "r");
    assert!(ev.contains(&Event::CancelTool { call_id: "c1".into(), reason: CancelReason::Disconnected }));
    assert!(ev.iter().any(|e| matches!(e, Event::Warning(_))));
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
}

#[test]
fn stop_from_dormant() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.go_dormant();
    h.c.stop(h.now);
    assert_eq!(h.drain(), vec![Event::StateChanged(ConnectionState::Stopped)]);
    assert!(h.c.handle_wake("app-mcp-wake:x", h.now), "是本 SDK 的唤醒参数");
    assert_eq!(h.c.state(), &ConnectionState::Stopped);
    assert!(!h.c.wake(h.now));
}

// ---------------------------------------------------------------------------
// 握手超时
// ---------------------------------------------------------------------------

#[test]
fn handshake_timeout_reconnects() {
    let mut h = Harness::new(LifecycleMode::Persistent);
    h.c.start(h.now);
    h.drain();
    h.link();
    assert_eq!(h.c.poll_timeout(), Some(h.now + 10_000));
    let ev = h.advance(10_000);
    assert!(ev.contains(&Event::Disconnect));
    assert!(matches!(h.c.state(), ConnectionState::Backoff { .. }));

    // pending（等待用户确认）不受握手超时限制
    let mut h = Harness::new(LifecycleMode::Persistent);
    h.c.start(h.now);
    h.drain();
    let hello = h.link();
    h.respond(&hello, json!({"status": "pending", "protocolVersion": "1", "hostVersion": "h"}));
    assert_eq!(h.c.poll_timeout(), None);

    // 0 表示不限
    let mut cfg = config(LifecycleMode::Persistent);
    cfg.handshake_timeout_ms = 0;
    let mut h = Harness::with(cfg);
    h.c.start(h.now);
    h.drain();
    h.link();
    assert_eq!(h.c.poll_timeout(), None);
}

// ---------------------------------------------------------------------------
// toolsHash
// ---------------------------------------------------------------------------

#[test]
fn tools_hash_fixed_vector() {
    // 与 crates/protocol/src/hash.rs 的固定向量相同的定义。
    let mut h = Harness::new(LifecycleMode::Idle);
    assert_eq!(h.c.tools_hash(), "69c61b185225ee82", "空注册表");
    h.c.register_tool(ToolDef {
        name: "todo.add".into(),
        description: "添加待办".into(),
        input_schema: json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
        risk: Risk::Write,
        activation: None,
        title: None,
        enabled: true,
        scope: None,
        annotations: None,
        output_schema: None,
        surface: ToolSurface::App,
        page: None,
    })
    .unwrap();
    let checkout = h
        .c
        .register_tool(ToolDef {
            name: "cart.checkout".into(),
            description: "结算".into(),
            input_schema: json!({"type": "object"}),
            risk: Risk::Payment,
            activation: Some(Activation::Foreground),
            title: Some("Checkout".into()),
            enabled: true,
            scope: None,
            annotations: None,
            output_schema: None,
            surface: ToolSurface::App,
            page: None,
        })
        .unwrap();
    h.c.register_resource(ResourceDef {
        name: "cart.state".into(),
        description: "购物车".into(),
        mime_type: None,
        scope: None,
        realtime: false,
        annotations: None,
    })
    .unwrap();
    assert_eq!(h.c.tools_hash(), "ba703035ddca2f91");
    // 禁用的工具不计入
    h.c.update_tool(checkout, ToolUpdate { enabled: Some(false), ..Default::default() }).unwrap();
    assert_ne!(h.c.tools_hash(), "ba703035ddca2f91");
    h.c.update_tool(checkout, ToolUpdate { enabled: Some(true), ..Default::default() }).unwrap();
    assert_eq!(h.c.tools_hash(), "ba703035ddca2f91");
}

// ---------------------------------------------------------------------------
// 4e 第二部分（spec/lifecycle.md 第 13 节）：B1 合并窗口、B3 订阅、B4 后台立即休眠
// ---------------------------------------------------------------------------

#[test]
fn merge_window_applies_only_after_a_call_and_respects_lease() {
    // 连上后没有调用：仍按 idleTimeoutMs
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    let t0 = h.now;
    h.connect();
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t0 + IDLE);

    // 调用后有租约：在线到租约到期（合并窗口不缩短租约）
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.notify("app/lease", json!({"ttlMs": 30_000}));
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + 30_000);

    // 合并窗口不小于空闲时长：等同旧行为
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.merge_window_ms = 10 * IDLE;
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);

    // "处理过调用"随连接清除：回连后没有调用又按 idleTimeoutMs
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    h.go_dormant();
    assert!(h.c.wake(h.now));
    h.drain();
    let hello = h.link();
    h.paired(&hello, false);
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);
}

#[test]
fn plain_subscription_does_not_block_idle_and_changes_are_replayed_on_resubscribe() {
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(resource("cart")).unwrap();
    let other = h.c.register_resource(resource("other")).unwrap();
    let t0 = h.now;
    h.connect();
    h.request("resources/subscribe", json!({"name": "cart"}));
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t0 + IDLE, "普通资源的订阅不阻止休眠");
    h.accept_sleep(&sleep, "resume-1");

    // 休眠期间变化：不回连，只记下
    h.c.notify_resource_changed(r, h.now).unwrap();
    h.c.notify_resource_changed(other, h.now).unwrap();
    assert!(h.drain().is_empty(), "普通资源变化不回连");
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    assert_eq!(h.c.poll_timeout(), None);

    // 回连后 Host 重新订阅：先回复 {}，再补发 resources/updated；未订阅过的资源不补发
    assert!(h.c.wake(h.now));
    h.drain();
    let hello = h.link();
    h.paired(&hello, true);
    let ev = h.request("resources/subscribe", json!({"name": "cart"}));
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["<response>", "resources/updated"]);
    assert_eq!(msgs[1]["params"]["name"], "cart");
    // 只补发一次
    h.request("resources/unsubscribe", json!({"name": "cart"}));
    let ev = h.request("resources/subscribe", json!({"name": "cart"}));
    assert_eq!(methods(&sends(&ev)), vec!["<response>"]);
}

#[test]
fn change_before_resubscribe_is_replayed_and_unclaimed_subscriptions_are_dropped() {
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(resource("cart")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "cart"}));
    // 断线（不是休眠）后也补发
    h.c.handle_disconnected(h.now);
    h.drain();
    h.c.notify_resource_changed(r, h.now).unwrap();
    h.advance(1_000);
    let hello = h.link();
    h.paired(&hello, false);
    // 已连接但 Host 尚未重新订阅时的变化同样记下
    h.c.notify_resource_changed(r, h.now).unwrap();
    assert!(sends(&h.drain()).is_empty());
    let ev = h.request("resources/subscribe", json!({"name": "cart"}));
    assert_eq!(methods(&sends(&ev)), vec!["<response>", "resources/updated"]);

    // Host 这次连接没有重新订阅：下次断开后不再跟踪
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(resource("cart")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "cart"}));
    h.c.handle_disconnected(h.now);
    h.drain();
    h.advance(1_000);
    let hello = h.link();
    h.paired(&hello, false);
    h.c.handle_disconnected(h.now);
    h.drain();
    h.c.notify_resource_changed(r, h.now).unwrap();
    h.advance(1_000);
    let hello = h.link();
    h.paired(&hello, false);
    let ev = h.request("resources/subscribe", json!({"name": "cart"}));
    assert_eq!(methods(&sends(&ev)), vec!["<response>"]);
}

#[test]
fn realtime_change_while_dormant_wakes_to_push() {
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(realtime("order")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "order"}));
    // 实时订阅阻止自动休眠；App 显式休眠
    assert!(h.c.sleep(h.now));
    let sleep = find(&sends(&h.drain()), "app/sleep").cloned().unwrap();
    h.accept_sleep(&sleep, "resume-1");
    assert_eq!(h.c.state(), &ConnectionState::Dormant);

    h.c.notify_resource_changed(r, h.now).unwrap();
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Waking)]);
    let hello = h.link();
    assert_eq!(hello["params"]["wakeReason"], "app");
    h.paired(&hello, true);
    let ev = h.request("resources/subscribe", json!({"name": "order"}));
    assert_eq!(methods(&sends(&ev)), vec!["<response>", "resources/updated"]);
}

#[test]
fn legacy_timers_restore_subscription_and_merge_rules() {
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.legacy_timers = true;
    let mut h = Harness::with(cfg);
    let r = h.c.register_resource(resource("cart")).unwrap();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "cart"}));
    assert_eq!(h.wait_sleep(3 * IDLE), None, "旧行为：任何订阅都阻止休眠");
    h.request("resources/unsubscribe", json!({"name": "cart"}));
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    let t = h.now;
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t + IDLE, "旧行为：调用后仍按空闲时长");
    h.accept_sleep(&sleep, "resume-1");
    h.c.notify_resource_changed(r, h.now).unwrap();
    assert!(h.drain().is_empty());
}

fn background_cfg(mode: LifecycleMode) -> ClientConfig {
    let mut cfg = config(mode);
    cfg.lifecycle.sleep_on_background = true;
    cfg
}

#[test]
fn entering_background_sleeps_at_once_ignoring_lease() {
    let mut h = Harness::with(background_cfg(LifecycleMode::Idle));
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.notify("app/lease", json!({"ttlMs": 60_000}));
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    let msgs = sends(&h.drain());
    assert_eq!(methods(&msgs), vec!["app/visibility", "app/sleep"]);
    assert_eq!(msgs[1]["params"]["reason"], "background");

    // Host 拒绝：回到普通规则（租约 + 隐藏空闲时长）
    let t = h.now;
    h.respond(&msgs[1], json!({"accepted": false}));
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t + 60_000);
    assert_eq!(sleep["params"]["reason"], "idle");
}

#[test]
fn background_sleep_waits_for_running_call_then_sleeps_at_once() {
    let mut h = Harness::with(background_cfg(LifecycleMode::OnDemand));
    h.c.register_tool(tool("a")).unwrap();
    h.c.start(h.now);
    h.drain();
    assert!(h.c.connect_now(h.now));
    h.drain();
    let hello = h.link();
    h.paired(&hello, false);
    h.invoke("c1", "a");
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    assert_eq!(methods(&sends(&h.drain())), vec!["app/visibility"], "调用进行中不休眠");
    h.advance(5_000);
    h.notify("app/lease", json!({"ttlMs": 60_000}));
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(h.c.poll_timeout(), Some(h.now), "调用结束即到期，不等租约");
    let sleep = find(&sends(&h.advance(0)), "app/sleep").cloned().expect("应立即休眠");
    assert_eq!(sleep["params"]["reason"], "background");

    // 回到可见清除标记：普通规则
    let mut h = Harness::with(background_cfg(LifecycleMode::Idle));
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    let hold = h.c.hold(h.now);
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    h.c.set_visibility(Visibility::Visible, true, h.now);
    h.drain();
    h.c.release_hold(hold, h.now);
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);
}

#[test]
fn background_in_backoff_goes_dormant_and_switch_is_scoped() {
    let mut h = Harness::with(background_cfg(LifecycleMode::Idle));
    h.c.start(h.now);
    h.drain();
    h.c.handle_disconnected(h.now);
    h.drain();
    assert!(matches!(h.c.state(), ConnectionState::Backoff { .. }));
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    assert_eq!(h.c.poll_timeout(), None);

    // 关闭（默认）、persistent、legacy_timers：进入后台不立即休眠
    for (mode, flag, legacy) in
        [(LifecycleMode::Idle, false, false), (LifecycleMode::Persistent, true, false), (LifecycleMode::Idle, true, true)]
    {
        let mut cfg = config(mode);
        cfg.lifecycle.sleep_on_background = flag;
        cfg.lifecycle.legacy_timers = legacy;
        let mut h = Harness::with(cfg);
        h.connect();
        h.c.set_visibility(Visibility::Hidden, false, h.now);
        assert_eq!(methods(&sends(&h.drain())), vec!["app/visibility"], "{mode:?} {flag} {legacy}");
    }

    // 隐藏 → 冻结不是"进入后台"；已隐藏时连上的不触发
    let mut h = Harness::with(background_cfg(LifecycleMode::Idle));
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    h.connect();
    h.c.set_visibility(Visibility::Frozen, false, h.now);
    assert_eq!(methods(&sends(&h.drain())), vec!["app/visibility"]);
}
