//! Agent 身份（第 16 项 N5，spec/hub-api.md 3.6「Agent 身份」）：经 HTTP `/mcp` 出示 Agent 令牌的请求按 Agent 区分主体。
//!
//! - 无会话请求：每个 Agent 一个主体任务与各自的任务句柄（上限分别计算、句柄不能跨 Agent 出示）；
//! - legacy 会话：建立时的 Agent 身份记在会话任务上；
//! - Agent 令牌不能访问 `/status`、`/agents`；`POST /agents` 替换登记（不合法时保留之前的登记）；
//! - `/status` 的 `agents` 只列名字，不含令牌。
//!
//! 所有 TCP 监听都绑定端口 0 并从监听器取实际地址。

#![cfg(feature = "mcp-server")]

use std::net::SocketAddr;
use std::time::Duration;

use app_mcp_hub::{AgentCredential, AgentsConfig, HttpOptions, Hub, HubConfig, HubStatus};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

#[path = "support/mcp_http.rs"]
mod mcp_http;

use mcp_http::{Reply, http, modern_call, modern_request};

const T: Duration = Duration::from_secs(10);
const LOCAL: &str = "local-0123456789abcdef0123456789abcdef";
const CLAUDE: &str = "claude-0123456789abcdef0123456789abcdef";
const CURSOR: &str = "cursor-0123456789abcdef0123456789abcdef";
/// 未配置本机令牌的 Hub 上出示的未知令牌：按未携带处理（本机主体）。
const LOCAL_UNCONFIGURED: &str = "unknown-0123456789abcdef0123456789abcdef";

fn agents(list: &[(&str, &str)]) -> AgentsConfig {
    AgentsConfig {
        agents: list.iter().map(|(n, t)| AgentCredential { name: (*n).into(), token: (*t).into() }).collect(),
    }
}

async fn start() -> Hub {
    Hub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: None,
        mcp_http: true,
        http: HttpOptions { token: Some(LOCAL.into()), ..Default::default() },
        agents: agents(&[("claude", CLAUDE), ("cursor", CURSOR)]),
        ..Default::default()
    })
    .await
    .expect("hub")
}

/// `apps.task.begin` → 任务 ID。
async fn begin(addr: SocketAddr, token: Option<&str>) -> String {
    let r = modern_call(addr, token, "apps.task.begin", json!({})).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v = r.json();
    v["result"]["structuredContent"]["taskId"].as_str().unwrap_or_else(|| panic!("{v}")).to_owned()
}

async fn status(addr: SocketAddr, token: &str) -> Reply {
    http(addr, "GET", "/status", &[("authorization", &format!("Bearer {token}"))], "").await
}

async fn hub_status(addr: SocketAddr) -> HubStatus {
    let r = status(addr, LOCAL).await;
    assert_eq!(r.status, 200, "{}", r.body);
    serde_json::from_str(&r.body).unwrap()
}

