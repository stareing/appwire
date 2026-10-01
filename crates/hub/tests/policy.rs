//! 第 16 项 P2 策略挂点与第 18 项 L2 的集成测试：tokio-tungstenite 客户端扮演 App 端 SDK。
//!
//! 覆盖：`hide`（App 级 / 工具级 / 按注解）从 Hub API 与 MCP 出口的所有列表中去掉，调用按 `TOOL_NOT_FOUND`、资源按不存在；
//! `deny`（call / wake）返回 `POLICY_DENIED` 且只附规则 id、不转发、不消耗限流令牌、不触发审批；无规则时与之前一致；
//! `set_policy` 不合法时保留旧规则并记下错误；命中计数；`USER_ACTION_REQUIRED` 原样转发与结果大小上限。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::wake::{WakeRequest, Waker};
use app_mcp_hub::{
    ApprovalHandler, ApprovalPolicy, ApprovalRequest, CallRequest, ErrorKind, Hub, HubConfig, HubError, LimitPolicy,
    PolicyConfig, RateLimit, Risk, ToolFilter, async_trait,
};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as Ws;

const T: Duration = Duration::from_secs(5);

/// 假 App：注册工具与资源，`tools/invoke` 按工具名回复；返回收到的 `tools/invoke` 次数计数器。
async fn connect_app(hub: &Hub, app_id: &str) -> Arc<Mutex<Vec<String>>> {
    let url = format!("ws://{}/app", hub.listen_addr().expect("listen addr"));
    let (ws, _) = tokio_tungstenite::connect_async(url).await.expect("ws");
    let (mut sink, mut stream) = ws.split();
    let send = |v: Value| Ws::text(v.to_string());
    sink.send(send(json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": {
        "appId": app_id, "appName": app_id, "protocolVersion": "1", "sdkVersion": "0", "clientKind": "native",
        "instanceId": format!("{app_id}-1")
    }})))
    .await
    .unwrap();
    loop {
        let Some(Ok(Ws::Text(t))) = timeout(T, stream.next()).await.expect("hello") else { panic!("握手失败") };
        let v: Value = serde_json::from_str(t.as_str()).unwrap();
        if v["id"] == 1 {
            assert_eq!(v["result"]["status"], "paired");
            break;
        }
    }
    let schema = json!({"type": "object"});
    let tools = json!([
        {"name": "order.submit", "description": "下单", "inputSchema": schema, "risk": "payment"},
        {"name": "cart.add", "description": "加购", "inputSchema": schema},
        {"name": "admin.reset", "description": "重置", "inputSchema": schema},
        {"name": "login.check", "description": "需要登录", "inputSchema": schema, "risk": "read"},
        {"name": "login.bare", "description": "需要切到前台", "inputSchema": schema, "risk": "read"},
        {"name": "huge.error", "description": "超大错误", "inputSchema": schema, "risk": "read"},
    ]);
    for m in [
        // 资源先于工具同步：工具出现时资源也已登记（同一连接按顺序处理）。
        json!({"jsonrpc": "2.0", "method": "resources/sync", "params": {"resources": [{"name": "state", "description": "状态"}]}}),
        json!({"jsonrpc": "2.0", "method": "tools/sync", "params": {"tools": tools}}),
        json!({"jsonrpc": "2.0", "method": "app/ready", "params": {}}),
    ] {
        sink.send(send(m)).await.unwrap();
    }
    let invoked = Arc::new(Mutex::new(Vec::new()));
    let seen = invoked.clone();
    tokio::spawn(async move {
        while let Some(Ok(Ws::Text(t))) = stream.next().await {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            let (Some(id), Some(method)) = (v.get("id").cloned(), v["method"].as_str()) else { continue };
            let name = v["params"]["name"].as_str().unwrap_or_default().to_owned();
            if method == "tools/invoke" {
                seen.lock().unwrap().push(name.clone());
            }
            let reply = match (method, name.as_str()) {
                ("tools/invoke", "login.check") => json!({"jsonrpc": "2.0", "id": id, "error": {
                    "code": -32019, "message": "登录已过期，请在 App 内重新登录后重试。",
                    "data": {"kind": "USER_ACTION_REQUIRED", "reason": "login", "uri": "shop://login"}}}),
                ("tools/invoke", "login.bare") => json!({"jsonrpc": "2.0", "id": id, "error": {
                    "code": -32019, "message": "请把 App 切到前台。", "data": {"kind": "USER_ACTION_REQUIRED"}}}),
                ("tools/invoke", "huge.error") => json!({"jsonrpc": "2.0", "id": id, "error": {
                    "code": -32019, "message": "x".repeat(400), "data": {"kind": "USER_ACTION_REQUIRED"}}}),
                ("tools/invoke", _) => json!({"jsonrpc": "2.0", "id": id, "result": {"data": {"ok": true}}}),
                ("resources/read", _) => json!({"jsonrpc": "2.0", "id": id, "result": {"contents": {"n": 1}}}),
                _ => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
            };
            if sink.send(send(reply)).await.is_err() {
                break;
            }
        }
    });
    // 用 /status 判断（被隐藏的 App 不出现在 Agent 可见的列表中）。
    timeout(T, async {
        while !hub.status().apps.iter().any(|a| a.app_id == app_id && a.tools.len() == 6) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("工具没有出现");
    invoked
}

fn policy(v: Value) -> PolicyConfig {
    PolicyConfig::from_json(&v.to_string()).expect("规则合法")
}

fn config(rules: PolicyConfig) -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        response_timeout: Duration::from_secs(3),
        policy: rules,
        ..Default::default()
    }
}

