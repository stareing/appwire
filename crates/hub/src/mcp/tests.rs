use std::time::Duration;

use app_mcp_native::{CallHandle, LifecycleMode, NativeClient, NativeConfig, ToolHandler, ToolSpec};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ErrorCode, ProtocolVersion, GetMeta, ReadResourceRequestParams,
    SubscriptionFilter,
};
use rmcp::service::{
    ClientCacheConfig, ClientInitializeError, ClientLifecycleMode, RunningService, ServiceError, Subscription,
    SubscriptionEnd,
};
use rmcp::{ClientServiceExt, RoleClient, ServiceExt};
use serde_json::{Value, json};
use app_mcp_protocol::{AppOverview, ErrorKind};

use crate::task::{CallerKey, CallerKind, Principal};
use crate::{
    ApprovalHandler, ApprovalPolicy, ApprovalRequest, CallRequest, Hub, HubConfig, InstanceState, LeasePolicy,
    McpProtocolMode, Risk, ToolExposure,
};

mod modern;
mod task_handles;

const T: Duration = Duration::from_secs(10);
const INSTANCE: &str = "shop-1";
const SHOP_SUMMARY: &str = "演示用购物商城";

struct Echo;
impl ToolHandler for Echo {
    fn invoke(&self, call: CallHandle) {
        let _ = call.complete(Some(&json!({ "tool": call.tool_name() }).to_string()), vec![]);
    }
}

fn config(idle_revoke: Duration, task_idle_ttl: Duration) -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: None,
        list_changed_debounce: Duration::from_millis(10),
        lease: LeasePolicy { idle_revoke, ..LeasePolicy::default() },
        task_idle_ttl,
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

async fn start(cfg: HubConfig) -> (Hub, NativeClient) {
    let (hub, mut clients) = start_instances(cfg, &[INSTANCE], false).await;
    (hub, clients.remove(0))
}

/// 启动 Hub 与商城 App 的若干实例（按顺序连接，各注册 `cart.add`）；`overview`：App 带总览。
async fn start_instances(cfg: HubConfig, instances: &[&str], overview: bool) -> (Hub, Vec<NativeClient>) {
    let hub = Hub::start(cfg).await.expect("hub");
    let mut clients = Vec::new();
    for (i, instance) in instances.iter().enumerate() {
        let mut c = NativeConfig::new("shop", "商城");
        c.host_url = format!("ws://{}/app", hub.listen_addr().expect("listen"));
        c.instance_id = Some((*instance).to_owned());
        c.lifecycle.mode = LifecycleMode::Persistent;
        if overview {
            c.overview = Some(AppOverview { summary: SHOP_SUMMARY.into(), body: Some("下单前先加入购物车。".into()), locale: None });
        }
        let client = NativeClient::new(c, None).expect("client");
        client.register_tool(ToolSpec::new("cart.add", "加入购物车"), std::sync::Arc::new(Echo)).expect("tool");
        client.start();
        let n = i + 1;
        eventually("App 实例注册工具", || {
            hub.status().apps.iter().any(|a| {
                a.app_id == "shop" && a.tools.len() == 1 && a.instances.iter().filter(|x| x.state == InstanceState::Connected).count() == n
            })
        })
        .await;
        clients.push(client);
    }
    (hub, clients)
}

/// 一条 MCP 连接（`hub.mcp_session()` 的一个处理器）。`modern` = 不握手、每请求自带 `_meta`（`server/discover`，
/// 以 2025-11-25 协商——S4–S6 的无会话路径）。
async fn connect(hub: &Hub, modern: bool) -> RunningService<RoleClient, ()> {
    let lifecycle = match modern {
        true => ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2025_11_25] },
        false => ClientLifecycleMode::Initialize,
    };
    connect_with(hub, lifecycle).await.expect("connect")
}

/// 以 MCP 2026-07-28 协商的无会话连接（S7）。
async fn connect_2026(hub: &Hub) -> RunningService<RoleClient, ()> {
    let svc = connect_with(hub, ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] })
        .await
        .expect("discover 2026-07-28");
    assert_eq!(svc.peer().peer_info().map(|i| i.protocol_version.clone()), Some(ProtocolVersion::V_2026_07_28));
    svc
}