/// 无会话请求：每个 Agent 一个主体（任务、句柄、句柄上限分开）；`clientInfo` 自报的名字不影响主体。
#[tokio::test(flavor = "multi_thread")]
async fn stateless_requests_are_separated_by_agent_token() {
    let hub = start().await;
    let addr = hub.listen_addr().unwrap();

    let claude = begin(addr, Some(CLAUDE)).await;
    let cursor = begin(addr, Some(CURSOR)).await;
    let local = begin(addr, Some(LOCAL)).await;
    let anonymous = begin(addr, None).await;

    let st = hub_status(addr).await;
    assert_eq!(st.agents.as_deref(), Some(&["claude".to_owned(), "cursor".to_owned()][..]));
    let task = |id: &str| st.tasks.as_ref().unwrap().iter().find(|t| t.id == id).cloned().unwrap_or_else(|| panic!("{id}"));
    assert_eq!(task(&claude).caller, format!("principal:agent:claude/{claude}"));
    assert_eq!(task(&claude).agent.as_deref(), Some("claude"));
    assert_eq!(task(&cursor).caller, format!("principal:agent:cursor/{cursor}"));
    assert_eq!(task(&local).caller, format!("principal:local/{local}"));
    assert_eq!((task(&local).agent, task(&anonymous).agent), (None, None), "本机令牌与不带令牌的回环请求同为本机主体");
    assert!(!serde_json::to_string(&st).unwrap().contains(CLAUDE), "/status 不含令牌");

    // 句柄归签发它的 Agent：其他 Agent / 本机出示与不存在相同
    for token in [CURSOR, LOCAL] {
        let v = modern_call(addr, Some(token), "apps.list", json!({"taskId": claude})).await.json();
        assert_eq!(v["result"]["isError"], true, "{v}");
        assert_eq!(v["result"]["structuredContent"]["error"]["details"]["reason"], "task-expired", "{v}");
        let v = modern_call(addr, Some(token), "apps.task.end", json!({"taskId": claude})).await.json();
        assert_eq!(v["result"]["structuredContent"]["ended"], false, "其他主体结束不了：{v}");
    }
    let r = modern_call(addr, Some(CLAUDE), "apps.task.end", json!({"taskId": claude})).await;
    assert_eq!(r.json()["result"]["structuredContent"]["ended"], true, "{}", r.body);
    hub.shutdown().await;
}

/// 句柄上限按 Agent 分别计算（一个 Agent 用满不影响另一个）。
#[tokio::test(flavor = "multi_thread")]
async fn task_handle_cap_is_per_agent() {
    let hub = Hub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: None,
        mcp_http: true,
        max_task_handles: 1,
        agents: agents(&[("claude", CLAUDE), ("cursor", CURSOR)]),
        ..Default::default()
    })
    .await
    .expect("hub");
    let addr = hub.listen_addr().unwrap();
    begin(addr, Some(CLAUDE)).await;
    let r = modern_call(addr, Some(CLAUDE), "apps.task.begin", json!({})).await.json();
    assert_eq!(r["result"]["structuredContent"]["error"]["kind"], "RATE_LIMITED", "{r}");
    begin(addr, Some(CURSOR)).await;
    begin(addr, None).await;
    hub.shutdown().await;
}

/// legacy 会话：以 Agent 令牌 `initialize` 的会话，其任务带该 Agent；会话内的请求沿用建立时的身份。
#[tokio::test(flavor = "multi_thread")]
async fn legacy_session_records_agent() {
    let hub = start().await;
    let addr = hub.listen_addr().unwrap();
    // 一个在线 App 实例（apps.select 需要）
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/app")).await.unwrap();
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": {
        "appId": "probe", "appName": "探测", "protocolVersion": "1", "sdkVersion": "t",
        "clientKind": "native", "instanceId": "probe-1"
    }});
    ws.send(WsMessage::text(hello.to_string())).await.unwrap();
    while let Some(Ok(m)) = tokio::time::timeout(T, ws.next()).await.unwrap() {
        if matches!(&m, WsMessage::Text(t) if t.contains("\"id\":1")) {
            break;
        }
    }

    let auth = format!("Bearer {CLAUDE}");
    let base = [
        ("content-type", "application/json"),
        ("accept", "application/json, text/event-stream"),
        ("authorization", auth.as_str()),
    ];
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "local-pretender", "version": "1"}
    }})
    .to_string();
    let r = http(addr, "POST", "/mcp", &base, &init).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let sid = r.headers.get("mcp-session-id").cloned().expect("Mcp-Session-Id");
    let mut headers = base.to_vec();
    headers.push(("mcp-session-id", &sid));
    headers.push(("mcp-protocol-version", "2025-06-18"));
    let select = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "apps.select", "arguments": {"appId": "probe", "instanceId": "probe-1"}}})
    .to_string();
    let r = http(addr, "POST", "/mcp", &headers, &select).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_ne!(r.json()["result"]["isError"], true, "{}", r.body);

    let st = hub_status(addr).await;
    let t = st.tasks.unwrap().into_iter().find(|t| t.caller.starts_with("mcp:")).expect("legacy 会话任务");
    assert_eq!(t.agent.as_deref(), Some("claude"));
    assert_eq!(t.selections.len(), 1);
    hub.shutdown().await;
}

