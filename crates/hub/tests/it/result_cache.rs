//! 只读结果缓存（第 16 项 O3，spec/hub-api.md 3.20）端到端：真实 WebSocket App 连接（[`fake_app`]，声明里直接给出 `cache`），
//! 以 App 收到的 `tools/invoke` / `resources/read` 计数判定是否命中。
//!
//! 覆盖：命中不转发（参数键顺序无关）、休眠后命中不唤醒、写调用整 App 失效、`resources/updated` 与 `stateHints` 定向失效、
//! `tools/changed` 失效、回连全量同步只在声明变化时失效、绕过、TTL 到期、策略 hide / deny 先于命中、`isError` / `pending` 不存、写工具上的 cache 被忽略、
//! 观测（`HubStatus.cache`、`app-mcp://apps/hub`）；MCP 出口（`mcp-server`）：private / shared 跨主体、`_meta` 的
//! `dev.appwire/cached` 与 `dev.appwire/cache: "bypass"`、`resources/read` 的 `ttlMs` / `cacheScope`。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{
    Availability, CacheStatus, CallOutcome, CallRequest, ErrorKind, Hub, HubConfig, HubError, PolicyConfig, ToolFilter, WakeRequest,
    Waker, async_trait,
};
use serde_json::{Value, json};

mod fake_app;

use fake_app::{FakeApp, resource, tool};

const T: Duration = Duration::from_secs(10);
const APP: &str = "shop";
const INSTANCE: &str = "shop-1";
/// 足够长的 TTL：测试期间不会到期。
const LONG: u64 = 60_000;
/// 短 TTL（到期用例）。
const SHORT_MS: u64 = 200;

fn tools() -> Value {
    json!([
        tool("list", "read", json!({"ttlMs": LONG})),
        tool("feed", "read", json!({"ttlMs": LONG, "scope": "shared"})),
        tool("quick", "read", json!({"ttlMs": SHORT_MS})),
        tool("add", "write", json!({"ttlMs": LONG})),
        tool("peek", "read", Value::Null),
    ])
}

fn resources() -> Value {
    json!([
        resource("cart", json!({"ttlMs": LONG})),
        resource("news", json!({"ttlMs": LONG, "scope": "shared"})),
        resource("live", Value::Null),
    ])
}

fn config() -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: None,
        list_changed_debounce: Duration::from_millis(10),
        wake_timeout: Duration::from_secs(1),
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

fn availability(hub: &Hub, name: &str) -> Option<Availability> {
    hub.tools(&ToolFilter::default()).into_iter().find(|t| t.name == name).map(|t| t.availability)
}

async fn start_with(config: HubConfig) -> (Hub, FakeApp) {
    let hub = Hub::start(config).await.expect("hub");
    let app = FakeApp::connect(&hub, APP, INSTANCE, tools(), resources()).await;
    eventually("工具可用", || availability(&hub, "shop.list") == Some(Availability::Available)).await;
    eventually("资源可读", || hub.resources().iter().any(|r| r.uri == "app-mcp://shop/cart")).await;
    (hub, app)
}

async fn start() -> (Hub, FakeApp) {
    start_with(config()).await
}

async fn call_req(hub: &Hub, req: CallRequest) -> CallOutcome {
    tokio::time::timeout(T, hub.call_tool(req)).await.expect("调用超时").expect("调用")
}

async fn call(hub: &Hub, name: &str, args: Value) -> CallOutcome {
    call_req(hub, CallRequest::new(name, args)).await
}

/// 读 App 资源，返回内容中的读取序号（App 端每次 `resources/read` 加一）。
async fn read_n(hub: &Hub, uri: &str) -> u64 {
    let r = tokio::time::timeout(T, hub.read_resource(uri)).await.expect("读取超时").expect("读取");
    let v: Value = serde_json::from_str(r.text.as_deref().unwrap_or_default()).expect("json");
    v["n"].as_u64().unwrap_or_else(|| panic!("{v}"))
}

fn n_of(o: &CallOutcome) -> u64 {
    o.result.as_ref().expect("调用成功")["n"].as_u64().unwrap_or_else(|| panic!("{o:?}"))
}