/// @output 错误装箱：`ClientInitializeError` 在 Windows 上超过 clippy `result_large_err` 的阈值。
async fn connect_with(hub: &Hub, lifecycle: ClientLifecycleMode) -> Result<RunningService<RoleClient, ()>, Box<ClientInitializeError>> {
    let (c, s) = tokio::io::duplex(1 << 20);
    let session = hub.mcp_session();
    tokio::spawn(async move {
        if let Ok(svc) = session.serve(s).await {
            let _ = svc.waiting().await;
        }
    });
    let svc = ().serve_with_lifecycle(c, lifecycle).await.map_err(Box::new)?;
    // rmcp 客户端按结果的 ttlMs 缓存列表（SEP-2549）：关掉，每次 tools/list 都真实到达 Hub。
    svc.peer().set_response_cache_config(ClientCacheConfig::disabled()).await;
    Ok(svc)
}

async fn call(agent: &RunningService<RoleClient, ()>, name: &str, args: Value) -> CallToolResult {
    let mut params = CallToolRequestParams::new(name.to_owned());
    params.arguments = args.as_object().cloned();
    let r = tokio::time::timeout(T, agent.peer().call_tool(params)).await.expect("MCP 调用超时").expect("call");
    assert_ne!(r.is_error, Some(true), "{name}: {r:?}");
    r
}

fn principal() -> CallerKey {
    CallerKey::principal(&Principal::Local)
}

/// （任务数, 主体任务 ID, 主体任务的选择, 主体任务的租约数）
fn principal_task(hub: &Hub) -> (usize, Option<String>, Option<String>, usize) {
    let tasks = hub.shared().agent_tasks();
    let t = tasks.get(&principal());
    (
        tasks.iter().count(),
        t.map(|t| t.id.clone()),
        t.and_then(|t| t.selected.get("shop").map(|s| s.instance_id.clone())),
        t.map_or(0, |t| t.leases.len()),
    )
}

fn legacy_keys(hub: &Hub) -> Vec<String> {
    let tasks = hub.shared().agent_tasks();
    tasks.iter().map(|(k, _)| k.to_string()).filter(|k| k.starts_with("mcp:")).collect()
}

/// 连续 N 个无会话请求（同一连接与每请求一个新处理器——后者即 rmcp 无状态 HTTP 路径的"每请求调用工厂"）
/// 只用一个主体任务：不新建、不删除任务，处理器析构无副作用；`apps.select` 与租约记在主体任务上。
#[tokio::test(flavor = "multi_thread")]
async fn stateless_requests_share_one_principal_task() {
    let (hub, client) = start(config(Duration::ZERO, Duration::ZERO)).await;
    let agent = connect(&hub, true).await;
    assert_eq!(principal_task(&hub).0, 0, "server/discover 不建任务");

    call(&agent, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE})).await;
    let (n, id, selected, _) = principal_task(&hub);
    let id = id.expect("主体任务");
    assert_eq!((n, selected.as_deref()), (1, Some(INSTANCE)));
    for i in 0..5 {
        agent.peer().list_all_tools().await.expect("tools/list");
        let r = call(&agent, "shop.cart.add", json!({ "i": i })).await;
        assert_eq!(r.structured_content.as_ref().map(|v| v["tool"].clone()), Some(json!("cart.add")));
        assert_eq!(principal_task(&hub), (1, Some(id.clone()), Some(INSTANCE.to_owned()), 1), "第 {i} 次");
    }
    let _ = agent.cancel().await;
    // 每请求一个新连接 / 处理器，用完即断开
    for i in 0..3 {
        let one = connect(&hub, true).await;
        call(&one, "shop.cart.add", json!({})).await;
        let _ = one.cancel().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(principal_task(&hub), (1, Some(id.clone()), Some(INSTANCE.to_owned()), 1), "第 {i} 个连接");
    }
    // 租约统计也按主体键记（`LeasePairStatus.session`）
    let lease = hub.status().lease.expect("lease");
    assert!(lease.pairs.iter().any(|p| p.session == "principal:local" && p.app_id == "shop"), "{:?}", lease.pairs);
    assert!(lease.pairs.iter().all(|p| !p.session.starts_with("mcp:")), "{:?}", lease.pairs);
    client.stop();
    hub.shutdown().await;
}

