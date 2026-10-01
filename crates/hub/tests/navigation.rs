//! 第 4c 项（spec/hub-api.md 3.14、spec/protocol.md 3.4）：页面目录的渐进披露与"调用不在当前页面的工具时先导航"。
//!
//! App 端是真实的 `app-mcp-native` 客户端（导航回调在页面 `cart` 上注册 `cart.checkout`），覆盖核心 → 原生运行时 →
//! Hub 的完整链路：`apps.tools` 页面摘要、`apps.page`、跨页调用自动导航、`navigable: false`、App 拒绝、导航后工具未出现、
//! 不支持导航、P2 策略（hide / deny）、休眠 App 先唤醒再导航、`view` 工具只列首选实例的。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{CallRequest, ErrorKind, Hub, HubConfig, HubError, PolicyConfig, ToolFilter, WakeRequest, Waker, async_trait};
use app_mcp_native::{
    CallHandle, LifecycleMode, NavigateHandle, NavigationHandler, NativeClient, NativeConfig, StateStatus, ToolHandler,
    ToolOptions, ToolSpec, ToolSurface, WakeDescriptor as NativeWake, WakeKind as NativeWakeKind,
};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);

fn manifest() -> app_mcp_manifest::Manifest {
    app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": "shop", "name": "商城",
            "pages": [
                {"name": "cart", "title": "购物车", "description": "查看与结算购物车", "tools": [
                    {"name": "cart.checkout", "description": "结算", "inputSchema": {"type": "object"}, "surface": "view",
                     "annotations": {"readOnlyHint": false}}
                ]},
                {"name": "admin", "navigable": false, "tools": [
                    {"name": "admin.reset", "description": "重置", "inputSchema": {"type": "object"}}
                ]},
                {"name": "locked", "tools": [
                    {"name": "locked.op", "description": "被 App 拒绝", "inputSchema": {"type": "object"}}
                ]},
                {"name": "slow", "tools": [
                    {"name": "slow.op", "description": "导航后不出现", "inputSchema": {"type": "object"}}
                ]}
            ]
        })
        .to_string(),
    )
    .expect("manifest")
}

fn config(policy: PolicyConfig) -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        list_changed_debounce: Duration::from_millis(10),
        wake_timeout: Duration::from_secs(2),
        manifests: vec![manifest()],
        policy,
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

struct Echo;
impl ToolHandler for Echo {
    fn invoke(&self, call: CallHandle) {
        let name = call.tool_name();
        std::thread::spawn(move || {
            let _ = call.complete(Some(&json!({ "tool": name }).to_string()), vec![]);
        });
    }
}

fn view_tool(page: &str) -> ToolOptions {
    ToolOptions { surface: ToolSurface::View, page: Some(page.to_owned()), ..ToolOptions::default() }
}

/// 导航回调：记录请求；`cart` 注册 `cart.checkout` 后完成，`locked` 拒绝，`slow` 完成但不注册，其他失败。
struct Navigator {
    client: Mutex<Option<NativeClient>>,
    pages: Mutex<Vec<String>>,
}

impl NavigationHandler for Navigator {
    fn navigate(&self, request: NavigateHandle) {
        let page = request.page();
        self.pages.lock().unwrap().push(page.clone());
        let client = self.client.lock().unwrap().clone();
        match (page.as_str(), client) {
            ("cart", Some(c)) => {
                // 已注册（重复导航）时忽略重名错误。
                let _ = c.register_tool_with(ToolSpec::new("cart.checkout", "结算"), view_tool("cart"), Arc::new(Echo));
                request.complete().unwrap();
            }
            ("locked", _) => request.deny("正在编辑订单，请先保存").unwrap(),
            ("slow", _) => request.complete().unwrap(),
            _ => request.fail("没有该页面").unwrap(),
        }
    }
}