/// Agent 令牌只用于 `/mcp`；`POST /agents` 由本机令牌替换登记，不合法时保留之前的登记。
#[tokio::test(flavor = "multi_thread")]
async fn agents_endpoint_replaces_registry_and_agent_tokens_are_mcp_only() {
    let hub = start().await;
    let addr = hub.listen_addr().unwrap();

    assert_eq!(status(addr, CLAUDE).await.status, 401, "Agent 令牌不能读 /status");
    let only_claude = serde_json::to_string(&agents(&[("claude", CLAUDE)])).unwrap();
    let post = |token: &'static str, body: String| async move {
        http(addr, "POST", "/agents", &[("authorization", &format!("Bearer {token}")), ("content-type", "application/json")], &body).await
    };
    assert_eq!(post(CLAUDE, only_claude.clone()).await.status, 401, "Agent 令牌不能改登记");
    assert_eq!(http(addr, "GET", "/agents", &[("authorization", &format!("Bearer {LOCAL}"))], "").await.status, 405);

    let r = post(LOCAL, only_claude).await;
    assert_eq!((r.status, r.json()["agents"].clone()), (200, json!(1)), "{}", r.body);
    assert_eq!(modern_call(addr, Some(CURSOR), "apps.list", json!({})).await.status, 401, "被移除的 Agent 令牌失效");
    assert_eq!(modern_call(addr, Some(CLAUDE), "apps.list", json!({})).await.status, 200);

    // 不合法：400，之前的登记继续生效；错误信息不含令牌
    let dup = serde_json::to_string(&agents(&[("a", CLAUDE), ("b", CLAUDE)])).unwrap();
    let r = post(LOCAL, dup).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.json()["error"].as_str().is_some_and(|e| !e.contains(CLAUDE)), "{}", r.body);
    assert_eq!(post(LOCAL, "{".into()).await.status, 400);
    assert_eq!(hub.status().agents.as_deref(), Some(&["claude".to_owned()][..]));

    // Hub API 同样替换
    hub.set_agents(agents(&[("cursor", CURSOR)])).expect("set_agents");
    assert_eq!(modern_call(addr, Some(CLAUDE), "apps.list", json!({})).await.status, 401);
    assert!(hub.set_agents(agents(&[("bad name", CURSOR)])).is_err());
    assert_eq!(hub.status().agents.as_deref(), Some(&["cursor".to_owned()][..]));
    hub.shutdown().await;
}

/// 启动时校验登记：不合法 → `InvalidInput`。
#[tokio::test(flavor = "multi_thread")]
async fn invalid_agents_config_rejected_at_start() {
    let r = Hub::start(HubConfig {
        listen: None,
        ipc_endpoint: None,
        agents: agents(&[("claude", "short")]),
        ..Default::default()
    })
    .await;
    assert_eq!(r.err().map(|e| e.kind()), Some(std::io::ErrorKind::InvalidInput));
}