/// legacy 会话与无会话请求并发：各自的选择、租约互不影响；legacy 会话结束只结束它自己的任务。
#[tokio::test(flavor = "multi_thread")]
async fn legacy_and_stateless_do_not_affect_each_other() {
    let (hub, client) = start(config(Duration::ZERO, Duration::ZERO)).await;
    let legacy = connect(&hub, false).await;
    let modern = connect(&hub, true).await;

    let (a, b) = tokio::join!(
        call(&legacy, "shop.cart.add", json!({})),
        call(&modern, "shop.cart.add", json!({})),
    );
    drop((a, b));
    let keys = legacy_keys(&hub);
    assert_eq!(keys.len(), 1, "legacy 会话一个任务：{keys:?}");
    let legacy_key = keys[0].clone();
    let (n, id, selected, leases) = principal_task(&hub);
    assert_eq!((n, selected, leases), (2, None, 1));

    // 只有 legacy 会话选定实例：主体任务不受影响
    call(&legacy, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE})).await;
    assert_eq!(principal_task(&hub).2, None);
    let selected_in = |r: CallToolResult| r.structured_content.map(|v| v["apps"][0]["selectedInstanceId"].clone());
    assert_eq!(selected_in(call(&legacy, "apps.list", json!({})).await), Some(json!(INSTANCE)));
    assert_eq!(selected_in(call(&modern, "apps.list", json!({})).await), Some(Value::Null));

    // 无会话请求的 apps.release 只收回主体任务的租约，legacy 会话的租约保留
    let r = call(&modern, "apps.release", json!({"appId": "shop"})).await;
    assert_eq!(r.structured_content.as_ref().map(|v| v["released"].clone()), Some(json!(1)));
    assert_eq!(principal_task(&hub).3, 0);
    {
        let tasks = hub.shared().agent_tasks();
        let legacy_task = tasks.iter().find(|(k, _)| k.to_string() == legacy_key).map(|(_, t)| t);
        assert_eq!(legacy_task.map(|t| (t.leases.len(), t.selected.len())), Some((1, 1)));
    }

    // legacy 会话结束：它的任务结束，主体任务仍在（同一 ID）
    let _ = legacy.cancel().await;
    eventually("legacy 会话的任务结束", || legacy_keys(&hub).is_empty()).await;
    assert_eq!(principal_task(&hub).1, id);
    let _ = modern.cancel().await;
    client.stop();
    hub.shutdown().await;
}

/// 无会话请求的租约按请求流空闲收回（`lease.idle_revoke`），任务按 `task_idle_ttl` 回收；legacy 会话的任务不按空闲回收。
#[tokio::test(flavor = "multi_thread")]
async fn stateless_leases_and_task_reclaimed_by_idle() {
    let (hub, client) = start(config(Duration::from_millis(150), Duration::from_millis(600))).await;
    let legacy = connect(&hub, false).await;
    let modern = connect(&hub, true).await;
    call(&legacy, "shop.cart.add", json!({})).await;
    call(&modern, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE})).await;
    call(&modern, "shop.cart.add", json!({})).await;
    assert_eq!(principal_task(&hub).3, 1);

    // 请求流空闲：两个调用方的默认租约都被收回（各计一次）
    eventually("默认租约空闲收回", || hub.status().lease.is_some_and(|l| l.revoked_idle >= 2)).await;
    // 任务空闲到期：主体任务（选择、租约统计）被回收；legacy 会话的任务保留
    eventually("主体任务回收", || principal_task(&hub).0 == 1).await;
    assert_eq!(principal_task(&hub).1, None);
    assert_eq!(legacy_keys(&hub).len(), 1);
    let lease = hub.status().lease.expect("lease");
    assert!(lease.pairs.iter().all(|p| p.session != "principal:local"), "{:?}", lease.pairs);

    // 之后的无会话请求得到一个新任务，没有旧选择
    call(&modern, "shop.cart.add", json!({})).await;
    let (_, id, selected, _) = principal_task(&hub);
    assert!(id.is_some() && selected.is_none());
    let _ = (legacy.cancel().await, modern.cancel().await);
    client.stop();
    hub.shutdown().await;
}

/// 一个 `tools/list` 的线上 JSON（逐字节比较用）。
async fn tools_json(agent: &RunningService<RoleClient, ()>) -> String {
    let r = tokio::time::timeout(T, agent.peer().list_tools(None)).await.expect("tools/list 超时").expect("tools/list");
    serde_json::to_string(&r).expect("json")
}

fn tool_names(json: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(json).expect("json");
    v["tools"].as_array().into_iter().flatten().filter_map(|t| t["name"].as_str().map(str::to_owned)).collect()
}

fn instance_of(r: &CallToolResult) -> Option<String> {
    r.meta.as_ref().and_then(|m| m.get(crate::names::META_INSTANCE_ID)).and_then(Value::as_str).map(str::to_owned)
}

fn text_of(r: &CallToolResult) -> String {
    crate::call::result_text(r)
}

