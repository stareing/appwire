//! Hub 侧生命周期集成测试（spec/lifecycle.md §9）：休眠、快速恢复、唤醒、租约。
//!
//! 两类 App 端：真实的 `app-mcp-native` 客户端（idle 模式、短空闲时间），以及手写的 WebSocket
//! 客户端（精确控制 `app/sleep`、`app/hello` 与挂起的调用）。唤醒用进程内的假 [`Waker`]。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{
    Availability, CallRequest, ErrorKind, Hub, HubConfig, HubError, HubEvent, ToolFilter, WakeKind,
    WakeRequest, Waker, async_trait,
};
use app_mcp_native::{
    CallHandle, LifecycleMode, NativeClient, NativeConfig, StateStatus, ToolHandler, ToolSpec,
    WakeDescriptor as NativeWake, WakeKind as NativeWakeKind,
};
use app_mcp_protocol::{ResourcesSyncParams, ToolsSyncParams, tools_hash};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as Ws;

const T: Duration = Duration::from_secs(10);

fn config(lease_ms: u64) -> HubConfig {
    HubConfig {
        ws_addr: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        list_changed_debounce: Duration::from_millis(10),
        lease_ttl: Duration::from_millis(lease_ms),
        wake_timeout: Duration::from_secs(5),
        ..Default::default()
    }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + T;
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "等待超时：{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn next_event(rx: &mut broadcast::Receiver<HubEvent>, pred: impl Fn(&HubEvent) -> bool) -> HubEvent {
    timeout(T, async {
        loop {
            let ev = rx.recv().await.expect("event channel");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
    .expect("没有等到事件")
}

fn availability(hub: &Hub, name: &str) -> Option<Availability> {
    hub.tools(&ToolFilter::default())
        .into_iter()
        .find(|t| t.name == name)
        .map(|t| t.availability)
}

// ---------------------------------------------------------------------------
// 真实 native 客户端
// ---------------------------------------------------------------------------

struct Add;
impl ToolHandler for Add {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or(Value::Null);
        let sum = args["a"].as_i64().unwrap_or(0) + args["b"].as_i64().unwrap_or(0);
        std::thread::spawn(move || {
            let _ = call.complete(Some(&json!({ "sum": sum }).to_string()), vec![]);
        });
    }
}

fn native_client(hub: &Hub, instance_id: &str, idle_ms: u64) -> NativeClient {
    let mut c = NativeConfig::new("calc", "计算器");
    c.host_url = format!("ws://{}", hub.ws_addr().unwrap());
    c.instance_id = Some(instance_id.to_owned());
    c.launch_token = Some(String::new());
    c.lifecycle.mode = LifecycleMode::Idle;
    c.lifecycle.idle_timeout_ms = idle_ms;
    c.lifecycle.wake = Some(NativeWake {
        kind: NativeWakeKind::Uri,
        target: Some("calc-app".into()),
        background: true,
    });
    let client = NativeClient::new(c, None).unwrap();
    let mut spec = ToolSpec::new("math.add", "加法");
    spec.input_schema_json = Some(r#"{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}}}"#.into());
    client.register_tool(spec, Arc::new(Add)).unwrap();
    client
}

/// 假 Waker：记录请求；`client` 存在时把激活参数交给它的 `handle_wake`（模拟 OS 激活）。
#[derive(Default)]
struct FakeWaker {
    requests: Mutex<Vec<WakeRequest>>,
    client: Mutex<Option<NativeClient>>,
    fail: bool,
}

#[async_trait]
impl Waker for FakeWaker {
    async fn wake(&self, req: WakeRequest) -> Result<(), HubError> {
        self.requests.lock().unwrap().push(req.clone());
        if self.fail {
            return Err(HubError::new(ErrorKind::LaunchFailed, "测试：激活失败"));
        }
        if let Some(c) = self.client.lock().unwrap().clone() {
            assert!(c.handle_wake(&req.activation_arg));
        }
        Ok(())
    }
}

async fn wait_status(client: &NativeClient, status: StateStatus) {
    let c = client.clone();
    eventually(&format!("客户端进入 {status:?}"), move || c.state().status == status).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn native_sleep_wake_fast_resume_and_changed_tools() {
    let hub = Hub::start(config(0)).await.unwrap();
    let waker = Arc::new(FakeWaker::default());
    hub.set_waker(waker.clone());
    let mut rx = hub.events();

    let client = native_client(&hub, "calc-1", 150);
    *waker.client.lock().unwrap() = Some(client.clone());
    client.start();

    // 空闲后休眠被接受：工具仍列出且标记 Dormant，apps 显示休眠实例
    let ev = next_event(&mut rx, |e| matches!(e, HubEvent::AppDormant { .. })).await;
    assert_eq!(ev, HubEvent::AppDormant { app_id: "calc".into(), instance_id: "calc-1".into() });
    wait_status(&client, StateStatus::Dormant).await;
    assert_eq!(availability(&hub, "calc.math.add"), Some(Availability::Dormant));
    let app = hub.apps().into_iter().find(|a| a.app_id == "calc").unwrap();
    assert!(!app.connected);
    assert!(app.instances.is_empty());
    assert_eq!(app.dormant_instances.len(), 1);
    assert_eq!(app.dormant_instances[0].instance_id, "calc-1");

    // 调用触发唤醒 → 快速恢复 → 结果正确
    let out = hub
        .call_tool(CallRequest::new("calc.math.add", json!({"a": 20, "b": 22})))
        .await
        .unwrap();
    assert_eq!(out.result.unwrap()["sum"], 42);
    assert_eq!(out.instance_id.as_deref(), Some("calc-1"));
    let reqs = waker.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].instance_id.as_deref(), Some("calc-1"));
    assert_eq!(reqs[0].descriptor.kind, WakeKind::Uri);
    assert_eq!(reqs[0].descriptor.target.as_deref(), Some("calc-app"));
    assert_eq!(reqs[0].token.len(), 32);
    assert!(matches!(
        next_event(&mut rx, |e| matches!(e, HubEvent::AppWaking { .. })).await,
        HubEvent::AppWaking { instance_id: Some(_), .. }
    ));
    // 回连后又会空闲休眠
    next_event(&mut rx, |e| matches!(e, HubEvent::AppDormant { .. })).await;
    wait_status(&client, StateStatus::Dormant).await;

    // 休眠期间注册新工具（不唤醒）→ 回连时摘要不一致 → 完整同步，新工具可用
    let mut spec = ToolSpec::new("math.neg", "取负");
    spec.input_schema_json = None;
    client.register_tool(spec, Arc::new(Add)).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(client.state().status, StateStatus::Dormant);
    assert_eq!(availability(&hub, "calc.math.neg"), None);
    let out = hub
        .call_tool(CallRequest::new("calc.math.add", json!({"a": 1, "b": 2})))
        .await
        .unwrap();
    assert_eq!(out.result.unwrap()["sum"], 3);
    eventually("新工具出现", || availability(&hub, "calc.math.neg") == Some(Availability::Available)).await;
    assert_eq!(waker.requests.lock().unwrap().len(), 2);

    client.stop();
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lease_keeps_client_awake_until_expiry() {
    let hub = Hub::start(config(1200)).await.unwrap();
    let mut rx = hub.events();
    let client = native_client(&hub, "calc-lease", 150);
    client.start();
    eventually("工具可用", || availability(&hub, "calc.math.add") == Some(Availability::Available)).await;

    let out = hub
        .call_tool(CallRequest::new("calc.math.add", json!({"a": 1, "b": 1})))
        .await
        .unwrap();
    assert_eq!(out.result.unwrap()["sum"], 2);
    let done = tokio::time::Instant::now();
    // 租约 1.2s 内不休眠（空闲时间只有 150ms）
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(client.state().status, StateStatus::Connected);
    assert_eq!(availability(&hub, "calc.math.add"), Some(Availability::Available));
    // 租约到期后休眠
    next_event(&mut rx, |e| matches!(e, HubEvent::AppDormant { .. })).await;
    assert!(done.elapsed() >= Duration::from_millis(1100), "{:?}", done.elapsed());

    client.stop();
    hub.shutdown().await;
}

// ---------------------------------------------------------------------------
// 手写客户端
// ---------------------------------------------------------------------------

fn raw_tools() -> Value {
    json!([{"name": "echo", "description": "回显", "inputSchema": {"type": "object"}, "risk": "read"}])
}

fn raw_hash(tools: &Value) -> String {
    let t: ToolsSyncParams = serde_json::from_value(json!({ "tools": tools })).unwrap();
    tools_hash(&t, &ResourcesSyncParams::default())
}

struct Raw {
    out: mpsc::UnboundedSender<Option<String>>,
    inbox: mpsc::UnboundedReceiver<Value>,
    hello: Value,
    next_id: i64,
}

impl Raw {
    /// 连接并握手；`paired` 且 `sync` 时发送 tools/sync（未 toolsCurrent 时）、visibility、ready。
    async fn connect(hub: &Hub, instance_id: &str, extra: Value, tools: Value) -> Raw {
        let url = format!("ws://{}", hub.ws_addr().unwrap());
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
        sink.send(Ws::text(json!({"jsonrpc": "2.0", "id": 0, "method": "app/hello", "params": hello}).to_string()))
            .await
            .unwrap();
        let hello_result = loop {
            let Some(Ok(Ws::Text(t))) = timeout(T, stream.next()).await.unwrap() else { panic!("握手时断开") };
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["id"] == 0 {
                break v["result"].clone();
            }
        };
        let note = |m: &str, p: Value| json!({"jsonrpc": "2.0", "method": m, "params": p}).to_string();
        if hello_result["toolsCurrent"] != true {
            sink.send(Ws::text(note("tools/sync", json!({ "tools": tools })))).await.unwrap();
            sink.send(Ws::text(note("resources/sync", json!({ "resources": [] })))).await.unwrap();
        }
        sink.send(Ws::text(note("app/visibility", json!({"visibility": "visible", "focused": false})))).await.unwrap();
        sink.send(Ws::text(note("app/ready", json!({})))).await.unwrap();

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
                        let Some(Ok(Ws::Text(t))) = m else { break };
                        let v: Value = serde_json::from_str(t.as_str()).unwrap();
                        if v["method"] == "ping" {
                            let _ = sink.send(Ws::text(json!({"jsonrpc": "2.0", "id": v["id"], "result": {}}).to_string())).await;
                            continue;
                        }
                        let _ = in_tx.send(v);
                    }
                }
            }
        });
        Raw { out: out_tx, inbox: in_rx, hello: hello_result, next_id: 100 }
    }

    fn send(&self, v: Value) {
        let _ = self.out.send(Some(v.to_string()));
    }

    fn close(&self) {
        let _ = self.out.send(None);
    }

    async fn expect(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        timeout(T, async {
            loop {
                let v = self.inbox.recv().await.expect("连接已关闭");
                if pred(&v) {
                    return v;
                }
            }
        })
        .await
        .expect("没有等到消息")
    }

    async fn sleep(&mut self, tools_hash: &str) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": "app/sleep",
            "params": {"reason": "idle", "toolsHash": tools_hash}}));
        self.expect(|v| v["id"] == id).await["result"].clone()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sleep_rejected_while_call_pending_then_fast_resume() {
    let hub = Arc::new(Hub::start(config(0)).await.unwrap());
    let tools = raw_tools();
    let hash = raw_hash(&tools);
    let mut raw = Raw::connect(&hub, "r1", json!({}), tools.clone()).await;
    assert_eq!(raw.hello["status"], "paired");
    assert!(raw.hello.get("toolsCurrent").is_none());
    eventually("工具可用", || availability(&hub, "raw.echo") == Some(Availability::Available)).await;

    // 挂起一个调用：此时 app/sleep 被拒绝并给出 retryAfterMs
    let h = hub.clone();
    let call = tokio::spawn(async move { h.call_tool(CallRequest::new("raw.echo", json!({"x": 1}))).await });
    let inv = raw.expect(|v| v["method"] == "tools/invoke").await;
    let r = raw.sleep(&hash).await;
    assert_eq!(r["accepted"], false);
    assert!(r["retryAfterMs"].as_u64().unwrap() > 0);
    raw.send(json!({"jsonrpc": "2.0", "id": inv["id"], "result": {"data": {"ok": 1}}}));
    assert_eq!(call.await.unwrap().unwrap().result.unwrap(), json!({"ok": 1}));

    // 空闲：接受
    let r = raw.sleep(&hash).await;
    assert_eq!(r["accepted"], true);
    let resume = r["resumeToken"].as_str().unwrap().to_owned();
    assert!(resume.len() >= 32);
    raw.close();
    assert_eq!(availability(&hub, "raw.echo"), Some(Availability::Dormant));

    // 令牌正确、摘要一致 → toolsCurrent，不同步也能调用
    let mut raw = Raw::connect(&hub, "r1", json!({"resumeToken": resume, "toolsHash": hash}), tools.clone()).await;
    assert_eq!(raw.hello["toolsCurrent"], true);
    eventually("工具恢复可用", || availability(&hub, "raw.echo") == Some(Availability::Available)).await;
    let h = hub.clone();
    let call = tokio::spawn(async move { h.call_tool(CallRequest::new("raw.echo", json!({}))).await });
    let inv = raw.expect(|v| v["method"] == "tools/invoke").await;
    raw.send(json!({"jsonrpc": "2.0", "id": inv["id"], "result": {"data": "ok"}}));
    assert_eq!(call.await.unwrap().unwrap().result.unwrap(), json!("ok"));

    // 再休眠；这次回连带错误的摘要 → toolsCurrent=false，完整同步
    let r = raw.sleep(&hash).await;
    let resume = r["resumeToken"].as_str().unwrap().to_owned();
    raw.close();
    let raw = Raw::connect(&hub, "r1", json!({"resumeToken": resume, "toolsHash": "0000000000000000"}), tools.clone()).await;
    assert!(raw.hello.get("toolsCurrent").is_none());
    // 恢复令牌只能用一次
    raw.close();
    eventually("实例断开", || availability(&hub, "raw.echo") != Some(Availability::Available)).await;
    let raw = Raw::connect(&hub, "r1", json!({"resumeToken": resume, "toolsHash": hash}), tools).await;
    assert!(raw.hello.get("toolsCurrent").is_none());
    raw.close();
}