fn cache_status(hub: &Hub) -> CacheStatus {
    hub.status().cache.expect("HubStatus.cache")
}

/// 命中不发 `tools/invoke`；参数键顺序不同仍命中同一条；结果内容与原结果相同、`instanceId` 为原实例、`woke = false`；
/// 统计计入命中 / 未命中；`app-mcp://apps/hub` 同样给出统计。
#[tokio::test(flavor = "multi_thread")]
async fn hit_does_not_invoke_app() {
    let (hub, app) = start().await;
    let first = call(&hub, "shop.list", json!({"q": "x", "page": 1})).await;
    assert_eq!(n_of(&first), 1);
    let again = call(&hub, "shop.list", json!({"page": 1, "q": "x"})).await;
    assert_eq!(app.invokes("list"), 1, "命中：App 没有收到第二次调用");
    assert_eq!(first.cached_age_ms, None, "未命中：CallOutcome 不带命中标记");
    assert!(again.cached_age_ms.is_some(), "命中：CallOutcome.cached_age_ms 为 Some：{again:?}");
    assert_eq!(again.result, first.result, "内容与原结果相同");
    assert_eq!(again.instance_id.as_deref(), Some(INSTANCE));
    assert!(!again.woke);
    assert_ne!(again.call_id, first.call_id, "命中照常生成新的 callId");
    let other = call(&hub, "shop.list", json!({"q": "y"})).await;
    assert_eq!((n_of(&other), app.invokes("list")), (2, 2), "参数不同：未命中");

    let st = cache_status(&hub);
    assert_eq!((st.entries, st.hits, st.misses, st.evictions), (2, 1, 2, 0), "{st:?}");
    assert!(st.bytes > 0);
    let view = hub.read_resource("app-mcp://apps/hub").await.expect("读取 Hub 状态");
    let view: Value = serde_json::from_str(view.text.as_deref().unwrap_or_default()).unwrap();
    assert_eq!(view["cache"]["hits"], 1, "{view}");
    assert_eq!(view["cache"]["entries"], 2, "{view}");
    hub.shutdown().await;
}

/// 计数的假 Waker：命中时不应被调用。
#[derive(Default)]
struct CountingWaker {
    requests: Mutex<Vec<WakeRequest>>,
}

#[async_trait]
impl Waker for CountingWaker {
    async fn wake(&self, req: WakeRequest) -> Result<(), HubError> {
        self.requests.lock().unwrap().push(req);
        Ok(())
    }
}

/// App 休眠后：工具与资源命中缓存，不唤醒（休眠不使缓存失效）；未缓存的工具照常唤醒。
#[tokio::test(flavor = "multi_thread")]
async fn hit_does_not_wake_dormant_app() {
    let (hub, app) = start().await;
    let waker = Arc::new(CountingWaker::default());
    hub.set_waker(waker.clone());
    let first = call(&hub, "shop.list", json!({})).await;
    assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 1);
    app.sleep_and_close().await;
    eventually("工具可唤醒", || availability(&hub, "shop.list") == Some(Availability::Dormant)).await;

    let hit = call(&hub, "shop.list", json!({})).await;
    assert_eq!(hit.result, first.result);
    assert!(!hit.woke, "命中不唤醒");
    assert_eq!(hit.instance_id.as_deref(), Some(INSTANCE), "instanceId 为原结果的实例");
    assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 1, "资源同样命中");
    assert!(waker.requests.lock().unwrap().is_empty(), "命中时没有唤醒请求");

    let miss = tokio::time::timeout(T, hub.call_tool(CallRequest::new("shop.peek", json!({})))).await.expect("超时");
    assert!(miss.is_err() || miss.is_ok_and(|o| o.result.is_err()), "未缓存的工具需要唤醒（假 Waker 不回连 → 失败）");
    assert_eq!(waker.requests.lock().unwrap().len(), 1, "未缓存的工具照常唤醒");
    hub.shutdown().await;
}

