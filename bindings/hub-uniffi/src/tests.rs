//! 端到端单元测试：嵌入式 Hub（随机端口）+ 同进程 `app-mcp-native` App，经绑定层对象驱动。

use std::sync::mpsc;
use std::time::{Duration, Instant};

use app_mcp_native as native;
use futures::executor::block_on as wait;
use serde_json::{Value, json};

use super::{callbacks::Once, *};

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
        idempotency_key: None,
        priority: None,
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
        // principal / client_name 只在 MCP 出口发起的审批中出现
        assert_eq!((seen[0].principal.as_deref(), seen[0].client_name.as_deref()), (None, None));
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
    // Hub API 会话的 Agent 任务（spec/hub-api.md 3.6）
    assert_eq!(hub.status().expect("status").tasks, Some(vec![]), "调用前没有任务");
    assert_eq!(hub.status().expect("status").mcp_listen_streams, Some(0), "没有 MCP 客户端时 listen 流为 0");
    let out = wait(hub.call_tool(CallRequest { session: Some("s1".into()), ..req("notes.notes.add", json!({"text": "x"})) }));
    assert!(out.expect("调用").error.is_none());
    let tasks = hub.status().expect("status").tasks.expect("tasks");
    let task = tasks.iter().find(|t| t.caller == "api:s1").expect("api:s1 任务");
    assert_eq!((task.kind, task.inflight), (CallerKind::Api, 0));
    assert!(task.id.starts_with("task-"), "{}", task.id);
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

    // 宿主（独立 Hub App）启动前据此报告缺少的能力，而不是等 serve_mcp_fd 失败。
    let f = hub_features();
    assert_eq!(
        (f.mcp_server, f.upstream, f.schema_validation),
        (hub::features::MCP_SERVER, hub::features::UPSTREAM, hub::features::SCHEMA_VALIDATION)
    );
}

struct SubmitOrder;

