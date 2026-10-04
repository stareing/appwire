//! 4e 功耗：Hub 侧心跳规则（spec/lifecycle.md 第 11 节 A3）、功耗观测与唤醒速率上限（第 12 节 O1 / O4）。
//!
//! App 端为手写的 WebSocket 客户端：收到的每条消息（含 Hub 的 `ping`）都交给测试，不自动回复。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{
    AppStatus, AwakeReason, CallRequest, ErrorKind, Hub, HubConfig, HubError, InstancePower, WakeRequest, Waker,
    async_trait,
};
use app_mcp_protocol::{LifecycleMode, ResourcesSyncParams, ToolsSyncParams, tools_hash};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};
use tokio_tungstenite::tungstenite::Message as Ws;

const T: Duration = Duration::from_secs(10);

/// 短心跳 / 空闲时间，便于观察 ping 与无消息断开。
fn config() -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        list_changed_debounce: Duration::from_millis(10),
        ping_interval: Duration::from_millis(50),
        idle_timeout: Duration::from_millis(250),
        hidden_idle_timeout: Duration::from_millis(250),
        lease_ttl: Duration::ZERO,
        wake_timeout: Duration::from_millis(300),
        ..Default::default()
    }
}

fn tools() -> Value {
    json!([{"name": "echo", "description": "回显", "inputSchema": {"type": "object"}, "risk": "read"}])
}

/// 收到的消息；`None` 表示连接已被 Hub 关闭。
type Inbox = mpsc::UnboundedReceiver<Option<Value>>;

struct Raw {
    out: mpsc::UnboundedSender<Option<String>>,
    inbox: Inbox,
}

impl Raw {
    async fn connect(hub: &Hub, instance_id: &str, extra: Value) -> Raw {
        let url = format!("ws://{}/app", hub.listen_addr().unwrap());
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let (mut sink, mut stream) = ws.split();
        let mut hello = json!({
            "appId": "raw", "appName": "Raw", "protocolVersion": "1", "sdkVersion": "0",
            "clientKind": "native", "instanceId": instance_id
        });
        if let Value::Object(m) = extra {
            for (k, v) in m {
                hello[k] = v;
            }
        }
        let note = |m: &str, p: Value| Ws::text(json!({"jsonrpc": "2.0", "method": m, "params": p}).to_string());
        sink.send(Ws::text(json!({"jsonrpc": "2.0", "id": 0, "method": "app/hello", "params": hello}).to_string()))
            .await
            .unwrap();
        loop {
            let Some(Ok(Ws::Text(t))) = timeout(T, stream.next()).await.unwrap() else { panic!("握手时断开") };
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["id"] == 0 {
                assert_eq!(v["result"]["status"], "paired");
                break;
            }
        }
        sink.send(note("tools/sync", json!({ "tools": tools() }))).await.unwrap();
        sink.send(note("resources/sync", json!({ "resources": [] }))).await.unwrap();
        sink.send(note("app/visibility", json!({"visibility": "visible", "focused": false}))).await.unwrap();
        sink.send(note("app/ready", json!({}))).await.unwrap();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Option<String>>();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    m = out_rx.recv() => match m {
                        Some(Some(t)) => { let _ = sink.send(Ws::text(t)).await; }
                        _ => { let _ = sink.close().await; break; }
                    },
                    m = stream.next() => {
                        let Some(Ok(Ws::Text(t))) = m else {
                            let _ = in_tx.send(None);
                            break;
                        };
                        let _ = in_tx.send(Some(serde_json::from_str(t.as_str()).unwrap()));
                    }
                }
            }
        });
        Raw { out: out_tx, inbox: in_rx }
    }

    fn send(&self, v: Value) {
        let _ = self.out.send(Some(v.to_string()));
    }

    fn close(&self) {
        let _ = self.out.send(None);
    }

    /// 观察 `window` 内收到的消息：`(Hub ping 次数, 连接是否被关闭)`。
    async fn observe(&mut self, window: Duration) -> (usize, bool) {
        let deadline = Instant::now() + window;
        let mut pings = 0;
        loop {
            match tokio::time::timeout_at(deadline, self.inbox.recv()).await {
                Err(_) => return (pings, false),
                Ok(None | Some(None)) => return (pings, true),
                Ok(Some(Some(v))) => {
                    if v["method"] == "ping" {
                        pings += 1;
                    }
                }
            }
        }
    }

    async fn expect(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        timeout(T, async {
            loop {
                let v = self.inbox.recv().await.flatten().expect("连接已关闭");
                if pred(&v) {
                    return v;
                }
            }
        })
        .await
        .expect("没有等到消息")
    }
}

