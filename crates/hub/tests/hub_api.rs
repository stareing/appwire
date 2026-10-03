//! Hub API 集成测试：tokio-tungstenite 客户端扮演 App 端 SDK，连接嵌入式 Hub。
//!
//! 覆盖：格式导出 + dispatch 往返、名称编码冲突、审批、会话维度的总览附带、事件、配对钩子、取消。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use app_mcp_hub::{
    ApprovalHandler, ApprovalPolicy, ApprovalRequest, CallRequest, ErrorKind, Hub, HubConfig,
    HubEvent, PairingHandler, PairingRequest, Risk, ToolExposure, ToolFilter, ToolFormat,
    async_trait,
};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as Ws;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const T: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// 假 SDK
// ---------------------------------------------------------------------------

struct Spec {
    app_id: &'static str,
    instance_id: &'static str,
    tools: Vec<Value>,
    overview: Option<Value>,
    origin: Option<&'static str>,
}

impl Spec {
    fn new(app_id: &'static str, instance_id: &'static str) -> Self {
        Self {
            app_id,
            instance_id,
            tools: vec![
                tool("echo", "read", json!({"text": {"type": "string"}}), &["text"]),
                tool("cart.checkout", "payment", json!({}), &[]),
                tool("slow", "write", json!({}), &[]),
            ],
            overview: Some(json!({"summary": "测试商城", "body": "可以回显。"})),
            origin: Some("http://localhost:5173"),
        }
    }
}

fn tool(name: &str, risk: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name, "description": format!("{name} 工具"), "risk": risk,
        "inputSchema": {"type": "object", "properties": props, "required": required}
    })
}

struct Sdk {
    /// Hub 发来的全部消息。
    events: mpsc::UnboundedReceiver<Value>,
    hello: Value,
    /// `Some` = 发送一条消息，`None` = 关闭连接。
    cmd: mpsc::UnboundedSender<Option<Value>>,
}

impl Sdk {
    fn notify(&self, method: &str, params: Value) {
        let _ = self
            .cmd
            .send(Some(json!({"jsonrpc": "2.0", "method": method, "params": params})));
    }

    fn close(&self) {
        let _ = self.cmd.send(None);
    }

    async fn expect(&mut self, method: &str) -> Value {
        timeout(T, async {
            loop {
                let v = self.events.recv().await.expect("sdk closed");
                if v["method"] == method {
                    return v;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("没有收到 {method}"))
    }
}

async fn connect(hub: &Hub, spec: Spec) -> Sdk {
    let url = format!("ws://{}/app", hub.listen_addr().expect("listen addr"));
    let mut req = url.into_client_request().unwrap();
    if let Some(o) = spec.origin {
        req.headers_mut().insert("Origin", o.parse().unwrap());
    }
    let (ws, _) = tokio_tungstenite::connect_async(req).await.expect("ws");
    let (mut sink, mut stream) = ws.split();
    let mut hello = json!({
        "appId": spec.app_id, "appName": "测试商城", "protocolVersion": "1", "sdkVersion": "0",
        "clientKind": "web", "instanceId": spec.instance_id
    });
    if let Some(o) = &spec.overview {
        hello["overview"] = o.clone();
    }
    sink.send(Ws::text(
        json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": hello}).to_string(),
    ))
    .await
    .unwrap();
    let hello_result: Value = loop {
        let Some(Ok(Ws::Text(t))) = timeout(T, stream.next()).await.expect("hello") else {
            panic!("握手时连接关闭")
        };
        let v: Value = serde_json::from_str(t.as_str()).unwrap();
        if v["id"] == 1 {
            break v["result"].clone();
        }
    };
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Option<Value>>();
    let note = |m: &str, p: Value| Ws::text(json!({"jsonrpc": "2.0", "method": m, "params": p}).to_string());
    let sync = [
        note("tools/sync", json!({"tools": spec.tools})),
        note("resources/sync", json!({"resources": [{"name": "cart.state", "description": "购物车"}]})),
        note("app/visibility", json!({"visibility": "visible", "focused": true})),
        note("app/ready", json!({})),
    ];
    let mut synced = false;
    if hello_result["status"] == "paired" {
        for m in sync.clone() {
            sink.send(m).await.unwrap();
        }
        synced = true;
    }
    let instance_id = spec.instance_id;
    tokio::spawn(async move {
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(Some(v)) => { let _ = sink.send(Ws::text(v.to_string())).await; }
                    _ => { let _ = sink.close().await; break; }
                },
                msg = stream.next() => {
                    let Some(Ok(Ws::Text(t))) = msg else { break };
                    let v: Value = serde_json::from_str(t.as_str()).unwrap();
                    let _ = ev_tx.send(v.clone());
                    if v["method"] == "app/pairingResult" && v["params"]["status"] == "paired" && !synced {
                        for m in sync.clone() {
                            let _ = sink.send(m).await;
                        }
                        synced = true;
                    }
                    let (Some(id), Some(method)) = (v.get("id").cloned(), v["method"].as_str()) else { continue };
                    let reply = match method {
                        "tools/invoke" => match v["params"]["name"].as_str().unwrap_or_default() {
                            "slow" => None,
                            "fail" => Some(json!({"error": {"code": -32006, "message": "坏了", "data": {"kind": "HANDLER_ERROR"}}})),
                            name => Some(json!({"result": {
                                "data": {"tool": name, "instanceId": instance_id, "args": v["params"]["arguments"]},
                                "stateHints": ["cart.state"]
                            }})),
                        },
                        "resources/read" => Some(json!({"result": {"contents": {"items": [1]}}})),
                        _ => Some(json!({"result": {}})),
                    };
                    if let Some(mut r) = reply {
                        r["jsonrpc"] = json!("2.0");
                        r["id"] = id;
                        let _ = sink.send(Ws::text(r.to_string())).await;
                    }
                }
            }
        }
    });
    Sdk {
        events: ev_rx,
        hello: hello_result,
        cmd: cmd_tx,
    }
}

