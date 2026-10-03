//! 进程内集成测试：tokio-tungstenite 客户端扮演 SDK，rmcp client 通过 duplex 连接 MCP 侧。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use app_mcp_hub::{Hub as Host, HubConfig as HostConfig};
use futures::{SinkExt, StreamExt};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientConfig, ReadResourceRequestParams,
    ResourceContents, ResourceUpdatedNotificationParam, SubscribeRequestParams,
};
use rmcp::service::{NotificationContext, RunningService};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, mpsc};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as Ws;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const T: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// 假 SDK
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SdkSpec {
    app_id: String,
    instance_id: String,
    tools: Vec<Value>,
    resources: Vec<Value>,
    overview: Option<Value>,
    origin: Option<String>,
    protocol_version: String,
    /// 握手后上报的 (visibility, focused)。
    visibility: Option<(&'static str, bool)>,
}

impl SdkSpec {
    fn new(app_id: &str, instance_id: &str) -> Self {
        Self {
            app_id: app_id.into(),
            instance_id: instance_id.into(),
            tools: vec![echo_tool(), tool("fail"), tool("slow")],
            resources: vec![json!({"name": "cart.state", "description": "购物车"})],
            overview: None,
            origin: Some("http://localhost:5173".into()),
            protocol_version: "1".into(),
            visibility: None,
        }
    }
}

fn tool(name: &str) -> Value {
    json!({"name": name, "description": format!("{name} 工具"), "inputSchema": {"type": "object"}})
}

fn echo_tool() -> Value {
    json!({
        "name": "echo", "description": "回显参数", "risk": "read",
        "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}
    })
}

enum Cmd {
    Send(Value),
    Close,
}

struct Sdk {
    cmd: mpsc::UnboundedSender<Cmd>,
    /// Host 发来的全部请求 / 通知。
    events: mpsc::UnboundedReceiver<Value>,
    hello: Value,
}

impl Sdk {
    fn send(&self, v: Value) {
        let _ = self.cmd.send(Cmd::Send(v));
    }

    fn visibility(&self, visibility: &str, focused: bool) {
        self.send(json!({"jsonrpc": "2.0", "method": "app/visibility", "params": {"visibility": visibility, "focused": focused}}));
    }

    fn close(&self) {
        let _ = self.cmd.send(Cmd::Close);
    }

    /// 等待某方法的消息（跳过其他）。
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

    fn drain_methods(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(v) = self.events.try_recv() {
            if let Some(m) = v["method"].as_str() {
                out.push(m.to_owned());
            }
        }
        out
    }
}

/// 连接并握手；`paired` 后同步工具 / 资源 / 可见性并发送 ready。
async fn connect_sdk(host: &Host, spec: SdkSpec) -> Sdk {
    let url = format!("ws://{}/app", host.listen_addr().expect("listen"));
    let mut req = url.into_client_request().unwrap();
    if let Some(o) = &spec.origin {
        req.headers_mut().insert("Origin", o.parse().unwrap());
    }
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .expect("ws connect");
    let (mut sink, mut stream) = ws.split();

    let mut hello = json!({
        "appId": spec.app_id, "appName": "示例商城", "protocolVersion": spec.protocol_version,
        "sdkVersion": "0.0.0", "clientKind": "web", "instanceId": spec.instance_id,
        "instanceTitle": format!("tab {}", spec.instance_id)
    });
    if let Some(ov) = &spec.overview {
        hello["overview"] = ov.clone();
    }
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": hello});
    sink.send(Ws::text(msg.to_string())).await.unwrap();
    let hello_result: Value = loop {
        let Some(Ok(Ws::Text(t))) = timeout(T, stream.next()).await.expect("hello timeout") else {
            panic!("连接在握手时关闭")
        };
        let v: Value = serde_json::from_str(t.as_str()).unwrap();
        if v["id"] == 1 {
            break v["result"].clone();
        }
    };

    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel::<Value>();
    if hello_result["status"] == "paired" {
        let note =
            |m: &str, p: Value| json!({"jsonrpc": "2.0", "method": m, "params": p}).to_string();
        sink.send(Ws::text(note("tools/sync", json!({"tools": spec.tools}))))
            .await
            .unwrap();
        sink.send(Ws::text(note(
            "resources/sync",
            json!({"resources": spec.resources}),
        )))
        .await
        .unwrap();
        if let Some((v, f)) = spec.visibility {
            sink.send(Ws::text(note(
                "app/visibility",
                json!({"visibility": v, "focused": f}),
            )))
            .await
            .unwrap();
        }
        sink.send(Ws::text(note("app/ready", json!({}))))
            .await
            .unwrap();
    }

    let instance_id = spec.instance_id.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(Cmd::Send(v)) => { let _ = sink.send(Ws::text(v.to_string())).await; }
                    Some(Cmd::Close) | None => { let _ = sink.close().await; break; }
                },
                msg = stream.next() => {
                    let Some(Ok(msg)) = msg else { break };
                    let Ws::Text(t) = msg else { continue };
                    let v: Value = serde_json::from_str(t.as_str()).unwrap();
                    let _ = ev_tx.send(v.clone());
                    let Some(id) = v.get("id").cloned() else { continue };
                    let Some(method) = v["method"].as_str() else { continue };
                    let reply = match method {
                        "tools/invoke" => match v["params"]["name"].as_str().unwrap_or_default() {
                            "echo" => Some(json!({"result": {
                                "data": {"instanceId": instance_id, "args": v["params"]["arguments"]},
                                "stateHints": ["cart.state"]
                            }})),
                            "fail" => Some(json!({"error": {
                                "code": -32006, "message": "库存不足，无法下单", "data": {"kind": "HANDLER_ERROR"}
                            }})),
                            _ => None, // slow：不响应
                        },
                        "resources/read" => Some(json!({"result": {"contents": {"items": [1, 2], "instanceId": instance_id}}})),
                        "resources/subscribe" | "resources/unsubscribe" | "ping" => Some(json!({"result": {}})),
                        _ => Some(json!({"error": {"code": -32601, "message": "nope"}})),
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
        cmd: cmd_tx,
        events: ev_rx,
        hello: hello_result,
    }
}

// ---------------------------------------------------------------------------
// MCP 客户端
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct TestClient {
    tool_changes: Arc<AtomicUsize>,
    tool_changed: Arc<Notify>,
    updated: Arc<std::sync::Mutex<Vec<String>>>,
    updated_notify: Arc<Notify>,
}

impl ClientHandler for TestClient {
    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.tool_changes.fetch_add(1, Ordering::SeqCst);
        self.tool_changed.notify_one();
    }

    async fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.updated.lock().unwrap().push(params.uri);
        self.updated_notify.notify_one();
    }

    fn get_info(&self) -> ClientConfig {
        ClientConfig::default()
    }
}

