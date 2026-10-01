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
        listen: Some("127.0.0.1:0".into()),
        // 测试不占用本机常驻 Host 的默认 IPC 端点。
        enable_ipc: false,
        approval_min_risk: approval,
        ..Default::default()
    })
    .expect("启动 Hub")
}

fn start_app(hub: &AppMcpHub) -> native::NativeClient {
    let mut cfg = native::NativeConfig::new("notes", "笔记");
    cfg.host_url = format!("ws://{}/app", hub.listen_addr().expect("监听地址"));
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

    // 参数不合法 → INVALID_INPUT（在 outcome 中）；无 schema-validation 时参数原样交给 App（spec/hub-api.md 3.10）
    if hub::features::SCHEMA_VALIDATION {
        let bad = wait(hub.call_tool(req("notes.notes.add", json!({})))).unwrap();
        assert_eq!(bad.error.unwrap().kind, "INVALID_INPUT");
    }

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
    cfg.host_url = format!("ws://{}/app", hub.listen_addr().unwrap());
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
        enable_listen: false,
        enable_ipc: false,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(hub.listen_addr(), None);
    assert_eq!(hub.ipc_endpoint(), None);
    let tools = hub.tools(ToolFilter::default());
    assert!(tools.iter().any(|t| t.name == "apps.list"));
    hub.shutdown();
}

#[test]
fn native_app_over_ipc_reports_pid() {
    #[cfg(unix)]
    let (endpoint, dir) = {
        let dir = std::env::temp_dir().join(format!("app-mcp-hub-uniffi-ipc-{}", std::process::id()));
        (format!("unix:{}", dir.join("hub.sock").display()), Some(dir))
    };
    #[cfg(windows)]
    let (endpoint, dir) = (
        format!(r"pipe:\\.\pipe\app-mcp-hub-uniffi-test-{}", std::process::id()),
        None::<std::path::PathBuf>,
    );
    let hub = AppMcpHub::start(HubConfig {
        enable_listen: false,
        ipc_endpoint: Some(endpoint.clone()),
        ..Default::default()
    })
    .expect("启动 Hub");
    assert_eq!(hub.ipc_endpoint().as_deref(), Some(endpoint.as_str()));
    let mut cfg = native::NativeConfig::new("notes", "笔记");
    cfg.host_url = endpoint;
    cfg.launch_token = Some(String::new());
    let app = native::NativeClient::new(cfg, None).expect("客户端");
    app.start();
    let deadline = Instant::now() + Duration::from_secs(10);
    let pid = loop {
        let apps = hub.apps();
        if let Some(i) = apps.iter().find(|a| a.app_id == "notes").and_then(|a| a.instances.first()) {
            break i.pid;
        }
        assert!(Instant::now() < deadline, "App 未经 IPC 连上");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(pid, Some(std::process::id()));
    app.stop();
    hub.shutdown();
    if let Some(d) = dir {
        let _ = std::fs::remove_dir_all(d);
    }
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
        listen: Some("127.0.0.1:0".into()),
        enable_ipc: false,
        lease_ttl_ms: Some(0),
        wake_timeout_ms: Some(10_000),
        list_changed_debounce_ms: Some(20),
        ..Default::default()
    })
    .expect("启动 Hub");
    let (tx, rx) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx)))));

    let mut cfg = native::NativeConfig::new("sleepy", "会睡觉的 App");
    cfg.host_url = format!("ws://{}/app", hub.listen_addr().unwrap_or_default());
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

#[test]
fn status_reports_instances_connection_ids_and_shutdown() {
    let hub = start_hub(None);
    let st = hub.status().expect("status");
    assert_eq!(st.service, "app-mcp");
    assert_eq!(st.listen, hub.listen_addr());
    assert!(st.apps.is_empty() && st.reports.is_empty());
    assert!(!st.mcp_http && !st.auth.token_configured);
    assert!(st.started_at_ms > 0);
    let lease = st.lease.expect("lease");
    assert_eq!((lease.mode.as_str(), lease.default_ms, lease.window), ("adaptive", 60_000, 20));

    let app = start_app(&hub);
    let deadline = Instant::now() + Duration::from_secs(10);
    let (status_cid, app_cid) = loop {
        let st = hub.status().expect("status");
        let inst = st
            .apps
            .iter()
            .find(|a| a.app_id == "notes" && a.state == AppState::Connected)
            .and_then(|a| a.instances.first().cloned());
        if let (Some(i), Some(cid)) = (inst, app.connection_id()) {
            assert_eq!(i.state, InstanceState::Connected);
            // 功耗观测（4e）：已连接实例有计数
            let p = i.power.expect("power");
            assert_eq!(p.reconnects, 0);
            assert_eq!(st.apps.iter().find(|a| a.app_id == "notes").map(|a| a.wakes), Some(0));
            break (i.info.connection_id, cid);
        }
        assert!(Instant::now() < deadline, "App 未连上");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status_cid.as_deref(), Some(app_cid.as_str()));
    // apps() 中的 InstanceInfo 也带连接 ID
    let apps = hub.apps();
    let notes = apps.iter().find(|a| a.app_id == "notes").expect("notes");
    assert_eq!(notes.instances[0].connection_id.as_deref(), Some(app_cid.as_str()));
    app.stop();
    hub.shutdown();
    assert_eq!(hub.status().unwrap_err(), HubError::Shutdown);
}