impl native::ToolHandler for SubmitOrder {
    fn invoke(&self, call: native::CallHandle) {
        let _ = call.complete_with(native::CallResult {
            data_json: Some(r#"{"orderId":"o1"}"#.into()),
            status: native::ResultStatus::Pending,
            state_resource: Some("order.state".into()),
            summary: Some("已提交，等待付款".into()),
            annotations: Some(native::ContentAnnotations { priority: Some(0.5), ..Default::default() }),
            ..native::CallResult::default()
        });
    }
}

/// 第 14 / 19 项：限流与大小上限、工具注解 / outputSchema、结构化结果经绑定层可见。
#[test]
fn limits_annotations_and_structured_result() {
    let hub = AppMcpHub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        enable_ipc: false,
        limits: Some(LimitsConfig {
            tool_rate_per_minute: Some(1),
            tool_rate_burst: Some(1),
            max_arguments_bytes: Some(64),
            ..Default::default()
        }),
        output_validation: Some(OutputValidation::Reject),
        ..Default::default()
    })
    .expect("启动 Hub");
    let mut cfg = native::NativeConfig::new("orders", "订单");
    cfg.host_url = format!("ws://{}/app", hub.listen_addr().expect("监听地址"));
    let app = native::NativeClient::new(cfg, None).expect("App");
    let options = native::ToolOptions {
        annotations: Some(native::ToolAnnotations { idempotent_hint: Some(false), ..Default::default() }),
        output_schema_json: Some(r#"{"type":"object","properties":{"orderId":{"type":"string"}}}"#.into()),
        ..native::ToolOptions::default()
    };
    app.register_tool_with(native::ToolSpec::new("order.submit", "下单"), options, Arc::new(SubmitOrder))
        .expect("注册");
    app.register_tool(native::ToolSpec::new("echo", "回显"), Arc::new(AddNote)).expect("注册");
    app.start();

    let deadline = Instant::now() + Duration::from_secs(10);
    let tools = loop {
        let t = hub.tools(ToolFilter { apps: Some(vec!["orders".into()]), include_builtin: false, ..Default::default() });
        if t.len() == 2 && t.iter().all(|t| t.availability == Availability::Available) {
            break t;
        }
        assert!(Instant::now() < deadline, "App 未连上");
        std::thread::sleep(Duration::from_millis(20));
    };
    let submit = tools.iter().find(|t| t.tool == "order.submit").expect("order.submit");
    assert_eq!(submit.annotations.idempotent_hint, Some(false));
    assert_eq!(submit.annotations.read_only_hint, Some(false), "缺少的字段按 risk（write）推导");
    let schema: Value = serde_json::from_str(submit.output_schema_json.as_deref().expect("outputSchema")).unwrap();
    assert_eq!(schema["properties"]["orderId"]["type"], "string");
    let echo = tools.iter().find(|t| t.tool == "echo").expect("echo");
    assert_eq!(echo.output_schema_json, None);

    let out = wait(hub.call_tool(req("orders.order.submit", json!({})))).expect("调用");
    assert!(out.error.is_none(), "{out:?}");
    assert_eq!(out.status, ResultStatus::Pending);
    assert_eq!(out.state_resource.as_deref(), Some("app-mcp://orders/order.state"));
    assert_eq!(out.summary.as_deref(), Some("已提交，等待付款"));
    assert_eq!(out.annotations.and_then(|a| a.priority), Some(0.5));

    // 工具级突发 1：紧接着的第二次调用被限流
    let out = wait(hub.call_tool(req("orders.order.submit", json!({})))).expect("调用");
    assert_eq!(out.error.as_ref().map(|e| e.kind.as_str()), Some("RATE_LIMITED"), "{out:?}");
    // 参数超过 64 字节
    let out = wait(hub.call_tool(req("orders.echo", json!({ "text": "x".repeat(100) })))).expect("调用");
    assert_eq!(out.error.as_ref().map(|e| e.kind.as_str()), Some("PAYLOAD_TOO_LARGE"), "{out:?}");
    let plain = wait(hub.call_tool(req("orders.echo", json!({})))).expect("调用");
    // 超限的调用不消耗令牌；普通结果仍为 Done、无附加字段
    assert!(plain.error.is_none(), "{plain:?}");
    assert_eq!((plain.status, plain.summary.as_deref()), (ResultStatus::Done, None));

    let st = hub.status().expect("status");
    let limits = st.limits.expect("limits");
    assert_eq!((limits.tool_rate_per_minute, limits.tool_rate_burst), (Some(1), Some(1)));
    assert_eq!(limits.max_arguments_bytes, Some(64));
    assert_eq!(limits.app_rate_per_minute, Some(600));
    assert_eq!(st.output_validation, Some(OutputValidation::Reject));
    let orders = st.apps.iter().find(|a| a.app_id == "orders").expect("orders");
    assert_eq!((orders.rate_limited, orders.too_large), (1, 1));
    let decl = orders.tools.iter().find(|t| t.name == "order.submit").expect("声明");
    assert_eq!(decl.risk, Risk::Write);
    assert_eq!(decl.annotations.as_ref().and_then(|a| a.idempotent_hint), Some(false));
    assert_eq!(decl.effective.read_only_hint, Some(false));
    assert!(decl.output_schema);
    app.stop();
    hub.shutdown();
}

/// 调用的错误类别：工具层面的失败在 `CallOutcome.error`，名称无法解析时为 `HubError::Tool`。
fn call_error_kind(hub: &AppMcpHub, name: &str) -> (Option<String>, Option<Value>) {
    match wait(hub.call_tool(req(name, json!({ "text": "x" })))) {
        Ok(out) => (
            out.error.as_ref().map(|e| e.kind.clone()),
            out.error.and_then(|e| e.details_json).and_then(|d| serde_json::from_str(&d).ok()),
        ),
        Err(HubError::Tool { kind, details_json, .. }) => {
            (Some(kind), details_json.and_then(|d| serde_json::from_str(&d).ok()))
        }
        Err(e) => panic!("意外错误：{e:?}"),
    }
}

