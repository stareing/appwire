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
    }
}

fn resource(name: &str) -> ResourceDef {
    ResourceDef { name: name.into(), description: format!("{name} 资源"), mime_type: None, scope: None }
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

    // 任务完成后经过 graceMs（默认 10s）休眠，原因 grace
    h.invoke("c1", "a");
    h.advance(3_000);
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    let done = h.now;
    let (sleep, at) = h.wait_sleep(IDLE).unwrap();
    assert_eq!(at, done + 10_000);
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
    assert_eq!(at, done + IDLE, "从最后一个调用完成时重新计时");
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
    assert_eq!(at, done + IDLE);
}

#[test]
fn subscription_blocks_idle() {
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(resource("r")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "r"}));
    assert!(h.c.is_subscribed(r));
    assert_eq!(h.wait_sleep(3 * IDLE), None, "有订阅时不休眠");
    h.request("resources/unsubscribe", json!({"name": "r"}));
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);

    // 注销资源导致订阅被清除时同样重新判定
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(resource("r")).unwrap();
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
    assert_eq!(sleep["params"]["reason"], "background");

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
    assert_eq!(at, lease_end + IDLE, "租约期间不休眠，租约结束后重新计时");

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
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);
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
    h.c.complete_call("c1", Ok(CallOutput { data: json!(7), state_hints: vec![] }), h.now).unwrap();
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
    // 再次休眠
    let t = h.now;
    let (sleep2, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t + IDLE);
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
        })
        .unwrap();
    h.c.register_resource(ResourceDef {
        name: "cart.state".into(),
        description: "购物车".into(),
        mime_type: None,
        scope: None,
    })
    .unwrap();
    assert_eq!(h.c.tools_hash(), "ba703035ddca2f91");
    // 禁用的工具不计入
    h.c.update_tool(checkout, ToolUpdate { enabled: Some(false), ..Default::default() }).unwrap();
    assert_ne!(h.c.tools_hash(), "ba703035ddca2f91");
    h.c.update_tool(checkout, ToolUpdate { enabled: Some(true), ..Default::default() }).unwrap();
    assert_eq!(h.c.tools_hash(), "ba703035ddca2f91");
}
