//! 测试：
//! - 桥接（与 Tauri 无关）：真实 `NativeClient` 经本地 IPC 连接嵌入式 Hub，用记录事件的 [`PageSink`] 充当页面；
//! - 插件：Tauri `MockRuntime`，经真实 IPC 命令 `plugin:app-mcp|op`（含 ACL）登记到同一个 Hub。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{CallRequest, Hub, HubConfig, ToolFilter};
use serde_json::{Value, json};

use super::bridge::{Bridge, PageSink, Sessions};
use super::*;

const T: Duration = Duration::from_secs(10);

fn endpoint(tag: &str) -> String {
    let pid = std::process::id();
    #[cfg(unix)]
    {
        let dir = std::env::temp_dir().join(format!("app-mcp-tauri-{pid}-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        format!("unix:{}", dir.join("hub.sock").display())
    }
    #[cfg(windows)]
    {
        format!(r"pipe:\\.\pipe\app-mcp-tauri-test-{pid}-{tag}")
    }
}

async fn start_hub(tag: &str) -> (Arc<Hub>, String) {
    let ep = endpoint(tag);
    let hub = Hub::start(HubConfig {
        listen: None,
        ipc_endpoint: Some(ep.clone()),
        ..Default::default()
    })
    .await
    .expect("启动 Hub");
    (Arc::new(hub), ep)
}

fn native_config(endpoint: &str, app_id: &str) -> NativeConfig {
    let mut c = NativeConfig::new(app_id, "Tauri 测试");
    c.host_url = endpoint.to_owned();
    c.launch_token = Some(String::new());
    c.client_kind = ClientKind::Hybrid;
    c
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    tokio::time::timeout(T, async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("等待超时：{what}"));
}

fn tool_names(hub: &Hub, app_id: &str) -> Vec<String> {
    let mut names: Vec<String> = hub
        .tools(&ToolFilter::default())
        .into_iter()
        .filter(|t| t.app_id == app_id)
        .map(|t| t.tool)
        .collect();
    names.sort();
    names
}

/// 记录事件的页面。
#[derive(Default)]
struct FakePage {
    events: Mutex<Vec<Value>>,
    gone: AtomicBool,
}

impl FakePage {
    fn take(&self, ty: &str) -> Vec<Value> {
        let mut events = self.events.lock().unwrap_or_else(|p| p.into_inner());
        let (hit, rest): (Vec<Value>, Vec<Value>) = events.drain(..).partition(|e| e["type"] == ty);
        *events = rest;
        hit
    }

    fn all(&self) -> Vec<Value> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

struct Sink(Arc<FakePage>);

impl PageSink for Sink {
    fn deliver(&self, event: &Value) -> bool {
        if self.0.gone.load(Ordering::SeqCst) {
            return false;
        }
        self.0
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(event.clone());
        true
    }
}

async fn shutdown(hub: Arc<Hub>) {
    if let Ok(hub) = Arc::try_unwrap(hub) {
        hub.shutdown().await;
    }
}

struct Fixture {
    hub: Arc<Hub>,
    bridge: Bridge,
    sessions: Arc<Sessions>,
}

impl Fixture {
    async fn new(tag: &str, accept: Option<Arc<bridge::AcceptFn>>) -> Self {
        let (hub, ep) = start_hub(tag).await;
        let sessions = Arc::new(Sessions::new());
        let listener = PluginListener {
            sessions: sessions.clone(),
            user: None,
            idle_exit: None,
        };
        let client = NativeClient::new(native_config(&ep, tag), Some(Arc::new(listener)))
            .expect("创建客户端");
        let bridge = Bridge::new(client, sessions.clone(), accept);
        Self {
            hub,
            bridge,
            sessions,
        }
    }

    fn op(&self, page: &Arc<FakePage>, label: &str, window: &str, op: Value) -> Value {
        let page = page.clone();
        self.bridge
            .handle(label, window, move || Arc::new(Sink(page)), op)
    }

    async fn connected(&self, app_id: &str) {
        self.bridge.client().start();
        eventually("App 连上 Hub", || {
            self.hub
                .apps()
                .iter()
                .any(|a| a.app_id == app_id && a.connected)
        })
        .await;
    }
}

fn register(id: u64, name: &str) -> Value {
    json!({
        "op": "tool.register", "id": id, "name": name,
        "spec": { "description": "页面工具", "risk": "read",
                  "inputSchema": { "type": "object", "properties": { "a": { "type": "integer" } } } }
    })
}

async fn wait_event(page: &FakePage, ty: &str) -> Value {
    let mut found = None;
    eventually(&format!("页面收到 {ty}"), || {
        found = page.take(ty).into_iter().next();
        found.is_some()
    })
    .await;
    found.unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread")]
async fn page_tool_roundtrip_with_rust_tool() {
    let fx = Fixture::new("roundtrip", None).await;
    let page = Arc::new(FakePage::default());

    let hello = fx.op(&page, "main", "main", json!({ "op": "hello" }));
    assert_eq!(hello["ok"], true);
    assert_eq!(
        hello["value"]["instanceId"],
        fx.bridge.client().instance_id()
    );
    assert_eq!(hello["value"]["state"]["status"], "idle");
    assert_eq!(
        fx.op(&page, "main", "main", register(1, "page.add")),
        json!({ "ok": true })
    );

    struct Native;
    impl ToolHandler for Native {
        fn invoke(&self, call: CallHandle) {
            let _ = call.complete(Some(r#"{"from":"rust"}"#), vec![]);
        }
    }
    fx.bridge
        .client()
        .register_tool(ToolSpec::new("app.native", "Rust 侧工具"), Arc::new(Native))
        .expect("注册 Rust 工具");
    fx.connected("roundtrip").await;
    eventually("Hub 看到 Rust 与页面工具", || {
        tool_names(&fx.hub, "roundtrip") == vec!["app.native", "page.add"]
    })
    .await;
    // 页面收到连接状态。
    eventually("页面收到 connected", || {
        page.all()
            .iter()
            .any(|e| e["type"] == "state" && e["state"]["status"] == "connected")
    })
    .await;

    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move {
        hub.call_tool(CallRequest::new("roundtrip.page.add", json!({ "a": 41 })))
            .await
    });
    let call = wait_event(&page, "call").await;
    assert_eq!(call["toolId"], 1);
    assert_eq!(call["input"], json!({ "a": 41 }));
    let reply = fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "call.result", "callId": call["callId"], "ok": true, "data": { "sum": 42 }, "stateHints": ["cart"] }),
    );
    assert_eq!(reply["ok"], true);
    let out = pending.await.expect("join").expect("调用");
    assert_eq!(out.result.expect("成功")["sum"], 42);
    assert_eq!(out.state_hints, vec!["cart".to_owned()]);

    let native = fx
        .hub
        .call_tool(CallRequest::new("roundtrip.app.native", json!({})))
        .await
        .expect("调用 Rust 工具");
    assert_eq!(native.result.expect("成功")["from"], "rust");

    // 页面报告的错误类别原样传给 Host；无法识别的类别按 HANDLER_ERROR。
    for (kind, expected) in [
        ("USER_REJECTED", ErrorKind::UserRejected),
        ("WHATEVER", ErrorKind::HandlerError),
    ] {
        let hub = fx.hub.clone();
        let pending = tokio::spawn(async move {
            hub.call_tool(CallRequest::new("roundtrip.page.add", json!({})))
                .await
        });
        let call = wait_event(&page, "call").await;
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "call.result", "callId": call["callId"], "ok": false, "kind": kind, "message": "不行" }),
        );
        let out = pending.await.expect("join").expect("调用");
        let err = out.result.expect_err("失败");
        assert_eq!(err.kind, expected);
        assert_eq!(err.message, "不行");
    }
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