#[tokio::test(flavor = "multi_thread")]
async fn wake_timeout_and_launch_failure() {
    let mut cfg = config(0);
    cfg.wake_timeout = Duration::from_millis(300);
    let hub = Hub::start(cfg).await.unwrap();
    let waker = Arc::new(FakeWaker::default());
    hub.set_waker(waker.clone());
    let tools = raw_tools();
    let mut raw = Raw::connect(&hub, "r2", json!({}), tools.clone()).await;
    eventually("工具可用", || availability(&hub, "raw.echo") == Some(Availability::Available)).await;
    raw.send(json!({"jsonrpc": "2.0", "id": 1, "method": "app/sleep",
        "params": {"reason": "idle", "toolsHash": raw_hash(&tools), "wake": {"kind": "uri", "target": "raw-app"}}}));
    assert_eq!(raw.expect(|v| v["id"] == 1).await["result"]["accepted"], true);
    raw.close();

    // 唤醒后没有回连 → APP_NOT_RESPONDING
    let out = hub.call_tool(CallRequest::new("raw.echo", json!({}))).await.unwrap();
    assert_eq!(out.result.unwrap_err().kind, ErrorKind::AppNotResponding);
    assert_eq!(waker.requests.lock().unwrap().len(), 1);
    // 参数不合法：唤醒前就拒绝，不激活
    let out = hub.call_tool(CallRequest::new("raw.echo", json!("x"))).await;
    assert!(out.is_ok());
    assert_eq!(waker.requests.lock().unwrap().len(), 1);

    // Waker 报错 → 错误透传（LAUNCH_FAILED）
    let failing = Arc::new(FakeWaker { fail: true, ..Default::default() });
    hub.set_waker(failing.clone());
    let out = hub.call_tool(CallRequest::new("raw.echo", json!({}))).await.unwrap();
    let e = out.result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::LaunchFailed);
    assert!(e.message.contains("测试：激活失败"), "{}", e.message);

    // App 以新实例 ID 连接 → 旧休眠记录被移除
    let _raw = Raw::connect(&hub, "r3", json!({}), tools).await;
    eventually("休眠记录移除", || {
        hub.apps().iter().find(|a| a.app_id == "raw").is_some_and(|a| a.dormant_instances.is_empty() && a.connected)
    })
    .await;
}

