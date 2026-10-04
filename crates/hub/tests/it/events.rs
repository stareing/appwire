//! 事件、订阅与信箱（第 16 项 N3 + P4，spec/hub-api.md 3.17）：App 端是真实的 `app-mcp-native` 客户端
//! （`declare_event` / `emit_event`），Agent 端经 Hub API 与 HTTP `/mcp`（已登记 Agent 令牌）。
//!
//! 所有 TCP 监听都绑定端口 0；持久化用临时目录。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{AppEvent, CallRequest, EventHandler, EventLimits, Hub, HubConfig, HubEvent};
use app_mcp_native::{CallHandle, EventInfo, LifecycleMode, NativeClient, NativeConfig, ToolHandler, ToolSpec};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);
const EVENTS_SELF: &str = "app-mcp://apps/events";

fn config() -> HubConfig {
    HubConfig { listen: Some("127.0.0.1:0".into()), listen_alternates: Vec::new(), ipc_endpoint: None, ..Default::default() }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + T;
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "等待超时：{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

struct Echo;
impl ToolHandler for Echo {
    fn invoke(&self, call: CallHandle) {
        let _ = call.complete(Some("{}"), vec![]);
    }
}

fn event_info(name: &str) -> EventInfo {
    EventInfo { name: name.into(), description: format!("{name} 发生时"), payload_schema: None }
}

/// 商城 App：运行时声明 `order.shipped` 与 `cart.changed`（没有清单，经 `events/sync`）。
fn shop(hub: &Hub) -> NativeClient {
    let mut c = NativeConfig::new("shop", "商城");
    c.host_url = format!("ws://{}/app", hub.listen_addr().expect("listen"));
    c.instance_id = Some("shop-1".to_owned());
    c.lifecycle.mode = LifecycleMode::Persistent;
    let client = NativeClient::new(c, None).expect("client");
    client.register_tool(ToolSpec::new("echo", "回显"), Arc::new(Echo)).expect("tool");
    client.declare_event(event_info("order.shipped")).expect("declare");
    client.declare_event(event_info("cart.changed")).expect("declare");
    client.start();
    client
}

async fn call(hub: &Hub, session: Option<&str>, name: &str, args: Value) -> Result<Value, app_mcp_hub::ToolError> {
    let req = CallRequest { session: session.map(str::to_owned), ..CallRequest::new(name, args) };
    hub.call_tool(req).await.expect("名称可解析").result
}

/// 等到 Hub 收到 App 的事件声明（`apps.tools` 的 `events`）。
async fn declared(hub: &Hub) {
    let deadline = tokio::time::Instant::now() + T;
    loop {
        if let Ok(v) = call(hub, None, "apps.tools", json!({ "appId": "shop" })).await
            && v["events"].as_array().is_some_and(|e| e.len() == 2)
        {
            assert_eq!(v["events"][0]["name"], "cart.changed");
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "等待事件声明超时");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn emit(app: &NativeClient, name: &str, payload: Value) {
    assert!(app.emit_event(name, Some(&payload.to_string())).expect("emit"), "已连接时发送");
}

/// 某订阅方各订阅经其入箱的事件总数（`HubStatus.events`）。
fn delivered(hub: &Hub, subscriber: &str) -> u64 {
    hub.status().events.map_or(0, |e| e.subscriptions.iter().filter(|s| s.subscriber == subscriber).map(|s| s.delivered).sum())
}

struct Recorder(Mutex<Vec<AppEvent>>);
impl EventHandler for Recorder {
    fn on_event(&self, event: &AppEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

/// App 声明并发出事件 → 订阅方按 filter 入箱、取件移出、信箱满计 dropped；厂商回调与事件流收到每个事件。
#[tokio::test(flavor = "multi_thread")]
async fn native_events_reach_matching_inboxes() {
    let hub = Hub::start(HubConfig {
        event_limits: EventLimits { max_inbox_events: 2, ..EventLimits::default() },
        ..config()
    })
    .await
    .expect("hub");
    let rec = Arc::new(Recorder(Mutex::new(Vec::new())));
    hub.set_event_handler(rec.clone());
    let mut rx = hub.events();
    let app = shop(&hub);
    declared(&hub).await;

    let a = call(&hub, Some("a"), "apps.events.subscribe", json!({ "appId": "shop", "event": "order.shipped", "filter": { "carrier": "sf" } }))
        .await
        .expect("订阅");
    assert!(a["subscriptionId"].as_str().unwrap().starts_with("sub-"));
    call(&hub, Some("b"), "apps.events.subscribe", json!({ "appId": "shop" })).await.expect("订阅");
    let err = call(&hub, Some("a"), "apps.events.subscribe", json!({ "appId": "shop", "event": "nope" })).await.unwrap_err();
    assert_eq!(err.kind, app_mcp_hub::ErrorKind::InvalidInput);

    emit(&app, "order.shipped", json!({ "carrier": "sf", "id": 1 }));
    emit(&app, "order.shipped", json!({ "carrier": "ems", "id": 2 }));
    emit(&app, "cart.changed", json!({ "count": 3 }));
    eventually("b 的订阅收到 3 个事件", || delivered(&hub, "api:b") == 3).await;
    eventually("a 的订阅收到 1 个事件", || delivered(&hub, "api:a") == 1).await;

    let got = call(&hub, Some("a"), "apps.events", json!({})).await.unwrap();
    let events = got["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!((events[0]["name"].clone(), events[0]["payload"]["id"].clone()), (json!("order.shipped"), json!(1)));
    assert_eq!((events[0]["appId"].clone(), events[0]["instanceId"].clone()), (json!("shop"), json!("shop-1")));
    assert!(call(&hub, Some("a"), "apps.events", json!({})).await.unwrap()["events"].as_array().unwrap().is_empty(), "取出即移出");

    // b：信箱上限 2，最旧的被丢弃并计数；取件后清零。
    let got = call(&hub, Some("b"), "apps.events", json!({})).await.unwrap();
    let ids: Vec<i64> = got["events"].as_array().unwrap().iter().map(|e| e["payload"]["id"].as_i64().unwrap_or(-1)).collect();
    assert_eq!(ids, [2, -1], "保留后两个（第二个是 cart.changed，载荷无 id）");
    assert_eq!(got["dropped"], 1);
    assert_eq!(got["subscriptions"].as_array().unwrap().len(), 1);
    assert_eq!(call(&hub, Some("b"), "apps.events", json!({})).await.unwrap()["dropped"], 0);

    // 厂商回调与事件流：每个通过校验的事件一次（不论有无订阅）。
    eventually("回调收到 3 个事件", || rec.0.lock().unwrap().len() == 3).await;
    let mut streamed = 0;
    while let Ok(e) = rx.try_recv() {
        if matches!(e, HubEvent::AppEvent(ref ev) if ev.app_id == "shop") {
            streamed += 1;
        }
    }
    assert_eq!(streamed, 3);
    let status = hub.status().events.expect("events 状态");
    assert_eq!((status.subscriptions.len(), status.dropped_invalid), (2, 0));

    // 退订后不再入箱；他人退订 → TOOL_NOT_FOUND。
    let id = a["subscriptionId"].as_str().unwrap();
    let err = call(&hub, Some("b"), "apps.events.unsubscribe", json!({ "subscriptionId": id })).await.unwrap_err();
    assert_eq!(err.kind, app_mcp_hub::ErrorKind::ToolNotFound);
    call(&hub, Some("a"), "apps.events.unsubscribe", json!({ "subscriptionId": id })).await.expect("退订");
    // 会话结束：匿名订阅方的订阅与信箱一并回收。
    hub.reset_session(Some("b"));
    assert!(hub.status().events.unwrap().subscriptions.is_empty());
    app.stop();
    hub.shutdown().await;
}

/// `app-mcp://apps/events`：读取不移出；订阅了该 URI 的订阅方只在**自己的**信箱有新事件时收到提醒。
#[tokio::test(flavor = "multi_thread")]
async fn events_self_resource_reminds_only_its_owner() {
    let hub = Hub::start(config()).await.expect("hub");
    let app = shop(&hub);
    declared(&hub).await;
    call(&hub, None, "apps.events.subscribe", json!({ "appId": "shop", "event": "order.shipped" })).await.unwrap();
    call(&hub, Some("other"), "apps.events.subscribe", json!({ "appId": "shop", "event": "cart.changed" })).await.unwrap();
    hub.subscribe(EVENTS_SELF).expect("订阅 events/self");
    let mut rx = hub.events();
    let reminders = |rx: &mut tokio::sync::broadcast::Receiver<HubEvent>| {
        std::iter::from_fn(|| rx.try_recv().ok()).filter(|e| matches!(e, HubEvent::ResourceUpdated { uri } if uri == EVENTS_SELF)).count()
    };

    emit(&app, "cart.changed", json!({}));
    eventually("other 收到事件", || delivered(&hub, "api:other") == 1).await;
    assert_eq!(reminders(&mut rx), 0, "他人信箱的新事件不提醒");
    emit(&app, "order.shipped", json!({ "id": 7 }));
    eventually("自己的订阅收到事件", || delivered(&hub, "api") == 1).await;
    assert_eq!(reminders(&mut rx), 1);

    let read = || async {
        let c = hub.read_resource(EVENTS_SELF).await.expect("读取");
        serde_json::from_str::<Value>(c.text.as_deref().unwrap()).unwrap()
    };
    let v = read().await;
    assert_eq!((v["pending"].clone(), v["events"][0]["payload"]["id"].clone()), (json!(1), json!(7)));
    assert_eq!(v["subscriptions"].as_array().unwrap().len(), 1);
    assert_eq!(read().await["pending"], 1, "读取不移出");
    let me = hub.read_resource("app-mcp://apps/self").await.unwrap();
    let me: Value = serde_json::from_str(me.text.as_deref().unwrap()).unwrap();
    assert_eq!(me["events"], json!({ "pending": 1, "subscriptions": 1 }));
    app.stop();
    hub.shutdown().await;
}

#[cfg(feature = "mcp-server")]
use crate::support::mcp_http;

#[cfg(feature = "mcp-server")]
mod agents {
    use super::*;

    use std::path::PathBuf;

    use app_mcp_hub::{AgentCredential, AgentsConfig, HttpOptions};

    use super::mcp_http::modern_call;

    const LOCAL: &str = "local-0123456789abcdef0123456789abcdef";
    const CLAUDE: &str = "claude-0123456789abcdef0123456789abcdef";

    fn temp_dir(tag: &str) -> PathBuf {
        let n: u64 = rand::random();
        std::env::temp_dir().join(format!("app-mcp-events-it-{tag}-{}-{n:x}", std::process::id()))
    }

    async fn start(state_dir: &std::path::Path) -> Hub {
        Hub::start(HubConfig {
            mcp_http: true,
            http: HttpOptions { token: Some(LOCAL.into()), ..Default::default() },
            agents: AgentsConfig { agents: vec![AgentCredential { name: "claude".into(), token: CLAUDE.into() }] },
            state_dir: Some(state_dir.to_path_buf()),
            task_idle_ttl: Duration::from_millis(300),
            ..config()
        })
        .await
        .expect("hub")
    }

    async fn tool(hub: &Hub, token: &str, name: &str, args: Value) -> Value {
        let r = modern_call(hub.listen_addr().unwrap(), Some(token), name, args).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let v = r.json();
        assert_ne!(v["result"]["isError"], true, "{v}");
        v["result"]["structuredContent"].clone()
    }

    /// 已登记 Agent 的订阅与信箱：任务空闲回收（会话结束）后仍在、可取件；Hub 带 `state_dir` 重启后仍在。
    /// 本机主体（匿名）的订阅随任务回收、不持久化。
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_inbox_survives_task_end_and_restart() {
        let dir = temp_dir("restart");
        let hub = start(&dir).await;
        let app = shop(&hub);
        declared(&hub).await;
        tool(&hub, CLAUDE, "apps.events.subscribe", json!({ "appId": "shop" })).await;
        tool(&hub, LOCAL, "apps.events.subscribe", json!({ "appId": "shop" })).await;
        eventually("两个订阅", || hub.status().events.unwrap().subscriptions.len() == 2).await;
        // 任务空闲回收：本机主体的订阅一并删除，Agent 的保留。
        eventually("任务空闲回收", || hub.status().tasks.unwrap_or_default().iter().all(|t| !t.caller.starts_with("principal:"))).await;
        let subs = hub.status().events.unwrap().subscriptions;
        assert_eq!(subs.iter().map(|s| s.subscriber.as_str()).collect::<Vec<_>>(), ["agent:claude"]);

        emit(&app, "order.shipped", json!({ "id": 1 }));
        emit(&app, "cart.changed", json!({ "id": 2 }));
        eventually("Agent 信箱收到 2 个", || delivered(&hub, "agent:claude") == 2).await;
        let got = tool(&hub, CLAUDE, "apps.events", json!({ "max": 1 })).await;
        assert_eq!((got["events"][0]["payload"]["id"].clone(), got["pending"].clone()), (json!(1), json!(1)));
        app.stop();
        hub.shutdown().await;

        let hub = start(&dir).await;
        let got = tool(&hub, CLAUDE, "apps.events", json!({})).await;
        assert_eq!(got["events"].as_array().unwrap().len(), 1, "重启后信箱仍在：{got}");
        assert_eq!(got["events"][0]["payload"]["id"], 2);
        assert_eq!(got["subscriptions"].as_array().unwrap().len(), 1, "订阅仍在");
        let local = tool(&hub, LOCAL, "apps.events", json!({})).await;
        assert!(local["subscriptions"].as_array().unwrap().is_empty(), "本机主体不持久化");
        // 重启后的新事件照常入箱（订阅随 Agent 保留）。
        let app = shop(&hub);
        declared(&hub).await;
        emit(&app, "cart.changed", json!({ "id": 3 }));
        eventually("重启后的新事件入箱", || delivered(&hub, "agent:claude") >= 1).await;
        let got = tool(&hub, CLAUDE, "apps.events", json!({})).await;
        assert_eq!(got["events"][0]["payload"]["id"], 3);
        assert!(got["events"][0]["id"].as_str().unwrap() != "ev-1", "序号在读回的之后继续");
        app.stop();
        hub.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