/// 报告两次进度（间隔超过 Hub 默认合并间隔 250 ms）后完成。
struct SlowWithProgress;

impl native::ToolHandler for SlowWithProgress {
    fn invoke(&self, call: native::CallHandle) {
        std::thread::spawn(move || {
            let _ = call.report_progress(1.0, Some(2.0), Some("第一步"));
            std::thread::sleep(Duration::from_millis(350));
            let _ = call.report_progress(2.0, Some(2.0), None);
            std::thread::sleep(Duration::from_millis(350));
            let _ = call.complete(Some(r#"{"done":true}"#), vec![]);
        });
    }
}

struct Progress(Mutex<Vec<ProgressUpdate>>);

impl ProgressListener for Progress {
    fn on_progress(&self, update: ProgressUpdate) {
        lock(&self.0).push(update);
    }
}

/// 回调 panic（外部未预期异常）不影响后续进度与调用结果。
struct PanickingProgress(Mutex<usize>);

impl ProgressListener for PanickingProgress {
    fn on_progress(&self, _update: ProgressUpdate) {
        *lock(&self.0) += 1;
        panic!("UI 崩溃");
    }
}

/// 第 16 项 O2：Hub 绑定的进度回调——结果返回前收到合并后的全部进度；普通 `call_tool` 不受影响。
#[test]
fn call_tool_with_progress_delivers_updates() {
    let hub = start_hub(None);
    let (tx, rx) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx)))));
    let mut cfg = native::NativeConfig::new("slow", "慢");
    cfg.host_url = format!("ws://{}/app", hub.listen_addr().expect("监听地址"));
    let app = native::NativeClient::new(cfg, None).expect("App");
    let mut spec = native::ToolSpec::new("work", "长任务");
    spec.risk = native::Risk::Read;
    app.register_tool(spec, Arc::new(SlowWithProgress)).expect("注册");
    app.start();
    wait_for(&rx, |e| matches!(e, HubEvent::AppConnected { app_id, .. } if app_id == "slow"));
    wait_for(&rx, |e| matches!(e, HubEvent::ToolsChanged));

    let listener = Arc::new(Progress(Mutex::new(Vec::new())));
    let out = wait(hub.call_tool_with_progress(req("slow.work", json!({})), listener.clone())).expect("调用");
    assert_eq!(out.error, None);
    assert_eq!(out.data_json.as_deref().map(|d| serde_json::from_str::<Value>(d).unwrap_or_default()), Some(json!({"done": true})));
    let got = lock(&listener.0).clone();
    assert_eq!(
        got,
        vec![
            ProgressUpdate { progress: 1.0, total: Some(2.0), message: Some("第一步".into()) },
            ProgressUpdate { progress: 2.0, total: Some(2.0), message: None },
        ]
    );

    let panicking = Arc::new(PanickingProgress(Mutex::new(0)));
    let out = wait(hub.call_tool_with_progress(req("slow.work", json!({})), panicking.clone())).expect("调用");
    assert_eq!(out.error, None, "回调异常不影响结果");
    assert_eq!(*lock(&panicking.0), 2, "回调异常后继续接收");

    let out = wait(hub.call_tool(req("slow.work", json!({})))).expect("调用");
    assert_eq!(out.error, None);
    app.stop();
    hub.shutdown();
}

/// 回显 Agent 给出的幂等键（spec/hub-api.md 3.15）。
struct KeyEcho;

impl native::ToolHandler for KeyEcho {
    fn invoke(&self, call: native::CallHandle) {
        let data = json!({ "key": call.idempotency_key() });
        let _ = call.complete(Some(&data.to_string()), vec![]);
    }
}

