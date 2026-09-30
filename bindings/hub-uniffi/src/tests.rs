//! 端到端单元测试：嵌入式 Hub（随机端口）+ 同进程 `app-mcp-native` App，经绑定层对象驱动。

use std::sync::mpsc;
use std::time::{Duration, Instant};

use app_mcp_native as native;
use futures::executor::block_on as wait;
use serde_json::{Value, json};

use super::*;

struct AddNote;

impl native::ToolHandler for AddNote {
    fn invoke(&self, call: native::CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or_default();
        let data = json!({ "saved": args["text"] });
        let _ = call.complete(Some(&data.to_string()), vec!["notes.list".into()]);
    }
}

struct ClearNotes;

impl native::ToolHandler for ClearNotes {
    fn invoke(&self, call: native::CallHandle) {
        let _ = call.complete(Some(r#"{"cleared":true}"#), vec![]);
    }
}

struct ListNotes;

impl native::ResourceReader for ListNotes {
    fn read(&self, read: native::ReadHandle) {
        let _ = read.complete(r#"["买牛奶"]"#);
    }
}

struct Events(Mutex<mpsc::Sender<HubEvent>>);

impl HubEventListener for Events {
    fn on_event(&self, event: HubEvent) {
        let _ = lock(&self.0).send(event);
    }
    fn on_lagged(&self, _skipped: u64) {}
}

/// 记录请求并在另一个线程上稍后按 `answer` 回答；`answer = None` 时不回答、直接释放句柄（模拟外部异常 / 遗忘）。
struct Approver {
    answer: Option<bool>,
    seen: Mutex<Vec<ApprovalRequest>>,
}

impl ApprovalHandler for Approver {
    fn on_request(&self, request: ApprovalRequest, responder: Arc<ApprovalResponder>) {
        lock(&self.seen).push(request);
        let answer = self.answer;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(a) = answer {
                assert!(responder.complete(a));
                assert!(!responder.complete(!a), "只有第一次生效");
            }
        });
    }
}

/// 回调本身 panic（外部未预期异常）→ 视为拒绝。
struct PanickingApprover;

impl ApprovalHandler for PanickingApprover {
    fn on_request(&self, _request: ApprovalRequest, _responder: Arc<ApprovalResponder>) {
        panic!("UI 崩溃");
    }
}

fn wait_for(rx: &mpsc::Receiver<HubEvent>, pred: impl Fn(&HubEvent) -> bool) -> HubEvent {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(e) if pred(&e) => return e,
            Ok(_) => {}
            Err(e) => panic!("等待事件超时：{e}"),
        }
    }
}

fn start_hub(approval: Option<Risk>) -> Arc<AppMcpHub> {
    AppMcpHub::start(HubConfig {
        ws_addr: Some("127.0.0.1:0".into()),
        approval_min_risk: approval,
        ..Default::default()
    })
    .expect("启动 Hub")
}

fn start_app(hub: &AppMcpHub) -> native::NativeClient {
    let mut cfg = native::NativeConfig::new("notes", "笔记");
    cfg.host_url = format!("ws://{}", hub.ws_addr().expect("ws 地址"));
    cfg.overview = Some(native::AppOverview {
        summary: "测试笔记 App".into(),
        body: None,
        locale: None,
    });
    let app = native::NativeClient::new(cfg, None).expect("App");
    let mut add = native::ToolSpec::new("notes.add", "添加笔记");
    add.input_schema_json = Some(
        json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]})
            .to_string(),
    );
    add.risk = native::Risk::Write;
    app.register_tool(add, Arc::new(AddNote)).expect("注册");
    let mut clear = native::ToolSpec::new("notes.clear", "清空笔记");
    clear.risk = native::Risk::Destructive;
    app.register_tool(clear, Arc::new(ClearNotes)).expect("注册");
    app.register_resource(
        native::ResourceSpec {
            name: "notes.list".into(),
            description: "全部笔记".into(),
            mime_type: None,
        },
        Arc::new(ListNotes),
    )
    .expect("注册资源");
    app.start();
    app
}

fn req(name: &str, args: Value) -> CallRequest {
    CallRequest {
        name: name.into(),
        arguments_json: Some(args.to_string()),
        instance_id: None,
        timeout_ms: Some(5000),
        call_id: None,
        session: None,
    }
}

