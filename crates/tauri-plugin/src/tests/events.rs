//! 桥接：页面事件（`event.declare` / `event.remove` / `event.emit`，spec/protocol.md 3.5）。

use app_mcp_hub::{AppEvent, EventHandler};

use super::*;

#[derive(Default)]
struct Captured(Mutex<Vec<AppEvent>>);

impl EventHandler for Captured {
    fn on_event(&self, event: &AppEvent) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push(event.clone());
    }
}

impl Captured {
    fn names(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).iter().map(|e| e.name.clone()).collect()
    }
}

fn declare(name: &str) -> Value {
    json!({ "op": "event.declare", "event": { "name": name, "description": "页面事件" } })
}

fn emit(name: &str, payload: Value) -> Value {
    json!({ "op": "event.emit", "name": name, "payload": payload })
}

/// 声明后发出经 Hub 校验送达；未声明 / 载荷不是对象回复错误；未连接时 `value` 为 false。
#[tokio::test(flavor = "multi_thread")]
async fn page_events_reach_hub() {
    let fx = Fixture::new("pevents", None).await;
    let captured = Arc::new(Captured::default());
    fx.hub.set_event_handler(captured.clone());
    let page = Arc::new(FakePage::default());
    assert_eq!(fx.op(&page, "main", "main", declare("order.shipped"))["ok"], true);
    assert_eq!(fx.op(&page, "main", "main", emit("order.shipped", json!({})))["value"], false, "未连接时丢弃");
    fx.connected("pevents").await;

    let sent = fx.op(&page, "main", "main", emit("order.shipped", json!({ "orderId": "o1" })));
    assert_eq!(sent["value"], true);
    eventually("Hub 收到事件", || captured.names() == ["order.shipped"]).await;
    let event = captured.0.lock().unwrap_or_else(|p| p.into_inner())[0].clone();
    assert_eq!(event.payload, Some(json!({ "orderId": "o1" })));

    let undeclared = fx.op(&page, "main", "main", emit("nope", json!({})));
    assert_eq!((undeclared["ok"].clone(), undeclared["code"].clone()), (json!(false), json!("INVALID_NAME")));
    let not_object = fx.op(&page, "main", "main", emit("order.shipped", json!([1])));
    assert_eq!((not_object["ok"].clone(), not_object["code"].clone()), (json!(false), json!("INVALID_JSON")));
    let bad_name = fx.op(&page, "main", "main", declare("bad name"));
    assert_eq!(bad_name["code"], "INVALID_NAME");

    let Fixture { hub, bridge, sessions } = fx;
    drop(bridge);
    drop(sessions);
    shutdown(hub).await;
}

/// 声明归页面：撤销 / 刷新 / 关闭窗口后撤销客户端上的声明；另一页仍声明同名事件时保留。
#[tokio::test(flavor = "multi_thread")]
async fn page_event_declarations_follow_pages() {
    let fx = Fixture::new("pdecl", None).await;
    let (a, b) = (Arc::new(FakePage::default()), Arc::new(FakePage::default()));
    let client = fx.bridge.client();
    let declared = |name: &str| client.emit_event(name, None).is_ok();

    fx.op(&a, "a", "main", declare("shared"));
    fx.op(&b, "b", "main", declare("shared"));
    fx.op(&a, "a", "main", declare("only.a"));
    assert!(declared("shared") && declared("only.a"));

    assert_eq!(fx.op(&a, "a", "main", json!({ "op": "event.remove", "name": "only.a" }))["value"], true);
    assert_eq!(fx.op(&a, "a", "main", json!({ "op": "event.remove", "name": "only.a" }))["value"], false);
    assert!(!declared("only.a"), "唯一声明的页面撤销后客户端上撤销");

    fx.op(&a, "a", "main", json!({ "op": "hello" }));
    assert!(declared("shared"), "另一页仍声明");
    fx.op(&b, "b", "other", json!({ "op": "hello" }));
    assert!(!declared("shared"), "两页都刷新后撤销");

    fx.op(&b, "b", "other", declare("win"));
    fx.sessions.end_window("other");
    assert!(!declared("win"), "窗口关闭后撤销");

    let Fixture { hub, bridge, sessions } = fx;
    drop(bridge);
    drop(sessions);
    shutdown(hub).await;
}

/// 同名事件仍有其他页面声明时，以其声明重新声明（Hub 的事件目录随之更新）。
#[tokio::test(flavor = "multi_thread")]
async fn remaining_page_declaration_is_redeclared() {
    let fx = Fixture::new("predecl", None).await;
    let (a, b) = (Arc::new(FakePage::default()), Arc::new(FakePage::default()));
    fx.connected("predecl").await;
    let shared = |description: &str| json!({ "op": "event.declare", "event": { "name": "shared", "description": description } });
    fx.op(&b, "b", "main", shared("来自 b"));
    fx.op(&a, "a", "main", shared("来自 a"));
    let catalog = || async {
        let outcome = fx.hub.call_tool(CallRequest::new("apps.tools", json!({ "appId": "predecl" }))).await.expect("apps.tools");
        outcome.result.ok().and_then(|v| v["events"][0]["description"].as_str().map(str::to_owned))
    };
    let mut description = None;
    for _ in 0..200 {
        description = catalog().await;
        if description.as_deref() == Some("来自 a") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(description.as_deref(), Some("来自 a"));
    fx.op(&a, "a", "main", json!({ "op": "hello" }));
    for _ in 0..200 {
        description = catalog().await;
        if description.as_deref() == Some("来自 b") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(description.as_deref(), Some("来自 b"), "a 刷新后以 b 的声明重新声明");

    let Fixture { hub, bridge, sessions } = fx;
    drop(bridge);
    drop(sessions);
    shutdown(hub).await;
}
