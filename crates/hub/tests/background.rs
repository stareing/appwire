//! 第 4c 项后续（spec/hub-api.md 3.14「后台替代」、spec/protocol.md 3.4「后台与前台」）：App 在后台时
//! view 工具的调用——SDK 立即以 USER_ACTION_REQUIRED（foreground）拒绝导航（不等超时），Hub 原样转给调用方；
//! view 工具声明了 `backgroundTool` 时 Hub 改调该 app 工具（已知在后台时直接改调，导航被以 foreground 拒绝时再改调），
//! 结果带 `routed_to`；声明不可用时不改调；策略与资源保护按被改调的工具执行。
//!
//! App 端是真实的 `app-mcp-native` 客户端，覆盖核心 → 原生运行时 → Hub 的完整链路。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_mcp_hub::{CallOutcome, CallRequest, ErrorKind, Hub, HubConfig, PolicyConfig};
use app_mcp_native::{
    CallHandle, NavigateHandle, NavigationHandler, NativeClient, NativeConfig, ToolHandle, ToolHandler, ToolOptions,
    ToolSpec, ToolSurface, Visibility,
};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);

const SKU_SCHEMA: &str = r#"{"type": "object", "properties": {"sku": {"type": "string"}}, "required": ["sku"]}"#;

fn manifest() -> app_mcp_manifest::Manifest {
    let sku = serde_json::from_str::<Value>(SKU_SCHEMA).unwrap();
    app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": "shop", "name": "商城",
            "tools": [
                {"name": "cart.add", "description": "加入购物车（后台可用）", "inputSchema": sku}
            ],
            "pages": [
                {"name": "cart", "tools": [
                    {"name": "cart.viewAdd", "description": "在购物车页加入", "inputSchema": sku, "surface": "view",
                     "backgroundTool": "cart.add"}
                ]},
                {"name": "orders", "tools": [
                    {"name": "orders.view", "description": "只能在前台", "inputSchema": {"type": "object"}, "surface": "view"}
                ]},
                {"name": "broken", "tools": [
                    {"name": "broken.unknown", "description": "替代不存在", "inputSchema": {"type": "object"},
                     "surface": "view", "backgroundTool": "nope"}
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
        wake_timeout: Duration::from_secs(3),
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

fn view(page: &str, background_tool: Option<&str>) -> ToolOptions {
    ToolOptions {
        surface: ToolSurface::View,
        page: Some(page.to_owned()),
        background_tool: background_tool.map(str::to_owned),
        ..ToolOptions::default()
    }
}

fn sku_spec(name: &str) -> ToolSpec {
    let mut s = ToolSpec::new(name, "加入");
    s.input_schema_json = Some(SKU_SCHEMA.to_owned());
    s
}

/// 导航回调：记录请求；`cart` 注册 `cart.viewAdd` 后完成——`refuse` 置位时改为以 USER_ACTION_REQUIRED（foreground）
/// 回复（App 自行判断不能回到前台）；`orders` 注册 `orders.view`；其他失败。
struct Navigator {
    client: Mutex<Option<NativeClient>>,
    pages: Mutex<Vec<String>>,
    refuse: AtomicBool,
    cart_tool: Mutex<Option<ToolHandle>>,
}

impl NavigationHandler for Navigator {
    fn navigate(&self, request: NavigateHandle) {
        let page = request.page();
        self.pages.lock().unwrap().push(page.clone());
        if self.refuse.load(Ordering::SeqCst) {
            request.fail_user_action("已发通知，请点开 App 继续", Some("foreground"), Some("shop://cart")).unwrap();
            return;
        }
        let client = self.client.lock().unwrap().clone();
        match (page.as_str(), client) {
            ("cart", Some(c)) => {
                if let Ok(h) = c.register_tool_with(sku_spec("cart.viewAdd"), view("cart", Some("cart.add")), Arc::new(Echo)) {
                    *self.cart_tool.lock().unwrap() = Some(h);
                }
                request.complete().unwrap();
            }
            ("orders", Some(c)) => {
                let _ = c.register_tool_with(ToolSpec::new("orders.view", "只能在前台"), view("orders", None), Arc::new(Echo));
                request.complete().unwrap();
            }
            _ => request.fail("没有该页面").unwrap(),
        }
    }
}

impl Navigator {
    /// 模拟离开购物车页：view 工具注销。
    fn leave_cart(&self) {
        if let Some(h) = self.cart_tool.lock().unwrap().take() {
            h.dispose();
        }
    }
}

fn app(hub: &Hub) -> (NativeClient, Arc<Navigator>) {
    let mut c = NativeConfig::new("shop", "商城");
    c.host_url = format!("ws://{}/app", hub.listen_addr().unwrap());
    c.instance_id = Some("shop-1".to_owned());
    let client = NativeClient::new(c, None).unwrap();
    client.register_tool_with(sku_spec("cart.add"), ToolOptions::default(), Arc::new(Echo)).unwrap();
    let nav = Arc::new(Navigator {
        client: Mutex::new(Some(client.clone())),
        pages: Mutex::new(Vec::new()),
        refuse: AtomicBool::new(false),
        cart_tool: Mutex::new(None),
    });
    client.set_navigation_handler(Some(nav.clone()));
    // 模拟不能自行回到前台的平台（Android / iOS）：不可见时导航立即被拒绝。
    client.set_navigate_in_background(false);
    (client, nav)
}

/// 等实例注册 `cart.add`（`tools/sync` 之后紧接 `app/ready`，再留一点时间让 Hub 处理）。
async fn connected(hub: &Hub) {
    eventually("App 注册 cart.add", || {
        hub.tools(&Default::default())
            .iter()
            .any(|t| t.name == "shop.cart.add" && t.availability == app_mcp_hub::Availability::Available)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

async fn call(hub: &Hub, name: &str, args: Value) -> CallOutcome {
    tokio::time::timeout(T, hub.call_tool(CallRequest::new(name, args))).await.expect("调用超时").unwrap()
}

fn reason(e: &app_mcp_hub::ToolError) -> Option<&str> {
    e.details.as_ref()?.get("reason")?.as_str()
}

/// Hub 看到的实例可见性已变为 `v`。
async fn hub_sees(hub: &Hub, v: &str) {
    eventually("Hub 收到 app/visibility", || {
        hub.status()
            .apps
            .iter()
            .flat_map(|a| a.instances.iter())
            .any(|i| serde_json::to_value(i.info.visibility).ok() == Some(json!(v)))
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn foreground_routing_and_fast_refusal() {
    let hub = Hub::start(config(PolicyConfig::default())).await.unwrap();
    let (client, nav) = app(&hub);
    client.start();
    connected(&hub).await;

    // 前台：照常导航，不改调
    let out = call(&hub, "shop.cart.viewAdd", json!({"sku": "A"})).await;
    assert_eq!(out.result.unwrap()["tool"], "cart.viewAdd");
    assert_eq!(out.routed_to, None);
    assert_eq!(nav.pages.lock().unwrap().clone(), ["cart"]);

    // 运行时上报、指向 view 工具的替代（清单校验不到）：先注册再注销，留在页面目录中
    let h = client
        .register_tool_with(ToolSpec::new("broken.toView", "替代是 view 工具"), view("broken", Some("orders.view")), Arc::new(Echo))
        .unwrap();
    eventually("broken.toView 注册", || hub.tools(&Default::default()).iter().any(|t| t.name == "shop.broken.toView")).await;
    h.dispose();
    let _ = client.register_tool_with(ToolSpec::new("orders.view", "只能在前台"), view("orders", None), Arc::new(Echo)).map(|h| h.dispose());
    eventually("view 工具注销", || hub.tools(&Default::default()).iter().all(|t| !t.name.contains("broken.toView"))).await;

    // 进入后台（view 工具随界面注销）：已知在后台 → 直接改调 cart.add，不导航
    nav.leave_cart();
    client.set_visibility(Visibility::Hidden, false);
    hub_sees(&hub, "hidden").await;
    let out = call(&hub, "shop.cart.viewAdd", json!({"sku": "A"})).await;
    assert_eq!(out.result.unwrap()["tool"], "cart.add");
    assert_eq!(out.routed_to.as_deref(), Some("shop.cart.add"));
    assert_eq!(nav.pages.lock().unwrap().len(), 1, "没有导航");

    // 没有后台替代：导航被 SDK 立即拒绝（USER_ACTION_REQUIRED / foreground），不等超时
    let started = Instant::now();
    let out = call(&hub, "shop.orders.view", json!({})).await;
    let e = out.result.unwrap_err();
    assert_eq!((e.kind, reason(&e)), (ErrorKind::UserActionRequired, Some("foreground")));
    assert_eq!(e.details.as_ref().and_then(|d| d.get("page")), Some(&json!("orders")));
    assert!(started.elapsed() < Duration::from_secs(2), "应立即返回：{:?}", started.elapsed());
    assert_eq!(out.routed_to, None);
    assert_eq!(nav.pages.lock().unwrap().len(), 1, "导航回调没有被调用");

    // 声明不可用（替代不存在 / 是 view 工具）：不改调，按导航处理
    for name in ["shop.broken.unknown", "shop.broken.toView"] {
        let out = call(&hub, name, json!({})).await;
        assert_eq!(out.result.unwrap_err().kind, ErrorKind::UserActionRequired, "{name}");
        assert_eq!(out.routed_to, None, "{name}");
    }
    // 参数不符合替代的 inputSchema：不改调
    if app_mcp_hub::features::SCHEMA_VALIDATION {
        let out = call(&hub, "shop.cart.viewAdd", json!({"sku": 1})).await;
        assert_eq!(out.routed_to, None);
        assert!(out.result.is_err());
    }

    // Hub 认为在前台、App 自行以 foreground 拒绝导航（如已发通知）：导航后再改调
    client.set_visibility(Visibility::Visible, true);
    hub_sees(&hub, "visible").await;
    nav.refuse.store(true, Ordering::SeqCst);
    let out = call(&hub, "shop.cart.viewAdd", json!({"sku": "B"})).await;
    assert_eq!(out.result.unwrap()["tool"], "cart.add");
    assert_eq!(out.routed_to.as_deref(), Some("shop.cart.add"));
    assert_eq!(nav.pages.lock().unwrap().len(), 2, "先导航了一次");
    // 没有替代时 App 的回复（含 uri）原样给调用方
    let out = call(&hub, "shop.orders.view", json!({})).await;
    let e = out.result.unwrap_err();
    assert_eq!((e.kind, reason(&e)), (ErrorKind::UserActionRequired, Some("foreground")));
    assert_eq!(e.details.as_ref().and_then(|d| d.get("uri")), Some(&json!("shop://cart")));
    assert!(e.message.contains("已发通知"));

    client.stop();
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn routed_call_obeys_policy_of_routed_tool() {
    let rules = PolicyConfig::from_json(
        &json!({"rules": [{"id": "no-add", "action": "deny", "app": "shop", "tool": "cart.add"}]}).to_string(),
    )
    .unwrap();
    let hub = Hub::start(config(rules)).await.unwrap();
    let (client, nav) = app(&hub);
    client.set_visibility(Visibility::Hidden, false);
    client.start();
    connected(&hub).await;
    hub_sees(&hub, "hidden").await;
    let out = call(&hub, "shop.cart.viewAdd", json!({"sku": "A"})).await;
    let e = out.result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::PolicyDenied);
    assert_eq!(e.details.as_ref().and_then(|d| d.get("ruleId")), Some(&json!("no-add")));
    assert_eq!(out.routed_to.as_deref(), Some("shop.cart.add"));
    assert!(nav.pages.lock().unwrap().is_empty());
    client.stop();
    hub.shutdown().await;
}

/// App 未运行（没有已连接实例）：同样按在后台处理，改调替代（之后按 app 工具的规则唤醒 / 报告未连接）。
#[tokio::test(flavor = "multi_thread")]
async fn app_not_running_routes_to_background_tool() {
    let hub = Hub::start(config(PolicyConfig::default())).await.unwrap();
    let out = call(&hub, "shop.cart.viewAdd", json!({"sku": "A"})).await;
    assert_eq!(out.routed_to.as_deref(), Some("shop.cart.add"));
    let e = out.result.unwrap_err();
    assert!(matches!(e.kind, ErrorKind::AppDisconnected | ErrorKind::LaunchFailed | ErrorKind::AppNotInstalled), "{e:?}");
    hub.shutdown().await;
}