struct Echo;
impl app_mcp_native::ToolHandler for Echo {
    fn invoke(&self, call: app_mcp_native::CallHandle) {
        let _ = call.complete(Some(r#"{"ok":true}"#), vec![]);
    }
}

/// 在线的商城 App（工具 `cart.add`），等到 Hub 注册了它的工具。
async fn start_shop(hub: &Hub) -> app_mcp_native::NativeClient {
    let mut c = app_mcp_native::NativeConfig::new("shop", "商城");
    c.host_url = format!("ws://{}/app", hub.listen_addr().unwrap());
    c.lifecycle.mode = app_mcp_native::LifecycleMode::Persistent;
    let app = app_mcp_native::NativeClient::new(c, None).expect("client");
    app.register_tool(app_mcp_native::ToolSpec::new("cart.add", "加入购物车"), std::sync::Arc::new(Echo)).expect("tool");
    app.start();
    let deadline = std::time::Instant::now() + T;
    while !hub.status().apps.iter().any(|a| a.app_id == "shop" && a.tools.len() == 1) {
        assert!(std::time::Instant::now() < deadline, "App 未注册工具");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    app
}

/// 第 16 项 P2：按 Agent 的 `deny` 规则只拒绝该 Agent 的调用（`POLICY_DENIED` 带规则 id），其他 Agent 与本机主体照常；
/// 工具对所有 Agent 都列出（`deny` 不改变列表）。
#[tokio::test(flavor = "multi_thread")]
async fn deny_rule_applies_only_to_named_agent() {
    let policy = app_mcp_hub::PolicyConfig::from_json(
        r#"{"rules": [{"id": "no-cart-cursor", "action": "deny", "app": "shop", "tool": "cart.*", "agent": "cursor"}]}"#,
    )
    .unwrap();
    let hub = Hub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: None,
        mcp_http: true,
        agents: agents(&[("claude", CLAUDE), ("cursor", CURSOR)]),
        policy,
        ..Default::default()
    })
    .await
    .expect("hub");
    let addr = hub.listen_addr().unwrap();
    let app = start_shop(&hub).await;

    let denied = modern_call(addr, Some(CURSOR), "shop.cart.add", json!({})).await.json();
    let err = &denied["result"]["structuredContent"]["error"];
    assert_eq!((err["kind"].as_str(), err["details"]["ruleId"].as_str()), (Some("POLICY_DENIED"), Some("no-cart-cursor")), "{denied}");
    for token in [Some(CLAUDE), Some(LOCAL_UNCONFIGURED), None] {
        let r = modern_call(addr, token, "shop.cart.add", json!({})).await.json();
        assert_ne!(r["result"]["isError"], true, "{token:?}: {r}");
    }
    let hits = hub.policy().rules[0].hits;
    assert_eq!(hits, 1, "只有 cursor 的调用命中");
    app.stop();
    hub.shutdown().await;
}

/// 第 16 项 P3：每 Agent 一级限流（跨所有 App 合计，`scope: agent`）只作用于该 Agent；`/status` 的 `usage` 按主体记调用、
/// 被限流次数与字节数，按 App 细分。
#[tokio::test(flavor = "multi_thread")]
async fn agent_quota_and_usage_accounting() {
    let mut limits = app_mcp_hub::LimitPolicy::unlimited();
    limits.agent_rate = app_mcp_hub::RateLimit { per_minute: 1, burst: 2 };
    let hub = Hub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: None,
        mcp_http: true,
        agents: agents(&[("claude", CLAUDE), ("cursor", CURSOR)]),
        limits,
        ..Default::default()
    })
    .await
    .expect("hub");
    let addr = hub.listen_addr().unwrap();
    let app = start_shop(&hub).await;

    for _ in 0..2 {
        let r = modern_call(addr, Some(CLAUDE), "shop.cart.add", json!({"n": 1})).await.json();
        assert_ne!(r["result"]["isError"], true, "{r}");
    }
    let r = modern_call(addr, Some(CLAUDE), "shop.cart.add", json!({"n": 1})).await.json();
    let e = &r["result"]["structuredContent"]["error"];
    assert_eq!((e["kind"].as_str(), e["details"]["scope"].as_str()), (Some("RATE_LIMITED"), Some("agent")), "{r}");
    for token in [Some(CURSOR), None, None, None] {
        let r = modern_call(addr, token, "shop.cart.add", json!({})).await.json();
        assert_ne!(r["result"]["isError"], true, "其他 Agent 与本机不受 claude 的配额影响：{r}");
    }

    let usage = hub.status().usage.expect("usage");
    let of = |subject: &str| usage.iter().find(|u| u.subject == subject).cloned().unwrap_or_else(|| panic!("{subject}: {usage:?}"));
    let claude = of("agent:claude");
    assert_eq!((claude.agent.as_deref(), claude.total.calls, claude.total.rate_limited), (Some("claude"), 2, 1));
    assert_eq!(claude.total.arguments_bytes, 2 * r#"{"n":1}"#.len() as u64);
    assert!(claude.total.result_bytes > 0);
    assert_eq!((claude.apps.len(), claude.apps[0].app_id.as_str(), claude.apps[0].counts.calls), (1, "shop", 2));
    assert_eq!((of("agent:cursor").total.calls, of("local").total.calls, of("local").agent.clone()), (1, 3, None));
    app.stop();
    hub.shutdown().await;
}

