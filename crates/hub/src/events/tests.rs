//! Hub 层的事件规则（spec/hub-api.md 3.17）：校验与去重、订阅错误、归属隔离、回收、策略、提醒、持久化与厂商回调。
//! 纯状态的规则（filter、频率上限、信箱满、TTL、只入箱一次）见 `inbox.rs` / `catalog.rs` / `store.rs` 的测试。

use std::sync::{Arc, Mutex};

use app_mcp_protocol::{ErrorKind, EventEmitParams, EventInfo, MAX_EVENT_PAYLOAD_BYTES};
use rmcp::model::{CallToolResult, ResourceContents};
use serde_json::{Value, json};

use crate::hub::{API_SUBSCRIBER, HubConfig, HubShared};
use crate::names::RESOURCE_APPS_EVENTS_URI;
use crate::policy::PolicyConfig;
use crate::task::CallerKey;
use crate::types::HubEvent;

use super::{AppEvent, EventHandler, EventLimits};

const CONN: u64 = 7;

fn manifest() -> app_mcp_manifest::Manifest {
    app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": "shop", "name": "商城",
            "events": [
                { "name": "order.shipped", "description": "订单已发货" },
                { "name": "cart.changed", "description": "购物车变化" }
            ]
        })
        .to_string(),
    )
    .unwrap()
}

fn shared_with(f: impl FnOnce(&mut HubConfig)) -> Arc<HubShared> {
    let mut config = HubConfig { listen: None, ipc_endpoint: None, manifests: vec![manifest()], ..HubConfig::default() };
    f(&mut config);
    HubShared::new_for_test(config)
}

fn shared() -> Arc<HubShared> {
    shared_with(|_| {})
}

fn emit_on(shared: &Arc<HubShared>, conn: u64, id: &str, name: &str, payload: Option<Value>) {
    let p = EventEmitParams { name: name.into(), event_id: id.into(), payload };
    shared.event_emit("shop", "i1", conn, "test-cid", p);
}

fn emit(shared: &Arc<HubShared>, id: &str, name: &str, payload: Option<Value>) {
    emit_on(shared, CONN, id, name, payload);
}

fn body(r: Result<CallToolResult, app_mcp_protocol::ToolError>) -> Value {
    r.expect("工具成功").structured_content.expect("结构化结果")
}

fn subscribe(shared: &HubShared, caller: &CallerKey, args: Value) -> Result<Value, app_mcp_protocol::ToolError> {
    shared.builtin_events_subscribe(caller, &args).map(|r| r.structured_content.unwrap_or_default())
}

fn fetch(shared: &HubShared, caller: &CallerKey) -> Value {
    body(shared.builtin_events_fetch(caller, &json!({})))
}

fn names(v: &Value) -> Vec<String> {
    v["events"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap().to_owned()).collect()
}

#[cfg(feature = "mcp-server")]
fn agent(name: &str) -> CallerKey {
    CallerKey::principal(&crate::task::Principal::Agent(crate::agents::AgentName::for_test(name)))
}

#[test]
fn emit_validation_and_dedup() {
    let s = shared();
    let a = CallerKey::api(Some("a"));
    subscribe(&s, &a, json!({ "appId": "shop" })).unwrap();
    emit(&s, "e1", "order.shipped", Some(json!({ "id": 1 })));
    emit(&s, "e1", "order.shipped", Some(json!({ "id": 1 })));
    assert_eq!(s.events_status().dropped_invalid, 0, "重复不算不合法");
    // 另一条连接（App 重启后 eventId 从头计数）不受去重影响。
    emit_on(&s, CONN + 1, "e1", "order.shipped", None);
    // 未声明（不在清单，也不在本连接的运行时声明）、载荷不是对象、超限、eventId 为空 → 丢弃并计数。
    emit(&s, "e2", "nope", None);
    emit(&s, "e3", "order.shipped", Some(json!([1])));
    emit(&s, "e4", "order.shipped", Some(json!({ "big": "x".repeat(MAX_EVENT_PAYLOAD_BYTES) })));
    emit(&s, "", "order.shipped", None);
    assert_eq!(s.events_status().dropped_invalid, 4);
    // 运行时声明只对声明它的连接有效。
    s.events_sync("shop", CONN, "c", vec![EventInfo { name: "runtime.only".into(), description: "d".into(), payload_schema: None }]);
    emit_on(&s, CONN + 1, "e5", "runtime.only", None);
    assert_eq!(s.events_status().dropped_invalid, 5);
    emit(&s, "e6", "runtime.only", None);
    let got = fetch(&s, &a);
    assert_eq!(names(&got), ["order.shipped", "order.shipped", "runtime.only"]);
    let ids: Vec<&str> = got["events"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["ev-1", "ev-2", "ev-3"]);
    assert_eq!(got["events"][0]["instanceId"], "i1");
    assert!(got["events"][0]["at"].as_u64().unwrap() > 0);
    // 断开后去重窗口释放：同一连接号的同一 eventId 再次投递（连接号不复用，此处只验证窗口释放）。
    s.events_disconnected("shop", CONN);
    emit(&s, "e1", "order.shipped", None);
    assert_eq!(names(&fetch(&s, &a)), ["order.shipped"]);
}

