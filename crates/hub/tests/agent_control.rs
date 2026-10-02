//! Agent 显式控制（spec/hub-api.md 3.15）：`apps.navigate`（显式导航，带页面参数）、`apps.activate` / `apps.release`、
//! MCP 请求 `_meta` 的截止时间与幂等键、`HubTool.surface` / `page`、`navigate_timeout`。
//!
//! App 端是真实的 `app-mcp-native` 客户端：覆盖核心 → 原生运行时 → Hub 的完整链路。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_mcp_hub::{
    CallRequest, ErrorKind, Hub, HubConfig, HubError, PolicyConfig, ToolError, ToolFilter, ToolSurface, WakeRequest, Waker,
    async_trait,
};
use app_mcp_native::{
    CallHandle, LifecycleMode, NavigateHandle, NavigationHandler, NativeClient, NativeConfig, StateStatus, ToolHandler,
    ToolOptions, ToolSpec, ToolSurface as NativeSurface, WakeDescriptor as NativeWake, WakeKind as NativeWakeKind,
};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);

fn manifest() -> app_mcp_manifest::Manifest {
    app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": "shop", "name": "商城",
            "pages": [
                {"name": "detail", "title": "商品详情",
                 "params": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]},
                 "tools": [{"name": "detail.buy", "description": "购买", "inputSchema": {"type": "object"}, "surface": "view"}]},
                {"name": "admin", "navigable": false, "tools": [
                    {"name": "admin.reset", "description": "重置", "inputSchema": {"type": "object"}}
                ]},
                {"name": "locked", "tools": [
                    {"name": "locked.op", "description": "被 App 拒绝", "inputSchema": {"type": "object"}}
                ]},
                {"name": "never", "tools": [
                    {"name": "never.op", "description": "导航回调不完成", "inputSchema": {"type": "object"}}
                ]}
            ]
        })
        .to_string(),
    )
    .expect("manifest")
}

fn config() -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        list_changed_debounce: Duration::from_millis(10),
        wake_timeout: Duration::from_secs(5),
        navigate_timeout: Duration::from_millis(400),
        manifests: vec![manifest()],
        ..Default::default()
    }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + T;
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "等待超时：{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 返回工具名与 handler 上下文中的幂等键；参数带 `delayMs` 时先等待。
struct Echo;
impl ToolHandler for Echo {
    fn invoke(&self, call: CallHandle) {
        std::thread::spawn(move || {
            let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or(Value::Null);
            if let Some(ms) = args["delayMs"].as_u64() {
                std::thread::sleep(Duration::from_millis(ms));
            }
            let out = json!({ "tool": call.tool_name(), "idempotencyKey": call.idempotency_key() });
            let _ = call.complete(Some(&out.to_string()), vec![]);
        });
    }
}

/// 导航回调：记录（页面, 参数）；`locked` 拒绝，`never` 不完成（Hub 侧超时），其他完成。
#[derive(Default)]
struct Navigator {
    requests: Mutex<Vec<(String, Option<Value>)>>,
    pending: Mutex<Vec<NavigateHandle>>,
}

impl NavigationHandler for Navigator {
    fn navigate(&self, request: NavigateHandle) {
        let params = request.params_json().and_then(|p| serde_json::from_str(&p).ok());
        let page = request.page();
        self.requests.lock().unwrap().push((page.clone(), params));
        match page.as_str() {
            "locked" => request.deny("正在编辑订单，请先保存").unwrap(),
            "never" => self.pending.lock().unwrap().push(request),
            _ => request.complete().unwrap(),
        }
    }
}

impl Navigator {
    fn pages(&self) -> Vec<String> {
        self.requests.lock().unwrap().iter().map(|(p, _)| p.clone()).collect()
    }
}

fn app(hub: &Hub, navigation: bool, mode: LifecycleMode) -> (NativeClient, Arc<Navigator>) {
    let mut c = NativeConfig::new("shop", "商城");
    c.host_url = format!("ws://{}/app", hub.listen_addr().unwrap());
    c.instance_id = Some("shop-1".to_owned());
    c.lifecycle.mode = mode;
    if mode == LifecycleMode::Idle {
        c.launch_token = Some(String::new());
        c.lifecycle.idle_timeout_ms = 150;
        c.lifecycle.wake = Some(NativeWake { kind: NativeWakeKind::Uri, target: Some("shop-app".into()), background: true });
    }
    let client = NativeClient::new(c, None).unwrap();
    let home = ToolOptions { surface: NativeSurface::View, page: Some("home".into()), ..ToolOptions::default() };
    client.register_tool_with(ToolSpec::new("home.search", "搜索"), home, Arc::new(Echo)).unwrap();
    client.register_tool(ToolSpec::new("cart.add", "加入购物车"), Arc::new(Echo)).unwrap();
    let nav = Arc::new(Navigator::default());
    if navigation {
        client.set_navigation_handler(Some(nav.clone()));
    }
    (client, nav)
}

