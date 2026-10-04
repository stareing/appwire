//! 工具检索 `apps.search`（第 16 项 O1，spec/hub-api.md 3.18）。
//!
//! App 端是真实的 `app-mcp-native` 客户端：已连接的 App、休眠的 App（确认检索不唤醒）、只有清单的 App 与页面目录；
//! 覆盖候选来源与可用性、`hide` 过滤、`appId` 限定、`limit` / `total`、输入校验、渐进暴露、使用后排序变化。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{
    Availability, CallRequest, ErrorKind, Hub, HubConfig, HubError, PolicyConfig, ToolError, ToolExposure, ToolFilter, WakeRequest,
    Waker, async_trait,
};
use app_mcp_native::{
    CallHandle, LifecycleMode, NativeClient, NativeConfig, StateStatus, ToolHandler, ToolSpec, WakeDescriptor as NativeWake,
    WakeKind as NativeWakeKind,
};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);

fn manifest(v: Value) -> app_mcp_manifest::Manifest {
    app_mcp_manifest::parse(&v.to_string()).expect("manifest")
}

fn manifests() -> Vec<app_mcp_manifest::Manifest> {
    vec![
        manifest(json!({
            "manifestVersion": 1, "appId": "shop", "name": "商城",
            "tools": [
                {"name": "orders.export", "title": "Export orders", "description": "导出订单为 CSV", "inputSchema": {"type": "object"}}
            ],
            "pages": [
                {"name": "cart", "title": "购物车", "description": "查看与结算", "tools": [
                    {"name": "cart.checkout", "description": "提交订单并付款", "inputSchema": {"type": "object"}}
                ]}
            ]
        })),
        manifest(json!({
            "manifestVersion": 1, "appId": "mail", "name": "邮件",
            "tools": [
                {"name": "send", "description": "发送一封信", "inputSchema": {"type": "object", "required": ["to"]}},
                {"name": "admin.purge", "description": "清空全部邮件", "inputSchema": {"type": "object"}}
            ]
        })),
        manifest(json!({
            "manifestVersion": 1, "appId": "vault", "name": "保险箱",
            "tools": [{"name": "orders.peek", "description": "查看订单", "inputSchema": {"type": "object"}}]
        })),
    ]
}

fn config() -> HubConfig {
    let policy = PolicyConfig::from_json(
        &json!({"rules": [
            {"id": "hide-vault", "action": "hide", "app": "vault"},
            {"id": "hide-purge", "action": "hide", "app": "mail", "tool": "admin.*"},
        ]})
        .to_string(),
    )
    .expect("规则合法");
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        wake_timeout: Duration::from_secs(2),
        manifests: manifests(),
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

/// 成功返回；工具名以 `.fail` 结尾时返回错误。
struct Handler;
impl ToolHandler for Handler {
    fn invoke(&self, call: CallHandle) {
        std::thread::spawn(move || {
            if call.tool_name().ends_with(".fail") {
                let _ = call.fail(ErrorKind::HandlerError, "失败");
            } else {
                let _ = call.complete(Some("{}"), vec![]);
            }
        });
    }
}

fn native(hub: &Hub, app_id: &str, name: &str, mode: LifecycleMode, tools: &[(&str, &str)]) -> NativeClient {
    let mut c = NativeConfig::new(app_id, name);
    c.host_url = format!("ws://{}/app", hub.listen_addr().unwrap());
    c.instance_id = Some(format!("{app_id}-1"));
    c.lifecycle.mode = mode;
    if mode == LifecycleMode::Idle {
        c.launch_token = Some(String::new());
        c.lifecycle.idle_timeout_ms = 150;
        c.lifecycle.wake = Some(NativeWake { kind: NativeWakeKind::Uri, target: Some(format!("{app_id}-app")), background: true });
    }
    let client = NativeClient::new(c, None).unwrap();
    for (tool, description) in tools {
        client.register_tool(ToolSpec::new(*tool, *description), Arc::new(Handler)).unwrap();
    }
    client.start();
    client
}

/// 计数的 Waker：检索不应调用它。
#[derive(Default)]
struct CountingWaker(Mutex<usize>);

#[async_trait]
impl Waker for CountingWaker {
    async fn wake(&self, _req: WakeRequest) -> Result<(), HubError> {
        *self.0.lock().unwrap() += 1;
        Ok(())
    }
}

fn has_tool(hub: &Hub, name: &str, availability: Availability) -> bool {
    hub.tools(&ToolFilter { include_builtin: false, ..Default::default() }).iter().any(|t| t.name == name && t.availability == availability)
}

async fn search(hub: &Hub, session: Option<&str>, args: Value) -> Result<Value, ToolError> {
    let req = CallRequest { session: session.map(str::to_owned), ..CallRequest::new("apps.search", args) };
    tokio::time::timeout(T, hub.call_tool(req)).await.expect("调用超时").unwrap().result
}

fn names(v: &Value) -> Vec<String> {
    v["results"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_owned()).collect()
}

fn hit<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["results"].as_array().unwrap().iter().find(|r| r["name"] == name).unwrap_or_else(|| panic!("缺少 {name}：{v}"))
}