/// `HubTool.surface` / `page`、`CallRequest.idempotency_key` 原样转交（不合法 → INVALID_INPUT）、`routed_to` 未改调时为空、
/// 内置工具 apps.activate / apps.release 与（有页面目录时）apps.page / apps.navigate。
#[test]
fn surface_page_idempotency_key_and_builtins() {
    let hub = AppMcpHub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        enable_ipc: false,
        navigate_timeout_ms: Some(800),
        ..Default::default()
    })
    .expect("启动 Hub");
    let mut cfg = native::NativeConfig::new("shop", "商店");
    cfg.host_url = format!("ws://{}/app", hub.listen_addr().expect("监听地址"));
    let app = native::NativeClient::new(cfg, None).expect("App");
    let view = native::ToolOptions {
        surface: native::ToolSurface::View,
        page: Some("cart".into()),
        ..native::ToolOptions::default()
    };
    app.register_tool_with(native::ToolSpec::new("cart.checkout", "结算"), view, Arc::new(KeyEcho)).expect("注册");
    app.register_tool(native::ToolSpec::new("order.submit", "下单"), Arc::new(KeyEcho)).expect("注册");
    app.start();

    let deadline = Instant::now() + Duration::from_secs(10);
    let tools = loop {
        let t = hub.tools(ToolFilter { apps: Some(vec!["shop".into()]), include_builtin: false, ..Default::default() });
        if t.len() == 2 && t.iter().all(|t| t.availability == Availability::Available) {
            break t;
        }
        assert!(Instant::now() < deadline, "App 未连上");
        std::thread::sleep(Duration::from_millis(20));
    };
    let checkout = tools.iter().find(|t| t.tool == "cart.checkout").expect("view 工具");
    assert_eq!((checkout.surface, checkout.page.as_deref()), (Some(ToolSurface::View), Some("cart")));
    let submit = tools.iter().find(|t| t.tool == "order.submit").expect("app 工具");
    assert_eq!((submit.surface, submit.page.as_deref()), (Some(ToolSurface::App), None));

    let all = hub.tools(ToolFilter::default());
    for n in ["apps.list", "apps.select", "apps.overview", "apps.activate", "apps.release", "apps.page", "apps.navigate"] {
        let t = all.iter().find(|t| t.name == n).unwrap_or_else(|| panic!("缺少内置工具 {n}"));
        assert_eq!((t.surface, t.page.as_deref()), (None, None), "内置工具不带 surface / page");
    }

    let mut r = req("shop.order.submit", json!({}));
    r.idempotency_key = Some("order-7".into());
    let out = wait(hub.call_tool(r.clone())).expect("调用");
    assert!(out.error.is_none(), "{out:?}");
    let data: Value = serde_json::from_str(out.data_json.as_deref().unwrap_or("null")).unwrap_or_default();
    assert_eq!(data["key"], "order-7");
    assert_eq!(out.routed_to, None, "未改调");
    r.idempotency_key = Some(String::new());
    let out = wait(hub.call_tool(r)).expect("调用");
    assert_eq!(out.error.as_ref().map(|e| e.kind.as_str()), Some("INVALID_INPUT"), "{out:?}");
    app.stop();
    hub.shutdown();
}

/// 宿主名字服务（spec/naming.md 4.2）：socketpair 模拟 Android 的 `bindService` + `open()`——"App 进程"是同进程的
/// `NativeClient`，拨号时把一端交给它的 `accept_channel`，另一端以 fd 交回 Hub。
#[cfg(unix)]
#[derive(Default)]
struct FakeNameService {
    app: Mutex<Option<native::NativeClient>>,
    dials: Mutex<u64>,
    released: Mutex<Vec<u64>>,
}

#[cfg(unix)]
const FD_APP: &str = "fd-notes";

#[cfg(unix)]
fn fd_app_manifest(app_id: &str) -> String {
    json!({"manifestVersion": 1, "appId": app_id, "name": "fd 笔记",
        "tools": [{"name": "echo", "description": "原样返回", "inputSchema": {"type": "object"}}]})
    .to_string()
}