/// 写调用完成（含失败）→ 清空该 App 全部条目（工具与资源）。
#[tokio::test(flavor = "multi_thread")]
async fn write_call_invalidates_whole_app() {
    let (hub, app) = start().await;
    call(&hub, "shop.list", json!({})).await;
    assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 1);
    assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 1, "已缓存");
    call(&hub, "shop.add", json!({})).await;
    assert_eq!(n_of(&call(&hub, "shop.list", json!({})).await), 2, "写调用后重新调用 App");
    assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 2, "资源同样失效");
    assert_eq!(app.reads("cart"), 2);

    app.script("add", json!({"error": {"code": -32006, "message": "失败", "data": {"kind": "HANDLER_ERROR"}}}));
    let failed = call(&hub, "shop.add", json!({})).await;
    assert!(failed.result.is_err());
    assert_eq!(n_of(&call(&hub, "shop.list", json!({})).await), 3, "失败的写调用同样清空");
    hub.shutdown().await;
}

/// `resources/updated` 与调用结果的 `stateHints` 只清所点名的资源。
#[tokio::test(flavor = "multi_thread")]
async fn updated_and_state_hints_are_targeted() {
    let (hub, app) = start().await;
    call(&hub, "shop.list", json!({})).await;
    assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 1);
    assert_eq!(read_n(&hub, "app-mcp://shop/news").await, 1);

    app.notify("resources/updated", json!({"name": "cart"}));
    eventually("resources/updated 生效", || cache_status(&hub).entries == 2).await;
    assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 2, "被点名的资源失效");
    assert_eq!(read_n(&hub, "app-mcp://shop/news").await, 1, "其他资源仍命中");

    app.script("peek", json!({"result": {"data": {"ok": true}, "stateHints": ["news"]}}));
    call(&hub, "shop.peek", json!({})).await;
    assert_eq!(read_n(&hub, "app-mcp://shop/news").await, 2, "stateHints 点名的资源失效");
    assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 2, "未点名的资源仍命中");
    call(&hub, "shop.list", json!({})).await;
    assert_eq!(app.invokes("list"), 1, "只读调用不清空工具条目");
    hub.shutdown().await;
}

/// `tools/changed`（声明变化）→ 清空该 App。
#[tokio::test(flavor = "multi_thread")]
async fn tools_changed_invalidates_app() {
    let (hub, app) = start().await;
    call(&hub, "shop.list", json!({})).await;
    assert_eq!(cache_status(&hub).entries, 1);
    app.notify("tools/changed", json!({"upserted": [tool("extra", "read", Value::Null)], "removed": []}));
    eventually("tools/changed 生效", || cache_status(&hub).entries == 0).await;
    assert_eq!(n_of(&call(&hub, "shop.list", json!({})).await), 2);
    hub.shutdown().await;
}

/// 写入工具与资源缓存 → 休眠断开 → 以 `instance` 与给定声明重新握手（`tools/sync` / `resources/sync`）。
async fn cache_then_resync(instance: &str, tools: Value, resources: Value) -> (Hub, FakeApp) {
    let (hub, app) = start().await;
    call(&hub, "shop.list", json!({})).await;
    assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 1);
    app.sleep_and_close().await;
    eventually("进入休眠", || availability(&hub, "shop.list") == Some(Availability::Dormant)).await;
    let app = FakeApp::connect(&hub, APP, instance, tools, resources).await;
    eventually("回连可用", || availability(&hub, "shop.list") == Some(Availability::Available)).await;
    eventually("资源同步完成", || hub.resources().iter().any(|r| r.uri == "app-mcp://shop/cart")).await;
    (hub, app)
}

/// 冷启动回连（同一实例或新实例 ID）以相同声明全量同步：缓存保留，不转发给 App（数据新旧由 TTL 兜底）。
#[tokio::test(flavor = "multi_thread")]
async fn resync_with_same_declarations_keeps_cache() {
    for instance in [INSTANCE, "shop-2"] {
        let (hub, app) = cache_then_resync(instance, tools(), resources()).await;
        assert_eq!(n_of(&call(&hub, "shop.list", json!({})).await), 1, "{instance}：工具仍命中");
        assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 1, "{instance}：资源仍命中");
        assert_eq!((app.invokes("list"), app.reads("cart")), (0, 0), "{instance}：没有转发给 App");
        hub.shutdown().await;
    }
}