/// 候选覆盖已注册 / 清单 / 页面目录 / 休眠快照；隐藏的不出现；检索不唤醒。
#[tokio::test(flavor = "multi_thread")]
async fn searches_every_source_without_waking() {
    let hub = Hub::start(config()).await.unwrap();
    let waker = Arc::new(CountingWaker::default());
    hub.set_waker(waker.clone());
    let notes = native(&hub, "notes", "笔记", LifecycleMode::Idle, &[("memo.search", "搜索订单备忘")]);
    let n = notes.clone();
    eventually("notes 休眠", move || n.state().status == StateStatus::Dormant).await;
    eventually("休眠快照", || has_tool(&hub, "notes.memo.search", Availability::Dormant)).await;
    let shop = native(&hub, "shop", "商城", LifecycleMode::Persistent, &[("orders.list", "列出订单"), ("orders.cancel", "取消订单")]);
    eventually("shop 注册工具", || has_tool(&hub, "shop.orders.list", Availability::Available)).await;

    let v = search(&hub, None, json!({"query": "订单"})).await.unwrap();
    // 当前界面 +1；其余只有 description 命中；同分按全名升序；隐藏的 vault 不出现
    assert_eq!(
        names(&v),
        ["shop.orders.cancel", "shop.orders.list", "notes.memo.search", "shop.cart.checkout", "shop.orders.export"],
        "{v}"
    );
    assert_eq!(v["total"], 5);
    assert_eq!((hit(&v, "shop.orders.list")["score"].as_f64(), hit(&v, "shop.orders.export")["score"].as_f64()), (Some(2.0), Some(1.0)));
    assert_eq!(hit(&v, "shop.orders.list")["availability"], "available");
    assert_eq!(hit(&v, "notes.memo.search")["availability"], "dormant");
    let export = hit(&v, "shop.orders.export");
    assert_eq!((export["availability"].as_str(), export["title"].as_str(), export["appId"].as_str()), (Some("notRegistered"), Some("Export orders"), Some("shop")));
    assert!(export["inputSchema"].is_object());
    let checkout = hit(&v, "shop.cart.checkout");
    assert_eq!((checkout["page"].as_str(), checkout["availability"].as_str()), (Some("cart"), Some("notRegistered")), "页面目录工具");
    assert!(v["message"].as_str().is_some_and(|m| m.contains('5')), "{v}");

    // 页面标题 / 描述与 App 名称算上下文（1 分）；全名 3 分、title 2 分，同一词取最高
    let v = search(&hub, None, json!({"query": "购物车"})).await.unwrap();
    assert_eq!((names(&v), hit(&v, "shop.cart.checkout")["score"].as_f64()), (vec!["shop.cart.checkout".to_owned()], Some(2.0)));
    let v = search(&hub, None, json!({"query": "Export orders"})).await.unwrap();
    assert_eq!(names(&v)[0], "shop.orders.export");
    assert_eq!(hit(&v, "shop.orders.export")["score"].as_f64(), Some(6.0));
    assert_eq!(hit(&v, "shop.orders.list")["score"].as_f64(), Some(4.0), "orders 3 + 当前界面 1");

    // hide：工具级隐藏的不出现，整体隐藏的 App 按不存在
    let v = search(&hub, None, json!({"query": "邮件"})).await.unwrap();
    assert_eq!(names(&v), ["mail.send"], "只命中 App 名称；admin.purge 被隐藏：{v}");
    assert_eq!(hit(&v, "mail.send")["availability"], "disconnected");
    assert_eq!(search(&hub, None, json!({"query": "订单", "appId": "vault"})).await.unwrap_err().kind, ErrorKind::ToolNotFound);
    assert_eq!(search(&hub, None, json!({"query": "订单", "appId": "nope"})).await.unwrap_err().kind, ErrorKind::ToolNotFound);

    // appId 限定、limit 与 total
    let v = search(&hub, None, json!({"query": "订单", "appId": "notes"})).await.unwrap();
    assert_eq!((names(&v), v["total"].as_u64()), (vec!["notes.memo.search".to_owned()], Some(1)));
    let v = search(&hub, None, json!({"query": "订单", "limit": 2})).await.unwrap();
    assert_eq!((names(&v), v["total"].as_u64()), (vec!["shop.orders.cancel".to_owned(), "shop.orders.list".to_owned()], Some(5)));

    // 不含内置工具；无结果时给出提示
    let v = search(&hub, None, json!({"query": "apps select overview"})).await.unwrap();
    assert_eq!((v["results"].as_array().map(Vec::len), v["total"].as_u64()), (Some(0), Some(0)), "{v}");
    assert!(v["message"].as_str().is_some_and(|m| m.contains("apps.list")), "{v}");

    // 输入校验
    for args in [json!({"query": ""}), json!({"query": "   "}), json!({"query": "x".repeat(201)}), json!({"query": "x", "limit": 0}), json!({"query": "x", "limit": 51}), json!({})] {
        assert_eq!(search(&hub, None, args.clone()).await.unwrap_err().kind, ErrorKind::InvalidInput, "{args}");
    }

    assert_eq!(*waker.0.lock().unwrap(), 0, "检索不唤醒");
    assert_eq!(notes.state().status, StateStatus::Dormant);
    shop.stop();
    notes.stop();
    hub.shutdown().await;
}