#[cfg(unix)]
impl HubNameService for FakeNameService {
    fn discover(&self) -> Vec<NamedApp> {
        vec![
            NamedApp {
                app_id: FD_APP.into(),
                activatable: true,
                running: false,
                detail: "dev.example.notes/dev.appmcp.android.ToolsService".into(),
                manifest_json: Some(fd_app_manifest(FD_APP)),
            },
            // 不合法的 appId 与清单不一致的条目被忽略 / 不带清单。
            NamedApp { app_id: "Bad".into(), activatable: true, running: false, detail: String::new(), manifest_json: None },
        ]
    }

    fn dial(&self, app_id: String, _timeout_ms: u64) -> DialOutcome {
        use std::os::fd::IntoRawFd;
        if app_id == "walled" {
            return DialOutcome::Blocked {
                package_name: "dev.example.walled".into(),
                app_label: "围墙笔记".into(),
                message: "系统拒绝绑定 ComponentInfo{dev.example.walled/x}".into(),
            };
        }
        if app_id != FD_APP {
            return DialOutcome::Failed { code: "HUB_NOT_TRUSTED".into(), message: "App 拒绝了该 Hub".into() };
        }
        let (app_end, hub_end) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let app = lock(&self.app)
            .get_or_insert_with(|| {
                let mut cfg = native::NativeConfig::new(FD_APP, "fd 笔记");
                cfg.lifecycle.mode = native::LifecycleMode::OnDemand;
                cfg.host_url = format!("unix:{}", std::env::temp_dir().join("app-mcp-no-hub.sock").display());
                let app = native::NativeClient::new(cfg, None).expect("App");
                app.register_tool(native::ToolSpec::new("echo", "原样返回"), Arc::new(AddNote)).expect("注册");
                app.start();
                app
            })
            .clone();
        if let Err(e) = app.accept_channel(app_end) {
            return DialOutcome::Failed { code: "CHANNEL_LIMIT".into(), message: e.to_string() };
        }
        let mut dials = lock(&self.dials);
        *dials += 1;
        DialOutcome::Channel {
            fd: hub_end.into_raw_fd(),
            lease: *dials,
            peer_uid: Some(app_mcp_protocol::endpoint::current_uid()),
        }
    }

    fn release(&self, lease: u64) {
        lock(&self.released).push(lease);
    }
}

#[cfg(unix)]
fn eventually(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "等待超时：{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn name_service_discover_dial_release_and_events() {
    let service = Arc::new(FakeNameService::default());
    let hub = AppMcpHub::start_with_name_service(
        HubConfig {
            enable_listen: false,
            enable_ipc: false,
            channel_grace_ms: Some(200),
            lease_ttl_ms: Some(0),
            list_changed_debounce_ms: Some(10),
            ..Default::default()
        },
        "android".into(),
        service.clone(),
    )
    .expect("启动 Hub");
    let names = |hub: &AppMcpHub| hub.tools(ToolFilter::default()).into_iter().map(|t| t.name).collect::<Vec<_>>();

    // 发现只读元数据：清单中的工具列出，未拨号。
    eventually("按清单列出", || names(&hub).contains(&format!("{FD_APP}.echo")));
    assert_eq!(*lock(&service.dials), 0);
    assert!(!hub.apps().iter().any(|a| a.app_id == "Bad"));

    // 调用 → 拨号 → 宽限后释放一次。
    let out = wait(hub.call_tool(req(&format!("{FD_APP}.echo"), json!({"text": "hi"})))).expect("调用");
    assert!(out.error.is_none(), "{out:?}");
    assert_eq!(serde_json::from_str::<Value>(out.data_json.as_deref().unwrap_or("null")).unwrap()["saved"], "hi");
    eventually("宽限后释放租约", || *lock(&service.released) == vec![1]);
    let app = lock(&service.app).clone().expect("App 已创建");
    eventually("App 回到休眠", || app.state().status == native::StateStatus::Dormant);

    // 宿主推送安装 / 卸载；宿主错误码进入 details.code。
    hub.name_service_installed(NamedApp {
        app_id: "late".into(),
        activatable: true,
        running: false,
        detail: String::new(),
        manifest_json: Some(fd_app_manifest("late")),
    });
    eventually("安装后列出", || names(&hub).contains(&"late.echo".to_owned()));
    let (kind, details) = call_error_kind(&hub, "late.echo");
    assert_eq!(kind.as_deref(), Some("LAUNCH_FAILED"));
    assert_eq!(details.as_ref().and_then(|d| d["code"].as_str()), Some("HUB_NOT_TRUSTED"), "{details:?}");
    hub.name_service_removed("late".into());
    eventually("卸载后移除", || !names(&hub).contains(&"late.echo".to_owned()));

    // 系统拦截已安装的 App：USER_ACTION_REQUIRED（os-permission），details 带包名与应用名，消息不含组件名。
    hub.name_service_installed(NamedApp {
        app_id: "walled".into(),
        activatable: true,
        running: false,
        detail: String::new(),
        manifest_json: Some(fd_app_manifest("walled")),
    });
    eventually("walled 列出", || names(&hub).contains(&"walled.echo".to_owned()));
    let out = wait(hub.call_tool(req("walled.echo", json!({})))).expect("调用");
    let err = out.error.expect("应失败");
    assert_eq!(err.kind, "USER_ACTION_REQUIRED");
    assert_eq!(err.message, "系统阻止了 AppWire Hub 启动『围墙笔记』。请在系统设置中允许『围墙笔记』自启动 / 关联启动后重试。");
    let details: Value = serde_json::from_str(err.details_json.as_deref().unwrap_or("null")).expect("details");
    assert_eq!(details["reason"], "os-permission");
    assert_eq!(details["packageName"], "dev.example.walled");
    assert_eq!(details["appName"], "围墙笔记");

    hub.shutdown();
    app.stop();
}