/// 冷启动：App 未运行、清单显式声明 `wake` → 唤醒；带令牌的新实例回连后派发。
#[tokio::test(flavor = "multi_thread")]
async fn cold_wake_from_manifest_with_launch_token() {
    let manifest = app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": "raw", "name": "Raw",
            "wake": {"web": [{"kind": "web-url", "target": "http://localhost:5173/"}]},
            "tools": [{"name": "echo", "description": "回显", "inputSchema": {"type": "object"}}]
        })
        .to_string(),
    )
    .unwrap();
    let mut cfg = config(0);
    cfg.manifests = vec![manifest];
    let hub = Arc::new(Hub::start(cfg).await.unwrap());
    assert_eq!(availability(&hub, "raw.echo"), Some(Availability::Disconnected));

    struct Spawner {
        hub: Mutex<Option<Arc<Hub>>>,
        seen: Mutex<Vec<WakeRequest>>,
    }
    #[async_trait]
    impl Waker for Spawner {
        async fn wake(&self, req: WakeRequest) -> Result<(), HubError> {
            self.seen.lock().unwrap().push(req.clone());
            let hub = self.hub.lock().unwrap().clone().unwrap();
            tokio::spawn(async move {
                let mut raw = Raw::connect(&hub, "cold-1", json!({"launchToken": req.token, "wakeReason": "os-activation"}), raw_tools()).await;
                let inv = raw.expect(|v| v["method"] == "tools/invoke").await;
                raw.send(json!({"jsonrpc": "2.0", "id": inv["id"], "result": {"data": {"cold": true}}}));
                tokio::time::sleep(Duration::from_secs(1)).await;
            });
            Ok(())
        }
    }
    let spawner = Arc::new(Spawner { hub: Mutex::new(Some(hub.clone())), seen: Mutex::new(Vec::new()) });
    hub.set_waker(spawner.clone());
    let out = hub.call_tool(CallRequest::new("raw.echo", json!({}))).await.unwrap();
    assert_eq!(out.result.unwrap(), json!({"cold": true}));
    assert_eq!(out.instance_id.as_deref(), Some("cold-1"));
    let seen = spawner.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].instance_id, None);
    assert_eq!(seen[0].descriptor.kind, WakeKind::WebUrl);
    *spawner.hub.lock().unwrap() = None;
}