/// 第 16 项 N6：一个 Agent 经 MCP 加的 App 锁拦截其他 Agent 与本机主体的写调用（`LOCKED`，`holder` 为 `agent:<名>`、不含任务 ID），
/// 不拦截持有者自己；其他 Agent 释放不了；持有者解锁后恢复。
#[tokio::test(flavor = "multi_thread")]
async fn app_lock_is_held_per_agent() {
    let hub = start().await;
    let addr = hub.listen_addr().unwrap();
    let app = start_shop(&hub).await;

    let r = modern_call(addr, Some(CLAUDE), "apps.lock", json!({"appId": "shop"})).await.json();
    assert_ne!(r["result"]["isError"], true, "{r}");
    for token in [Some(CURSOR), Some(LOCAL)] {
        let r = modern_call(addr, token, "shop.cart.add", json!({})).await.json();
        let e = &r["result"]["structuredContent"]["error"];
        assert_eq!((e["kind"].as_str(), e["details"]["holder"].as_str()), (Some("LOCKED"), Some("agent:claude")), "{r}");
    }
    let r = modern_call(addr, Some(CLAUDE), "shop.cart.add", json!({})).await.json();
    assert_ne!(r["result"]["isError"], true, "持有者自己照常：{r}");
    let r = modern_call(addr, Some(CURSOR), "apps.unlock", json!({"appId": "shop"})).await.json();
    assert_eq!(r["result"]["structuredContent"]["released"], false, "{r}");
    let st = hub_status(addr).await;
    let locks = st.locks.expect("locks");
    assert_eq!((locks.len(), locks[0].caller.as_str(), locks[0].holder.as_str()), (1, "principal:agent:claude", "agent:claude"));

    let r = modern_call(addr, Some(CLAUDE), "apps.unlock", json!({"appId": "shop"})).await.json();
    assert_eq!(r["result"]["structuredContent"]["released"], true, "{r}");
    let r = modern_call(addr, Some(CURSOR), "shop.cart.add", json!({})).await.json();
    assert_ne!(r["result"]["isError"], true, "{r}");
    app.stop();
    hub.shutdown().await;
}