/// 回连时工具或资源声明有变化（描述不同、少了一项）：清空该 App 的条目。
#[tokio::test(flavor = "multi_thread")]
async fn resync_with_changed_declarations_invalidates() {
    let mut described = tools();
    described[0]["description"] = json!("列出（新描述）");
    let mut fewer = tools();
    fewer.as_array_mut().expect("数组").retain(|t| t["name"] != "peek");
    let mut res_described = resources();
    res_described[1]["description"] = json!("新闻（新描述）");
    let cases = [
        ("工具描述变化", described, resources()),
        ("少了一个工具", fewer, resources()),
        ("资源描述变化", tools(), res_described),
    ];
    for (what, tools, resources) in cases {
        let (hub, app) = cache_then_resync(INSTANCE, tools, resources).await;
        assert_eq!(n_of(&call(&hub, "shop.list", json!({})).await), 1, "{what}：重新调用 App");
        assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 1, "{what}：资源同样失效");
        assert_eq!((app.invokes("list"), app.reads("cart")), (1, 1), "{what}");
        hub.shutdown().await;
    }
}

/// `cache_bypass`：不查缓存、照常调用，并以新结果覆盖。
#[tokio::test(flavor = "multi_thread")]
async fn bypass_calls_app_and_overwrites() {
    let (hub, app) = start().await;
    call(&hub, "shop.list", json!({})).await;
    let mut req = CallRequest::new("shop.list", json!({}));
    req.cache_bypass = true;
    assert_eq!(n_of(&call_req(&hub, req).await), 2, "绕过：调用 App");
    assert_eq!(n_of(&call(&hub, "shop.list", json!({})).await), 2, "之后命中的是新结果");
    assert_eq!(app.invokes("list"), 2);
    hub.shutdown().await;
}

/// TTL 到期（访问时判断）后重新调用 App。
#[tokio::test(flavor = "multi_thread")]
async fn ttl_expiry_refetches() {
    let (hub, app) = start().await;
    call(&hub, "shop.quick", json!({})).await;
    call(&hub, "shop.quick", json!({})).await;
    assert_eq!(app.invokes("quick"), 1, "TTL 内命中");
    tokio::time::sleep(Duration::from_millis(SHORT_MS + 100)).await;
    assert_eq!(n_of(&call(&hub, "shop.quick", json!({})).await), 2, "到期后重新调用");
    hub.shutdown().await;
}

/// 策略在查缓存之前：`deny` → `POLICY_DENIED`，`hide` → `TOOL_NOT_FOUND`；`set_policy` 不清缓存，撤销规则后照常命中。
#[tokio::test(flavor = "multi_thread")]
async fn policy_applies_before_hit() {
    let (hub, app) = start().await;
    call(&hub, "shop.list", json!({})).await;
    assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 1);
    let rules = |v: Value| PolicyConfig::from_json(&v.to_string()).expect("规则合法");

    hub.set_policy(rules(json!({"rules": [{"id": "d", "action": "deny", "app": APP, "tool": "list"}]}))).unwrap();
    let denied = call(&hub, "shop.list", json!({})).await;
    assert_eq!(denied.result.expect_err("被拒绝").kind, ErrorKind::PolicyDenied);

    hub.set_policy(rules(json!({"rules": [{"id": "h", "action": "hide", "app": APP}]}))).unwrap();
    let hidden = tokio::time::timeout(T, hub.call_tool(CallRequest::new("shop.list", json!({})))).await.unwrap();
    assert_eq!(hidden.expect_err("隐藏").kind(), ErrorKind::ToolNotFound);
    let e = hub.read_resource("app-mcp://shop/cart").await.expect_err("隐藏的 App 的资源不可读");
    assert_eq!(e.kind(), ErrorKind::ResourceNotFound);

    hub.set_policy(PolicyConfig::default()).unwrap();
    call(&hub, "shop.list", json!({})).await;
    assert_eq!(read_n(&hub, "app-mcp://shop/cart").await, 1);
    assert_eq!(app.invokes("list"), 1, "策略变化不清缓存；撤销后照常命中");
    hub.shutdown().await;
}