/// MCP 出口：调用后发送租约，会话关闭时 `ttlMs: 0`。
#[tokio::test(flavor = "multi_thread")]
async fn lease_sent_after_call_and_cancelled_when_session_closes() {
    use rmcp::ServiceExt;
    let hub = Arc::new(Hub::start(config(60_000)).await.unwrap());
    let mut raw = Raw::connect(&hub, "r4", json!({}), raw_tools()).await;
    eventually("工具可用", || availability(&hub, "raw.echo") == Some(Availability::Available)).await;

    let (a, b) = tokio::io::duplex(64 * 1024);
    let server = hub.mcp_session();
    let srv = tokio::spawn(async move {
        let s = server.serve(a).await.unwrap();
        let _ = s.waiting().await;
    });
    let client = ().serve(b).await.unwrap();
    let peer = client.peer().clone();
    let call = tokio::spawn(async move {
        peer.call_tool(rmcp::model::CallToolRequestParams::new("raw.echo")).await
    });
    let inv = raw.expect(|v| v["method"] == "tools/invoke").await;
    raw.send(json!({"jsonrpc": "2.0", "id": inv["id"], "result": {"data": 1}}));
    call.await.unwrap().unwrap();
    let lease = raw.expect(|v| v["method"] == "app/lease").await;
    assert_eq!(lease["params"]["ttlMs"], 60_000);

    client.cancel().await.unwrap();
    let _ = srv.await;
    let lease = raw.expect(|v| v["method"] == "app/lease").await;
    assert_eq!(lease["params"]["ttlMs"], 0);

    // API 会话：reset_session 同样取消
    let h = hub.clone();
    let call = tokio::spawn(async move {
        let mut req = CallRequest::new("raw.echo", json!({}));
        req.session = Some("s1".into());
        h.call_tool(req).await
    });
    let inv = raw.expect(|v| v["method"] == "tools/invoke").await;
    raw.send(json!({"jsonrpc": "2.0", "id": inv["id"], "result": {"data": 1}}));
    call.await.unwrap().unwrap();
    assert_eq!(raw.expect(|v| v["method"] == "app/lease").await["params"]["ttlMs"], 60_000);
    hub.reset_session(Some("s1"));
    assert_eq!(raw.expect(|v| v["method"] == "app/lease").await["params"]["ttlMs"], 0);
}