/// 渐进暴露生效时，命中的 App 随后出现在调用方的工具列表中；其他调用方不受影响。
#[tokio::test(flavor = "multi_thread")]
async fn hits_are_exposed_progressively() {
    let hub = Hub::start(HubConfig { tool_exposure: ToolExposure::Progressive, ..config() }).await.unwrap();
    let shop = native(&hub, "shop", "商城", LifecycleMode::Persistent, &[("orders.list", "列出订单")]);
    eventually("shop 注册工具", || {
        hub.tools(&ToolFilter { apps: Some(vec!["shop".into()]), ..Default::default() }).iter().any(|t| t.name == "shop.orders.list")
    })
    .await;
    let listed = |session: &str| -> Vec<String> {
        hub.tools(&ToolFilter { session: Some(session.into()), include_builtin: false, ..Default::default() })
            .into_iter()
            .map(|t| t.name)
            .collect()
    };
    assert!(listed("s1").is_empty());

    let v = search(&hub, Some("s1"), json!({"query": "列出订单"})).await.unwrap();
    assert_eq!(names(&v)[0], "shop.orders.list");
    assert!(v["message"].as_str().is_some_and(|m| m.contains("已加入本会话的工具列表")), "{v}");
    assert!(listed("s1").contains(&"shop.orders.list".to_owned()), "{:?}", listed("s1"));
    assert!(!listed("s1").iter().any(|n| n.starts_with("mail.")), "未命中的 App 不暴露");
    assert!(listed("s2").is_empty(), "其他调用方不受影响");
    shop.stop();
    hub.shutdown().await;
}

/// 使用统计改变排序：24 小时内用过 +1，成功率高 +0.5、低 −0.5（调用 ≥ 3 次）。
#[tokio::test(flavor = "multi_thread")]
async fn usage_changes_ranking() {
    let hub = Hub::start(config()).await.unwrap();
    let kit = native(&hub, "kit", "工具箱", LifecycleMode::Persistent, &[("note.a", "整理笔记"), ("note.b", "整理笔记"), ("note.fail", "整理笔记")]);
    eventually("kit 注册工具", || has_tool(&hub, "kit.note.fail", Availability::Available)).await;
    let scores = |v: &Value| -> Vec<(String, f64)> {
        v["results"].as_array().unwrap().iter().map(|r| (r["name"].as_str().unwrap().to_owned(), r["score"].as_f64().unwrap())).collect()
    };
    let v = search(&hub, None, json!({"query": "整理"})).await.unwrap();
    assert_eq!(scores(&v), [("kit.note.a".into(), 2.0), ("kit.note.b".into(), 2.0), ("kit.note.fail".into(), 2.0)]);

    let call = |name: &'static str| {
        let hub = &hub;
        async move { tokio::time::timeout(T, hub.call_tool(CallRequest::new(name, json!({})))).await.expect("调用超时").unwrap().result }
    };
    call("kit.note.b").await.unwrap();
    let v = search(&hub, None, json!({"query": "整理"})).await.unwrap();
    assert_eq!(names(&v)[0], "kit.note.b", "用过一次即 +1：{v}");
    assert_eq!(hit(&v, "kit.note.b")["score"].as_f64(), Some(3.0));

    call("kit.note.b").await.unwrap();
    call("kit.note.b").await.unwrap();
    for _ in 0..3 {
        assert_eq!(call("kit.note.fail").await.unwrap_err().kind, ErrorKind::HandlerError);
    }
    let v = search(&hub, None, json!({"query": "整理"})).await.unwrap();
    assert_eq!(scores(&v), [("kit.note.b".into(), 3.5), ("kit.note.fail".into(), 2.5), ("kit.note.a".into(), 2.0)], "{v}");
    kit.stop();
    hub.shutdown().await;
}