async fn call(hub: &Hub, name: &str) -> Result<app_mcp_hub::CallOutcome, HubError> {
    hub.call_tool(CallRequest::new(name, json!({}))).await
}

fn names(hub: &Hub) -> Vec<String> {
    hub.tools(&ToolFilter::default()).into_iter().map(|t| t.name).collect()
}

#[tokio::test]
async fn no_rules_changes_nothing() {
    let hub = Hub::start(config(PolicyConfig::default())).await.unwrap();
    connect_app(&hub, "shop").await;
    let all = names(&hub);
    assert!(all.contains(&"shop.admin.reset".to_owned()) && all.contains(&"shop.order.submit".to_owned()));
    assert!(call(&hub, "shop.admin.reset").await.unwrap().result.is_ok());
    let st = hub.policy();
    assert!(st.rules.is_empty() && st.last_error.is_none());
    assert_eq!(hub.status().policy.unwrap().rules.len(), 0);
}

#[tokio::test]
async fn hide_removes_from_every_list_and_calls_are_not_found() {
    let rules = policy(json!({"rules": [
        {"id": "hide-notes", "action": "hide", "app": "notes"},
        {"id": "hide-admin", "action": "hide", "app": "shop", "tool": "admin.*"},
        {"id": "hide-pay", "action": "hide", "app": "*", "annotations": {"destructiveHint": true}},
    ]}));
    let hub = Hub::start(config(rules)).await.unwrap();
    let shop_calls = connect_app(&hub, "shop").await;
    let notes_calls = connect_app(&hub, "notes").await;

    let all = names(&hub);
    assert!(all.contains(&"shop.cart.add".to_owned()), "{all:?}");
    for hidden in ["shop.admin.reset", "shop.order.submit", "notes.cart.add"] {
        assert!(!all.contains(&hidden.to_owned()), "{hidden} 应被隐藏：{all:?}");
    }
    // 显式按 App 过滤也看不到（hide 全局生效）
    let explicit = hub.tools(&ToolFilter { apps: Some(vec!["notes".into(), "shop".into()]), include_builtin: false, ..Default::default() });
    assert!(explicit.iter().all(|t| t.app_id == "shop" && t.tool != "admin.reset"));
    assert!(hub.apps().iter().all(|a| a.app_id != "notes"));
    assert!(hub.resources().iter().all(|r| r.app_id != "notes"));
    assert!(hub.resources().iter().any(|r| r.app_id == "shop"));
    assert!(hub.overview("notes").is_none());
    let export = hub.export_tools(app_mcp_hub::ToolFormat::OpenAiChat, &ToolFilter::default()).to_string();
    assert!(!export.contains("admin__reset") && !export.contains("notes__"), "{export}");

    // 调用：App 级隐藏 = appId 未知（Err），工具级隐藏 = TOOL_NOT_FOUND；都不转发
    let e = call(&hub, "notes.cart.add").await.unwrap_err();
    assert_eq!(e.0.kind, ErrorKind::ToolNotFound);
    assert!(e.0.message.contains("没有 appId"), "{}", e.0.message);
    for name in ["shop.admin.reset", "shop.order.submit"] {
        let e = call(&hub, name).await.unwrap().result.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ToolNotFound, "{name}");
        assert!(!e.message.contains("hide") && e.details.is_none(), "不暴露规则：{e:?}");
    }
    assert!(shop_calls.lock().unwrap().is_empty() && notes_calls.lock().unwrap().is_empty());
    assert!(hub.read_resource("app-mcp://notes/state").await.unwrap_err().0.kind == ErrorKind::ResourceNotFound);
    assert!(hub.subscribe("app-mcp://notes/state").is_err());

    // 内置工具：apps.list / apps.tools / apps.overview / apps.select
    let list = call(&hub, "apps.list").await.unwrap().result.unwrap();
    let text = list.to_string();
    assert!(!text.contains("\"notes\"") && !text.contains("admin.reset") && !text.contains("order.submit"), "{text}");
    let r = hub.call_tool(CallRequest::new("apps.tools", json!({"appId": "shop"}))).await.unwrap().result.unwrap();
    assert_eq!(r["tools"].as_array().unwrap().len(), 4, "{r}");
    for (tool, args) in [
        ("apps.tools", json!({"appId": "notes"})),
        ("apps.overview", json!({"appId": "notes"})),
    ] {
        let e = hub.call_tool(CallRequest::new(tool, args)).await.unwrap().result.unwrap_err();
        assert_eq!(e.kind, ErrorKind::ToolNotFound, "{tool}");
    }
    let e = hub
        .call_tool(CallRequest::new("apps.select", json!({"appId": "notes", "instanceId": "notes-1"})))
        .await
        .unwrap()
        .result
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::AppDisconnected, "与不存在的实例相同");

    // 命中计数：被隐藏的调用 / 读取计数，列表过滤不计
    let hits: Vec<(String, u64)> = hub.policy().rules.into_iter().map(|r| (r.rule.id, r.hits)).collect();
    assert_eq!(hits, [("hide-notes".into(), 2), ("hide-admin".into(), 1), ("hide-pay".into(), 1)]);
    // /status 仍显示被隐藏的 App（诊断用）
    assert!(hub.status().apps.iter().any(|a| a.app_id == "notes"));
}