/// 回归：静态清单工具 + App 未连接（无显式 wake）→ 立即返回 APP_DISCONNECTED（曾因注册表锁重入死锁）。
#[tokio::test(flavor = "multi_thread")]
async fn static_tool_without_wake_returns_disconnected() {
    let manifest = app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": "shop", "name": "商城",
            "launch": {"web": [{"type": "url", "href": "http://localhost:5173/"}]},
            "tools": [{"name": "orders.search", "description": "搜索", "inputSchema": {"type": "object"}}]
        })
        .to_string(),
    )
    .unwrap();
    let mut cfg = config(0);
    cfg.manifests = vec![manifest];
    let hub = Hub::start(cfg).await.unwrap();
    let waker = Arc::new(FakeWaker::default());
    hub.set_waker(waker.clone());
    let out = timeout(Duration::from_secs(3), hub.call_tool(CallRequest::new("shop.orders.search", json!({}))))
        .await
        .expect("调用未在超时内返回")
        .unwrap();
    let e = out.result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::AppDisconnected);
    assert_eq!(e.details.unwrap()["launchUrl"], "http://localhost:5173/");
    // 默认不从 launch 推导唤醒方式
    assert!(waker.requests.lock().unwrap().is_empty());
}

/// `waker: none`：休眠实例与未运行 App 的调用都直接返回 APP_DISCONNECTED（带 launchUrl），不尝试唤醒。
#[tokio::test(flavor = "multi_thread")]
async fn waker_none_returns_disconnected_without_waking() {
    let manifest = app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": "calc", "name": "计算器",
            "launch": {"web": [{"type": "url", "href": "http://localhost:5173/"}]},
            "wake": {"web": [{"kind": "web-url", "target": "http://localhost:5173/"}]},
            "tools": [{"name": "math.add", "description": "加法", "inputSchema": {"type": "object"}}]
        })
        .to_string(),
    )
    .unwrap();
    let mut cfg = config(0);
    cfg.manifests = vec![manifest];
    cfg.waker = app_mcp_hub::WakerConfig::None;
    let hub = Hub::start(cfg).await.unwrap();
    let mut rx = hub.events();

    // 未运行（清单声明了 wake）：不唤醒
    let out = timeout(Duration::from_secs(3), hub.call_tool(CallRequest::new("calc.math.add", json!({}))))
        .await
        .expect("调用未在超时内返回")
        .unwrap();
    let e = out.result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::AppDisconnected);
    assert_eq!(e.details.unwrap()["launchUrl"], "http://localhost:5173/");

    // 休眠实例：同样不唤醒，工具仍列为 Dormant
    let client = native_client(&hub, "calc-1", 150);
    client.start();
    next_event(&mut rx, |e| matches!(e, HubEvent::AppDormant { .. })).await;
    assert_eq!(availability(&hub, "calc.math.add"), Some(Availability::Dormant));
    let out = timeout(Duration::from_secs(3), hub.call_tool(CallRequest::new("calc.math.add", json!({"a": 1, "b": 2}))))
        .await
        .expect("调用未在超时内返回")
        .unwrap();
    let e = out.result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::AppDisconnected);
    assert_eq!(e.details.unwrap()["launchUrl"], "http://localhost:5173/");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(client.state().status, StateStatus::Dormant);
    while let Ok(ev) = rx.try_recv() {
        assert!(!matches!(ev, HubEvent::AppWaking { .. }), "不应发出唤醒：{ev:?}");
    }
    client.stop();
    hub.shutdown().await;
}