#[test]
fn subscribe_errors_and_duplicates() {
    let s = shared_with(|c| c.event_limits = EventLimits { max_subscriptions: 2, ..EventLimits::default() });
    let a = CallerKey::api(Some("a"));
    let err = subscribe(&s, &a, json!({ "appId": "nope" })).unwrap_err();
    assert_eq!(err.kind, ErrorKind::ToolNotFound);
    let err = subscribe(&s, &a, json!({ "appId": "shop", "event": "bad name" })).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput);
    let err = subscribe(&s, &a, json!({ "appId": "shop", "event": "other" })).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput);
    assert!(err.message.contains("order.shipped") && err.message.contains("cart.changed"), "{}", err.message);
    let err = subscribe(&s, &a, json!({ "appId": "shop", "filter": [1] })).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidInput);
    let first = subscribe(&s, &a, json!({ "appId": "shop", "event": "order.shipped", "filter": { "k": 1 } })).unwrap();
    assert_eq!(first["existing"], false);
    let again = subscribe(&s, &a, json!({ "appId": "shop", "event": "order.shipped", "filter": { "k": 1 } })).unwrap();
    assert_eq!((again["existing"].clone(), again["subscriptionId"].clone()), (json!(true), first["subscriptionId"].clone()));
    subscribe(&s, &a, json!({ "appId": "shop" })).unwrap();
    let err = subscribe(&s, &a, json!({ "appId": "shop", "event": "cart.changed" })).unwrap_err();
    assert_eq!(err.kind, ErrorKind::RateLimited);
    assert_eq!(err.details.as_ref().unwrap()["scope"], "events");
    // 上限按订阅方：另一调用方不受影响；重复订阅在上限时仍返回已有订阅。
    subscribe(&s, &CallerKey::api(Some("b")), json!({ "appId": "shop" })).unwrap();
    assert_eq!(subscribe(&s, &a, json!({ "appId": "shop" })).unwrap()["existing"], true);
}

#[test]
fn app_without_declarations_accepts_any_event_name() {
    let s = shared();
    s.events_sync("notes", CONN, "c", vec![]);
    let a = CallerKey::api(None);
    let r = subscribe(&s, &a, json!({ "appId": "notes", "event": "later.declared" })).unwrap();
    assert!(r["message"].as_str().unwrap().contains("尚未声明"));
}

#[test]
fn ownership_isolation_and_unsubscribe() {
    let s = shared();
    let (a, b) = (CallerKey::api(Some("a")), CallerKey::api(Some("b")));
    let sub = subscribe(&s, &a, json!({ "appId": "shop", "event": "order.shipped" })).unwrap();
    let id = sub["subscriptionId"].as_str().unwrap().to_owned();
    emit(&s, "1", "order.shipped", None);
    let got = fetch(&s, &b);
    assert!(got["events"].as_array().unwrap().is_empty() && got["subscriptions"].as_array().unwrap().is_empty(), "看不到他人的信箱");
    let err = s.builtin_events_unsubscribe(&b, &json!({ "subscriptionId": id })).unwrap_err();
    assert_eq!(err.kind, ErrorKind::ToolNotFound, "退订他人的订阅与不存在的相同");
    let err = s.builtin_events_unsubscribe(&a, &json!({ "subscriptionId": "sub-none" })).unwrap_err();
    assert_eq!(err.kind, ErrorKind::ToolNotFound);
    body(s.builtin_events_unsubscribe(&a, &json!({ "subscriptionId": id })));
    emit(&s, "2", "order.shipped", None);
    let got = fetch(&s, &a);
    assert_eq!(names(&got), ["order.shipped"], "退订前入箱的仍可取出，退订后的不再入箱");
    assert!(got["subscriptions"].as_array().unwrap().is_empty());
    assert_eq!(s.events_status().subscriptions.len(), 0);
}

#[test]
fn anonymous_owner_is_reclaimed_with_task() {
    let s = shared();
    let a = CallerKey::api(Some("a"));
    subscribe(&s, &a, json!({ "appId": "shop" })).unwrap();
    assert!(s.agent_tasks().get(&a).is_some(), "订阅时确保任务存在（随任务回收）");
    emit(&s, "1", "cart.changed", None);
    assert_eq!(s.events_summary(&a).pending, 1);
    s.end_task(&a);
    assert_eq!(s.events_summary(&a), super::EventsSummary::default());
    assert!(s.events_status().subscriptions.is_empty());
}