fn hub_tool(hub: &Hub, name: &str) -> Option<app_mcp_hub::HubTool> {
    hub.tools(&ToolFilter::default())
        .into_iter()
        .find(|t| t.name == name)
}

/// 第 14 项 S1 / 第 19 项 R1–R3：页面与 Rust 工具的注解、输出 schema 与结构化结果经桥接到达 Hub。
#[tokio::test(flavor = "multi_thread")]
async fn annotations_output_schema_and_structured_results() {
    let fx = Fixture::new("annot", None).await;
    let page = Arc::new(FakePage::default());
    fx.op(&page, "main", "main", json!({ "op": "hello" }));
    let spec = json!({
        "description": "下单", "risk": "write",
        "annotations": { "title": "下单", "idempotentHint": true, "openWorldHint": true },
        "outputSchema": { "type": "object", "properties": { "orderId": { "type": "string" } } }
    });
    let reply = fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "tool.register", "id": 1, "name": "page.order", "spec": spec }),
    );
    assert_eq!(reply, json!({ "ok": true }));

    struct Native;
    impl ToolHandler for Native {
        fn invoke(&self, call: CallHandle) {
            let _ = call.complete_with(CallResult {
                summary: Some("没有需要清理的项".to_owned()),
                status: ResultStatus::Noop,
                ..CallResult::default()
            });
        }
    }
    fx.bridge
        .client()
        .register_tool_with(
            ToolSpec::new("app.clean", "清理"),
            ToolOptions {
                annotations: Some(ToolAnnotations {
                    destructive_hint: Some(true),
                    ..ToolAnnotations::default()
                }),
                output_schema_json: Some(r#"{"type":"array"}"#.to_owned()),
            },
            Arc::new(Native),
        )
        .expect("注册 Rust 工具");
    fx.connected("annot").await;
    eventually("Hub 看到两个工具", || tool_names(&fx.hub, "annot").len() == 2).await;

    let page_tool = hub_tool(&fx.hub, "annot.page.order").expect("页面工具");
    assert_eq!(page_tool.annotations.idempotent_hint, Some(true));
    assert_eq!(page_tool.annotations.open_world_hint, Some(true));
    assert_eq!(page_tool.annotations.title.as_deref(), Some("下单"));
    assert_eq!(page_tool.annotations.read_only_hint, Some(false), "缺少的字段按 risk 推导");
    assert_eq!(
        page_tool.output_schema,
        Some(json!({ "type": "object", "properties": { "orderId": { "type": "string" } } }))
    );
    let rust_tool = hub_tool(&fx.hub, "annot.app.clean").expect("Rust 工具");
    assert_eq!(rust_tool.annotations.destructive_hint, Some(true));
    assert_eq!(rust_tool.output_schema, Some(json!({ "type": "array" })));

    // 结构化结果：pending + stateResource + summary + 内容注解。
    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move {
        hub.call_tool(CallRequest::new("annot.page.order", json!({})))
            .await
    });
    let call = wait_event(&page, "call").await;
    let reply = fx.op(
        &page,
        "main",
        "main",
        json!({
            "op": "call.result", "callId": call["callId"], "ok": true, "data": { "orderId": "o1" },
            "status": "pending", "stateResource": "order.state", "summary": "已提交，等待用户确认",
            "annotations": { "audience": ["user"], "priority": 0.8 }
        }),
    );
    assert_eq!(reply["ok"], true);
    let out = pending.await.expect("join").expect("调用");
    assert_eq!(out.result.expect("成功")["orderId"], "o1");
    assert_eq!(out.status, ResultStatus::Pending);
    assert_eq!(out.summary.as_deref(), Some("已提交，等待用户确认"));
    assert!(
        out.state_resource.as_deref().is_some_and(|r| r.ends_with("order.state")),
        "{:?}",
        out.state_resource
    );
    let ann = out.annotations.expect("内容注解");
    assert_eq!(ann.audience, Some(vec![Audience::User]));
    assert_eq!(ann.priority, Some(0.8));

    let native = fx
        .hub
        .call_tool(CallRequest::new("annot.app.clean", json!({})))
        .await
        .expect("调用 Rust 工具");
    assert_eq!(native.status, ResultStatus::Noop);
    assert_eq!(native.summary.as_deref(), Some("没有需要清理的项"));

    // 取值不合法的 status：调用以 HANDLER_ERROR 结束，页面收到 INVALID_RESULT。
    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move {
        hub.call_tool(CallRequest::new("annot.page.order", json!({})))
            .await
    });
    let call = wait_event(&page, "call").await;
    let reply = fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "call.result", "callId": call["callId"], "ok": true, "data": 1, "status": "bogus" }),
    );
    assert_eq!(reply["code"], "INVALID_RESULT");
    let err = pending.await.expect("join").expect("调用").result.expect_err("失败");
    assert_eq!(err.kind, ErrorKind::HandlerError);

    // 整体更新（页面发送完整定义）：缺少 annotations / outputSchema 即清除声明。
    fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "tool.update", "id": 1, "spec": { "description": "下单", "risk": "read" } }),
    );
    eventually("Hub 看到注解被清除", || {
        hub_tool(&fx.hub, "annot.page.order")
            .is_some_and(|t| t.output_schema.is_none() && t.annotations.idempotent_hint.is_none())
    })
    .await;
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_id_reaches_page_and_bridge_drop_stops_client() {
    let fx = Fixture::new("cid", None).await;
    let page = Arc::new(FakePage::default());

    let hello = fx.op(&page, "main", "main", json!({ "op": "hello" }));
    assert!(hello["value"].get("connectionId").is_none(), "未连接时不带连接 ID");
    fx.op(&page, "main", "main", register(1, "page.add"));
    fx.connected("cid").await;

    let hub_cid = fx
        .hub
        .apps()
        .into_iter()
        .find(|a| a.app_id == "cid")
        .and_then(|a| a.instances.into_iter().next())
        .and_then(|i| i.connection_id)
        .expect("Hub 分配了连接 ID");
    assert_eq!(fx.bridge.client().connection_id().as_deref(), Some(hub_cid.as_str()));
    let mut connected = None;
    eventually("页面收到带连接 ID 的 connected", || {
        connected = page
            .all()
            .into_iter()
            .find(|e| e["type"] == "state" && e["state"]["status"] == "connected");
        connected.is_some()
    })
    .await;
    assert_eq!(connected.unwrap_or(Value::Null)["connectionId"], hub_cid.as_str());
    let hello = fx.op(&page, "main", "main", json!({ "op": "hello" }));
    assert_eq!(hello["value"]["connectionId"], hub_cid.as_str());

    // Sessions 与客户端之间的引用环由 Bridge 的 Drop 断开：丢弃 Bridge 后客户端停止、App 从 Hub 消失。
    let Fixture { hub, bridge, sessions } = fx;
    drop(bridge);
    eventually("丢弃 Bridge 后 App 断开", || {
        !hub.apps().iter().any(|a| a.app_id == "cid" && a.connected)
    })
    .await;
    drop(sessions);
    shutdown(hub).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn scopes_resources_and_updates() {
    let fx = Fixture::new("scopes", None).await;
    let page = Arc::new(FakePage::default());
    fx.connected("scopes").await;

    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "scope.create", "id": 10, "name": "cart" })
        )["ok"],
        true
    );
    let mut in_scope = register(11, "cart.clear");
    in_scope["scopeId"] = json!(10);
    assert_eq!(fx.op(&page, "main", "main", in_scope)["ok"], true);
    assert_eq!(
        fx.op(&page, "main", "main", register(12, "todo.add"))["ok"],
        true
    );
    eventually("两个页面工具", || {
        tool_names(&fx.hub, "scopes").len() == 2
    })
    .await;

    // 更新：整体替换定义。
    let update = json!({ "op": "tool.update", "id": 12, "spec": { "description": "新的说明", "title": "添加待办" } });
    assert_eq!(fx.op(&page, "main", "main", update)["ok"], true);
    eventually("更新生效", || {
        fx.hub.tools(&ToolFilter::default()).iter().any(|t| {
            t.tool == "todo.add"
                && t.description == "新的说明"
                && t.title.as_deref() == Some("添加待办")
        })
    })
    .await;

    // 注销 scope：其下工具一并注销。
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "scope.dispose", "id": 10 })
        )["ok"],
        true
    );
    eventually("scope 注销", || {
        tool_names(&fx.hub, "scopes") == vec!["todo.add"]
    })
    .await;
    let unknown = fx.op(&page, "main", "main", {
        let mut op = register(13, "x.y");
        op["scopeId"] = json!(10);
        op
    });
    assert_eq!(unknown["code"], "UNKNOWN_SCOPE");

    // 资源：读取转给页面。
    let res = json!({ "op": "resource.register", "id": 20, "name": "cart", "description": "购物车", "mimeType": "application/json" });
    assert_eq!(fx.op(&page, "main", "main", res)["ok"], true);
    let mut uri = String::new();
    eventually("资源出现", || {
        if let Some(r) = fx
            .hub
            .resources()
            .into_iter()
            .find(|r| r.app_id == "scopes" && r.name == "scopes.cart")
        {
            uri = r.uri;
            true
        } else {
            false
        }
    })
    .await;
    let hub = fx.hub.clone();
    let reading = tokio::spawn(async move { hub.read_resource(&uri).await });
    let read = wait_event(&page, "read").await;
    assert_eq!(read["resourceId"], 20);
    let reply = fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "read.result", "readId": read["readId"], "ok": true, "data": { "items": 3 } }),
    );
    assert_eq!(reply["ok"], true);
    let content = reading.await.expect("join").expect("读取");
    let text = content.text.unwrap_or_default();
    assert!(text.contains("\"items\":3"), "{text}");
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "resource.notify", "id": 20 })
        )["ok"],
        true
    );

    // 工具 dispose。
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "tool.dispose", "id": 12 })
        )["ok"],
        true
    );
    eventually("工具注销", || tool_names(&fx.hub, "scopes").is_empty()).await;
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