fn app(hub: &Hub, instance_id: &str, navigation: bool, mode: LifecycleMode) -> (NativeClient, Arc<Navigator>) {
    let mut c = NativeConfig::new("shop", "商城");
    c.host_url = format!("ws://{}/app", hub.listen_addr().unwrap());
    c.instance_id = Some(instance_id.to_owned());
    c.lifecycle.mode = mode;
    if mode == LifecycleMode::Idle {
        c.launch_token = Some(String::new());
        c.lifecycle.idle_timeout_ms = 150;
        c.lifecycle.wake = Some(NativeWake { kind: NativeWakeKind::Uri, target: Some("shop-app".into()), background: true });
    }
    let client = NativeClient::new(c, None).unwrap();
    client
        .register_tool_with(ToolSpec::new("home.search", "搜索"), view_tool("home"), Arc::new(Echo))
        .unwrap();
    let nav = Arc::new(Navigator { client: Mutex::new(Some(client.clone())), pages: Mutex::new(Vec::new()) });
    if navigation {
        client.set_navigation_handler(Some(nav.clone()));
    }
    (client, nav)
}

async fn connected(hub: &Hub) {
    eventually("App 注册工具", || hub.status().apps.iter().any(|a| a.app_id == "shop" && !a.tools.is_empty())).await;
}

async fn call(hub: &Hub, name: &str, args: Value) -> Result<Value, app_mcp_hub::ToolError> {
    tokio::time::timeout(T, hub.call_tool(CallRequest::new(name, args))).await.expect("调用超时").unwrap().result
}

fn reason(e: &app_mcp_hub::ToolError) -> Option<&str> {
    e.details.as_ref()?.get("reason")?.as_str()
}