#[test]
fn filter_and_single_entry_through_hub() {
    let s = shared();
    let a = CallerKey::api(None);
    subscribe(&s, &a, json!({ "appId": "shop", "event": "order.shipped", "filter": { "carrier": "sf" } })).unwrap();
    subscribe(&s, &a, json!({ "appId": "shop" })).unwrap();
    emit(&s, "1", "order.shipped", Some(json!({ "carrier": "sf" })));
    emit(&s, "2", "cart.changed", None);
    let got = fetch(&s, &a);
    assert_eq!(names(&got), ["order.shipped", "cart.changed"], "两个订阅都匹配的事件只入箱一次");
    let subs = got["subscriptions"].as_array().unwrap();
    assert_eq!(subs.iter().map(|s| s["delivered"].as_u64().unwrap()).sum::<u64>(), 2);
}

#[test]
fn fetch_respects_max_and_reports_dropped() {
    let s = shared_with(|c| c.event_limits = EventLimits { max_inbox_events: 2, ..EventLimits::default() });
    let a = CallerKey::api(None);
    subscribe(&s, &a, json!({ "appId": "shop" })).unwrap();
    for i in 0..3 {
        emit(&s, &i.to_string(), "cart.changed", None);
    }
    let got = body(s.builtin_events_fetch(&a, &json!({ "max": 1 })));
    assert_eq!((got["events"].as_array().unwrap().len(), got["dropped"].as_u64(), got["pending"].as_u64()), (1, Some(1), Some(1)));
    assert_eq!(got["events"][0]["id"], "ev-2", "信箱满丢最旧");
    let got = fetch(&s, &a);
    assert_eq!((got["events"][0]["id"].clone(), got["dropped"].as_u64()), (json!("ev-3"), Some(0)), "取件后 dropped 清零");
}

#[test]
fn hide_blocks_subscribe_and_delivery() {
    let s = shared();
    let a = CallerKey::api(None);
    subscribe(&s, &a, json!({ "appId": "shop" })).unwrap();
    let hide: PolicyConfig = serde_json::from_value(json!({ "rules": [{ "id": "h", "action": "hide", "app": "shop" }] })).unwrap();
    s.set_policy(hide).unwrap();
    assert_eq!(subscribe(&s, &a, json!({ "appId": "shop", "event": "cart.changed" })).unwrap_err().kind, ErrorKind::ToolNotFound);
    emit(&s, "1", "cart.changed", None);
    assert!(fetch(&s, &a)["events"].as_array().unwrap().is_empty(), "隐藏的 App 的事件不投递");
}

#[cfg(feature = "mcp-server")]
#[test]
fn deny_matches_subscriber_agent_only() {
    let s = shared();
    let (claude, cursor) = (agent("claude"), agent("cursor"));
    subscribe(&s, &claude, json!({ "appId": "shop" })).unwrap();
    subscribe(&s, &cursor, json!({ "appId": "shop" })).unwrap();
    let deny: PolicyConfig = serde_json::from_value(json!({ "rules": [
        { "id": "no-ship", "action": "deny", "app": "shop", "tool": "order.*", "agent": "cursor" }
    ] }))
    .unwrap();
    s.set_policy(deny).unwrap();
    emit(&s, "1", "order.shipped", None);
    emit(&s, "2", "cart.changed", None);
    assert_eq!(names(&fetch(&s, &claude)), ["order.shipped", "cart.changed"]);
    assert_eq!(names(&fetch(&s, &cursor)), ["cart.changed"], "按订阅方主体匹配，事件名按工具模式匹配");
    assert_eq!(crate::hub::lock(&s.policy).status().rules[0].hits, 1);
}

#[cfg(feature = "mcp-server")]
#[test]
fn agent_owner_survives_task_end_and_shares_across_sessions() {
    let s = shared();
    let principal = agent("claude");
    let session = CallerKey::mcp_session(3, Some(crate::agents::AgentName::for_test("claude")));
    subscribe(&s, &session, json!({ "appId": "shop" })).unwrap();
    s.end_task(&session);
    emit(&s, "1", "cart.changed", None);
    assert_eq!(s.events_summary(&principal), super::EventsSummary { pending: 1, subscriptions: 1 }, "同名 Agent 的会话共享信箱");
    assert_eq!(s.events_status().subscriptions[0].subscriber, "agent:claude");
}