#[test]
fn end_to_end() {
    let hub = start_hub(Some(Risk::Destructive));
    let (tx, rx) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx)))));
    let approver = Arc::new(Approver {
        answer: Some(false),
        seen: Mutex::new(Vec::new()),
    });
    hub.set_approval_handler(approver.clone());

    let app = start_app(&hub);
    wait_for(&rx, |e| matches!(e, HubEvent::AppConnected { app_id, .. } if app_id == "notes"));
    wait_for(&rx, |e| matches!(e, HubEvent::ToolsChanged));

    // 列工具 / App / 资源
    let tools = hub.tools(ToolFilter {
        include_builtin: false,
        ..Default::default()
    });
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"notes.notes.add"), "{names:?}");
    let add = tools.iter().find(|t| t.tool == "notes.add").unwrap();
    assert_eq!(add.risk, Risk::Write);
    assert_eq!(add.availability, Availability::Available);
    assert!(add.input_schema_json.contains("\"text\""));
    let low = hub.tools(ToolFilter {
        max_risk: Some(Risk::Write),
        include_builtin: false,
        ..Default::default()
    });
    assert!(low.iter().all(|t| t.tool != "notes.clear"));
    let apps = hub.apps();
    assert!(apps.iter().any(|a| a.app_id == "notes" && a.connected && a.kind == AppKind::App));

    // callTool：成功 + 首次附带总览
    let out = wait(hub.call_tool(req("notes.notes.add", json!({"text": "买牛奶"})))).unwrap();
    assert_eq!(out.error, None);
    assert_eq!(
        serde_json::from_str::<Value>(out.data_json.as_deref().unwrap()).unwrap(),
        json!({"saved": "买牛奶"})
    );
    assert_eq!(out.state_hints, vec!["notes.list".to_owned()]);
    assert!(out.overview.is_some());
    assert!(out.call_id.starts_with("ffi-"));

    // 参数不合法 → INVALID_INPUT（在 outcome 中）
    let bad = wait(hub.call_tool(req("notes.notes.add", json!({})))).unwrap();
    assert_eq!(bad.error.unwrap().kind, "INVALID_INPUT");

    // 名称无法解析 → HubError
    let err = wait(hub.call_tool(req("nope.x", json!({})))).unwrap_err();
    assert!(matches!(err, HubError::Tool { .. }), "{err:?}");

    // exportTools + dispatch
    let exported: Value = serde_json::from_str(&hub.export_tools(
        ToolFormat::Anthropic,
        ToolFilter {
            include_builtin: false,
            ..Default::default()
        },
    ))
    .unwrap();
    let export_name = exported
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"].as_str().unwrap().ends_with("notes__add"))
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .unwrap();
    let call = json!({"type": "tool_use", "id": "toolu_1", "name": export_name, "input": {"text": "x"}});
    let result: Value = serde_json::from_str(
        &wait(hub.dispatch(ToolFormat::Anthropic, call.to_string(), Some("s1".into()))).unwrap(),
    )
    .unwrap();
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "toolu_1");
    assert!(result.get("is_error").is_none() || result["is_error"] == false);
    assert!(matches!(
        wait(hub.dispatch(ToolFormat::Anthropic, "{".into(), None)),
        Err(HubError::InvalidJson { .. })
    ));

    // 审批拒绝 → USER_REJECTED
    let rejected = wait(hub.call_tool(req("notes.notes.clear", json!({})))).unwrap();
    assert_eq!(rejected.error.unwrap().kind, "USER_REJECTED");
    {
        let seen = lock(&approver.seen);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].tool, "notes.clear");
        assert_eq!(seen[0].risk, Risk::Destructive);
    }

    // 句柄未完成即释放 → 视为拒绝
    hub.set_approval_handler(Arc::new(Approver {
        answer: None,
        seen: Mutex::new(Vec::new()),
    }));
    let rejected = wait(hub.call_tool(req("notes.notes.clear", json!({})))).unwrap();
    assert_eq!(rejected.error.unwrap().kind, "USER_REJECTED");
    // 回调异常 → 视为拒绝
    hub.set_approval_handler(Arc::new(PanickingApprover));
    let rejected = wait(hub.call_tool(req("notes.notes.clear", json!({})))).unwrap();
    assert_eq!(rejected.error.unwrap().kind, "USER_REJECTED");

    // 同意 → 成功
    hub.set_approval_handler(Arc::new(Approver {
        answer: Some(true),
        seen: Mutex::new(Vec::new()),
    }));
    let ok = wait(hub.call_tool(req("notes.notes.clear", json!({})))).unwrap();
    assert_eq!(ok.error, None);

    // 资源
    let res = hub.resources();
    let uri = res.iter().find(|r| r.name == "notes.notes.list").unwrap().uri.clone();
    let content = wait(hub.read_resource(uri.clone())).unwrap();
    assert_eq!(content.text.as_deref(), Some(r#"["买牛奶"]"#));
    hub.subscribe(uri.clone()).unwrap();
    hub.unsubscribe(uri);
    assert!(hub.overview("notes".into()).is_some());

    // App 断开 → 事件
    app.stop();
    wait_for(&rx, |e| matches!(e, HubEvent::AppDisconnected { .. }));

    hub.shutdown();
    hub.shutdown();
    assert!(hub.apps().is_empty());
    assert_eq!(
        wait(hub.call_tool(req("notes.notes.add", json!({})))).unwrap_err(),
        HubError::Shutdown
    );
    assert_eq!(hub.export_tools(ToolFormat::Gemini, ToolFilter::default()), r#"{"functionDeclarations":[]}"#);
}

#[test]
fn call_can_be_cancelled_by_dropping_future() {
    struct Slow;
    impl native::ToolHandler for Slow {
        fn invoke(&self, call: native::CallHandle) {
            // 故意不完成：等待取消。
            std::mem::forget(call);
        }
    }
    let hub = start_hub(None);
    let (tx, rx) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx)))));
    let mut cfg = native::NativeConfig::new("slow", "慢");
    cfg.host_url = format!("ws://{}", hub.ws_addr().unwrap());
    let app = native::NativeClient::new(cfg, None).unwrap();
    app.register_tool(native::ToolSpec::new("wait", "等"), Arc::new(Slow))
        .unwrap();
    app.start();
    wait_for(&rx, |e| matches!(e, HubEvent::ToolsChanged));

    let mut r = req("slow.wait", json!({}));
    r.call_id = Some("c-1".into());
    // 显式 cancel_call：结果为 CANCELLED。
    let h2 = hub.clone();
    let t = std::thread::spawn(move || wait(h2.call_tool(r)));
    std::thread::sleep(Duration::from_millis(200));
    hub.cancel_call("c-1".into());
    let out = t.join().unwrap().unwrap();
    assert_eq!(out.error.unwrap().kind, "CANCELLED");

    // 丢弃 future：CancelOnDrop 发出取消，不会 panic。
    let fut = hub.call_tool(req("slow.wait", json!({})));
    wait(async {
        futures::future::select(Box::pin(fut), Box::pin(sleep_ms(100))).await;
    });
    app.stop();
    hub.shutdown();
}