#[tokio::test(flavor = "multi_thread")]
async fn progressive_disclosure_and_cross_page_call() {
    let hub = Hub::start(config(PolicyConfig::default())).await.unwrap();
    let (client, nav) = app(&hub, "shop-1", true, LifecycleMode::Persistent);
    client.start();
    connected(&hub).await;

    // L1：页面目录中的工具不进列表；当前页面的 view 工具（首选实例）在列表中；有页面目录时列出 apps.page
    let listed: Vec<String> = hub.tools(&ToolFilter::default()).into_iter().map(|t| t.name).collect();
    assert!(listed.contains(&"shop.home.search".to_owned()), "{listed:?}");
    assert!(!listed.iter().any(|n| n == "shop.cart.checkout" || n == "shop.admin.reset"), "{listed:?}");
    assert!(listed.contains(&"apps.page".to_owned()), "{listed:?}");

    // L0：apps.list 的页面数与实例的导航能力
    let apps = call(&hub, "apps.list", json!({})).await.unwrap();
    let shop = apps["apps"].as_array().unwrap().iter().find(|a| a["appId"] == "shop").unwrap().clone();
    assert_eq!(shop["pageCount"], 5, "清单 4 页 + 运行时上报的 home");
    assert_eq!(shop["instances"][0]["navigation"], true);

    // L2：apps.tools 的页面摘要
    let out = call(&hub, "apps.tools", json!({"appId": "shop"})).await.unwrap();
    let pages = out["pages"].as_array().unwrap();
    let page = |n: &str| pages.iter().find(|p| p["name"] == n).cloned().unwrap_or(Value::Null);
    assert_eq!(page("home")["current"], true);
    assert_eq!((page("cart")["current"].clone(), page("cart")["toolCount"].clone()), (json!(false), json!(1)));
    assert_eq!(page("admin")["navigable"], false);

    // L3：apps.page
    let out = call(&hub, "apps.page", json!({"appId": "shop", "page": "cart"})).await.unwrap();
    assert_eq!(out["page"]["title"], "购物车");
    assert_eq!(out["tools"][0]["name"], "shop.cart.checkout");
    assert_eq!(out["tools"][0]["availability"], "notRegistered");
    let e = call(&hub, "apps.page", json!({"appId": "shop", "page": "nope"})).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::ToolNotFound);
    let e = call(&hub, "apps.page", json!({"appId": "nope", "page": "cart"})).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::ToolNotFound);

    // 跨页调用：导航 → 等待注册 → 派发
    let out = call(&hub, "shop.cart.checkout", json!({})).await.unwrap();
    assert_eq!(out["tool"], "cart.checkout");
    assert_eq!(nav.pages.lock().unwrap().clone(), ["cart"]);
    // 已在该页面：不再导航
    call(&hub, "shop.cart.checkout", json!({})).await.unwrap();
    assert_eq!(nav.pages.lock().unwrap().len(), 1);
    let out = call(&hub, "apps.page", json!({"appId": "shop", "page": "cart"})).await.unwrap();
    assert_eq!(out["page"]["current"], true);

    // navigable: false → NAVIGATION_DENIED（not-navigable），不发导航
    let e = call(&hub, "shop.admin.reset", json!({})).await.unwrap_err();
    assert_eq!((e.kind, reason(&e)), (ErrorKind::NavigationDenied, Some("not-navigable")));
    // App 拒绝 → NAVIGATION_DENIED（app），带页面
    let e = call(&hub, "shop.locked.op", json!({})).await.unwrap_err();
    assert_eq!((e.kind, reason(&e)), (ErrorKind::NavigationDenied, Some("app")));
    assert_eq!(e.details.as_ref().and_then(|d| d.get("page")), Some(&json!("locked")));
    assert!(e.message.contains("正在编辑订单"));
    // 导航完成但工具没有出现 → NAVIGATION_FAILED（tool-not-registered）
    let e = call(&hub, "shop.slow.op", json!({})).await.unwrap_err();
    assert_eq!((e.kind, reason(&e)), (ErrorKind::NavigationFailed, Some("tool-not-registered")));
    assert_eq!(nav.pages.lock().unwrap().clone(), ["cart", "locked", "slow"]);
    // 参数不符合目录定义：不导航（未启用 schema-validation 时 Hub 不校验）
    if app_mcp_hub::features::SCHEMA_VALIDATION {
        let e = call(&hub, "shop.slow.op", json!([1])).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidInput);
        assert_eq!(nav.pages.lock().unwrap().len(), 3);
    }
    client.stop();
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn app_without_navigation_is_unsupported() {
    let hub = Hub::start(config(PolicyConfig::default())).await.unwrap();
    let (client, nav) = app(&hub, "shop-1", false, LifecycleMode::Persistent);
    client.start();
    connected(&hub).await;
    let e = call(&hub, "shop.cart.checkout", json!({})).await.unwrap_err();
    assert_eq!((e.kind, reason(&e)), (ErrorKind::NavigationFailed, Some("unsupported")));
    assert!(nav.pages.lock().unwrap().is_empty());
    client.stop();
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn policy_hides_and_denies_page_tools() {
    let rules = PolicyConfig::from_json(
        &json!({"rules": [
            {"id": "hide-cart", "action": "hide", "app": "shop", "tool": "cart.*"},
            {"id": "no-locked", "action": "deny", "app": "shop", "tool": "locked.*"},
        ]})
        .to_string(),
    )
    .unwrap();
    let hub = Hub::start(config(rules)).await.unwrap();
    let (client, nav) = app(&hub, "shop-1", true, LifecycleMode::Persistent);
    client.start();
    connected(&hub).await;

    // hide：页面工具全部被隐藏的页面不出现在目录中，调用与不存在相同，不导航
    let out = call(&hub, "apps.tools", json!({"appId": "shop"})).await.unwrap();
    assert!(!out["pages"].as_array().unwrap().iter().any(|p| p["name"] == "cart"), "{out}");
    let e = call(&hub, "apps.page", json!({"appId": "shop", "page": "cart"})).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::ToolNotFound);
    let e = call(&hub, "shop.cart.checkout", json!({})).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::ToolNotFound);
    // deny：POLICY_DENIED，不导航
    let e = call(&hub, "shop.locked.op", json!({})).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::PolicyDenied);
    assert!(nav.pages.lock().unwrap().is_empty());
    let apps = call(&hub, "apps.list", json!({})).await.unwrap();
    assert_eq!(apps["apps"][0]["pageCount"], 4, "cart 被隐藏");
    client.stop();
    hub.shutdown().await;
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
async fn dormant_app_is_woken_then_navigated() {
    let hub = Hub::start(config(PolicyConfig::default())).await.unwrap();
    let waker = Arc::new(FakeWaker::default());
    hub.set_waker(waker.clone());
    let (client, nav) = app(&hub, "shop-1", true, LifecycleMode::Idle);
    *waker.client.lock().unwrap() = Some(client.clone());
    client.start();
    let c = client.clone();
    eventually("App 休眠", move || c.state().status == StateStatus::Dormant).await;

    let out = call(&hub, "shop.cart.checkout", json!({})).await.unwrap();
    assert_eq!(out["tool"], "cart.checkout");
    assert_eq!(*waker.count.lock().unwrap(), 1);
    assert_eq!(nav.pages.lock().unwrap().clone(), ["cart"]);
    client.stop();
    hub.shutdown().await;
}