/// 系统 IPC 交来的 fd 上的 MCP（TASKS 4g d）：每行一条 JSON-RPC；对端关闭后 `serve_mcp_fd` 返回。
#[cfg(unix)]
#[test]
fn mcp_over_fd() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::fd::IntoRawFd;
    let hub = start_hub(None);
    let _app = start_app(&hub);
    eventually("App 已连接", || hub.apps().iter().any(|a| a.app_id == "notes" && a.connected));

    let (agent, hub_end) = std::os::unix::net::UnixStream::pair().expect("socketpair");
    let server = {
        let hub = hub.clone();
        std::thread::spawn(move || wait(hub.serve_mcp_fd(hub_end.into_raw_fd())))
    };
    if !hub::features::MCP_SERVER {
        let r = server.join().expect("线程");
        assert!(matches!(r, Err(HubError::Unsupported { ref detail }) if detail.contains("`mcp-server`")), "{r:?}");
        hub.shutdown();
        return;
    }
    agent.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
    let mut reader = BufReader::new(agent.try_clone().expect("clone"));
    let mut writer = agent;
    let mut rpc = |msg: Value| -> Option<Value> {
        writeln!(writer, "{msg}").expect("写");
        let id = msg.get("id")?.clone();
        // @why Hub 会在回复之间推送通知（如 tools/list_changed），按 id 取对应回复。
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("读");
            let v: Value = serde_json::from_str(&line).expect("json");
            if v.get("id") == Some(&id) {
                return Some(v);
            }
        }
    };
    let init = rpc(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "agent-test", "version": "0"}}}))
    .expect("initialize 回复");
    assert!(init["result"]["serverInfo"].is_object(), "{init}");
    rpc(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let list = rpc(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).expect("tools/list 回复");
    let tools = list["result"]["tools"].as_array().cloned().unwrap_or_default();
    assert!(tools.iter().any(|t| t["name"].as_str().is_some_and(|n| n.contains("notes.add"))), "{list}");
    let called = rpc(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "notes.notes.add", "arguments": {"text": "经 fd"}}}))
    .expect("tools/call 回复");
    assert_eq!(called["result"]["isError"], false, "{called}");
    drop(writer);
    drop(reader);
    server.join().expect("线程").expect("对端关闭后正常返回");
    hub.shutdown();
}

mod agents;
mod app_events;
mod locks;
mod policy;