/// SDK 的 `app/diagnostic` 上报 → `HubEvent::AppDiagnostic` 与 `status().reports`（原始 WebSocket 模拟网页 SDK）。
#[test]
fn diagnostic_report_event_and_status() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

    let hub = start_hub(None);
    let (tx, rx) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx)))));
    let url = format!("ws://{}/app", hub.listen_addr().expect("监听地址"));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("运行时");
    let ws = rt.block_on(async {
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.expect("ws");
        let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": {
            "appId": "page", "appName": "页面", "protocolVersion": "1", "sdkVersion": "t",
            "clientKind": "web", "instanceId": "page-1"
        }});
        ws.send(WsMessage::text(hello.to_string())).await.expect("send");
        loop {
            if let Some(Ok(WsMessage::Text(t))) = ws.next().await {
                let v: Value = serde_json::from_str(t.as_str()).expect("json");
                if v["id"] == 1 {
                    assert_eq!(v["result"]["status"], "paired", "{v}");
                    break;
                }
            }
        }
        let n = json!({"jsonrpc": "2.0", "method": "app/diagnostic", "params": {
            "code": "BLOCKED_LOCAL_NETWORK_ACCESS", "message": "本地网络访问未授权", "count": 2
        }});
        ws.send(WsMessage::text(n.to_string())).await.expect("send");
        ws
    });
    let e = wait_for(&rx, |e| matches!(e, HubEvent::AppDiagnostic { .. }));
    assert_eq!(
        e,
        HubEvent::AppDiagnostic {
            app_id: "page".into(),
            instance_id: "page-1".into(),
            code: "BLOCKED_LOCAL_NETWORK_ACCESS".into(),
            message: "本地网络访问未授权".into(),
            count: 2,
        }
    );
    let st = hub.status().expect("status");
    assert_eq!(st.reports.len(), 1);
    let r = &st.reports[0];
    assert_eq!((r.app_id.as_str(), r.code.as_str(), r.count), ("page", "BLOCKED_LOCAL_NETWORK_ACCESS", 2));
    let cid = st
        .apps
        .iter()
        .find(|a| a.app_id == "page")
        .and_then(|a| a.instances.first())
        .and_then(|i| i.info.connection_id.clone());
    assert_eq!(cid.as_deref(), Some(r.connection_id.as_str()));
    drop(ws);
    drop(rt);
    hub.shutdown();
}

/// 关闭的能力（spec/hub-api.md 3.10）以 `HubError::Unsupported` 报告，可按类别区分；完整构建下这些调用成功。
/// 两种组合都要跑：`cargo test -p app-mcp-hub-uniffi` 与 `--no-default-features --features mobile`。
#[test]
fn disabled_features_report_unsupported() {
    let base = HubConfig { listen: Some("127.0.0.1:0".into()), enable_ipc: false, ..Default::default() };
    let expect = |present: bool, feature: &str, r: Result<(), HubError>| match (present, r) {
        (true, r) => r.expect("完整构建应支持"),
        (false, Err(HubError::Unsupported { detail })) => {
            assert!(detail.contains(&format!("`{feature}`")), "说明缺少 feature 名：{detail}")
        }
        (false, other) => panic!("缺少 {feature} 时应为 Unsupported：{other:?}"),
    };

    let mcp = AppMcpHub::start(HubConfig { mcp_http: true, ..base.clone() }).map(|h| h.shutdown());
    expect(hub::features::MCP_SERVER, "mcp-server", mcp);

    let up = UpstreamSpec { name: "up".into(), command: "true".into(), args: vec![], env: Default::default() };
    let upstream = AppMcpHub::start(HubConfig { upstreams: vec![up], ..base.clone() }).map(|h| h.shutdown());
    expect(hub::features::UPSTREAM, "upstream", upstream);

    let hub = AppMcpHub::start(base).expect("启动 Hub");
    let serve = wait(hub.serve_http("127.0.0.1:0".into(), false)).map(drop);
    expect(hub::features::MCP_SERVER, "mcp-server", serve);
    hub.shutdown();
}