#[cfg(feature = "mcp-server")]
#[tokio::test]
async fn mcp_egress_honours_hide_and_deny() {
    use rmcp::ServiceExt;
    use rmcp::model::CallToolRequestParams;

    let rules = policy(json!({"rules": [
        {"id": "hide-notes", "action": "hide", "app": "notes"},
        {"id": "hide-admin", "action": "hide", "app": "shop", "tool": "admin.reset"},
        {"id": "no-pay", "action": "deny", "app": "shop", "tool": "order.*"},
    ]}));
    let hub = Hub::start(config(rules)).await.unwrap();
    connect_app(&hub, "shop").await;
    connect_app(&hub, "notes").await;
    let (c, s) = tokio::io::duplex(1 << 20);
    let session = hub.mcp_session();
    tokio::spawn(async move {
        if let Ok(svc) = session.serve(s).await {
            let _ = svc.waiting().await;
        }
    });
    let client = ().serve(c).await.expect("mcp");
    let instructions = client.peer_info().and_then(|i| i.instructions.clone()).unwrap_or_default();
    assert!(!instructions.contains("notes"), "{instructions}");
    let listed: Vec<String> = client.list_all_tools().await.unwrap().into_iter().map(|t| t.name.to_string()).collect();
    assert!(listed.contains(&"shop.order.submit".to_owned()), "deny 的工具仍可见：{listed:?}");
    assert!(!listed.iter().any(|n| n.starts_with("notes.") || n == "shop.admin.reset"), "{listed:?}");
    let resources = client.list_all_resources().await.unwrap();
    assert!(resources.iter().all(|r| !r.uri.starts_with("app-mcp://notes/")));

    let r = client.call_tool(CallToolRequestParams::new("shop.order.submit")).await.unwrap();
    assert_eq!(r.is_error, Some(true));
    let err = &r.structured_content.as_ref().unwrap()["error"];
    assert_eq!(err["kind"], "POLICY_DENIED");
    assert_eq!(err["details"], json!({"ruleId": "no-pay", "hook": "call", "appId": "shop", "tool": "order.submit"}));
    let r = client.call_tool(CallToolRequestParams::new("notes.cart.add")).await.unwrap();
    assert_eq!(r.structured_content.as_ref().unwrap()["error"]["kind"], "TOOL_NOT_FOUND");
    let _ = client.cancel().await;
}