/// 第 12 项 S5 验收：无会话请求的两次 `tools/list` 之间夹任意 `apps.tools` / 调用 / `apps.select` / `apps.overview`，
/// 结果逐字节相同，并带 `ttlMs` / `cacheScope: private`；legacy 会话的渐进暴露与线上格式不变。
#[tokio::test(flavor = "multi_thread")]
async fn stateless_tools_list_is_pure_function_of_server_state() {
    for stateless_exposure in [ToolExposure::All, ToolExposure::Progressive] {
        let cfg = HubConfig {
            tool_exposure: ToolExposure::Progressive,
            stateless_tool_exposure: stateless_exposure,
            ..config(Duration::ZERO, Duration::ZERO)
        };
        let (hub, client) = start(cfg).await;
        let modern = connect(&hub, true).await;
        let legacy = connect(&hub, false).await;
        let before = tools_json(&modern).await;
        let names = tool_names(&before);
        let v: Value = serde_json::from_str(&before).expect("json");
        assert_eq!((v["ttlMs"].clone(), v["cacheScope"].clone()), (json!(5000), json!("private")), "{before}");
        match stateless_exposure {
            // 默认：全部列出，不含 apps.tools（与 legacy 未生效时相同）
            ToolExposure::All => assert!(names.contains(&"shop.cart.add".to_owned()) && !names.contains(&"apps.tools".to_owned()), "{names:?}"),
            _ => assert!(!names.contains(&"shop.cart.add".to_owned()) && names.contains(&"apps.tools".to_owned()), "{names:?}"),
        }

        call(&modern, "apps.tools", json!({"appId": "shop"})).await;
        assert_eq!(tools_json(&modern).await, before, "apps.tools 之后（{stateless_exposure:?}）");
        call(&modern, "shop.cart.add", json!({})).await;
        assert_eq!(tools_json(&modern).await, before, "调用之后（{stateless_exposure:?}）");
        call(&modern, "apps.select", json!({"appId": "shop", "instanceId": INSTANCE})).await;
        call(&modern, "apps.overview", json!({"appId": "shop"})).await;
        assert_eq!(tools_json(&modern).await, before, "apps.select 之后（{stateless_exposure:?}）");
        // 另一个处理器（rmcp 无状态 HTTP 路径每请求一个）看到同一列表
        let other = connect(&hub, true).await;
        assert_eq!(tools_json(&other).await, before);

        // legacy：渐进暴露照旧随 apps.tools 变化，结果不带缓存字段
        let l1 = tools_json(&legacy).await;
        assert!(!l1.contains("ttlMs") && !l1.contains("cacheScope"), "{l1}");
        assert!(!tool_names(&l1).contains(&"shop.cart.add".to_owned()));
        call(&legacy, "apps.tools", json!({"appId": "shop"})).await;
        assert!(tool_names(&tools_json(&legacy).await).contains(&"shop.cart.add".to_owned()));
        // 无会话列表只随服务器状态变化：全局选择（Hub::select_instance）在渐进暴露下加入列表
        hub.select_instance("shop", Some(INSTANCE));
        let after = tool_names(&tools_json(&modern).await);
        assert!(after.contains(&"shop.cart.add".to_owned()), "{after:?}");
        let _ = (modern.cancel().await, legacy.cancel().await, other.cancel().await);
        client.stop();
        hub.shutdown().await;
    }
}