fn app_status(hub: &Hub, app_id: &str) -> Option<AppStatus> {
    hub.status().apps.into_iter().find(|a| a.app_id == app_id)
}

fn power(hub: &Hub, instance_id: &str) -> InstancePower {
    app_status(hub, "raw")
        .and_then(|a| a.instances.into_iter().find(|i| i.info.instance_id == instance_id))
        .and_then(|i| i.power)
        .expect("实例功耗观测")
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + T;
    while !f() {
        assert!(Instant::now() < deadline, "等待超时：{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---------------------------------------------------------------------------
// A3：心跳规则
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn local_transport_no_ping_and_no_idle_disconnect() {
    let hub = Hub::start(config()).await.unwrap();
    let mut raw = Raw::connect(&hub, "l1", json!({"heartbeatMs": 0, "lifecycleMode": "idle"})).await;
    // 远超 idle_timeout（250 ms）：Hub 不发 ping，也不因"无消息"断开
    let (pings, closed) = raw.observe(Duration::from_millis(900)).await;
    assert_eq!((pings, closed), (0, false));
    let p = power(&hub, "l1");
    assert_eq!((p.heartbeat_ms, p.lifecycle_mode, p.heartbeats), (Some(0), Some(LifecycleMode::Idle), 0));
    raw.close();
}

#[tokio::test(flavor = "multi_thread")]
async fn sdk_heartbeat_only_and_idle_disconnect_scaled() {
    let hub = Hub::start(config()).await.unwrap();
    // 声明 400 ms：无消息断开 = max(250, 3 × 400) = 1200 ms
    let mut raw = Raw::connect(&hub, "s1", json!({"heartbeatMs": 400})).await;
    // SDK 按声明发 ping：超过配置的 250 ms 仍保持连接，Hub 不发 ping，计入心跳次数
    for i in 0..3 {
        raw.send(json!({"jsonrpc": "2.0", "id": 10 + i, "method": "ping"}));
        let (pings, closed) = raw.observe(Duration::from_millis(350)).await;
        assert_eq!((pings, closed), (0, false));
    }
    raw.send(json!({"jsonrpc": "2.0", "id": 20, "method": "ping"}));
    let last = Instant::now();
    eventually("SDK 心跳计数", || power(&hub, "s1").heartbeats == 4).await;
    // 停止心跳：距最后一条消息 600 ms 时仍在（超过配置值、未到 3 个间隔），之后被断开
    let (_, closed) = raw.observe((last + Duration::from_millis(600)).saturating_duration_since(Instant::now())).await;
    assert!(!closed, "未到 3 个心跳间隔不应断开");
    let (pings, closed) = raw.observe(Duration::from_millis(3000)).await;
    assert_eq!((pings, closed), (0, true));
    assert!(last.elapsed() >= Duration::from_millis(1100), "断开时刻应按 3 个心跳间隔：{:?}", last.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn old_sdk_keeps_bidirectional_heartbeat() {
    let hub = Hub::start(config()).await.unwrap();
    let mut raw = Raw::connect(&hub, "o1", json!({})).await;
    // 不回复 ping：Hub 照旧发 ping（计入心跳次数），并在 250 ms 无消息后断开
    let (pings, closed) = raw.observe(Duration::from_millis(150)).await;
    assert!(pings >= 1 && !closed, "旧 SDK 应收到 Hub 的 ping");
    let p = power(&hub, "o1");
    assert_eq!(p.heartbeat_ms, None);
    assert!(p.heartbeats >= 1);
    let (_, closed) = raw.observe(Duration::from_millis(2000)).await;
    assert!(closed, "旧 SDK 无消息应被断开");
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_heartbeat_switch_ignores_declaration() {
    let hub = Hub::start(HubConfig { legacy_heartbeat: true, ..config() }).await.unwrap();
    let mut raw = Raw::connect(&hub, "g1", json!({"heartbeatMs": 0})).await;
    let (pings, closed) = raw.observe(Duration::from_millis(2000)).await;
    assert!(pings >= 1 && closed, "回退开关：照旧发 ping 并按无消息断开（pings={pings}, closed={closed}）");
}

// ---------------------------------------------------------------------------
// O1：观测计数与未能休眠的原因
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn power_counters_and_awake_reasons() {
    let hub = Arc::new(Hub::start(HubConfig { lease_ttl: Duration::from_secs(60), ..config() }).await.unwrap());
    let mut raw = Raw::connect(&hub, "p1", json!({"heartbeatMs": 0, "lifecycleMode": "persistent"})).await;
    eventually("工具可用", || hub.status().apps.iter().any(|a| a.app_id == "raw")).await;
    assert_eq!(power(&hub, "p1").awake_reasons, vec![AwakeReason::Persistent]);

    // 调用进行中 → call；完成后有租约 → lease
    let h = hub.clone();
    let call = tokio::spawn(async move { h.call_tool(CallRequest::new("raw.echo", json!({}))).await });
    let inv = raw.expect(|v| v["method"] == "tools/invoke").await;
    assert!(power(&hub, "p1").awake_reasons.contains(&AwakeReason::Call));
    raw.send(json!({"jsonrpc": "2.0", "id": inv["id"], "result": {"data": "ok"}}));
    assert!(call.await.unwrap().unwrap().result.is_ok());
    assert_eq!(power(&hub, "p1").awake_reasons, vec![AwakeReason::Persistent, AwakeReason::Lease]);

    // 重连：回连次数累计，在线时长保留
    tokio::time::sleep(Duration::from_millis(1100)).await;
    raw.close();
    eventually("实例断开", || app_status(&hub, "raw").is_none_or(|a| a.instances.is_empty())).await;
    let _raw = Raw::connect(&hub, "p1", json!({"heartbeatMs": 0, "lifecycleMode": "idle"})).await;
    eventually("重新连接", || app_status(&hub, "raw").is_some_and(|a| !a.instances.is_empty())).await;
    let p = power(&hub, "p1");
    assert_eq!((p.reconnects, p.lifecycle_mode), (1, Some(LifecycleMode::Idle)));
    assert!(p.online_secs >= 1, "在线时长跨重连保留：{p:?}");
    // /status JSON 带功耗字段（camelCase）
    let v = serde_json::to_value(hub.status()).unwrap();
    let inst = &v["apps"][0]["instances"][0]["power"];
    assert_eq!(inst["reconnects"], 1);
    assert_eq!(inst["heartbeatMs"], 0);
    assert_eq!(v["apps"][0]["wakes"], 0);
}

// ---------------------------------------------------------------------------
// O4：唤醒速率上限
// ---------------------------------------------------------------------------

#[derive(Default)]
struct CountingWaker {
    requests: Mutex<Vec<WakeRequest>>,
}

#[async_trait]
impl Waker for CountingWaker {
    async fn wake(&self, req: WakeRequest) -> Result<(), HubError> {
        self.requests.lock().unwrap().push(req);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn wake_rate_limit_rejects_with_code() {
    let hub = Arc::new(Hub::start(HubConfig { wake_rate_limit: 1, ..config() }).await.unwrap());
    let waker = Arc::new(CountingWaker::default());
    hub.set_waker(waker.clone());

    // 休眠实例（带唤醒描述）
    let mut raw = Raw::connect(&hub, "w1", json!({"heartbeatMs": 0})).await;
    let t: ToolsSyncParams = serde_json::from_value(json!({ "tools": tools() })).unwrap();
    let hash = tools_hash(&t, &ResourcesSyncParams::default());
    raw.send(json!({"jsonrpc": "2.0", "id": 1, "method": "app/sleep",
        "params": {"reason": "idle", "toolsHash": hash, "wake": {"kind": "uri", "target": "raw-app"}}}));
    assert_eq!(raw.expect(|v| v["id"] == 1).await["result"]["accepted"], true);
    raw.close();

    // 第一次：发出激活，App 不回连 → APP_NOT_RESPONDING
    let first = hub.call_tool(CallRequest::new("raw.echo", json!({}))).await.unwrap();
    assert_eq!(first.result.unwrap_err().kind, ErrorKind::AppNotResponding);
    assert_eq!(waker.requests.lock().unwrap().len(), 1);

    // 第二次：一分钟内已达上限 → 不激活，LAUNCH_FAILED + WAKE_RATE_LIMITED
    let second = hub.call_tool(CallRequest::new("raw.echo", json!({}))).await.unwrap();
    let err = second.result.unwrap_err();
    assert_eq!(err.kind, ErrorKind::LaunchFailed);
    let d = err.details.expect("details");
    assert_eq!((d["appId"].as_str(), d["code"].as_str()), (Some("raw"), Some("WAKE_RATE_LIMITED")));
    let retry = d["retryAfterMs"].as_u64().unwrap();
    assert!(retry > 0 && retry <= 60_000, "{retry}");
    assert_eq!(waker.requests.lock().unwrap().len(), 1, "超限时不激活");

    let a = app_status(&hub, "raw").unwrap();
    assert_eq!(a.wakes, 1);
    assert_eq!(a.last_error.and_then(|e| e.code).as_deref(), Some("WAKE_RATE_LIMITED"));
    assert_eq!(power(&hub, "w1").wakes, 1);
}