fn config() -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        response_timeout: Duration::from_secs(3),
        ..Default::default()
    }
}

async fn wait_tool(hub: &Hub, name: &str) {
    // 显式指定 App：不受渐进暴露影响
    let filter = ToolFilter {
        apps: name.split_once('.').map(|(a, _)| vec![a.to_owned()]),
        ..Default::default()
    };
    timeout(T, async {
        while !hub
            .tools(&filter)
            .iter()
            .any(|t| t.name == name && t.availability == app_mcp_hub::Availability::Available)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("工具 {name} 没有出现"));
}

async fn next_event(rx: &mut broadcast::Receiver<HubEvent>, pred: impl Fn(&HubEvent) -> bool) -> HubEvent {
    timeout(T, async {
        loop {
            let ev = rx.recv().await.expect("event");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
    .expect("没有等到事件")
}

// ---------------------------------------------------------------------------
// 格式导出与分派
// ---------------------------------------------------------------------------

/// 构造某格式的一次工具调用。
fn tool_call(format: ToolFormat, name: &str, args: Value) -> Value {
    match format {
        ToolFormat::Mcp => json!({"name": name, "arguments": args}),
        ToolFormat::OpenAiChat => {
            json!({"id": "call_1", "type": "function", "function": {"name": name, "arguments": args.to_string()}})
        }
        ToolFormat::OpenAiResponses => {
            json!({"type": "function_call", "call_id": "fc_1", "name": name, "arguments": args.to_string()})
        }
        ToolFormat::Anthropic => json!({"type": "tool_use", "id": "toolu_1", "name": name, "input": args}),
        ToolFormat::Gemini => json!({"functionCall": {"name": name, "args": args}}),
    }
}

/// 从导出结果中取出所有工具名。
fn exported_names(format: ToolFormat, v: &Value) -> Vec<String> {
    let items = match format {
        ToolFormat::Gemini => v["functionDeclarations"].as_array().cloned().unwrap_or_default(),
        _ => v.as_array().cloned().unwrap_or_default(),
    };
    items
        .iter()
        .map(|t| {
            t.get("function")
                .unwrap_or(t)
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// 取出结果文本与是否为错误。
fn result_text(format: ToolFormat, v: &Value) -> (String, bool) {
    match format {
        ToolFormat::Mcp => {
            let text = v["content"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["text"].as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
                .join("\n\n");
            (text, v["isError"] == true)
        }
        ToolFormat::OpenAiChat => {
            assert_eq!(v["role"], "tool");
            assert_eq!(v["tool_call_id"], "call_1");
            (v["content"].as_str().unwrap().to_owned(), false)
        }
        ToolFormat::OpenAiResponses => {
            assert_eq!(v["type"], "function_call_output");
            assert_eq!(v["call_id"], "fc_1");
            (v["output"].as_str().unwrap().to_owned(), false)
        }
        ToolFormat::Anthropic => {
            assert_eq!(v["type"], "tool_result");
            assert_eq!(v["tool_use_id"], "toolu_1");
            (v["content"].as_str().unwrap().to_owned(), v["is_error"] == true)
        }
        ToolFormat::Gemini => {
            let r = &v["functionResponse"]["response"];
            match r.get("error") {
                Some(e) => (e.as_str().unwrap().to_owned(), true),
                None => (r["output"].as_str().unwrap().to_owned(), false),
            }
        }
    }
}

const FORMATS: [ToolFormat; 5] = [
    ToolFormat::Mcp,
    ToolFormat::OpenAiChat,
    ToolFormat::OpenAiResponses,
    ToolFormat::Anthropic,
    ToolFormat::Gemini,
];

#[tokio::test]
async fn export_and_dispatch_roundtrip_all_formats() {
    let hub = Hub::start(config()).await.unwrap();
    let _sdk = connect(&hub, Spec::new("shop", "i1")).await;
    wait_tool(&hub, "shop.echo").await;

    for format in FORMATS {
        let exported = hub.export_tools(format, &ToolFilter::default());
        let names = exported_names(format, &exported);
        assert!(names.contains(&"shop__echo".to_owned()), "{format:?}: {names:?}");
        assert!(names.contains(&"apps__list".to_owned()), "{format:?}");
        assert!(names.contains(&"shop__cart__checkout".to_owned()));
        for n in &names {
            assert!(n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'), "{n}");
        }
        // 风险标记
        let s = exported.to_string();
        assert!(s.contains("（风险：payment）"), "{format:?}");

        // 每种格式用独立会话，首次调用附带总览
        let session = format!("{format:?}");
        let r = hub
            .dispatch_in_session(format, tool_call(format, "shop__echo", json!({"text": "你好"})), Some(&session))
            .await;
        let (text, is_error) = result_text(format, &r);
        assert!(!is_error, "{format:?}: {r}");
        assert!(text.contains("<app-overview app=\"shop\""), "{format:?}: {text}");
        assert!(text.contains("\"text\":\"你好\""), "{format:?}: {text}");
        assert!(text.contains("app-mcp://shop/cart.state"), "{format:?}: {text}");

        // 第二次不再附带；全名也能分派
        let r = hub
            .dispatch_in_session(format, tool_call(format, "shop.echo", json!({"text": "再见"})), Some(&session))
            .await;
        let (text, _) = result_text(format, &r);
        assert!(!text.contains("<app-overview"), "{format:?}: {text}");
        assert!(text.contains("再见"));

        // 参数不合法 → 错误结果（Hub 侧校验，feature schema-validation）
        #[cfg(feature = "schema-validation")]
        {
            let r = hub
                .dispatch_in_session(format, tool_call(format, "shop__echo", json!({})), Some(&session))
                .await;
            let (text, is_error) = result_text(format, &r);
            assert!(text.starts_with("INVALID_INPUT: "), "{format:?}: {text}");
            if matches!(format, ToolFormat::Mcp | ToolFormat::Anthropic | ToolFormat::Gemini) {
                assert!(is_error, "{format:?}");
            }
        }

        // 未知工具
        let r = hub
            .dispatch_in_session(format, tool_call(format, "nope__x", json!({})), Some(&session))
            .await;
        let (text, _) = result_text(format, &r);
        assert!(text.starts_with("TOOL_NOT_FOUND: "), "{format:?}: {text}");
    }

    // 内置工具经 dispatch
    let r = hub
        .dispatch(ToolFormat::Anthropic, tool_call(ToolFormat::Anthropic, "apps__list", json!({})))
        .await;
    let (text, _) = result_text(ToolFormat::Anthropic, &r);
    assert!(text.contains("\"appId\":\"shop\""), "{text}");
}

#[tokio::test]
async fn gemini_export_strips_unsupported_keywords() {
    let hub = Hub::start(config()).await.unwrap();
    let mut spec = Spec::new("shop", "i1");
    spec.tools = vec![json!({
        "name": "find", "description": "查找", "risk": "read",
        "inputSchema": {"type": "object", "additionalProperties": false,
            "properties": {"q": {"type": "string"}}, "required": ["q"]}
    })];
    let _sdk = connect(&hub, spec).await;
    wait_tool(&hub, "shop.find").await;
    let filter = ToolFilter {
        apps: Some(vec!["shop".into()]),
        include_builtin: false,
        ..Default::default()
    };
    let g = hub.export_tools(ToolFormat::Gemini, &filter);
    let decl = &g["functionDeclarations"][0];
    assert_eq!(decl["name"], "shop__find");
    assert!(decl["parameters"].get("additionalProperties").is_none());
    assert_eq!(decl["parameters"]["properties"]["q"]["type"], "string");
    let a = hub.export_tools(ToolFormat::Anthropic, &filter);
    assert_eq!(a.as_array().unwrap().len(), 1);
    assert_eq!(a[0]["input_schema"]["additionalProperties"], false);
}

#[tokio::test]
async fn name_encoding_conflict_roundtrip() {
    let hub = Hub::start(config()).await.unwrap();
    let mut spec = Spec::new("shop", "i1");
    spec.tools = vec![
        tool("x.y", "read", json!({}), &[]),
        tool("x__y", "read", json!({}), &[]),
    ];
    let _sdk = connect(&hub, spec).await;
    wait_tool(&hub, "shop.x__y").await;
    wait_tool(&hub, "shop.x.y").await;
    let filter = ToolFilter {
        include_builtin: false,
        ..Default::default()
    };
    let exported = hub.export_tools(ToolFormat::OpenAiChat, &filter);
    let names = exported_names(ToolFormat::OpenAiChat, &exported);
    assert_eq!(names.len(), 2);
    assert_ne!(names[0], names[1]);
    for n in &names {
        assert!(n.starts_with("shop__x__y_") && n.len() == "shop__x__y_".len() + 4, "{n}");
    }
    // 每个导出名都分派到正确的工具
    let tools = hub.tools(&filter);
    for (t, exported_name) in tools.iter().zip(&names) {
        let r = hub
            .dispatch(ToolFormat::Anthropic, tool_call(ToolFormat::Anthropic, exported_name, json!({})))
            .await;
        let (text, is_error) = result_text(ToolFormat::Anthropic, &r);
        assert!(!is_error, "{text}");
        assert!(text.contains(&format!("\"tool\":\"{}\"", t.tool)), "{exported_name} → {text}");
    }
}

// ---------------------------------------------------------------------------
// call_tool / 会话 / 取消
// ---------------------------------------------------------------------------

#[tokio::test]
async fn overview_attached_once_per_session() {
    let hub = Hub::start(config()).await.unwrap();
    let _sdk = connect(&hub, Spec::new("shop", "i1")).await;
    wait_tool(&hub, "shop.echo").await;
    let call = |session: Option<&str>| {
        let mut r = CallRequest::new("shop.echo", json!({"text": "x"}));
        r.session = session.map(str::to_owned);
        hub.call_tool(r)
    };
    for session in [Some("a"), Some("b"), None] {
        let o = call(session).await.unwrap();
        let ov = o.overview.expect("首次附带");
        assert_eq!(ov.app_id, "shop");
        assert_eq!(ov.summary, "测试商城");
        assert!(ov.text.contains("<app-overview"));
        assert_eq!(o.state_hints, vec!["cart.state".to_owned()]);
        assert_eq!(o.instance_id.as_deref(), Some("i1"));
        assert_eq!(o.result.as_ref().unwrap()["args"]["text"], "x");
        assert!(call(session).await.unwrap().overview.is_none());
    }
    hub.reset_session(Some("a"));
    assert!(call(Some("a")).await.unwrap().overview.is_some());
    assert_eq!(hub.overview("shop").unwrap().summary, "测试商城");
}

#[tokio::test]
async fn call_errors_and_builtins() {
    let hub = Hub::start(config()).await.unwrap();
    let _sdk = connect(&hub, Spec::new("shop", "i1")).await;
    wait_tool(&hub, "shop.echo").await;
    // 名称无法解析 → Err
    let e = hub.call_tool(CallRequest::new("nope.x", json!({}))).await.unwrap_err();
    assert_eq!(e.kind(), ErrorKind::ToolNotFound);
    let e = hub.call_tool(CallRequest::new("noformat", json!({}))).await.unwrap_err();
    assert_eq!(e.kind(), ErrorKind::ToolNotFound);
    // 工具层失败 → Ok + result Err
    let o = hub.call_tool(CallRequest::new("shop.missing", json!({}))).await.unwrap();
    assert_eq!(o.result.unwrap_err().kind, ErrorKind::ToolNotFound);
    #[cfg(feature = "schema-validation")]
    {
        let o = hub.call_tool(CallRequest::new("shop.echo", json!({"text": 1}))).await.unwrap();
        assert_eq!(o.result.unwrap_err().kind, ErrorKind::InvalidInput);
    }
    // 指定不存在的实例
    let mut r = CallRequest::new("shop.echo", json!({"text": "x"}));
    r.instance_id = Some("zzz".into());
    let o = hub.call_tool(r).await.unwrap();
    assert_eq!(o.result.unwrap_err().kind, ErrorKind::ToolNotFound);
    // 内置
    let o = hub.call_tool(CallRequest::new("apps.list", Value::Null)).await.unwrap();
    assert_eq!(o.result.unwrap()["apps"][0]["appId"], "shop");
    // 超时
    let mut r = CallRequest::new("shop.slow", json!({}));
    r.timeout = Some(Duration::from_millis(200));
    let o = hub.call_tool(r).await.unwrap();
    assert_eq!(o.result.unwrap_err().kind, ErrorKind::Timeout);

    // 查询
    let apps = hub.apps();
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].instances[0].instance_id, "i1");
    assert!(apps[0].instances[0].focused);
    hub.select_instance("shop", Some("i1"));
    assert_eq!(hub.apps()[0].selected_instance.as_deref(), Some("i1"));
    let res = hub.resources();
    assert_eq!(res[0].uri, "app-mcp://shop/cart.state");
    let c = hub.read_resource("app-mcp://shop/cart.state").await.unwrap();
    assert_eq!(c.text.as_deref(), Some(r#"{"items":[1]}"#));
    assert!(hub.read_resource("app-mcp://nope/x").await.is_err());
    let filter = ToolFilter {
        max_risk: Some(Risk::Write),
        include_builtin: false,
        ..Default::default()
    };
    let names: Vec<String> = hub.tools(&filter).into_iter().map(|t| t.name).collect();
    assert_eq!(names, vec!["shop.echo".to_owned(), "shop.slow".to_owned()]);
}

#[tokio::test]
async fn cancel_call_sends_cancel_to_app() {
    let hub = Arc::new(Hub::start(config()).await.unwrap());
    let mut sdk = connect(&hub, Spec::new("shop", "i1")).await;
    wait_tool(&hub, "shop.slow").await;
    let mut r = CallRequest::new("shop.slow", json!({}));
    r.call_id = Some("my-call".into());
    let h = hub.clone();
    let task = tokio::spawn(async move { h.call_tool(r).await });
    let inv = sdk.expect("tools/invoke").await;
    assert_eq!(inv["params"]["callId"], "my-call");
    hub.cancel_call("my-call");
    let o = timeout(T, task).await.unwrap().unwrap().unwrap();
    assert_eq!(o.result.unwrap_err().kind, ErrorKind::Cancelled);
    let c = sdk.expect("tools/cancel").await;
    assert_eq!(c["params"]["callId"], "my-call");
}

// ---------------------------------------------------------------------------
// 审批
// ---------------------------------------------------------------------------

struct Approver {
    answer: Option<bool>, // None = 不回答（超时）
    asked: AtomicUsize,
    last: std::sync::Mutex<Option<ApprovalRequest>>,
}

#[async_trait]
impl ApprovalHandler for Approver {
    async fn approve(&self, req: ApprovalRequest) -> bool {
        self.asked.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() = Some(req);
        match self.answer {
            Some(a) => a,
            None => std::future::pending().await,
        }
    }
}

fn approver(answer: Option<bool>) -> Arc<Approver> {
    Arc::new(Approver {
        answer,
        asked: AtomicUsize::new(0),
        last: std::sync::Mutex::new(None),
    })
}

#[tokio::test]
async fn approval_policy() {
    let hub = Hub::start(HubConfig {
        approval: ApprovalPolicy {
            require_at_or_above: Some(Risk::Payment),
            timeout: Some(Duration::from_millis(200)),
        },
        ..config()
    })
    .await
    .unwrap();
    let _sdk = connect(&hub, Spec::new("shop", "i1")).await;
    wait_tool(&hub, "shop.cart.checkout").await;

    // 未设置处理器：需要审批的调用被拒绝
    let o = hub.call_tool(CallRequest::new("shop.cart.checkout", json!({}))).await.unwrap();
    assert_eq!(o.result.unwrap_err().kind, ErrorKind::UserRejected);

    // 拒绝
    let a = approver(Some(false));
    hub.set_approval_handler(a.clone());
    let mut r = CallRequest::new("shop.cart.checkout", json!({"n": 1}));
    r.session = Some("s1".into());
    let o = hub.call_tool(r).await.unwrap();
    assert_eq!(o.result.unwrap_err().kind, ErrorKind::UserRejected);
    let req = a.last.lock().unwrap().clone().unwrap();
    assert_eq!(req.app_id, "shop");
    assert_eq!(req.app_name, "测试商城");
    assert_eq!(req.tool, "cart.checkout");
    assert_eq!(req.risk, Risk::Payment);
    assert_eq!(req.arguments, json!({"n": 1}));
    assert_eq!(req.session.as_deref(), Some("s1"));

    // 低于阈值不询问
    let o = hub.call_tool(CallRequest::new("shop.echo", json!({"text": "x"}))).await.unwrap();
    assert!(o.result.is_ok());
    assert_eq!(a.asked.load(Ordering::SeqCst), 1);

    // 超时 → 拒绝
    let t = approver(None);
    hub.set_approval_handler(t.clone());
    let o = hub.call_tool(CallRequest::new("shop.cart.checkout", json!({}))).await.unwrap();
    let e = o.result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::UserRejected);
    assert!(e.message.contains("超时"), "{}", e.message);

    // 同意
    hub.set_approval_handler(approver(Some(true)));
    let o = hub.call_tool(CallRequest::new("shop.cart.checkout", json!({}))).await.unwrap();
    assert_eq!(o.result.unwrap()["tool"], "cart.checkout");

    // dispatch 同样经过审批
    hub.set_approval_handler(approver(Some(false)));
    let r = hub
        .dispatch(ToolFormat::Anthropic, tool_call(ToolFormat::Anthropic, "shop__cart__checkout", json!({})))
        .await;
    let (text, is_error) = result_text(ToolFormat::Anthropic, &r);
    assert!(is_error && text.starts_with("USER_REJECTED: "), "{text}");
}

// ---------------------------------------------------------------------------
// 事件与配对
// ---------------------------------------------------------------------------

#[tokio::test]
async fn events_for_connect_tools_visibility_disconnect() {
    let hub = Hub::start(config()).await.unwrap();
    let mut rx = hub.events();
    let sdk = connect(&hub, Spec::new("shop", "i1")).await;
    let ev = next_event(&mut rx, |e| matches!(e, HubEvent::AppConnected { .. })).await;
    assert_eq!(
        ev,
        HubEvent::AppConnected {
            app_id: "shop".into(),
            instance_id: "i1".into()
        }
    );
    // 可见性事件立即发出；列表变化事件经过合并窗口后发出。
    let ev = next_event(&mut rx, |e| matches!(e, HubEvent::VisibilityChanged { .. })).await;
    assert!(matches!(ev, HubEvent::VisibilityChanged { visibility: app_mcp_hub::Visibility::Visible, .. }));
    next_event(&mut rx, |e| *e == HubEvent::ToolsChanged).await;
    hub.subscribe("app-mcp://shop/cart.state").unwrap();
    sdk.close();
    next_event(&mut rx, |e| matches!(e, HubEvent::AppDisconnected { .. })).await;
    next_event(&mut rx, |e| *e == HubEvent::ToolsChanged).await;
    hub.unsubscribe("app-mcp://shop/cart.state");
}

#[tokio::test]
async fn resource_subscribe_and_updated_event() {
    let hub = Hub::start(config()).await.unwrap();
    let mut rx = hub.events();
    let mut sdk = connect(&hub, Spec::new("shop", "i1")).await;
    wait_tool(&hub, "shop.echo").await;
    hub.subscribe("app-mcp://shop/cart.state").unwrap();
    let sub = sdk.expect("resources/subscribe").await;
    assert_eq!(sub["params"]["name"], "cart.state");
    sdk.notify("resources/updated", json!({"name": "cart.state"}));
    let ev = next_event(&mut rx, |e| matches!(e, HubEvent::ResourceUpdated { .. })).await;
    assert_eq!(
        ev,
        HubEvent::ResourceUpdated {
            uri: "app-mcp://shop/cart.state".into()
        }
    );
    hub.unsubscribe("app-mcp://shop/cart.state");
    sdk.expect("resources/unsubscribe").await;
    assert!(hub.subscribe("bad-uri").is_err());
}

struct Pairer {
    answer: bool,
    asked: AtomicUsize,
}

#[async_trait]
impl PairingHandler for Pairer {
    async fn pair(&self, req: PairingRequest) -> bool {
        assert_eq!(req.app_id, "newapp");
        assert_eq!(req.client_kind, "web");
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.answer
    }
}

#[tokio::test]
async fn pairing_handler_hook() {
    // 未设置 handler：无清单的 App 直接配对（与原 Host 一致）
    let hub = Hub::start(config()).await.unwrap();
    let sdk = connect(&hub, Spec::new("plain", "p1")).await;
    assert_eq!(sdk.hello["status"], "paired");

    // 同意
    let p = Arc::new(Pairer {
        answer: true,
        asked: AtomicUsize::new(0),
    });
    hub.set_pairing_handler(p.clone());
    let mut sdk = connect(&hub, Spec::new("newapp", "n1")).await;
    assert_eq!(sdk.hello["status"], "pending");
    let r = sdk.expect("app/pairingResult").await;
    assert_eq!(r["params"]["status"], "paired");
    let token = r["params"]["token"].as_str().unwrap().to_owned();
    assert!(!token.is_empty());
    wait_tool(&hub, "newapp.echo").await;
    assert_eq!(p.asked.load(Ordering::SeqCst), 1);

    // 同一 appId + Origin 再次连接：不再询问
    let sdk2 = connect(&hub, Spec::new("newapp", "n2")).await;
    assert_eq!(sdk2.hello["status"], "paired");
    assert_eq!(p.asked.load(Ordering::SeqCst), 1);

    // 拒绝
    let hub2 = Hub::start(config()).await.unwrap();
    hub2.set_pairing_handler(Arc::new(Pairer {
        answer: false,
        asked: AtomicUsize::new(0),
    }));
    let mut sdk = connect(&hub2, Spec::new("newapp", "n1")).await;
    assert_eq!(sdk.hello["status"], "pending");
    let r = sdk.expect("app/pairingResult").await;
    assert_eq!(r["params"]["status"], "rejected");
    assert!(hub2.apps().is_empty());

    // 设置了 handler 时，白名单外的 Origin 也会询问（而不是直接拒绝）
    let hub3 = Hub::start(config()).await.unwrap();
    hub3.set_pairing_handler(Arc::new(Pairer {
        answer: true,
        asked: AtomicUsize::new(0),
    }));
    let mut spec = Spec::new("newapp", "n1");
    spec.origin = Some("https://evil.example");
    let sdk = connect(&hub3, spec).await;
    assert_eq!(sdk.hello["status"], "pending");
}

#[tokio::test]
async fn no_ws_and_shutdown() {
    let hub = Hub::start(HubConfig {
        listen: None,
        ipc_endpoint: None,
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(hub.listen_addr().is_none());
    assert!(hub.apps().is_empty());
    assert_eq!(hub.tools(&ToolFilter::default()).len(), 7); // 内置（apps.list / select / overview / activate / release / lock / unlock）
    assert!(hub.tools(&ToolFilter { include_builtin: false, ..Default::default() }).is_empty());
    hub.shutdown().await;

    let hub = Hub::start(config()).await.unwrap();
    let mut sdk = connect(&hub, Spec::new("shop", "i1")).await;
    wait_tool(&hub, "shop.echo").await;
    hub.shutdown().await;
    // 连接被关闭
    timeout(T, async {
        while sdk.events.recv().await.is_some() {}
    })
    .await
    .expect("连接应被关闭");
}

// ---------------------------------------------------------------------------
// MCP 出口与 Hub API 共用调用逻辑
// ---------------------------------------------------------------------------

#[cfg(feature = "mcp-server")]
#[tokio::test]
async fn mcp_session_uses_same_call_path() {
    use rmcp::ServiceExt;
    use rmcp::model::CallToolRequestParams;

    let hub = Hub::start(HubConfig {
        approval: ApprovalPolicy {
            require_at_or_above: Some(Risk::Payment),
            timeout: None,
        },
        ..config()
    })
    .await
    .unwrap();
    let a = approver(Some(false));
    hub.set_approval_handler(a.clone());
    let _sdk = connect(&hub, Spec::new("shop", "i1")).await;
    wait_tool(&hub, "shop.echo").await;

    let mcp = |hub: &Hub| {
        let (c, s) = tokio::io::duplex(1 << 20);
        let session = hub.mcp_session();
        tokio::spawn(async move {
            if let Ok(svc) = session.serve(s).await {
                let _ = svc.waiting().await;
            }
        });
        async move { ().serve(c).await.expect("mcp") }
    };
    let text = |r: &rmcp::model::CallToolResult| {
        r.content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let args = |v: Value| match v {
        Value::Object(m) => m,
        _ => Default::default(),
    };
    for _ in 0..2 {
        // 每个 MCP 会话首次调用各附带一次总览
        let client = mcp(&hub).await;
        let r = client
            .call_tool(CallToolRequestParams::new("shop.echo").with_arguments(args(json!({"text": "hi"}))))
            .await
            .unwrap();
        assert!(text(&r).contains("<app-overview"), "{}", text(&r));
        let r = client
            .call_tool(CallToolRequestParams::new("shop.echo").with_arguments(args(json!({"text": "hi"}))))
            .await
            .unwrap();
        assert!(!text(&r).contains("<app-overview"));
        // 审批同样作用于 MCP 出口
        let r = client
            .call_tool(CallToolRequestParams::new("shop.cart.checkout").with_arguments(args(json!({}))))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).starts_with("USER_REJECTED: "), "{}", text(&r));
        let _ = client.cancel().await;
    }
    assert_eq!(a.asked.load(Ordering::SeqCst), 2);
    let req = a.last.lock().unwrap().clone().unwrap();
    assert!(req.session.unwrap().starts_with("mcp:"));
}

// ---------------------------------------------------------------------------
// 渐进暴露（spec/hub-api.md 3.7）
// ---------------------------------------------------------------------------

fn names(tools: &[app_mcp_hub::HubTool]) -> Vec<String> {
    tools.iter().map(|t| t.name.clone()).collect()
}

fn session(s: &str) -> ToolFilter {
    ToolFilter {
        session: Some(s.into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn progressive_exposure_api_and_export() {
    let hub = Hub::start(HubConfig {
        tool_exposure: ToolExposure::Progressive,
        ..config()
    })
    .await
    .unwrap();
    let _a = connect(&hub, Spec::new("shop", "i1")).await;
    let _b = connect(&hub, Spec::new("notes", "n1")).await;
    wait_tool(&hub, "shop.echo").await;
    wait_tool(&hub, "notes.echo").await;
    let builtins =
        ["apps.list", "apps.select", "apps.overview", "apps.tools", "apps.activate", "apps.release", "apps.lock", "apps.unlock"];
    assert_eq!(names(&hub.tools(&ToolFilter::default())), builtins);
    assert_eq!(
        exported_names(ToolFormat::Anthropic, &hub.export_tools(ToolFormat::Anthropic, &ToolFilter::default())),
        [
            "apps__list",
            "apps__select",
            "apps__overview",
            "apps__tools",
            "apps__activate",
            "apps__release",
            "apps__lock",
            "apps__unlock"
        ]
    );
    // 显式指定 apps 时不受渐进暴露影响
    let explicit = ToolFilter { apps: Some(vec!["notes".into()]), include_builtin: false, ..Default::default() };
    assert_eq!(hub.tools(&explicit).len(), 3);

    // dispatch apps.tools（默认会话）→ 返回 schema，shop 加入默认会话的导出
    let r = hub
        .dispatch(ToolFormat::Anthropic, tool_call(ToolFormat::Anthropic, "apps__tools", json!({"appId": "shop"})))
        .await;
    let (text, is_error) = result_text(ToolFormat::Anthropic, &r);
    assert!(!is_error, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["appId"], "shop");
    let echo = v["tools"].as_array().unwrap().iter().find(|t| t["name"] == "shop.echo").unwrap();
    assert_eq!(echo["inputSchema"]["required"], json!(["text"]));
    let listed = names(&hub.tools(&ToolFilter::default()));
    assert!(listed.contains(&"shop.echo".to_string()), "{listed:?}");
    assert!(!listed.iter().any(|n| n.starts_with("notes.")), "{listed:?}");
    let exported = exported_names(ToolFormat::OpenAiChat, &hub.export_tools(ToolFormat::OpenAiChat, &ToolFilter::default()));
    assert!(exported.contains(&"shop__echo".to_string()), "{exported:?}");

    // 其他会话不受影响；未列出的工具按全名仍可调用，调用后加入该会话
    assert_eq!(names(&hub.tools(&session("c2"))), builtins);
    let mut req = CallRequest::new("notes.echo", json!({"text": "hi"}));
    req.session = Some("c2".into());
    let out = hub.call_tool(req).await.unwrap();
    assert!(out.result.is_ok(), "{:?}", out.result);
    let listed = names(&hub.tools(&session("c2")));
    assert!(listed.contains(&"notes.echo".to_string()), "{listed:?}");
    assert!(!listed.iter().any(|n| n.starts_with("shop.")), "{listed:?}");
    // 导出名按全部工具计算：未列出的工具经导出名分派也能调用
    let r = hub
        .dispatch_in_session(ToolFormat::Anthropic, tool_call(ToolFormat::Anthropic, "notes__echo", json!({"text": "x"})), Some("c3"))
        .await;
    assert!(!result_text(ToolFormat::Anthropic, &r).1);

    // 未知 App
    let out = hub.call_tool(CallRequest::new("apps.tools", json!({"appId": "nope"}))).await.unwrap();
    assert_eq!(out.result.unwrap_err().kind, ErrorKind::ToolNotFound);

    // 全局选定实例：所有会话都直接列出
    hub.select_instance("notes", Some("n1"));
    assert!(names(&hub.tools(&session("c4"))).contains(&"notes.echo".to_string()));
    hub.select_instance("notes", None);
    assert_eq!(names(&hub.tools(&session("c4"))), builtins);

    // reset_session 清除已展开的 App
    hub.reset_session(Some("c2"));
    assert_eq!(names(&hub.tools(&session("c2"))), builtins);
}

#[tokio::test]
async fn auto_exposure_switches_at_threshold() {
    let hub = Hub::start(HubConfig {
        tool_exposure_threshold: 3,
        ..config()
    })
    .await
    .unwrap();
    // 默认 Auto：3 个工具不超过阈值 → 全部列出，且不列 apps.tools
    let _a = connect(&hub, Spec::new("shop", "i1")).await;
    wait_tool(&hub, "shop.echo").await;
    let listed = names(&hub.tools(&ToolFilter::default()));
    assert_eq!(listed.len(), 10, "{listed:?}"); // 3 个 App 工具 + 7 个内置
    assert!(!listed.contains(&"apps.tools".to_string()));
    // apps.tools 任何时候都可调用
    let out = hub.call_tool(CallRequest::new("apps.tools", json!({"appId": "shop"}))).await.unwrap();
    assert_eq!(out.result.unwrap()["tools"].as_array().unwrap().len(), 3);
    // 第二个 App 连接后超过阈值 → 渐进；默认会话已展开 shop（上面的 apps.tools）
    let _b = connect(&hub, Spec::new("notes", "n1")).await;
    wait_tool(&hub, "notes.echo").await;
    let listed = names(&hub.tools(&ToolFilter::default()));
    assert!(listed.contains(&"apps.tools".to_string()), "{listed:?}");
    assert!(listed.contains(&"shop.echo".to_string()), "{listed:?}");
    assert!(!listed.iter().any(|n| n.starts_with("notes.")), "{listed:?}");

    // All：始终全部列出
    let all = Hub::start(HubConfig {
        tool_exposure: ToolExposure::All,
        tool_exposure_threshold: 0,
        ..config()
    })
    .await
    .unwrap();
    let _c = connect(&all, Spec::new("shop", "i1")).await;
    wait_tool(&all, "shop.echo").await;
    assert_eq!(all.tools(&ToolFilter::default()).len(), 10);
}