/// 第 12 项 S5：无会话请求的总览不"首次附带"，改经 `server/discover` 的 instructions（含 App 简介）与 `apps.tools`；
/// 资源列表 / 读取 / 模板列表带缓存提示。legacy 会话的首次附带不变。
#[tokio::test(flavor = "multi_thread")]
async fn stateless_overview_via_discover_and_apps_tools() {
    let (hub, clients) = start_instances(config(Duration::ZERO, Duration::ZERO), &[INSTANCE], true).await;
    let modern = connect(&hub, true).await;
    let info = modern.peer().peer_info().expect("discover 结果");
    let instructions = info.instructions.clone().unwrap_or_default();
    assert!(instructions.contains(&format!("- shop（商城）：{SHOP_SUMMARY}")), "{instructions}");
    assert!(instructions.contains("apps.overview") && !instructions.contains("首次调用"), "{instructions}");

    // 调用结果不附带总览（无论第几次）
    for _ in 0..2 {
        let r = call(&modern, "shop.cart.add", json!({})).await;
        assert!(!text_of(&r).contains("<app-overview"), "{r:?}");
    }
    // apps.tools 每次都附带（应请求）
    for _ in 0..2 {
        let r = call(&modern, "apps.tools", json!({"appId": "shop"})).await;
        let first = r.content.first().and_then(|c| c.as_text()).map(|t| t.text.clone()).unwrap_or_default();
        assert!(first.contains("<app-overview app=\"shop\"") && !first.contains("不会重复附带"), "{first}");
        assert_eq!(r.structured_content.as_ref().map(|v| v["overview"]["summary"].clone()), Some(json!(SHOP_SUMMARY)));
    }
    // 主体任务上不记"已附带"
    assert!(hub.shared().agent_tasks().get(&principal()).is_none_or(|t| t.delivered.is_empty()));

    // 资源相关结果的缓存提示
    let res = serde_json::to_value(modern.peer().list_resources(None).await.expect("resources/list")).expect("json");
    assert_eq!((res["ttlMs"].clone(), res["cacheScope"].clone()), (json!(5000), json!("private")));
    let tpl = serde_json::to_value(modern.peer().list_resource_templates(None).await.expect("templates")).expect("json");
    assert_eq!((tpl["ttlMs"].clone(), tpl["cacheScope"].clone()), (json!(5000), json!("private")));

    // legacy：首次调用附带、第二次不附带；apps.tools 不带 overview 字段；无缓存字段
    let legacy = connect(&hub, false).await;
    let r1 = call(&legacy, "shop.cart.add", json!({})).await;
    assert!(text_of(&r1).contains("本会话中不会重复附带"), "{r1:?}");
    let r2 = call(&legacy, "shop.cart.add", json!({})).await;
    assert!(!text_of(&r2).contains("<app-overview"));
    let r3 = call(&legacy, "apps.tools", json!({"appId": "shop"})).await;
    assert!(r3.structured_content.as_ref().is_some_and(|v| v.get("overview").is_none()));
    let legacy_info = legacy.peer().peer_info().and_then(|i| i.instructions.clone()).unwrap_or_default();
    assert!(legacy_info.contains("首次调用"), "{legacy_info}");
    let lres = serde_json::to_value(legacy.peer().list_resource_templates(None).await.expect("templates")).expect("json");
    assert!(lres.get("ttlMs").is_none() && lres.get("cacheScope").is_none(), "{lres}");
    let _ = (modern.cancel().await, legacy.cancel().await);
    clients.iter().for_each(NativeClient::stop);
    hub.shutdown().await;
}

struct Approver(std::sync::Mutex<Vec<ApprovalRequest>>);

#[async_trait::async_trait]
impl ApprovalHandler for Approver {
    async fn approve(&self, req: ApprovalRequest) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(req);
        true
    }
}