/// 页面资源的 `realtime` 传到 Hub：只有 realtime 资源的订阅算未休眠原因（spec/lifecycle.md 第 13 节 B3）。
#[tokio::test(flavor = "multi_thread")]
async fn page_resource_realtime_reaches_hub() {
    let fx = Fixture::new("realtime", None).await;
    let page = Arc::new(FakePage::default());
    fx.connected("realtime").await;

    let plain =
        json!({ "op": "resource.register", "id": 1, "name": "cart", "description": "购物车" });
    assert_eq!(fx.op(&page, "main", "main", plain)["ok"], true);
    let scope = json!({ "op": "scope.create", "id": 2, "name": "orders" });
    assert_eq!(fx.op(&page, "main", "main", scope)["ok"], true);
    let realtime = json!({ "op": "resource.register", "id": 3, "scopeId": 2, "name": "order",
                           "description": "订单状态", "realtime": true });
    assert_eq!(fx.op(&page, "main", "main", realtime)["ok"], true);

    let uri = |name: &str| -> Option<String> {
        fx.hub
            .resources()
            .into_iter()
            .find(|r| r.app_id == "realtime" && r.name == format!("realtime.{name}"))
            .map(|r| r.uri)
    };
    eventually("资源出现", || {
        uri("cart").is_some() && uri("order").is_some()
    })
    .await;
    let subscribed_realtime = || {
        fx.hub
            .status()
            .apps
            .into_iter()
            .filter(|a| a.app_id == "realtime")
            .flat_map(|a| a.instances)
            .filter_map(|i| i.power)
            .any(|p| {
                p.awake_reasons
                    .contains(&app_mcp_hub::AwakeReason::Subscription)
            })
    };

    let cart = uri("cart").unwrap_or_default();
    let order = uri("order").unwrap_or_default();
    fx.hub.subscribe(&cart).expect("订阅 cart");
    fx.hub.subscribe(&order).expect("订阅 order");
    eventually("realtime 订阅算未休眠原因", subscribed_realtime).await;
    // 只剩普通资源的订阅：不算。
    fx.hub.unsubscribe(&order);
    eventually("普通订阅不算未休眠原因", || {
        !subscribed_realtime()
    })
    .await;

    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_are_per_webview_and_end_with_window() {
    let fx = Fixture::new("windows", None).await;
    let main = Arc::new(FakePage::default());
    let other = Arc::new(FakePage::default());
    fx.connected("windows").await;

    fx.op(&main, "main", "main", json!({ "op": "hello" }));
    fx.op(&main, "main", "main", register(1, "main.tool"));
    fx.op(
        &other,
        "settings",
        "settings-window",
        json!({ "op": "hello" }),
    );
    fx.op(
        &other,
        "settings",
        "settings-window",
        register(1, "settings.tool"),
    );
    eventually("两个窗口的工具", || {
        tool_names(&fx.hub, "windows").len() == 2
    })
    .await;
    assert_eq!(fx.sessions.count(), 2);

    // 进行中的调用：窗口销毁时以 APP_DISCONNECTED 失败。
    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move {
        hub.call_tool(CallRequest::new("windows.settings.tool", json!({})))
            .await
    });
    wait_event(&other, "call").await;
    fx.sessions.end_window("settings-window");
    let out = pending.await.expect("join").expect("调用");
    assert_eq!(
        out.result.expect_err("失败").kind,
        ErrorKind::AppDisconnected
    );
    eventually("只剩主窗口工具", || {
        tool_names(&fx.hub, "windows") == vec!["main.tool"]
    })
    .await;
    assert_eq!(fx.sessions.count(), 1);

    // 页面刷新（hello）：旧登记作废，新页面可以用同一个 id / 名称重新登记。
    fx.op(&main, "main", "main", json!({ "op": "hello" }));
    eventually("刷新后注销", || {
        tool_names(&fx.hub, "windows").is_empty()
    })
    .await;
    assert_eq!(
        fx.op(&main, "main", "main", register(1, "main.tool"))["ok"],
        true
    );
    eventually("重新登记", || {
        tool_names(&fx.hub, "windows") == vec!["main.tool"]
    })
    .await;

    // reset（页面卸载）同样注销。
    assert_eq!(
        fx.op(&main, "main", "main", json!({ "op": "reset" })),
        json!({ "ok": true })
    );
    eventually("reset 后注销", || {
        tool_names(&fx.hub, "windows").is_empty()
    })
    .await;
    assert_eq!(fx.sessions.count(), 0);

    // 事件无法送达（WebView 已不存在）：会话随之注销。
    fx.op(&main, "main", "main", register(2, "gone.tool"));
    eventually("登记", || {
        tool_names(&fx.hub, "windows") == vec!["gone.tool"]
    })
    .await;
    main.gone.store(true, Ordering::SeqCst);
    let out = fx
        .hub
        .call_tool(CallRequest::new("windows.gone.tool", json!({})))
        .await
        .expect("调用");
    assert_eq!(
        out.result.expect_err("失败").kind,
        ErrorKind::AppDisconnected
    );
    eventually("送达失败后注销", || {
        tool_names(&fx.hub, "windows").is_empty()
    })
    .await;
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_is_forwarded_to_page() {
    let fx = Fixture::new("cancel", None).await;
    let page = Arc::new(FakePage::default());
    fx.connected("cancel").await;
    fx.op(&page, "main", "main", register(1, "slow.op"));
    eventually("登记", || tool_names(&fx.hub, "cancel").len() == 1).await;

    let hub = fx.hub.clone();
    let mut req = CallRequest::new("cancel.slow.op", json!({}));
    req.call_id = Some("call-to-cancel".into());
    let pending = tokio::spawn(async move { hub.call_tool(req).await });
    let call = wait_event(&page, "call").await;
    fx.hub.cancel_call("call-to-cancel");
    let cancel = wait_event(&page, "cancel").await;
    assert_eq!(cancel["callId"], call["callId"]);
    assert_eq!(cancel["kind"], "CANCELLED");
    let _ = pending.await;
    // 取消后页面的结果被忽略。
    let late = json!({ "op": "call.result", "callId": call["callId"], "ok": true, "data": null });
    assert_eq!(fx.op(&page, "main", "main", late), json!({ "ok": true }));
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_bad_ops_and_filtered_webviews() {
    let accept: Arc<bridge::AcceptFn> = Arc::new(|label: &str| label != "untrusted");
    let fx = Fixture::new("errors", Some(accept)).await;
    let page = Arc::new(FakePage::default());

    assert_eq!(
        fx.op(&page, "untrusted", "w", json!({ "op": "hello" }))["code"],
        "FORBIDDEN"
    );
    assert_eq!(
        fx.op(&page, "main", "main", json!({ "op": "nope" }))["code"],
        "INVALID_OP"
    );
    assert_eq!(
        fx.op(&page, "main", "main", json!("not an object"))["code"],
        "INVALID_OP"
    );
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "tool.register", "id": 1.5, "name": "a", "spec": { "description": "x" } })
        )["code"],
        "INVALID_OP"
    );
    assert_eq!(
        fx.op(&page, "main", "main", register(1, "bad name!"))["code"],
        "INVALID_NAME"
    );
    assert_eq!(
        fx.op(&page, "main", "main", register(2, "dup.tool"))["ok"],
        true
    );
    assert_eq!(
        fx.op(&page, "other", "other", register(1, "dup.tool"))["code"],
        "DUPLICATE_NAME"
    );
    assert_eq!(
        fx.op(&page, "main", "main", register(2, "another.tool"))["code"],
        "DUPLICATE_ID"
    );
    let bad_schema = json!({ "op": "tool.register", "id": 3, "name": "s.t", "spec": { "description": "x", "inputSchema": { "type": "string" } } });
    assert_eq!(
        fx.op(&page, "main", "main", bad_schema)["code"],
        "INVALID_SCHEMA"
    );

    // 生命周期操作转给原生客户端。
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "lifecycle.hold", "holdId": 7 })
        ),
        json!({ "ok": true })
    );
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "lifecycle.release", "holdId": 7 })
        ),
        json!({ "ok": true })
    );
    assert_eq!(
        fx.op(&page, "main", "main", json!({ "op": "lifecycle.wake" }))["ok"],
        true
    );
    assert!(fx.op(&page, "main", "main", json!({ "op": "lifecycle.sleep" }))["value"].is_boolean());

    // 停止后登记失败。
    fx.bridge.client().stop();
    assert_eq!(
        fx.op(&page, "main", "main", register(9, "late.tool"))["code"],
        "STOPPED"
    );
    shutdown(fx.hub).await;
}