struct CountingApproval(Mutex<u32>);

#[async_trait]
impl ApprovalHandler for CountingApproval {
    async fn approve(&self, _req: ApprovalRequest) -> bool {
        *self.0.lock().unwrap() += 1;
        true
    }
}

#[tokio::test]
async fn deny_before_limits_and_approval() {
    let rules = policy(json!({"rules": [{"id": "no-pay", "action": "deny", "app": "shop", "annotations": {"destructiveHint": true}}]}));
    let hub = Hub::start(HubConfig {
        limits: LimitPolicy { tool_rate: RateLimit { per_minute: 60, burst: 1 }, ..LimitPolicy::default() },
        approval: ApprovalPolicy { require_at_or_above: Some(Risk::Write), ..ApprovalPolicy::default() },
        ..config(rules)
    })
    .await
    .unwrap();
    let approvals = Arc::new(CountingApproval(Mutex::new(0)));
    hub.set_approval_handler(approvals.clone());
    let calls = connect_app(&hub, "shop").await;

    for _ in 0..3 {
        let e = call(&hub, "shop.order.submit").await.unwrap().result.unwrap_err();
        assert_eq!(e.kind, ErrorKind::PolicyDenied);
        assert_eq!(e.details.as_ref().unwrap()["ruleId"], "no-pay");
        assert!(!e.message.contains("destructiveHint"), "不附规则内容：{}", e.message);
    }
    assert!(calls.lock().unwrap().is_empty(), "被拒绝的调用不转发");
    assert_eq!(*approvals.0.lock().unwrap(), 0, "被拒绝的调用不触发审批");
    // 被拒绝的调用不消耗令牌：限流为突发 1，这里仍能调用一次
    assert!(call(&hub, "shop.cart.add").await.unwrap().result.is_ok());
    assert_eq!(hub.status().apps.iter().find(|a| a.app_id == "shop").unwrap().rate_limited, 0);
    assert_eq!(hub.policy().rules[0].hits, 3);
}