/// 不依赖 tokio 运行时的简单定时器（测试在普通线程上 poll）。
async fn sleep_ms(ms: u64) {
    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(ms));
        let _ = tx.send(());
    });
    let _ = rx.await;
}

#[test]
fn event_listener_can_be_replaced_and_removed() {
    let hub = start_hub(None);
    let (tx1, rx1) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx1)))));
    hub.set_event_listener(None);
    let (tx2, rx2) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx2)))));
    let app = start_app(&hub);
    wait_for(&rx2, |e| matches!(e, HubEvent::AppConnected { .. }));
    assert!(rx1.try_recv().is_err());
    app.stop();
    hub.shutdown();
}

#[test]
fn policy_without_handler_rejects() {
    let hub = start_hub(Some(Risk::Write));
    let (tx, rx) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx)))));
    let app = start_app(&hub);
    wait_for(&rx, |e| matches!(e, HubEvent::ToolsChanged));
    let out = wait(hub.call_tool(req("notes.notes.add", json!({"text": "a"})))).unwrap();
    assert_eq!(out.error.unwrap().kind, "USER_REJECTED");
    app.stop();
    drop(hub); // Drop 时自动停止
}

#[test]
fn parse_formats_and_ws_disabled() {
    assert_eq!(parse_tool_format("openai".into()), Ok(ToolFormat::OpenAiChat));
    assert_eq!(parse_tool_format("Anthropic".into()), Ok(ToolFormat::Anthropic));
    assert_eq!(parse_tool_format("openai_responses".into()), Ok(ToolFormat::OpenAiResponses));
    assert!(parse_tool_format("x".into()).is_err());

    let hub = AppMcpHub::start(HubConfig {
        enable_ws: false,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(hub.ws_addr(), None);
    let tools = hub.tools(ToolFilter::default());
    assert!(tools.iter().any(|t| t.name == "apps.list"));
    hub.shutdown();
}

#[test]
fn guards() {
    assert!(guarded(|| {}));
    assert!(!guarded(|| panic!("boom")));
    let (once, rx) = Once::new();
    drop(ApprovalResponder(once));
    assert!(wait(rx).is_err(), "未完成即释放：接收方得到 Err");
}

/// 自定义唤醒：记录请求，让同进程 App 处理激活参数。
struct TestWaker {
    app: Arc<Mutex<Option<native::NativeClient>>>,
    seen: Mutex<Vec<WakeRequest>>,
    fail: Option<String>,
}

impl HubWaker for TestWaker {
    fn wake(&self, request: WakeRequest, responder: Arc<WakeResponder>) {
        lock(&self.seen).push(request.clone());
        if let Some(kind) = &self.fail {
            responder.fail(kind.clone(), "没装".into());
            return;
        }
        let ok = lock(&self.app).as_ref().is_some_and(|a| a.handle_wake(&request.activation_arg));
        if ok {
            responder.succeed();
        } else {
            responder.fail("LAUNCH_FAILED".into(), "?".into());
        }
    }
}

/// 不给结果就释放句柄 → LAUNCH_FAILED。
struct SilentWaker;

impl HubWaker for SilentWaker {
    fn wake(&self, _request: WakeRequest, _responder: Arc<WakeResponder>) {}
}

#[test]
fn dormant_app_woken_by_foreign_waker() {
    let hub = AppMcpHub::start(HubConfig {
        ws_addr: Some("127.0.0.1:0".into()),
        lease_ttl_ms: Some(0),
        wake_timeout_ms: Some(10_000),
        list_changed_debounce_ms: Some(20),
        ..Default::default()
    })
    .expect("启动 Hub");
    let (tx, rx) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx)))));

    let mut cfg = native::NativeConfig::new("sleepy", "会睡觉的 App");
    cfg.host_url = format!("ws://{}", hub.ws_addr().unwrap_or_default());
    cfg.instance_id = Some("s1".into());
    cfg.lifecycle.mode = native::LifecycleMode::Idle;
    cfg.lifecycle.idle_timeout_ms = 300;
    cfg.lifecycle.wake = Some(native::WakeDescriptor {
        kind: native::WakeKind::AndroidIntent,
        target: Some("dev.example/.WakeReceiver".into()),
        background: true,
    });
    let client = native::NativeClient::new(cfg, None).expect("App");
    let _t = client
        .register_tool(native::ToolSpec::new("ping", "回显"), Arc::new(AddNote))
        .expect("注册");
    let slot = Arc::new(Mutex::new(None));
    let waker = Arc::new(TestWaker { app: slot.clone(), seen: Mutex::new(vec![]), fail: None });
    hub.set_waker(Some(waker.clone()));
    client.start();
    *lock(&slot) = Some(client);

    let e = wait_for(&rx, |e| matches!(e, HubEvent::AppDormant { .. }));
    assert_eq!(e, HubEvent::AppDormant { app_id: "sleepy".into(), instance_id: "s1".into() });
    let app = hub.apps().into_iter().find(|a| a.app_id == "sleepy").expect("App");
    assert!(!app.connected);
    assert_eq!(app.dormant_instances[0].instance_id, "s1");
    let tools = hub.tools(ToolFilter { apps: Some(vec!["sleepy".into()]), include_builtin: false, ..Default::default() });
    assert_eq!(tools[0].availability, Availability::Dormant);

    let out = wait(hub.call_tool(req("sleepy.ping", json!({"text": "hi"})))).expect("调用");
    assert!(out.error.is_none(), "{out:?}");
    assert_eq!(out.instance_id.as_deref(), Some("s1"));
    let seen = lock(&waker.seen).clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].descriptor.kind, WakeKind::AndroidIntent);
    assert_eq!(seen[0].activation_arg, format!("app-mcp-wake:{}", seen[0].token));
    wait_for(&rx, |e| matches!(e, HubEvent::AppWaking { instance_id: Some(i), .. } if i == "s1"));

    // 失败类别透传；清除后恢复默认实现
    wait_for(&rx, |e| matches!(e, HubEvent::AppDormant { .. }));
    hub.set_waker(Some(Arc::new(TestWaker { app: slot.clone(), seen: Mutex::new(vec![]), fail: Some("APP_NOT_INSTALLED".into()) })));
    let out = wait(hub.call_tool(req("sleepy.ping", json!({})))).expect("调用");
    assert_eq!(out.error.map(|e| e.kind).as_deref(), Some("APP_NOT_INSTALLED"));
    hub.set_waker(Some(Arc::new(SilentWaker)));
    let out = wait(hub.call_tool(req("sleepy.ping", json!({})))).expect("调用");
    assert_eq!(out.error.map(|e| e.kind).as_deref(), Some("LAUNCH_FAILED"));
    hub.set_waker(None);
    if let Some(c) = lock(&slot).take() {
        c.stop();
    }
    hub.shutdown();
}