async fn connected(hub: &Hub) {
    eventually("App 注册工具", || hub.status().apps.iter().any(|a| a.app_id == "shop" && !a.tools.is_empty())).await;
}

async fn call(hub: &Hub, name: &str, args: Value) -> Result<Value, ToolError> {
    tokio::time::timeout(T, hub.call_tool(CallRequest::new(name, args))).await.expect("调用超时").unwrap().result
}

fn reason(e: &ToolError) -> Option<&str> {
    e.details.as_ref()?.get("reason")?.as_str()
}

/// 假 Waker：把激活参数交给客户端的 `handle_wake`（模拟 OS 激活）。
#[derive(Default)]
struct FakeWaker {
    client: Mutex<Option<NativeClient>>,
    count: Mutex<usize>,
}

#[async_trait]
impl Waker for FakeWaker {
    async fn wake(&self, req: WakeRequest) -> Result<(), HubError> {
        *self.count.lock().unwrap() += 1;
        if let Some(c) = self.client.lock().unwrap().clone() {
            assert!(c.handle_wake(&req.activation_arg));
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn explicit_navigation_with_params() {
    let hub = Hub::start(config()).await.unwrap();
    let (client, nav) = app(&hub, true, LifecycleMode::Persistent);
    client.start();
    connected(&hub).await;

    // 有页面目录时列出 apps.navigate；activate / release 总是列出
    let listed: Vec<String> = hub.tools(&ToolFilter::default()).into_iter().map(|t| t.name).collect();
    for b in ["apps.navigate", "apps.activate", "apps.release"] {
        assert!(listed.contains(&b.to_owned()), "{b}: {listed:?}");
    }

    // 带参数导航：参数原样到达导航回调
    let out = call(&hub, "apps.navigate", json!({"appId": "shop", "page": "detail", "params": {"id": "42"}})).await.unwrap();
    assert_eq!((out["ok"].clone(), out["page"].clone(), out["woke"].clone()), (json!(true), json!("detail"), json!(false)));
    assert_eq!(out["instanceId"], "shop-1");
    assert_eq!(nav.requests.lock().unwrap().clone(), [("detail".to_owned(), Some(json!({"id": "42"})))]);

    // 参数不符合页面的 params schema：不导航（未启用 schema-validation 时 Hub 不校验）
    if app_mcp_hub::features::SCHEMA_VALIDATION {
        let e = call(&hub, "apps.navigate", json!({"appId": "shop", "page": "detail"})).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidInput);
        let e = call(&hub, "apps.navigate", json!({"appId": "shop", "page": "detail", "params": {"id": 1}})).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidInput);
        assert_eq!(nav.pages().len(), 1);
    }
    // 没有 params schema 的页面：不带参数导航，请求中没有 params
    call(&hub, "apps.navigate", json!({"appId": "shop", "page": "home"})).await.unwrap();
    assert_eq!(nav.requests.lock().unwrap().last().cloned(), Some(("home".to_owned(), None)));

    // 未知页面 / App → TOOL_NOT_FOUND；navigable: false → NAVIGATION_DENIED（not-navigable），不发请求
    let e = call(&hub, "apps.navigate", json!({"appId": "shop", "page": "nope"})).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::ToolNotFound);
    let e = call(&hub, "apps.navigate", json!({"appId": "nope", "page": "detail"})).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::ToolNotFound);
    let e = call(&hub, "apps.navigate", json!({"appId": "shop", "page": "admin"})).await.unwrap_err();
    assert_eq!((e.kind, reason(&e)), (ErrorKind::NavigationDenied, Some("not-navigable")));
    // App 拒绝 → NAVIGATION_DENIED（app）
    let e = call(&hub, "apps.navigate", json!({"appId": "shop", "page": "locked"})).await.unwrap_err();
    assert_eq!((e.kind, reason(&e)), (ErrorKind::NavigationDenied, Some("app")));
    // 回复超时按 navigate_timeout（400 ms），而不是 wake_timeout（5 s）
    let started = Instant::now();
    let e = call(&hub, "apps.navigate", json!({"appId": "shop", "page": "never"})).await.unwrap_err();
    assert_eq!((e.kind, reason(&e)), (ErrorKind::NavigationFailed, Some("timeout")));
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert_eq!(nav.pages(), ["detail", "home", "locked", "never"]);

    // 策略 call 执行点（App 级 deny）对显式导航生效，不发请求
    let rules = PolicyConfig::from_json(&json!({"rules": [{"id": "no-shop", "action": "deny", "app": "shop"}]}).to_string());
    hub.set_policy(rules.unwrap()).unwrap();
    let e = call(&hub, "apps.navigate", json!({"appId": "shop", "page": "home"})).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::PolicyDenied);
    assert_eq!(nav.pages().len(), 4);
    client.stop();
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn hub_tool_surface_page_and_idempotency_key() {
    let hub = Hub::start(config()).await.unwrap();
    let (client, _nav) = app(&hub, true, LifecycleMode::Persistent);
    client.start();
    connected(&hub).await;

    let tools = hub.tools(&ToolFilter { apps: Some(vec!["shop".into()]), include_builtin: false, ..Default::default() });
    let tool = |n: &str| tools.iter().find(|t| t.name == n).cloned().unwrap();
    assert_eq!((tool("shop.home.search").surface, tool("shop.home.search").page), (Some(ToolSurface::View), Some("home".into())));
    assert_eq!((tool("shop.cart.add").surface, tool("shop.cart.add").page), (Some(ToolSurface::App), None));
    let json = serde_json::to_value(tool("shop.home.search")).unwrap();
    assert_eq!((json["surface"].clone(), json["page"].clone()), (json!("view"), json!("home")));
    let builtin = hub.tools(&ToolFilter::default()).into_iter().find(|t| t.name == "apps.list").unwrap();
    let json = serde_json::to_value(builtin).unwrap();
    assert!(json.get("surface").is_none() && json.get("page").is_none(), "内置工具不带 surface / page：{json}");
    // apps.page 的工具带所在页面（清单中嵌套在页面下的工具没有声明 page）
    let out = call(&hub, "apps.page", json!({"appId": "shop", "page": "detail"})).await.unwrap();
    assert_eq!((out["tools"][0]["page"].clone(), out["tools"][0]["surface"].clone()), (json!("detail"), json!("view")));

    // Hub API 的幂等键原样到达 handler 上下文
    let mut req = CallRequest::new("shop.cart.add", json!({}));
    req.idempotency_key = Some("order-7".into());
    let out = hub.call_tool(req).await.unwrap();
    assert_eq!(out.result.unwrap()["idempotencyKey"], "order-7");
    let out = call(&hub, "shop.cart.add", json!({})).await.unwrap();
    assert_eq!(out["idempotencyKey"], Value::Null);
    // 不合法的键：不转发，INVALID_INPUT
    let mut req = CallRequest::new("shop.cart.add", json!({}));
    req.idempotency_key = Some(String::new());
    let e = hub.call_tool(req).await.unwrap().result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidInput);
    client.stop();
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn activate_release_and_navigate_wake_dormant_app() {
    let hub = Hub::start(config()).await.unwrap();
    let waker = Arc::new(FakeWaker::default());
    hub.set_waker(waker.clone());
    let (client, nav) = app(&hub, true, LifecycleMode::Idle);
    *waker.client.lock().unwrap() = Some(client.clone());
    client.start();
    let c = client.clone();
    eventually("App 休眠", move || c.state().status == StateStatus::Dormant).await;

    // activate：唤醒（不调用工具）并发租约；再次 activate 不重复唤醒
    let out = call(&hub, "apps.activate", json!({"appId": "shop"})).await.unwrap();
    assert_eq!((out["woke"].clone(), out["state"].clone(), out["instanceId"].clone()), (json!(true), json!("connected"), json!("shop-1")));
    assert_eq!(*waker.count.lock().unwrap(), 1);
    let out = call(&hub, "apps.activate", json!({"appId": "shop"})).await.unwrap();
    assert_eq!(out["woke"], false);
    assert_eq!(*waker.count.lock().unwrap(), 1);
    // 租约内不休眠（空闲时长 150 ms）
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(client.state().status, StateStatus::Connected);

    // release：收回本会话的租约 → App 按自己的空闲设置休眠
    let out = call(&hub, "apps.release", json!({"appId": "shop"})).await.unwrap();
    assert_eq!(out["released"], 1);
    let c = client.clone();
    eventually("释放后休眠", move || c.state().status == StateStatus::Dormant).await;
    let out = call(&hub, "apps.release", json!({"appId": "shop"})).await.unwrap();
    assert_eq!(out["released"], 0, "没有租约时不发消息");

    // 显式导航：休眠时先唤醒再导航
    let out = call(&hub, "apps.navigate", json!({"appId": "shop", "page": "detail", "params": {"id": "9"}})).await.unwrap();
    assert_eq!(out["woke"], true);
    assert_eq!(*waker.count.lock().unwrap(), 2);
    assert_eq!(nav.pages(), ["detail"]);

    // 策略 wake 执行点对 activate 生效
    let e = call(&hub, "apps.activate", json!({"appId": "nope"})).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::ToolNotFound);
    client.stop();
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn activate_respects_wake_policy_and_unsupported_navigation() {
    let mut cfg = config();
    cfg.policy = PolicyConfig::from_json(
        &json!({"rules": [{"id": "no-wake", "action": "deny", "app": "shop", "hooks": ["wake"]}]}).to_string(),
    )
    .unwrap();
    let hub = Hub::start(cfg).await.unwrap();
    let waker = Arc::new(FakeWaker::default());
    hub.set_waker(waker.clone());
    let (client, nav) = app(&hub, false, LifecycleMode::Idle);
    *waker.client.lock().unwrap() = Some(client.clone());
    client.start();
    let c = client.clone();
    eventually("App 休眠", move || c.state().status == StateStatus::Dormant).await;
    let e = call(&hub, "apps.activate", json!({"appId": "shop"})).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::PolicyDenied);
    assert_eq!(*waker.count.lock().unwrap(), 0);

    hub.set_policy(PolicyConfig::default()).unwrap();
    call(&hub, "apps.activate", json!({"appId": "shop"})).await.unwrap();
    // App 没有导航回调：显式导航 → NAVIGATION_FAILED（unsupported）
    let e = call(&hub, "apps.navigate", json!({"appId": "shop", "page": "detail", "params": {"id": "1"}})).await.unwrap_err();
    assert_eq!((e.kind, reason(&e)), (ErrorKind::NavigationFailed, Some("unsupported")));
    assert!(nav.pages().is_empty());
    client.stop();
    hub.shutdown().await;
}