#[test]
fn events_self_reads_without_removing_and_notifies_only_owner() {
    let s = shared();
    let (api, other) = (CallerKey::api(None), CallerKey::api(Some("x")));
    subscribe(&s, &api, json!({ "appId": "shop", "event": "cart.changed" })).unwrap();
    subscribe(&s, &other, json!({ "appId": "shop", "event": "order.shipped" })).unwrap();
    s.subscribe(API_SUBSCRIBER, RESOURCE_APPS_EVENTS_URI, &api).unwrap();
    let mut rx = s.hub_events_for_test();
    emit(&s, "1", "order.shipped", None);
    let updated = |rx: &mut tokio::sync::broadcast::Receiver<HubEvent>| {
        std::iter::from_fn(|| rx.try_recv().ok()).filter(|e| matches!(e, HubEvent::ResourceUpdated { .. })).count()
    };
    assert_eq!(updated(&mut rx), 0, "他人信箱的新事件不提醒");
    emit(&s, "2", "cart.changed", None);
    assert_eq!(updated(&mut rx), 1);
    let read = |caller: &CallerKey| -> Value {
        let r = s.read_events_self(RESOURCE_APPS_EVENTS_URI, caller).unwrap();
        match &r.contents[0] {
            ResourceContents::TextResourceContents { text, .. } => serde_json::from_str(text).unwrap(),
            _ => panic!("应为文本"),
        }
    };
    let v = read(&api);
    assert_eq!((v["pending"].as_u64(), v["events"][0]["name"].clone()), (Some(1), json!("cart.changed")));
    assert_eq!(read(&api)["pending"], 1, "读取不移出");
    assert_eq!(s.self_state_view_for_test(&api).events, super::EventsSummary { pending: 1, subscriptions: 1 });
    s.unsubscribe(API_SUBSCRIBER, RESOURCE_APPS_EVENTS_URI);
    emit(&s, "3", "cart.changed", None);
    assert_eq!(updated(&mut rx), 0, "退订资源后不再提醒");
}

struct Recorder(Mutex<Vec<AppEvent>>);

impl EventHandler for Recorder {
    fn on_event(&self, event: &AppEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

#[test]
fn handler_and_broadcast_get_every_valid_event() {
    let s = shared();
    let rec = Arc::new(Recorder(Mutex::new(Vec::new())));
    s.set_event_handler(rec.clone());
    let mut rx = s.hub_events_for_test();
    emit(&s, "1", "cart.changed", Some(json!({ "n": 1 })));
    emit(&s, "1", "cart.changed", None);
    emit(&s, "2", "nope", None);
    let got = rec.0.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "无订阅也回调；重复与不合法的不回调");
    assert_eq!((got[0].app_id.as_str(), got[0].payload.clone()), ("shop", Some(json!({ "n": 1 }))));
    assert!(matches!(rx.try_recv(), Ok(HubEvent::AppEvent(e)) if e == got[0]));
}

#[cfg(feature = "mcp-server")]
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let n: u64 = rand::random();
    std::env::temp_dir().join(format!("app-mcp-events-{tag}-{}-{n:x}", std::process::id()))
}

#[cfg(feature = "mcp-server")]
#[test]
fn agent_inbox_persists_across_restart_but_anonymous_does_not() {
    use std::time::Duration;
    let dir = temp_dir("persist");
    let with_dir = |c: &mut HubConfig| c.state_dir = Some(dir.clone());
    let claude = agent("claude");
    let anon = CallerKey::api(None);
    {
        let s = shared_with(with_dir);
        subscribe(&s, &claude, json!({ "appId": "shop" })).unwrap();
        subscribe(&s, &anon, json!({ "appId": "shop" })).unwrap();
        emit(&s, "1", "cart.changed", None);
        emit(&s, "2", "order.shipped", None);
    }
    let s = shared_with(with_dir);
    s.load_inboxes();
    let got = fetch(&s, &claude);
    assert_eq!(names(&got), ["cart.changed", "order.shipped"]);
    assert_eq!(got["subscriptions"].as_array().unwrap().len(), 1);
    assert!(fetch(&s, &anon)["subscriptions"].as_array().unwrap().is_empty(), "匿名订阅方不持久化");
    // 序号从读回的最大序号之后继续。
    emit(&s, "3", "cart.changed", None);
    assert_eq!(fetch(&s, &claude)["events"][0]["id"], "ev-3");
    // TTL：读回时丢弃过期事件。
    let short = |c: &mut HubConfig| {
        c.state_dir = Some(dir.clone());
        c.event_limits = EventLimits { inbox_ttl: Duration::from_millis(1), ..EventLimits::default() };
    };
    emit(&s, "4", "cart.changed", None);
    drop(s);
    std::thread::sleep(Duration::from_millis(20));
    let s = shared_with(short);
    s.load_inboxes();
    assert!(fetch(&s, &claude)["events"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