#[test]
fn state_json_matches_web_connection_state() {
    let info = |status, retry_in_ms, reason: Option<&str>| StateInfo {
        status,
        retry_in_ms,
        reason: reason.map(str::to_owned),
        code: None,
    };
    assert_eq!(
        bridge::state_json(&info(StateStatus::PendingPairing, None, None)),
        json!({ "status": "pending-pairing" })
    );
    assert_eq!(
        bridge::state_json(&info(StateStatus::HostMismatch, None, Some("不是 app-mcp"))),
        json!({ "status": "host-mismatch", "reason": "不是 app-mcp", "code": "HOST_NOT_APP_MCP" })
    );
    assert_eq!(
        bridge::state_json(&info(StateStatus::Rejected, None, None)),
        json!({ "status": "rejected", "reason": "", "code": "REJECTED" })
    );
    let failed = StateInfo {
        code: Some("CONNECT_FAILED".into()),
        ..info(StateStatus::Backoff, Some(10), Some("连不上"))
    };
    let failed = bridge::state_json(&failed);
    assert_eq!(
        (failed["reason"].as_str(), failed["code"].as_str()),
        (Some("连不上"), Some("CONNECT_FAILED"))
    );
    let backoff = bridge::state_json(&info(StateStatus::Backoff, Some(1500), None));
    assert_eq!(backoff["status"], "backoff");
    assert!(backoff["retryAt"].as_u64().unwrap_or(0) > 1_500);
    for (status, name) in [
        (StateStatus::Idle, "idle"),
        (StateStatus::Connecting, "connecting"),
        (StateStatus::Handshaking, "handshaking"),
        (StateStatus::Connected, "connected"),
        (StateStatus::Stopped, "stopped"),
        (StateStatus::Dormant, "dormant"),
        (StateStatus::Waking, "waking"),
    ] {
        assert_eq!(
            bridge::state_json(&info(status, None, None)),
            json!({ "status": name })
        );
    }
}

