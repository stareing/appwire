//! 端到端单元测试（续）：App 事件、订阅与信箱（spec/hub-api.md 3.17）经绑定层：
//! `HubEvent::AppEvent`、事件回调（替换 / 清除 / 异常）、内置工具取件与 `status().events`。

use super::*;

/// 记录收到的 App 事件。
#[derive(Default)]
struct Recorder(Mutex<Vec<AppEvent>>);

impl AppEventHandler for Recorder {
    fn on_app_event(&self, event: AppEvent) {
        lock(&self.0).push(event);
    }
}

/// 回调本身 panic（外部未预期异常）：不影响投递。
struct PanickingHandler;

impl AppEventHandler for PanickingHandler {
    fn on_app_event(&self, _event: AppEvent) {
        panic!("回调崩溃");
    }
}

fn session_call(hub: &AppMcpHub, name: &str, args: Value) -> Value {
    let out = wait(hub.call_tool(CallRequest { session: Some("s1".into()), ..req(name, args) })).expect("调用");
    assert!(out.error.is_none(), "{name}：{out:?}");
    serde_json::from_str(out.data_json.as_deref().unwrap_or("null")).unwrap_or_default()
}

/// App 已连接后发出事件（握手完成前 `emit_event` 返回 `false`，重试至发出）。
fn emit(app: &native::NativeClient, name: &str, payload: Option<&str>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.emit_event(name, payload).expect("发出事件") {
        assert!(Instant::now() < deadline, "等待 App 连接超时");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn next_app_event(rx: &mpsc::Receiver<HubEvent>) -> AppEvent {
    match wait_for(rx, |e| matches!(e, HubEvent::AppEvent { .. })) {
        HubEvent::AppEvent { event } => event,
        other => panic!("意外事件：{other:?}"),
    }
}

#[test]
fn app_event_reaches_listener_handler_and_inbox() {
    let hub = start_hub(None);
    let (tx, rx) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx)))));
    let recorder = Arc::new(Recorder::default());
    hub.set_event_handler(Some(Arc::new(PanickingHandler)));
    hub.set_event_handler(Some(recorder.clone()));
    let app = start_app(&hub);
    let info = native::EventInfo { name: "note.added".into(), description: "新增了笔记".into(), payload_schema: None };
    app.declare_event(info).expect("声明事件");
    wait_for(&rx, |e| matches!(e, HubEvent::AppConnected { .. }));

    let sub = session_call(&hub, "apps.events.subscribe", json!({"appId": "notes", "event": "note.added"}));
    let sub_id = sub["subscriptionId"].as_str().expect("subscriptionId").to_owned();
    emit(&app, "note.added", Some(r#"{"id":"n1"}"#));

    let event = next_app_event(&rx);
    assert_eq!((event.app_id.as_str(), event.name.as_str()), ("notes", "note.added"), "{event:?}");
    assert_eq!(event.payload_json.as_deref(), Some(r#"{"id":"n1"}"#));
    assert!(event.id.starts_with("ev-") && event.at_ms > 0, "{event:?}");
    assert_eq!(*lock(&recorder.0), vec![event.clone()], "回调收到同一事件");

    let events = hub.status().expect("status").events.expect("events");
    assert_eq!(events.dropped_invalid, 0);
    let s = &events.subscriptions[..];
    assert_eq!(s.len(), 1, "{s:?}");
    assert_eq!((s[0].subscription_id.as_str(), s[0].app_id.as_str(), s[0].event.as_deref()), (sub_id.as_str(), "notes", Some("note.added")));
    assert_eq!((s[0].delivered, s[0].dropped, s[0].pending), (1, 0, 1), "{s:?}");

    let inbox = session_call(&hub, "apps.events", json!({}));
    assert_eq!(inbox["events"][0]["name"], "note.added", "{inbox}");
    assert_eq!(inbox["events"][0]["payload"], json!({"id": "n1"}), "{inbox}");
    assert_eq!(inbox["pending"], 0, "{inbox}");

    // 清除回调后：事件流照常，回调不再收到。
    hub.set_event_handler(None);
    emit(&app, "note.added", None);
    let second = next_app_event(&rx);
    assert_eq!(second.payload_json, None);
    assert_eq!(lock(&recorder.0).len(), 1);
    app.stop();
    hub.shutdown();
}