/// MCP 请求 `_meta`：`app-mcp/idempotencyKey` 原样到达 handler；`app-mcp/timeoutMs` 与配置值取较小者；不合法时不执行。
#[cfg(feature = "mcp-server")]
mod mcp {
    use super::*;
    use app_mcp_hub::names::{META_IDEMPOTENCY_KEY, META_TIMEOUT_MS};
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, CallToolResult, RequestMetaObject};

    async fn mcp_call(hub: &Hub, name: &str, args: Value, meta: Value) -> CallToolResult {
        let (c, s) = tokio::io::duplex(1 << 20);
        let session = hub.mcp_session();
        tokio::spawn(async move {
            if let Ok(svc) = session.serve(s).await {
                let _ = svc.waiting().await;
            }
        });
        let client = ().serve(c).await.expect("mcp");
        let mut params = CallToolRequestParams::new(name.to_owned());
        params.arguments = args.as_object().cloned();
        params.meta = meta.as_object().cloned().map(RequestMetaObject::from);
        let r = tokio::time::timeout(T, client.peer().call_tool(params)).await.expect("MCP 调用超时").expect("call");
        let _ = client.cancel().await;
        r
    }

    fn structured(r: &CallToolResult) -> Value {
        r.structured_content.clone().unwrap_or(Value::Null)
    }

    fn text(r: &CallToolResult) -> String {
        r.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect::<Vec<_>>().join("\n")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn request_meta_deadline_and_idempotency_key() {
        let hub = Hub::start(config()).await.unwrap();
        let (client, _nav) = app(&hub, false, LifecycleMode::Persistent);
        client.start();
        connected(&hub).await;

        let r = mcp_call(&hub, "shop.cart.add", json!({}), json!({ META_IDEMPOTENCY_KEY: "k-1" })).await;
        assert_ne!(r.is_error, Some(true), "{}", text(&r));
        assert_eq!(structured(&r)["idempotencyKey"], "k-1");

        // Agent 截止时间 300 ms（配置的 response_timeout 为 35 s）：handler 要 3 s，按 300 ms 结束
        let started = Instant::now();
        let r = mcp_call(&hub, "shop.cart.add", json!({"delayMs": 3000}), json!({ META_TIMEOUT_MS: 300 })).await;
        assert_eq!(r.is_error, Some(true));
        assert!(text(&r).contains("TIMEOUT"), "{}", text(&r));
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());

        // 不合法的 _meta：不执行，INVALID_INPUT
        for bad in [json!({ META_TIMEOUT_MS: 0 }), json!({ META_TIMEOUT_MS: "5" }), json!({ META_IDEMPOTENCY_KEY: 7 })] {
            let r = mcp_call(&hub, "shop.cart.add", json!({}), bad).await;
            assert_eq!(r.is_error, Some(true));
            assert!(text(&r).contains("INVALID_INPUT"), "{}", text(&r));
        }
        client.stop();
        hub.shutdown().await;
    }
}