/// 存入条件：`isError` 与 `pending` 不存；写工具上的 `cache` 被忽略；未声明 `cache` 的资源不缓存。
#[tokio::test(flavor = "multi_thread")]
async fn only_cacheable_results_are_stored() {
    let (hub, app) = start().await;
    app.script("list", json!({"error": {"code": -32006, "message": "失败", "data": {"kind": "HANDLER_ERROR"}}}));
    assert!(call(&hub, "shop.list", json!({})).await.result.is_err());
    assert_eq!(n_of(&call(&hub, "shop.list", json!({})).await), 2, "isError 不存");

    app.script("feed", json!({"result": {"data": {"n": 0}, "status": "pending", "stateResource": "cart"}}));
    call(&hub, "shop.feed", json!({})).await;
    assert_eq!(n_of(&call(&hub, "shop.feed", json!({})).await), 2, "pending 不存");

    call(&hub, "shop.add", json!({})).await;
    call(&hub, "shop.add", json!({})).await;
    assert_eq!(app.invokes("add"), 2, "写工具上的 cache 被忽略");

    assert_eq!(read_n(&hub, "app-mcp://shop/live").await, 1);
    assert_eq!(read_n(&hub, "app-mcp://shop/live").await, 2, "未声明 cache 的资源不缓存");
    hub.shutdown().await;
}

/// `max_entries: 0` 关闭缓存。
#[tokio::test(flavor = "multi_thread")]
async fn disabled_cache_always_invokes() {
    let mut c = config();
    c.result_cache.max_entries = 0;
    let (hub, app) = start_with(c).await;
    call(&hub, "shop.list", json!({})).await;
    call(&hub, "shop.list", json!({})).await;
    assert_eq!(app.invokes("list"), 2);
    assert_eq!(cache_status(&hub), CacheStatus::default());
    hub.shutdown().await;
}

/// MCP 出口（无会话请求，经 `/mcp`）：private 按 Agent 主体隔离、shared 共用；命中结果 `_meta` 带 `dev.appwire/cached`；
/// `_meta` 的 `dev.appwire/cache: "bypass"` 绕过，其他值 `INVALID_INPUT`；`resources/read` 的 `ttlMs` / `cacheScope`。
#[cfg(feature = "mcp-server")]
mod mcp {
    use std::net::SocketAddr;

    use app_mcp_hub::names::{META_CACHED, META_INSTANCE_ID, META_WOKE};
    use app_mcp_hub::{AgentCredential, AgentsConfig};

    use super::*;
    use crate::support::mcp_http::modern_request;

    const CLAUDE: &str = "claude-0123456789abcdef0123456789abcdef";
    const CURSOR: &str = "cursor-0123456789abcdef0123456789abcdef";

    async fn start_mcp() -> (Hub, FakeApp, SocketAddr) {
        let mut c = config();
        c.mcp_http = true;
        c.agents = AgentsConfig {
            agents: [("claude", CLAUDE), ("cursor", CURSOR)]
                .iter()
                .map(|(n, t)| AgentCredential { name: (*n).into(), token: (*t).into() })
                .collect(),
        };
        let (hub, app) = start_with(c).await;
        let addr = hub.listen_addr().expect("listen");
        (hub, app, addr)
    }

    async fn mcp_call(addr: SocketAddr, token: &str, name: &str, meta: Value) -> Value {
        let params = json!({"name": name, "arguments": {}, "_meta": meta});
        let r = modern_request(addr, Some(token), "tools/call", name, params).await;
        assert_eq!(r.status, 200, "{}", r.body);
        r.json()["result"].clone()
    }