/// 第 12 项 S6：主体级 `apps.select` 让路由命中所选实例、工具列表不变，空闲有效期到期后回到默认路由（legacy 会话的选择
/// 不过期）；审批请求带 `principal` 与 `client_name`；`/status` 列出任务（种类、选择、租约、最近活动）。
#[tokio::test(flavor = "multi_thread")]
async fn principal_selection_ttl_approval_and_status_tasks() {
    const OTHER: &str = "shop-2";
    let ttl = Duration::from_millis(500);
    let cfg = HubConfig {
        principal_select_ttl: ttl,
        approval: ApprovalPolicy { require_at_or_above: Some(Risk::Read), timeout: None },
        ..config(Duration::ZERO, Duration::ZERO)
    };
    let (hub, clients) = start_instances(cfg, &[INSTANCE, OTHER], false).await;
    let approver = std::sync::Arc::new(Approver(std::sync::Mutex::new(Vec::new())));
    hub.set_approval_handler(approver.clone());
    // 默认路由：聚焦的实例（不随"最近完成调用"变化）；另一个实例转入后台
    clients[1].set_visibility(app_mcp_native::Visibility::Hidden, false);
    clients[0].set_visibility(app_mcp_native::Visibility::Visible, true);
    eventually("只有一个实例聚焦", || {
        let st = hub.status();
        let focused: Vec<String> = st
            .apps
            .iter()
            .flat_map(|a| a.instances.iter())
            .filter(|i| i.info.focused)
            .map(|i| i.info.instance_id.clone())
            .collect();
        focused == [INSTANCE]
    })
    .await;
    let modern = connect(&hub, true).await;
    let legacy = connect(&hub, false).await;
    assert_eq!(instance_of(&call(&modern, "shop.cart.add", json!({})).await).as_deref(), Some(INSTANCE));

    let before = tools_json(&modern).await;
    let r = call(&modern, "apps.select", json!({"appId": "shop", "instanceId": OTHER})).await;
    let msg = r.structured_content.as_ref().and_then(|v| v["message"].as_str().map(str::to_owned)).unwrap_or_default();
    assert!(msg.contains("0.5 秒内未再用于调用即失效") && msg.contains("工具列表不变"), "{msg}");
    call(&legacy, "apps.select", json!({"appId": "shop", "instanceId": OTHER})).await;
    // 有效期内连续使用会续期：间隔小于有效期的多次调用都命中所选实例
    for _ in 0..3 {
        tokio::time::sleep(ttl / 3).await;
        assert_eq!(instance_of(&call(&modern, "shop.cart.add", json!({})).await).as_deref(), Some(OTHER));
    }
    assert_eq!(tools_json(&modern).await, before, "apps.select 不改变无会话列表");

    // /status：主体任务的选择（带剩余有效期）、租约、最近活动；legacy 会话任务的选择不过期
    let st = hub.status();
    let tasks = st.tasks.clone().expect("tasks");
    let p = tasks.iter().find(|t| t.caller == "principal:local").expect("主体任务");
    assert_eq!(p.kind, CallerKind::Principal);
    assert!(p.id.starts_with("task-"));
    assert_eq!(p.selections.len(), 1);
    assert_eq!(p.selections[0].instance_id, OTHER);
    assert!(p.selections[0].expires_in_ms.is_some_and(|ms| ms <= 500), "{:?}", p.selections);
    assert!(!p.leases.is_empty() && p.leases.iter().all(|l| l.expires_in_ms > 0), "{:?}", p.leases);
    assert!(p.idle_ms.is_some() && p.inflight == 0, "{p:?}");
    let l = tasks.iter().find(|t| t.kind == CallerKind::McpSession).expect("legacy 任务");
    assert!(l.caller.starts_with("mcp:") && l.selections[0].expires_in_ms.is_none(), "{l:?}");
    let json = serde_json::to_value(&st).expect("json");
    assert_eq!(json["tasks"][0]["kind"], json!("mcpSession"), "{}", json["tasks"]);
    assert_eq!(json["tasks"][1]["selections"][0]["appId"], json!("shop"));

    // 到期：回到默认路由，apps.list 不再显示主体级选择；legacy 会话仍命中所选实例
    tokio::time::sleep(ttl + Duration::from_millis(100)).await;
    let listed = call(&modern, "apps.list", json!({})).await;
    assert_eq!(listed.structured_content.map(|v| v["apps"][0]["selectedInstanceId"].clone()), Some(Value::Null));
    assert_eq!(instance_of(&call(&modern, "shop.cart.add", json!({})).await).as_deref(), Some(INSTANCE));
    assert_eq!(instance_of(&call(&legacy, "shop.cart.add", json!({})).await).as_deref(), Some(OTHER));
    assert!(hub.status().tasks.expect("tasks").iter().all(|t| t.caller != "principal:local" || t.selections.is_empty()));

    // 审批请求：MCP 出口带主体与客户端名（仅显示），Hub API 不带
    hub.call_tool(CallRequest::new("shop.cart.add", json!({}))).await.expect("api call");
    let seen = approver.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let modern_req = seen.iter().find(|r| r.session.as_deref() == Some("principal:local")).expect("无会话审批");
    let legacy_req = seen.iter().find(|r| r.session.as_deref().is_some_and(|s| s.starts_with("mcp:"))).expect("legacy 审批");
    for r in [modern_req, legacy_req] {
        assert_eq!(r.principal.as_deref(), Some("local"));
        assert!(r.client_name.as_deref().is_some_and(|n| !n.is_empty()), "{r:?}");
    }
    let api_req = seen.last().expect("api 审批");
    assert_eq!((api_req.session.as_deref(), api_req.principal.as_deref(), api_req.client_name.as_deref()), (None, None, None));
    let api_json = serde_json::to_value(api_req).expect("json");
    assert!(api_json.get("principal").is_none() && api_json.get("clientName").is_none(), "{api_json}");
    let _ = (modern.cancel().await, legacy.cancel().await);
    clients.iter().for_each(NativeClient::stop);
    hub.shutdown().await;
}

async fn tool_list(agent: &RunningService<RoleClient, ()>) -> Vec<String> {
    tool_names(&tools_json(agent).await)
}