type Client = RunningService<RoleClient, TestClient>;

async fn mcp_client(host: &Host) -> (Client, TestClient) {
    let (c, s) = tokio::io::duplex(1 << 20);
    let session = host.mcp_session();
    tokio::spawn(async move {
        if let Ok(svc) = session.serve(s).await {
            let _ = svc.waiting().await;
        }
    });
    let handler = TestClient::default();
    let client = handler.clone().serve(c).await.expect("mcp initialize");
    // 等 notifications/initialized 被 Host 处理，确保会话已登记
    tokio::time::sleep(Duration::from_millis(50)).await;
    (client, handler)
}

fn config() -> HostConfig {
    HostConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        ..Default::default()
    }
}

async fn start(config: HostConfig) -> Host {
    Host::start(config).await.expect("host start")
}

async fn tool_names(client: &Client) -> Vec<String> {
    client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect()
}

/// 等到工具列表满足条件（SDK 的通知是异步处理的）。
async fn wait_tools(client: &Client, pred: impl Fn(&[String]) -> bool) -> Vec<String> {
    timeout(T, async {
        loop {
            let names = tool_names(client).await;
            if pred(&names) {
                return names;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("工具列表没有达到预期")
}

async fn call(client: &Client, name: &str, args: Value) -> CallToolResult {
    let mut p = CallToolRequestParams::new(name.to_owned());
    if let Value::Object(m) = args {
        p = p.with_arguments(m);
    } else {
        p = p.with_arguments(Map::new());
    }
    client.call_tool(p).await.expect("call_tool")
}

fn texts(r: &CallToolResult) -> Vec<String> {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

fn manifest(overview: Option<Value>) -> app_mcp_manifest::Manifest {
    let mut m = json!({
        "manifestVersion": 1, "appId": "shop", "name": "示例商城", "description": "购物",
        "launch": {"web": [{"type": "url", "href": "http://localhost:5173/"}]},
        "tools": [
            {"name": "orders.search", "description": "搜索订单", "inputSchema": {"type": "object"}},
            {"name": "echo", "description": "回显（静态）", "inputSchema": {"type": "object"}}
        ]
    });
    if let Some(o) = overview {
        m["overview"] = o;
    }
    app_mcp_manifest::parse(&m.to_string()).unwrap()
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_and_call_roundtrip() {
    let host = start(config()).await;
    let sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    assert_eq!(sdk.hello["status"], "paired");
    assert_eq!(sdk.hello["protocolVersion"], "1");
    assert!(sdk.hello["token"].as_str().is_some_and(|t| t.len() == 32));

    let (client, _) = mcp_client(&host).await;
    let names = wait_tools(&client, |n| n.contains(&"shop.echo".to_string())).await;
    for n in [
        "apps.list",
        "apps.select",
        "apps.overview",
        "shop.echo",
        "shop.fail",
        "shop.slow",
    ] {
        assert!(names.contains(&n.to_string()), "{n} 不在 {names:?}");
    }
    let echo = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.name == "shop.echo")
        .unwrap();
    assert_eq!(echo.annotations.unwrap().read_only_hint, Some(true));
    assert_eq!(echo.input_schema.get("required"), Some(&json!(["text"])));

    let r = call(&client, "shop.echo", json!({"text": "你好"})).await;
    assert_eq!(r.is_error, Some(false));
    let t = texts(&r);
    let data: Value = serde_json::from_str(&t[0]).unwrap();
    assert_eq!(data["args"]["text"], "你好");
    assert_eq!(data["instanceId"], "i1");
    assert!(t[1].contains("app-mcp://shop/cart.state"));
    assert_eq!(r.structured_content.as_ref().unwrap()["instanceId"], "i1");

    // apps.list
    let r = call(&client, "apps.list", json!({})).await;
    let v = r.structured_content.unwrap();
    let app = &v["apps"][0];
    assert_eq!(app["appId"], "shop");
    assert_eq!(app["connected"], true);
    assert_eq!(app["instances"][0]["instanceId"], "i1");
    assert_eq!(app["instances"][0]["ready"], true);
    assert_eq!(app["summary"], Value::Null);
}

#[tokio::test]
async fn invoke_params_sent_to_sdk() {
    let host = start(config()).await;
    let mut sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    let (client, _) = mcp_client(&host).await;
    wait_tools(&client, |n| n.contains(&"shop.echo".to_string())).await;
    call(&client, "shop.echo", json!({"text": "x"})).await;
    let inv = sdk.expect("tools/invoke").await;
    assert_eq!(inv["params"]["name"], "echo");
    assert_eq!(inv["params"]["timeoutMs"], 30000);
    assert!(
        inv["params"]["callId"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
}

#[tokio::test]
async fn invalid_arguments_are_rejected_before_sdk() {
    let host = start(config()).await;
    let mut sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    let (client, _) = mcp_client(&host).await;
    wait_tools(&client, |n| n.contains(&"shop.echo".to_string())).await;

    let r = call(&client, "shop.echo", json!({"text": 42})).await;
    assert_eq!(r.is_error, Some(true));
    assert!(texts(&r)[0].starts_with("INVALID_INPUT"), "{:?}", texts(&r));
    assert_eq!(
        r.structured_content.unwrap()["error"]["kind"],
        "INVALID_INPUT"
    );
    let r = call(&client, "shop.echo", json!({})).await;
    assert_eq!(r.is_error, Some(true));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!sdk.drain_methods().contains(&"tools/invoke".to_string()));

    // 内置工具也校验
    let r = call(&client, "apps.select", json!({"appId": "shop"})).await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn handler_error_is_tool_error_result() {
    let host = start(config()).await;
    let _sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    let (client, _) = mcp_client(&host).await;
    wait_tools(&client, |n| n.contains(&"shop.fail".to_string())).await;
    let r = call(&client, "shop.fail", json!({})).await;
    assert_eq!(r.is_error, Some(true));
    assert_eq!(texts(&r)[0], "HANDLER_ERROR: 库存不足，无法下单");

    let r = call(&client, "shop.nope", json!({})).await;
    assert_eq!(r.is_error, Some(true));
    assert!(texts(&r)[0].starts_with("TOOL_NOT_FOUND"));
    let r = call(&client, "noapp", json!({})).await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn timeout_sends_cancel() {
    let host = start(HostConfig {
        response_timeout: Duration::from_millis(300),
        invoke_timeout: Duration::from_millis(200),
        ..config()
    })
    .await;
    let mut sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    let (client, _) = mcp_client(&host).await;
    wait_tools(&client, |n| n.contains(&"shop.slow".to_string())).await;
    let r = call(&client, "shop.slow", json!({})).await;
    assert_eq!(r.is_error, Some(true));
    assert!(texts(&r)[0].starts_with("TIMEOUT"));
    let inv = sdk.expect("tools/invoke").await;
    assert_eq!(inv["params"]["timeoutMs"], 200);
    let cancel = sdk.expect("tools/cancel").await;
    assert_eq!(cancel["params"]["callId"], inv["params"]["callId"]);
}

#[tokio::test]
async fn disconnect_removes_tools_and_notifies() {
    let host = start(config()).await;
    let sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    let (client, handler) = mcp_client(&host).await;
    wait_tools(&client, |n| n.contains(&"shop.echo".to_string())).await;
    // 等连接阶段的通知都到达后再计数
    tokio::time::sleep(Duration::from_millis(150)).await;
    let before = handler.tool_changes.load(Ordering::SeqCst);

    sdk.close();
    timeout(T, async {
        while handler.tool_changes.load(Ordering::SeqCst) == before {
            handler.tool_changed.notified().await;
        }
    })
    .await
    .expect("没有收到 tools/list_changed");
    let names = tool_names(&client).await;
    assert!(!names.iter().any(|n| n.starts_with("shop.")), "{names:?}");
    assert!(names.contains(&"apps.list".to_string()));
}

#[tokio::test]
async fn list_changed_is_debounced() {
    let host = start(HostConfig {
        list_changed_debounce: Duration::from_millis(100),
        ..config()
    })
    .await;
    let (client, handler) = mcp_client(&host).await;
    let sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    for i in 0..5 {
        sdk.send(json!({"jsonrpc": "2.0", "method": "tools/changed", "params": {"upserted": [tool(&format!("t{i}"))], "removed": []}}));
    }
    wait_tools(&client, |n| n.contains(&"shop.t4".to_string())).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // hello + sync + 5 次 changed 合并为 1~2 次通知
    let n = handler.tool_changes.load(Ordering::SeqCst);
    assert!((1..=2).contains(&n), "通知次数 {n}");
}

#[tokio::test]
async fn routing_focus_and_select() {
    let host = start(config()).await;
    let a = connect_sdk(
        &host,
        SdkSpec {
            visibility: Some(("visible", false)),
            ..SdkSpec::new("shop", "a")
        },
    )
    .await;
    let b = connect_sdk(
        &host,
        SdkSpec {
            visibility: Some(("hidden", false)),
            ..SdkSpec::new("shop", "b")
        },
    )
    .await;
    let (client, _) = mcp_client(&host).await;
    wait_tools(&client, |n| n.contains(&"shop.echo".to_string())).await;
    let target = |r: CallToolResult| {
        r.structured_content.unwrap()["instanceId"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    // a 最近可见
    assert_eq!(
        target(call(&client, "shop.echo", json!({"text": "x"})).await),
        "a"
    );
    // b 获得焦点
    b.visibility("visible", true);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        target(call(&client, "shop.echo", json!({"text": "x"})).await),
        "b"
    );
    // a 获得焦点，b 失焦
    b.visibility("hidden", false);
    a.visibility("visible", true);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        target(call(&client, "shop.echo", json!({"text": "x"})).await),
        "a"
    );
    // apps.select 优先
    let r = call(
        &client,
        "apps.select",
        json!({"appId": "shop", "instanceId": "b"}),
    )
    .await;
    assert_eq!(r.is_error, Some(false));
    assert_eq!(
        target(call(&client, "shop.echo", json!({"text": "x"})).await),
        "b"
    );
    let list = call(&client, "apps.list", json!({}))
        .await
        .structured_content
        .unwrap();
    assert_eq!(list["apps"][0]["selectedInstanceId"], "b");
    // 选择只在本会话内生效
    let (client2, _) = mcp_client(&host).await;
    assert_eq!(
        target(call(&client2, "shop.echo", json!({"text": "x"})).await),
        "a"
    );
    // 选择不存在的实例
    let r = call(
        &client,
        "apps.select",
        json!({"appId": "shop", "instanceId": "zzz"}),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    // 选定实例断开后回到默认规则
    b.close();
    wait_tools(&client, |_| true).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        target(call(&client, "shop.echo", json!({"text": "x"})).await),
        "a"
    );
    drop(a);
}

#[tokio::test]
async fn static_manifest_tools() {
    let host = start(HostConfig {
        manifests: vec![manifest(None)],
        ..config()
    })
    .await;
    let (client, _) = mcp_client(&host).await;
    let names = tool_names(&client).await;
    assert!(names.contains(&"shop.orders.search".to_string()));
    let r = call(&client, "shop.orders.search", json!({})).await;
    assert_eq!(r.is_error, Some(true));
    let t = &texts(&r)[0];
    assert!(t.starts_with("APP_DISCONNECTED"), "{t}");
    assert!(t.contains("http://localhost:5173/"), "{t}");

    // 连接后：运行时工具为准，未注册的静态工具加前缀
    let _sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    wait_tools(&client, |n| n.contains(&"shop.fail".to_string())).await;
    let tools = client.list_all_tools().await.unwrap();
    let search = tools
        .iter()
        .find(|t| t.name == "shop.orders.search")
        .unwrap();
    assert!(
        search
            .description
            .as_deref()
            .unwrap()
            .starts_with("[当前不可用] ")
    );
    let echo = tools.iter().find(|t| t.name == "shop.echo").unwrap();
    assert_eq!(echo.description.as_deref(), Some("回显参数"));
    let r = call(&client, "shop.orders.search", json!({})).await;
    assert!(texts(&r)[0].starts_with("TOOL_NOT_FOUND"));
    let list = call(&client, "apps.list", json!({}))
        .await
        .structured_content
        .unwrap();
    assert_eq!(list["apps"][0]["staticToolCount"], 2);
    assert_eq!(list["apps"][0]["name"], "示例商城");
}

#[tokio::test]
async fn bad_origin_and_protocol_rejected() {
    let host = start(config()).await;
    let sdk = connect_sdk(
        &host,
        SdkSpec {
            origin: Some("https://evil.example".into()),
            ..SdkSpec::new("shop", "i1")
        },
    )
    .await;
    assert_eq!(sdk.hello["status"], "rejected");
    assert!(
        sdk.hello["reason"]
            .as_str()
            .unwrap()
            .contains("https://evil.example")
    );

    let sdk = connect_sdk(
        &host,
        SdkSpec {
            protocol_version: "2".into(),
            ..SdkSpec::new("shop", "i1")
        },
    )
    .await;
    assert_eq!(sdk.hello["status"], "rejected");
    assert!(sdk.hello["reason"].as_str().unwrap().contains("协议版本"));

    let sdk = connect_sdk(&host, SdkSpec::new("apps", "i1")).await;
    assert_eq!(sdk.hello["status"], "rejected");

    // 无 Origin（原生客户端）允许；--allow-origin 生效
    let sdk = connect_sdk(
        &host,
        SdkSpec {
            origin: None,
            ..SdkSpec::new("shop", "i1")
        },
    )
    .await;
    assert_eq!(sdk.hello["status"], "paired");
    let host2 = start(HostConfig {
        allow_origins: vec!["https://app.example.com".into()],
        ..config()
    })
    .await;
    let sdk = connect_sdk(
        &host2,
        SdkSpec {
            origin: Some("https://app.example.com".into()),
            ..SdkSpec::new("shop", "i1")
        },
    )
    .await;
    assert_eq!(sdk.hello["status"], "paired");

    let (client, _) = mcp_client(&host).await;
    let list = call(&client, "apps.list", json!({}))
        .await
        .structured_content
        .unwrap();
    assert_eq!(list["apps"][0]["instances"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn same_instance_id_replaces_old_connection() {
    let host = start(config()).await;
    let mut old = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    let _new = connect_sdk(
        &host,
        SdkSpec {
            tools: vec![echo_tool()],
            ..SdkSpec::new("shop", "i1")
        },
    )
    .await;
    let (client, _) = mcp_client(&host).await;
    let names = wait_tools(&client, |n| {
        !n.contains(&"shop.fail".to_string()) && n.contains(&"shop.echo".to_string())
    })
    .await;
    assert!(!names.contains(&"shop.slow".to_string()));
    // 旧连接被关闭
    timeout(T, async { while old.events.recv().await.is_some() {} })
        .await
        .expect("旧连接没有被关闭");
    let list = call(&client, "apps.list", json!({}))
        .await
        .structured_content
        .unwrap();
    assert_eq!(list["apps"][0]["instances"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn resources_read_subscribe_update() {
    let host = start(config()).await;
    let mut sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    let (client, handler) = mcp_client(&host).await;
    wait_tools(&client, |n| n.contains(&"shop.echo".to_string())).await;

    let resources = client.list_all_resources().await.unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].uri, "app-mcp://shop/cart.state");
    assert_eq!(resources[0].mime_type.as_deref(), Some("application/json"));

    let r = client
        .read_resource(ReadResourceRequestParams::new("app-mcp://shop/cart.state"))
        .await
        .unwrap();
    let ResourceContents::TextResourceContents { text, uri, .. } = &r.contents[0] else {
        panic!("expected text")
    };
    assert_eq!(uri, "app-mcp://shop/cart.state");
    let v: Value = serde_json::from_str(text).unwrap();
    assert_eq!(v["items"], json!([1, 2]));
    assert_eq!(
        sdk.expect("resources/read").await["params"]["name"],
        "cart.state"
    );

    assert!(
        client
            .read_resource(ReadResourceRequestParams::new("app-mcp://shop/nope"))
            .await
            .is_err()
    );
    assert!(
        client
            .read_resource(ReadResourceRequestParams::new("file:///etc/passwd"))
            .await
            .is_err()
    );

    #[allow(deprecated)]
    client
        .subscribe(SubscribeRequestParams::new("app-mcp://shop/cart.state"))
        .await
        .unwrap();
    assert_eq!(
        sdk.expect("resources/subscribe").await["params"]["name"],
        "cart.state"
    );

    sdk.send(
        json!({"jsonrpc": "2.0", "method": "resources/updated", "params": {"name": "cart.state"}}),
    );
    timeout(T, async {
        while handler.updated.lock().unwrap().is_empty() {
            handler.updated_notify.notified().await;
        }
    })
    .await
    .expect("没有收到 resources/updated");
    assert_eq!(
        handler.updated.lock().unwrap()[0],
        "app-mcp://shop/cart.state"
    );

    #[allow(deprecated)]
    client
        .unsubscribe(rmcp::model::UnsubscribeRequestParams::new(
            "app-mcp://shop/cart.state",
        ))
        .await
        .unwrap();
    assert_eq!(
        sdk.expect("resources/unsubscribe").await["params"]["name"],
        "cart.state"
    );
}

#[tokio::test]
async fn resubscribe_after_reconnect() {
    let host = start(config()).await;
    let sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    let (client, _) = mcp_client(&host).await;
    wait_tools(&client, |n| n.contains(&"shop.echo".to_string())).await;
    #[allow(deprecated)]
    client
        .subscribe(SubscribeRequestParams::new("app-mcp://shop/cart.state"))
        .await
        .unwrap();
    sdk.close();
    wait_tools(&client, |n| !n.contains(&"shop.echo".to_string())).await;
    let mut sdk2 = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    assert_eq!(
        sdk2.expect("resources/subscribe").await["params"]["name"],
        "cart.state"
    );
}

#[tokio::test]
async fn heartbeat_and_idle_timeout() {
    let host = start(HostConfig {
        ping_interval: Duration::from_millis(100),
        idle_timeout: Duration::from_millis(400),
        ..config()
    })
    .await;
    let mut sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    // 假 SDK 会响应 ping，所以连接保持
    sdk.expect("ping").await;
    sdk.expect("ping").await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let (client, _) = mcp_client(&host).await;
    assert!(tool_names(&client).await.contains(&"shop.echo".to_string()));

    // 握手后不再发送任何消息的连接会被断开
    let url = format!("ws://{}/app", host.listen_addr().expect("listen"));
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let closed = timeout(Duration::from_secs(3), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Ws::Close(_))) => break,
                _ => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "空闲连接没有被断开");
}

#[tokio::test]
async fn overview_instructions_attach_and_reattach() {
    let manifest_ov = json!({"summary": "演示用购物商城，可管理待办、浏览商品、操作购物车并结算", "body": "## 能力范围\n- 购物车"});
    let host = start(HostConfig {
        manifests: vec![manifest(Some(manifest_ov))],
        ..config()
    })
    .await;
    let (client, _) = mcp_client(&host).await;

    // instructions
    let info = client.peer_info().expect("peer info");
    let instructions = info.instructions.clone().unwrap();
    assert_eq!(
        instructions,
        "本机的 App 通过 app-mcp 提供工具，工具名格式为 <appId>.<工具名>。\n已知的 App：\n\
         - shop（示例商城）：演示用购物商城，可管理待办、浏览商品、操作购物车并结算\n\
         首次调用某个 App 的工具时，结果中会附带该 App 的完整总览；也可以随时调用 apps.overview 查看。"
    );

    // 首次接触（即使是失败的调用）附带静态清单总览
    let r = call(&client, "shop.orders.search", json!({})).await;
    assert_eq!(r.is_error, Some(true));
    let t = texts(&r);
    assert!(
        t[0].starts_with("[app-mcp] 以下是 App「示例商城」(shop) 的总览"),
        "{}",
        t[0]
    );
    assert!(t[0].contains("<app-overview app=\"shop\" version=\""));
    assert!(t[0].contains("## 能力范围"));
    assert!(t[1].starts_with("APP_DISCONNECTED"));

    // 不重复
    let r = call(&client, "shop.orders.search", json!({})).await;
    assert_eq!(texts(&r).len(), 1);
    assert!(texts(&r)[0].starts_with("APP_DISCONNECTED"));

    // 运行时总览优先：版本变化，再附带一次
    let long_summary = "新".repeat(150);
    let _sdk = connect_sdk(
        &host,
        SdkSpec {
            overview: Some(json!({"summary": long_summary, "body": "运行时正文"})),
            ..SdkSpec::new("shop", "i1")
        },
    )
    .await;
    wait_tools(&client, |n| n.contains(&"shop.fail".to_string())).await;
    let r = call(&client, "shop.echo", json!({"text": "x"})).await;
    let t = texts(&r);
    assert_eq!(t.len(), 3, "{t:?}");
    assert!(t[0].contains("运行时正文"));
    let expected_summary: String = "新".repeat(99) + "…";
    assert!(t[0].contains(&expected_summary));
    assert!(t[1].contains("\"instanceId\":\"i1\""));
    let r = call(&client, "shop.echo", json!({"text": "x"})).await;
    assert_eq!(texts(&r).len(), 2);

    // apps.list 带 summary；apps.overview 返回完整总览
    let list = call(&client, "apps.list", json!({}))
        .await
        .structured_content
        .unwrap();
    assert_eq!(list["apps"][0]["summary"], json!(expected_summary));
    let r = call(&client, "apps.overview", json!({"appId": "shop"})).await;
    assert_eq!(r.is_error, Some(false));
    assert!(texts(&r)[0].contains("<app-overview app=\"shop\""));
    let sc = r.structured_content.unwrap();
    assert_eq!(sc["source"], "runtime");
    assert_eq!(sc["body"], "运行时正文");
    assert_eq!(sc["version"].as_str().unwrap().len(), 12);
    let r = call(&client, "apps.overview", json!({"appId": "nope"})).await;
    assert_eq!(r.is_error, Some(true));

    // 新会话：apps.overview 记为已附带，之后首次调用不再附带
    let (client2, _) = mcp_client(&host).await;
    assert!(
        client2
            .peer_info()
            .unwrap()
            .instructions
            .clone()
            .unwrap()
            .contains(&expected_summary)
    );
    call(&client2, "apps.overview", json!({"appId": "shop"})).await;
    let r = call(&client2, "shop.echo", json!({"text": "x"})).await;
    assert_eq!(texts(&r).len(), 2);
}

#[tokio::test]
async fn no_overview_means_no_attachment() {
    let host = start(config()).await;
    let _sdk = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    let (client, _) = mcp_client(&host).await;
    wait_tools(&client, |n| n.contains(&"shop.echo".to_string())).await;
    let r = call(&client, "shop.echo", json!({"text": "x"})).await;
    assert_eq!(texts(&r).len(), 2);
    let r = call(&client, "apps.overview", json!({"appId": "shop"})).await;
    assert_eq!(r.is_error, Some(false));
    assert!(texts(&r)[0].contains("没有提供总览"));
}

#[tokio::test]
async fn hidden_instances_get_relaxed_idle_timeout() {
    let host = start(HostConfig {
        // 不发心跳，好让空闲超时生效
        ping_interval: Duration::from_secs(60),
        idle_timeout: Duration::from_millis(300),
        hidden_idle_timeout: Duration::from_millis(1500),
        ..config()
    })
    .await;
    let _visible = connect_sdk(
        &host,
        SdkSpec {
            visibility: Some(("visible", true)),
            ..SdkSpec::new("shop", "v")
        },
    )
    .await;
    let _hidden = connect_sdk(
        &host,
        SdkSpec {
            visibility: Some(("hidden", false)),
            ..SdkSpec::new("shop", "h")
        },
    )
    .await;
    let _frozen = connect_sdk(
        &host,
        SdkSpec {
            visibility: Some(("frozen", false)),
            ..SdkSpec::new("shop", "f")
        },
    )
    .await;
    let (client, _) = mcp_client(&host).await;
    let instances = |v: Value| -> Vec<String> {
        v["apps"]
            .as_array()
            .and_then(|a| a.first())
            .map(|app| {
                app["instances"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i["instanceId"].as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };

    tokio::time::sleep(Duration::from_millis(800)).await;
    let mut ids = instances(
        call(&client, "apps.list", json!({}))
            .await
            .structured_content
            .unwrap(),
    );
    ids.sort();
    assert_eq!(
        ids,
        vec!["f", "h"],
        "可见实例应按 idle_timeout 断开，后台实例保留"
    );

    tokio::time::sleep(Duration::from_millis(1200)).await;
    let ids = instances(
        call(&client, "apps.list", json!({}))
            .await
            .structured_content
            .unwrap(),
    );
    assert!(
        ids.is_empty(),
        "后台实例应在 hidden_idle_timeout 后断开：{ids:?}"
    );
}

// ---------------------------------------------------------------------------
// 渐进暴露（spec/hub-api.md 3.7）
// ---------------------------------------------------------------------------

/// 等到工具列表变化通知计数超过 `before`。
async fn wait_tool_change(handler: &TestClient, before: usize) {
    timeout(T, async {
        while handler.tool_changes.load(Ordering::SeqCst) <= before {
            handler.tool_changed.notified().await;
        }
    })
    .await
    .expect("没有收到 tools/list_changed");
}

#[tokio::test]
async fn progressive_exposure_over_mcp() {
    let host = start(HostConfig {
        tool_exposure: app_mcp_hub::ToolExposure::Progressive,
        ..config()
    })
    .await;
    let _shop = connect_sdk(&host, SdkSpec::new("shop", "i1")).await;
    let _notes = connect_sdk(&host, SdkSpec::new("notes", "n1")).await;
    // 等两个 App 都注册完工具
    timeout(T, async {
        while host.tools(&app_mcp_hub::ToolFilter { apps: Some(vec!["shop".into(), "notes".into()]), ..Default::default() }).len() < 6 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("工具没有注册");

    let (client, handler) = mcp_client(&host).await;
    let info = client.peer_info().expect("server info");
    assert!(info.instructions.as_deref().is_some_and(|s| s.contains("apps.tools")));
    tokio::time::sleep(Duration::from_millis(150)).await;
    // 默认只列出 apps.*
    let names = tool_names(&client).await;
    assert_eq!(
        names,
        ["apps.list", "apps.select", "apps.overview", "apps.tools", "apps.activate", "apps.release", "apps.lock", "apps.unlock"]
    );

    // apps.tools 返回 schema，并把 shop 加入本会话的列表（通知 list_changed）
    let before = handler.tool_changes.load(Ordering::SeqCst);
    let r = call(&client, "apps.tools", json!({"appId": "shop"})).await;
    assert_eq!(r.is_error, Some(false));
    let v = r.structured_content.clone().unwrap();
    let echo = v["tools"].as_array().unwrap().iter().find(|t| t["name"] == "shop.echo").unwrap();
    assert_eq!(echo["inputSchema"]["required"], json!(["text"]));
    wait_tool_change(&handler, before).await;
    let names = tool_names(&client).await;
    assert!(names.contains(&"shop.echo".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| n.starts_with("notes.")), "{names:?}");

    // 再次展开同一个 App 不再通知
    let before = handler.tool_changes.load(Ordering::SeqCst);
    call(&client, "apps.tools", json!({"appId": "shop"})).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(handler.tool_changes.load(Ordering::SeqCst), before);

    // 未列出的工具按全名仍可调用，调用后该 App 也加入列表
    let r = call(&client, "notes.echo", json!({"text": "hi"})).await;
    assert_eq!(r.is_error, Some(false), "{:?}", texts(&r));
    wait_tool_change(&handler, before).await;
    let names = tool_names(&client).await;
    assert!(names.contains(&"notes.echo".to_string()), "{names:?}");

    // 未知 App
    let r = call(&client, "apps.tools", json!({"appId": "nope"})).await;
    assert_eq!(r.is_error, Some(true));
    assert!(texts(&r)[0].starts_with("TOOL_NOT_FOUND"));

    // 另一个会话互不影响；apps.select 选定实例后该 App 直接列出
    let (client2, handler2) = mcp_client(&host).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(tool_names(&client2).await.len(), 8); // 只有 apps.*（含 apps.tools、activate、release、lock、unlock）
    let before = handler2.tool_changes.load(Ordering::SeqCst);
    let r = call(&client2, "apps.select", json!({"appId": "notes", "instanceId": "n1"})).await;
    assert_eq!(r.is_error, Some(false));
    wait_tool_change(&handler2, before).await;
    let names = tool_names(&client2).await;
    assert!(names.contains(&"notes.echo".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| n.starts_with("shop.")), "{names:?}");
}