/// `waker: {"exec": [...]}` 经 Hub 配置生效：WakeRequest 以 JSON 写入程序 stdin，程序失败时返回 LAUNCH_FAILED。
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn exec_waker_from_config() {
    let dir = std::env::temp_dir().join(format!("app-mcp-exec-waker-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("wake.jsonl");
    let manifest = app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": "raw", "name": "Raw",
            "wake": {"web": [{"kind": "web-url", "target": "http://localhost:5173/"}]},
            "tools": [{"name": "echo", "description": "回显", "inputSchema": {"type": "object"}}]
        })
        .to_string(),
    )
    .unwrap();
    let mut cfg = config(0);
    cfg.manifests = vec![manifest];
    cfg.wake_timeout = Duration::from_millis(500);
    // 测试程序：把 stdin 追加到文件，然后以退出码 3 失败（stderr 应出现在错误信息中）
    cfg.waker = app_mcp_hub::WakerConfig::Exec(vec![
        "sh".into(),
        "-c".into(),
        r#"cat >> "$0"; echo 激活失败 >&2; exit 3"#.into(),
        log.to_string_lossy().into_owned(),
    ]);
    let hub = Hub::start(cfg).await.unwrap();
    let out = hub.call_tool(CallRequest::new("raw.echo", json!({}))).await.unwrap();
    let e = out.result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::LaunchFailed, "{e:?}");
    assert!(e.message.contains("激活失败"), "{}", e.message);
    let line = std::fs::read_to_string(&log).unwrap();
    let req: WakeRequest = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(req.app_id, "raw");
    assert_eq!(req.instance_id, None);
    assert_eq!(req.descriptor.kind, WakeKind::WebUrl);
    assert_eq!(req.activation_arg, format!("app-mcp-wake:{}", req.token));
    hub.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);

    // 空 exec 在启动时报错
    let mut cfg = config(0);
    cfg.waker = app_mcp_hub::WakerConfig::Exec(vec![]);
    assert!(Hub::start(cfg).await.is_err());
}