#[tokio::test]
async fn set_policy_replaces_and_keeps_previous_on_error() {
    let hub = Hub::start(config(PolicyConfig::default())).await.unwrap();
    connect_app(&hub, "shop").await;
    let mut events = hub.events();

    hub.set_policy(policy(json!({"rules": [{"id": "h", "action": "hide", "app": "shop", "tool": "cart.add"}]}))).unwrap();
    assert!(!names(&hub).contains(&"shop.cart.add".to_owned()));
    timeout(T, async {
        loop {
            if let Ok(app_mcp_hub::HubEvent::ToolsChanged) = events.recv().await {
                break;
            }
        }
    })
    .await
    .expect("规则变化后发 ToolsChanged");

    let bad = PolicyConfig::from_json(r#"{"rules": [{"id": "x", "action": "deny", "app": "shop", "hooks": ["handle"]}]}"#);
    assert!(bad.is_err(), "句柄执行点尚未实现");
    let invalid: PolicyConfig = serde_json::from_value(json!({"rules": [{"id": "", "action": "hide", "app": "shop"}]})).unwrap();
    let e = hub.set_policy(invalid).unwrap_err();
    assert_eq!(e.0.kind, ErrorKind::InvalidInput);
    let st = hub.policy();
    assert_eq!(st.rules.len(), 1, "保留之前的规则");
    assert!(st.last_error.as_ref().is_some_and(|e| e.message.contains("id")));
    assert!(!names(&hub).contains(&"shop.cart.add".to_owned()));

    hub.set_policy(PolicyConfig::default()).unwrap();
    assert!(names(&hub).contains(&"shop.cart.add".to_owned()));
    assert!(hub.policy().last_error.is_none());
}

#[tokio::test]
async fn invalid_policy_rejects_start() {
    let invalid: PolicyConfig = serde_json::from_value(json!({"rules": [{"id": "a", "action": "hide", "app": "x*y"}]})).unwrap();
    let e = Hub::start(config(invalid)).await.err().expect("不合法的规则拒绝启动");
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
}

/// 记录唤醒请求的假唤醒器（总是失败：只验证是否发起唤醒）。
#[derive(Default)]
struct RecordingWaker(Mutex<u32>);

#[async_trait]
impl Waker for RecordingWaker {
    async fn wake(&self, _req: WakeRequest) -> Result<(), HubError> {
        *self.0.lock().unwrap() += 1;
        Err(HubError::new(ErrorKind::LaunchFailed, "测试：不真正启动"))
    }
}

#[tokio::test]
async fn deny_wake_blocks_cold_start_only() {
    let manifest = app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": "music", "name": "音乐",
            "wake": {"web": [{"kind": "web-url", "target": "http://localhost:5173/"}]},
            "tools": [{"name": "play", "description": "播放", "inputSchema": {"type": "object"}}]
        })
        .to_string(),
    )
    .unwrap();
    let rules = policy(json!({"rules": [{"id": "no-wake-music", "action": "deny", "app": "music", "hooks": ["wake"]}]}));
    let mut cfg = config(rules);
    cfg.manifests = vec![manifest];
    let hub = Hub::start(cfg).await.unwrap();
    let waker = Arc::new(RecordingWaker::default());
    hub.set_waker(waker.clone());

    let e = call(&hub, "music.play").await.unwrap().result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::PolicyDenied);
    assert_eq!(e.details.unwrap(), json!({"ruleId": "no-wake-music", "hook": "wake", "appId": "music", "tool": "play"}));
    assert_eq!(*waker.0.lock().unwrap(), 0, "没有发起唤醒");
    assert_eq!(hub.policy().rules[0].hits, 1);

    // 去掉规则后照常唤醒
    hub.set_policy(PolicyConfig::default()).unwrap();
    let e = call(&hub, "music.play").await.unwrap().result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::LaunchFailed);
    assert_eq!(*waker.0.lock().unwrap(), 1);
}

#[tokio::test]
async fn user_action_required_forwarded_unchanged() {

    let hub = Hub::start(HubConfig {
        limits: LimitPolicy { max_result_bytes: 300, ..LimitPolicy::default() },
        ..config(PolicyConfig::default())
    })
    .await
    .unwrap();
    connect_app(&hub, "shop").await;
    let e = call(&hub, "shop.login.check").await.unwrap().result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::UserActionRequired);
    assert_eq!(e.message, "登录已过期，请在 App 内重新登录后重试。");
    assert_eq!(e.details, Some(json!({"reason": "login", "uri": "shop://login"})));
    let e = call(&hub, "shop.login.bare").await.unwrap().result.unwrap_err();
    assert_eq!((e.kind, e.details), (ErrorKind::UserActionRequired, None));
    // App 给出的说明计入结果大小上限
    let e = call(&hub, "shop.huge.error").await.unwrap().result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::PayloadTooLarge);
    assert_eq!(e.details.unwrap()["part"], "result");
    #[cfg(feature = "mcp-server")]
    user_action_required_over_mcp(&hub).await;
}

#[cfg(feature = "mcp-server")]
async fn user_action_required_over_mcp(hub: &Hub) {
    use rmcp::ServiceExt;
    use rmcp::model::CallToolRequestParams;

    let (c, s) = tokio::io::duplex(1 << 20);
    let session = hub.mcp_session();
    tokio::spawn(async move {
        if let Ok(svc) = session.serve(s).await {
            let _ = svc.waiting().await;
        }
    });
    let client = ().serve(c).await.expect("mcp");
    let r = client.call_tool(CallToolRequestParams::new("shop.login.check")).await.unwrap();
    assert_eq!(r.is_error, Some(true));
    let text: Vec<String> = r.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect();
    assert!(text.iter().any(|t| t == "USER_ACTION_REQUIRED: 登录已过期，请在 App 内重新登录后重试。"), "{text:?}");
    assert_eq!(
        r.structured_content.unwrap()["error"],
        json!({"kind": "USER_ACTION_REQUIRED", "message": "登录已过期，请在 App 内重新登录后重试。",
               "details": {"reason": "login", "uri": "shop://login"}})
    );
    let _ = client.cancel().await;
}