    async fn mcp_read(addr: SocketAddr, uri: &str, meta: Value) -> Value {
        let r = modern_request(addr, Some(CLAUDE), "resources/read", uri, json!({"uri": uri, "_meta": meta})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        r.json()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn private_and_shared_scopes_across_agents() {
        let (hub, app, addr) = start_mcp().await;
        let first = mcp_call(addr, CLAUDE, "shop.list", json!({})).await;
        assert!(first["_meta"].get(META_CACHED).is_none(), "未命中不写 cached：{first}");
        let hit = mcp_call(addr, CLAUDE, "shop.list", json!({})).await;
        assert_eq!(app.invokes("list"), 1, "同一 Agent 命中");
        assert_eq!(hit["structuredContent"], first["structuredContent"]);
        assert!(hit["_meta"][META_CACHED]["ageMs"].is_u64(), "{hit}");
        assert_eq!((&hit["_meta"][META_WOKE], &hit["_meta"][META_INSTANCE_ID]), (&json!(false), &json!(INSTANCE)));
        mcp_call(addr, CURSOR, "shop.list", json!({})).await;
        assert_eq!(app.invokes("list"), 2, "private：另一个 Agent 不共用");
        assert_eq!(n_of(&call(&hub, "shop.list", json!({})).await), 3, "Hub API 主体 api 同样不共用");

        mcp_call(addr, CLAUDE, "shop.feed", json!({})).await;
        let shared = mcp_call(addr, CURSOR, "shop.feed", json!({})).await;
        assert_eq!(app.invokes("feed"), 1, "shared：全体调用方共用");
        assert!(shared["_meta"][META_CACHED].is_object(), "{shared}");

        let bypass = mcp_call(addr, CLAUDE, "shop.feed", json!({"dev.appwire/cache": "bypass"})).await;
        assert_eq!(app.invokes("feed"), 2, "_meta bypass 调用 App");
        assert!(bypass["_meta"].get(META_CACHED).is_none(), "{bypass}");
        let bad = mcp_call(addr, CLAUDE, "shop.feed", json!({"dev.appwire/cache": "refresh"})).await;
        assert_eq!(bad["isError"], true, "{bad}");
        assert!(bad["content"][0]["text"].as_str().unwrap_or_default().contains("INVALID_INPUT"), "{bad}");
        assert_eq!(app.invokes("feed"), 2, "不合法的 _meta 不执行");
        hub.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn resources_read_ttl_and_scope() {
        let (hub, app, addr) = start_mcp().await;
        let miss = mcp_read(addr, "app-mcp://shop/cart", json!({})).await;
        assert_eq!(miss["result"]["ttlMs"], LONG, "未命中：声明的 ttlMs：{miss}");
        assert_eq!(miss["result"]["cacheScope"], "private");
        assert!(miss["result"]["_meta"].get(META_CACHED).is_none(), "{miss}");
        tokio::time::sleep(Duration::from_millis(20)).await;
        let hit = mcp_read(addr, "app-mcp://shop/cart", json!({})).await;
        assert_eq!(app.reads("cart"), 1, "命中不发 resources/read");
        let ttl = hit["result"]["ttlMs"].as_u64().expect("ttlMs");
        assert!(ttl < LONG && ttl > LONG - 5_000, "剩余 TTL：{ttl}");
        assert!(hit["result"]["_meta"][META_CACHED]["ageMs"].as_u64().is_some_and(|a| a >= 20), "{hit}");
        assert_eq!(hit["result"]["contents"], miss["result"]["contents"]);

        let shared = mcp_read(addr, "app-mcp://shop/news", json!({})).await;
        assert_eq!(shared["result"]["cacheScope"], "public", "shared → public：{shared}");
        let live = mcp_read(addr, "app-mcp://shop/live", json!({})).await;
        assert_eq!((&live["result"]["ttlMs"], &live["result"]["cacheScope"]), (&json!(0), &json!("private")), "{live}");

        mcp_read(addr, "app-mcp://shop/cart", json!({"dev.appwire/cache": "bypass"})).await;
        assert_eq!(app.reads("cart"), 2, "_meta bypass 重新读取");
        let uri = "app-mcp://shop/cart";
        let bad = modern_request(addr, Some(CLAUDE), "resources/read", uri, json!({"uri": uri, "_meta": {"dev.appwire/cache": 1}})).await;
        assert_eq!(bad.json()["error"]["data"]["kind"], "INVALID_INPUT", "不合法的 _meta：{}", bad.body);
        assert_eq!(app.reads("cart"), 2, "不合法的 _meta 不读取");
        hub.shutdown().await;
    }
}