/// 第 16 项 P7：Hub 状态资源。`app-mcp://apps/self` 只含读取方自己的任务、锁、用量与配额余量（不含任务 ID），
/// `app-mcp://apps/hub` 列出 App 概况与所有锁（持有者只给主体）；两者出现在 `resources/list`、不可订阅。
#[tokio::test(flavor = "multi_thread")]
async fn hub_state_resources_show_own_view() {
    let mut limits = app_mcp_hub::LimitPolicy::unlimited();
    // 每分钟补 1 个：测试期间的补充不足以改变取整后的余量。
    limits.agent_rate = app_mcp_hub::RateLimit { per_minute: 1, burst: 5 };
    let hub = Hub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: None,
        mcp_http: true,
        agents: agents(&[("claude", CLAUDE), ("cursor", CURSOR)]),
        limits,
        ..Default::default()
    })
    .await
    .expect("hub");
    let addr = hub.listen_addr().unwrap();
    let app = start_shop(&hub).await;
    let read = |token: &'static str, uri: &'static str| async move {
        let r = modern_request(addr, Some(token), "resources/read", uri, json!({"uri": uri})).await.json();
        let text = r["result"]["contents"][0]["text"].as_str().unwrap_or_else(|| panic!("{r}")).to_owned();
        serde_json::from_str::<Value>(&text).unwrap()
    };

    let r = modern_request(addr, Some(CLAUDE), "resources/list", "", json!({})).await.json();
    let uris: Vec<&str> = r["result"]["resources"].as_array().unwrap().iter().filter_map(|x| x["uri"].as_str()).collect();
    assert!(uris.contains(&"app-mcp://apps/hub") && uris.contains(&"app-mcp://apps/self"), "{r}");

    let task = begin(addr, Some(CLAUDE)).await;
    let r = modern_call(addr, Some(CLAUDE), "apps.lock", json!({"appId": "shop"})).await.json();
    assert_ne!(r["result"]["isError"], true, "{r}");
    let r = modern_call(addr, Some(CLAUDE), "shop.cart.add", json!({})).await.json();
    assert_ne!(r["result"]["isError"], true, "{r}");

    let me = read(CLAUDE, "app-mcp://apps/self").await;
    assert_eq!((me["subject"].as_str(), me["agent"].as_str()), (Some("agent:claude"), Some("claude")), "{me}");
    let tasks = me["tasks"].as_array().unwrap();
    assert_eq!(tasks.iter().map(|t| t["handle"].as_bool()).collect::<Vec<_>>(), [Some(false), Some(true)], "主体任务在前：{me}");
    assert!(!me.to_string().contains(&task), "不含任务 ID：{me}");
    assert_eq!((me["locks"][0]["appId"].as_str(), me["locks"][0]["holder"].as_str()), (Some("shop"), Some("agent:claude")), "{me}");
    // begin、lock、cart.add 中只有 App 工具经过资源保护：用掉 1 个令牌。
    assert_eq!((me["usage"]["calls"].as_u64(), me["quota"]["burst"].as_u64()), (Some(1), Some(5)), "{me}");
    assert_eq!(me["quota"]["available"].as_u64(), Some(4), "{me}");

    let other = read(CURSOR, "app-mcp://apps/self").await;
    assert_eq!((other["tasks"].as_array().map(Vec::len), other["locks"].as_array().map(Vec::len)), (Some(0), Some(0)), "{other}");
    assert!(other["usage"].is_null() && other["quota"]["available"].as_u64() == Some(5), "{other}");
    let local = read(LOCAL_UNCONFIGURED, "app-mcp://apps/self").await;
    assert!(local["subject"] == "local" && local["quota"].is_null(), "本机主体不受每 Agent 配额：{local}");

    let st = read(CURSOR, "app-mcp://apps/hub").await;
    let shop = st["apps"].as_array().unwrap().iter().find(|a| a["appId"] == "shop").cloned().unwrap_or_else(|| panic!("{st}"));
    assert_eq!((shop["state"].as_str(), shop["connected"].as_u64(), shop["dormant"].as_u64()), (Some("connected"), Some(1), Some(0)));
    assert_eq!((st["locks"][0]["holder"].as_str(), st["locks"][0].get("caller")), (Some("agent:claude"), None), "{st}");

    let r = modern_request(addr, Some(CLAUDE), "resources/read", "app-mcp://apps/nope", json!({"uri": "app-mcp://apps/nope"})).await.json();
    assert!(r["error"].is_object(), "未知的 Hub 状态资源：{r}");
    let api = hub.read_resource("app-mcp://apps/self").await.expect("Hub API 读取");
    let api: Value = serde_json::from_str(api.text.as_deref().unwrap_or_default()).unwrap();
    assert_eq!(api["subject"], "api", "{api}");
    let e = hub.subscribe("app-mcp://apps/hub").expect_err("不可订阅");
    assert_eq!(e.0.kind, app_mcp_hub::ErrorKind::InvalidInput, "{e:?}");
    app.stop();
    hub.shutdown().await;
}
