//! 单元测试（续）：事件、订阅与信箱（v22，spec/hub-api.md 3.17）：App 端 emit → apps.events 取件、status.events 计数、
//! am_hub_set_app_event_cb 与 am_hub_set_event_cb（appEvent）回调。

use app_mcp_native::EventInfo;

use super::*;

/// 本文件专用的释放计数（不与并行测试共用 FREED）。
static APP_EVENT_FREED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn free_app_sender(ud: *mut c_void) {
    // SAFETY: Box::into_raw 得到的 Sender。
    drop(unsafe { Box::from_raw(ud as *mut Sender<String>) });
    APP_EVENT_FREED.fetch_add(1, Ordering::SeqCst);
}

static NULL_HUB_FREED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn free_null_hub_sender(ud: *mut c_void) {
    // SAFETY: Box::into_raw 得到的 Sender。
    drop(unsafe { Box::from_raw(ud as *mut Sender<String>) });
    NULL_HUB_FREED.fetch_add(1, Ordering::SeqCst);
}

/// 发出事件直到已连接（未连接时 emit 返回 false、不缓存）。
fn emit_when_connected(app: &NativeClient, name: &str, payload: &str) {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if app.emit_event(name, Some(payload)).expect("发出事件") {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("等待 App 连接超时");
}

fn status(hub: *mut AmHub) -> Value {
    // SAFETY: 有效参数。
    query_json(|o| unsafe { am_hub_status_json(hub, o) })
}

#[test]
fn app_event_reaches_inbox_status_and_callbacks() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0"}"#);
    let (ev_tx, ev_rx) = mpsc::channel::<String>();
    let (app_tx, app_rx) = mpsc::channel::<String>();
    let freed = APP_EVENT_FREED.load(Ordering::SeqCst);
    // SAFETY: 有效参数；事件流的 Sender 比 Hub 活得久，App 事件的 Sender 交给库释放。
    unsafe {
        assert_eq!(am_hub_set_event_cb(hub, Some(on_event), ud(&ev_tx), None), AmHubStatus::Ok);
        let boxed = Box::into_raw(Box::new(app_tx)) as *mut c_void;
        assert_eq!(am_hub_set_app_event_cb(hub, Some(on_event), boxed, Some(free_app_sender)), AmHubStatus::Ok);
    }

    let app = start_app(hub);
    app.client
        .declare_event(EventInfo { name: "note.added".into(), description: "新增了笔记".into(), payload_schema: None })
        .expect("声明事件");
    assert!(wait_event(&ev_rx, |e| e["type"] == "appConnected"), "App 未连接");

    let (tx, rx) = mpsc::channel::<String>();
    // 订阅：未声明的事件名 → INVALID_INPUT；之后按 App 订阅全部事件。
    call(hub, json!({"name":"apps.events.subscribe","arguments":{"appId":"notes","event":"nope"},"session":"s1"}), &tx);
    assert_eq!(recv(&rx)["result"]["error"]["kind"], "INVALID_INPUT");
    call(hub, json!({"name":"apps.events.subscribe","arguments":{"appId":"notes"},"session":"s1"}), &tx);
    let sub = recv(&rx);
    let sub_id = sub["result"]["ok"]["subscriptionId"].as_str().unwrap_or_default().to_owned();
    assert!(!sub_id.is_empty(), "{sub}");

    emit_when_connected(&app.client, "note.added", r#"{"text":"买牛奶"}"#);

    // 厂商回调收到 AppEvent；事件流收到 {"type":"appEvent", ...} 同一事件。
    let got = recv(&app_rx);
    assert_eq!((got["appId"].as_str(), got["instanceId"].as_str()), (Some("notes"), Some("n1")), "{got}");
    assert_eq!(got["name"], "note.added");
    assert_eq!(got["payload"], json!({ "text": "买牛奶" }));
    assert!(got["id"].as_str().is_some_and(|id| id.starts_with("ev-")), "{got}");
    assert!(got["at"].as_u64().is_some_and(|at| at > 0), "{got}");
    assert!(
        wait_event(&ev_rx, |e| e["type"] == "appEvent" && e["id"] == got["id"] && e["payload"] == got["payload"]),
        "事件流没有 appEvent"
    );

    // status.events：一个订阅，已投递 1、积压 1；取件后积压 0。
    let st = status(hub);
    let subs = st["events"]["subscriptions"].as_array().cloned().unwrap_or_default();
    assert_eq!(subs.len(), 1, "{st}");
    assert_eq!(subs[0]["subscriptionId"], sub_id.as_str());
    assert_eq!(
        (subs[0]["subscriber"].as_str(), subs[0]["appId"].as_str(), subs[0]["delivered"].as_u64(), subs[0]["pending"].as_u64()),
        (Some("api:s1"), Some("notes"), Some(1), Some(1)),
        "{st}"
    );
    assert_eq!(st["events"]["droppedInvalid"], 0, "{st}");

    // 只有订阅方能取到；取件移出信箱。
    call(hub, json!({"name":"apps.events","arguments":{},"session":"s2"}), &tx);
    assert_eq!(recv(&rx)["result"]["ok"]["events"], json!([]));
    call(hub, json!({"name":"apps.events","arguments":{},"session":"s1"}), &tx);
    let fetched = recv(&rx);
    let events = fetched["result"]["ok"]["events"].as_array().cloned().unwrap_or_default();
    assert_eq!(events.len(), 1, "{fetched}");
    assert_eq!(events[0], got, "{fetched}");
    assert_eq!(fetched["result"]["ok"]["pending"], 0, "{fetched}");
    assert_eq!(status(hub)["events"]["subscriptions"][0]["pending"], 0);

    // 清除厂商回调：释放 user_data，之后的事件不再回调（事件流仍收到）。
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_set_app_event_cb(hub, None, ptr::null_mut(), None) }, AmHubStatus::Ok);
    assert_eq!(APP_EVENT_FREED.load(Ordering::SeqCst), freed + 1);
    emit_when_connected(&app.client, "note.added", r#"{"text":"第二条"}"#);
    assert!(wait_event(&ev_rx, |e| e["type"] == "appEvent" && e["payload"]["text"] == "第二条"));
    assert!(app_rx.recv_timeout(Duration::from_millis(200)).is_err(), "清除后仍收到 App 事件回调");

    // 退订后不再入箱。
    call(hub, json!({"name":"apps.events.unsubscribe","arguments":{"subscriptionId":sub_id},"session":"s1"}), &tx);
    assert!(recv(&rx)["result"]["ok"].is_object());
    assert_eq!(status(hub)["events"]["subscriptions"], json!([]));

    app.client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

#[test]
fn app_event_cb_null_hub_and_freed_on_failure() {
    let freed = NULL_HUB_FREED.load(Ordering::SeqCst);
    let (tx, _rx) = mpsc::channel::<String>();
    let boxed = Box::into_raw(Box::new(tx)) as *mut c_void;
    // SAFETY: hub 为 NULL；user_data 无论成败归库所有。
    let st = unsafe { am_hub_set_app_event_cb(ptr::null_mut(), Some(on_event), boxed, Some(free_null_hub_sender)) };
    assert_eq!(st, AmHubStatus::InvalidArgument);
    assert_eq!(NULL_HUB_FREED.load(Ordering::SeqCst), freed + 1);
}