#[test]
fn bridge_script_matches_protocol() {
    assert!(BRIDGE_SCRIPT.contains("version: 1"));
    assert_eq!(BRIDGE_VERSION, 1);
    assert!(BRIDGE_SCRIPT.contains("'plugin:app-mcp|op'"));
    assert!(BRIDGE_SCRIPT.contains("__APP_MCP_TAURI_DISPATCH__"));
    assert!(DISPATCH_FN.ends_with("__APP_MCP_TAURI_DISPATCH__"));
}

// ---------------------------------------------------------------------------
// Tauri MockRuntime
// ---------------------------------------------------------------------------

mod mock {
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{
        INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
    };
    use tauri::utils::acl::ExecutionContext;
    use tauri::webview::InvokeRequest;
    use tauri::{App, WebviewWindow, WebviewWindowBuilder};

    use super::*;

    fn app(plugin: TauriPlugin<MockRuntime>) -> App<MockRuntime> {
        let mut context = mock_context(noop_assets());
        context
            .runtime_authority_mut()
            .__allow_command("plugin:app-mcp|op".into(), ExecutionContext::Local);
        mock_builder()
            .plugin(plugin)
            .build(context)
            .expect("构建 App")
    }

    fn invoke(webview: &WebviewWindow<MockRuntime>, op: Value) -> Value {
        let request = InvokeRequest {
            cmd: "plugin:app-mcp|op".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: if cfg!(any(windows, target_os = "android")) {
                "http://tauri.localhost"
            } else {
                "tauri://localhost"
            }
            .parse()
            .expect("url"),
            body: InvokeBody::Json(json!({ "op": op })),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        };
        match get_ipc_response(webview, request) {
            Ok(body) => body.deserialize::<Value>().expect("应答 JSON"),
            Err(e) => panic!("IPC 失败：{e}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plugin_registers_page_and_rust_tools_over_tauri_ipc() {
        let (hub, ep) = start_hub("mock").await;
        let app = app(Builder::new(native_config(&ep, "mock"))
            .wake_from_args(false)
            .build());
        let app_mcp = app.app_mcp().expect("插件状态");
        assert_eq!(app_mcp.page_count(), 0);

        struct Ping;
        impl ToolHandler for Ping {
            fn invoke(&self, call: CallHandle) {
                let _ = call.complete(Some("\"pong\""), vec![]);
            }
        }
        app_mcp
            .client()
            .register_tool(ToolSpec::new("app.ping", "Rust 工具"), Arc::new(Ping))
            .expect("注册");

        let main = WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("窗口");
        let hello = invoke(&main, json!({ "op": "hello" }));
        assert_eq!(hello["ok"], true);
        assert_eq!(hello["value"]["instanceId"], app_mcp.client().instance_id());
        assert_eq!(
            invoke(&main, register(1, "page.tool")),
            json!({ "ok": true })
        );
        assert_eq!(app_mcp.page_count(), 1);

        eventually("Hub 看到 Rust 与页面工具", || {
            tool_names(&hub, "mock") == vec!["app.ping", "page.tool"]
        })
        .await;
        let apps = hub.apps();
        assert!(apps.iter().any(|a| a.app_id == "mock" && a.connected));
        let out = hub
            .call_tool(CallRequest::new("mock.app.ping", json!({})))
            .await
            .expect("调用");
        assert_eq!(out.result.expect("成功"), json!("pong"));

        // 可见性：按窗口状态上报（MockRuntime 的窗口可见、未聚焦），重复上报被去重。
        app_mcp.refresh_visibility();
        assert_eq!(
            *app_mcp
                .last_visibility
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
            Some((Visibility::Visible, false))
        );

        // 页面卸载。
        assert_eq!(
            invoke(&main, json!({ "op": "reset" })),
            json!({ "ok": true })
        );
        eventually("页面工具注销", || {
            tool_names(&hub, "mock") == vec!["app.ping"]
        })
        .await;

        // 退出：停止客户端。
        invoke(&main, register(2, "page.again"));
        app_mcp.on_run_event(&RunEvent::Exit);
        assert_eq!(app_mcp.page_count(), 0);
        assert_eq!(app_mcp.client().state().status, StateStatus::Stopped);
        shutdown(hub).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn forbidden_webview_and_wake_args() {
        let (hub, ep) = start_hub("mockfilter").await;
        let app = app(Builder::new(native_config(&ep, "mockfilter"))
            .accept_webview(|label| label == "main")
            .auto_start(false)
            .wake_from_args(false)
            .build());
        let other = WebviewWindowBuilder::new(&app, "other", Default::default())
            .build()
            .expect("窗口");
        assert_eq!(
            invoke(&other, json!({ "op": "hello" }))["code"],
            "FORBIDDEN"
        );

        let app_mcp = app.app_mcp().expect("插件状态");
        assert!(!app_mcp.handle_wake_args(["--flag", "/some/path"]));
        // 唤醒令牌：on-demand 之外也会被识别（冷启动唤醒）。
        assert!(
            app_mcp.handle_wake_args(["--flag", "app-mcp-wake:0123456789abcdef0123456789abcdef"])
        );
        app_mcp.client().stop();
        shutdown(hub).await;
    }
}
