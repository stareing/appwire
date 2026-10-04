//! 标准意图 `apps.intents` 与机主默认表（第 16 项 N4，spec/intents.md 第 4 节）。
//!
//! App 端是真实的 `app-mcp-native` 客户端（已连接的 App 与休眠的 App）加只有清单的 App 与页面目录；覆盖不唤醒、默认排序与
//! `default` 标记、不兼容列出原因、未知动词 `known: false`、`intent` 带 / 不带版本、`hide` 过滤、暴露集合、默认表替换，
//! 以及 `apps.tools` / `apps.search` 带出 `implements`。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{
    Availability, CallRequest, ErrorKind, Hub, HubConfig, HubError, PolicyConfig, ToolError, ToolExposure, ToolFilter, WakeRequest,
    Waker, async_trait,
};
use app_mcp_native::{
    CallHandle, LifecycleMode, NativeClient, NativeConfig, StateStatus, ToolHandler, ToolOptions, ToolSpec,
    WakeDescriptor as NativeWake, WakeKind as NativeWakeKind,
};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);

fn send_schema() -> Value {
    json!({"type": "object", "properties": {"to": {"type": "array", "items": {"type": "string"}}, "text": {"type": "string"}}})
}

fn manifests() -> Vec<app_mcp_manifest::Manifest> {
    let parse = |v: Value| app_mcp_manifest::parse(&v.to_string()).expect("manifest");
    vec![
        parse(json!({
            "manifestVersion": 1, "appId": "mail", "name": "邮件",
            "tools": [
                {"name": "compose.send", "description": "写信并发送", "inputSchema": send_schema(), "implements": ["message.send@1"]},
                {"name": "admin.blast", "description": "群发", "inputSchema": send_schema(), "implements": ["message.send@1"]}
            ],
            "pages": [
                {"name": "files", "title": "附件", "tools": [
                    {"name": "files.share", "description": "分享附件", "implements": ["file.share@1"],
                     "inputSchema": {"type": "object", "properties": {"files": {"type": "array"}}}},
                    {"name": "files.secret", "description": "外发附件", "implements": ["file.share@1"],
                     "inputSchema": {"type": "object", "properties": {"files": {"type": "array"}}}}
                ]}
            ]
        })),
        parse(json!({
            "manifestVersion": 1, "appId": "vault", "name": "保险箱",
            "tools": [{"name": "leak", "description": "外发", "inputSchema": send_schema(), "implements": ["message.send@1"]}]
        })),
    ]
}

fn config() -> HubConfig {
    let policy = PolicyConfig::from_json(
        &json!({"rules": [
            {"id": "hide-vault", "action": "hide", "app": "vault"},
            {"id": "hide-blast", "action": "hide", "app": "mail", "tool": "admin.*"},
            {"id": "hide-secret", "action": "hide", "app": "mail", "tool": "files.secret"},
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
        intent_defaults: [("message.send".to_owned(), "chat.post".to_owned())].into(),
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

struct Handler;
impl ToolHandler for Handler {
    fn invoke(&self, call: CallHandle) {
        std::thread::spawn(move || {
            let _ = call.complete(Some("{}"), vec![]);
        });
    }
}

/// (工具名, implements, inputSchema)
type ToolDecl<'a> = (&'a str, &'a [&'a str], Value);

fn native(hub: &Hub, app_id: &str, mode: LifecycleMode, tools: &[ToolDecl<'_>]) -> NativeClient {
    let mut c = NativeConfig::new(app_id, app_id);
    c.host_url = format!("ws://{}/app", hub.listen_addr().unwrap());
    c.instance_id = Some(format!("{app_id}-1"));
    c.lifecycle.mode = mode;
    if mode == LifecycleMode::Idle {
        c.launch_token = Some(String::new());
        c.lifecycle.idle_timeout_ms = 150;
        c.lifecycle.wake = Some(NativeWake { kind: NativeWakeKind::Uri, target: Some(format!("{app_id}-app")), background: true });
    }
    let client = NativeClient::new(c, None).unwrap();
    for (tool, implements, schema) in tools {
        let mut spec = ToolSpec::new(*tool, "d");
        spec.input_schema_json = Some(schema.to_string());
        let options = ToolOptions { implements: implements.iter().map(|s| (*s).to_owned()).collect(), ..ToolOptions::default() };
        client.register_tool_with(spec, options, Arc::new(Handler)).unwrap();
    }
    client.start();
    client
}

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

async fn call(hub: &Hub, session: Option<&str>, name: &str, args: Value) -> Result<Value, ToolError> {
    let req = CallRequest { session: session.map(str::to_owned), ..CallRequest::new(name, args) };
    tokio::time::timeout(T, hub.call_tool(req)).await.expect("调用超时").unwrap().result
}

async fn intents(hub: &Hub, args: Value) -> Result<Value, ToolError> {
    call(hub, None, "apps.intents", args).await
}

fn entry<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["intents"].as_array().unwrap().iter().find(|e| e["intent"] == id).unwrap_or_else(|| panic!("缺少 {id}：{v}"))
}

fn ids(v: &Value) -> Vec<&str> {
    v["intents"].as_array().unwrap().iter().map(|e| e["intent"].as_str().unwrap()).collect()
}

/// (工具全名, default)
fn implementers(e: &Value) -> Vec<(&str, bool)> {
    e["implementations"].as_array().unwrap().iter().map(|i| (i["tool"].as_str().unwrap(), i["default"] == true)).collect()
}

/// 全部来源、不唤醒、默认排序、不兼容、未知动词、版本过滤、hide、默认表替换。
#[tokio::test(flavor = "multi_thread")]
async fn lists_implementers_from_every_source_without_waking() {
    let hub = Hub::start(config()).await.unwrap();
    let waker = Arc::new(CountingWaker::default());
    hub.set_waker(waker.clone());
    let link_schema = json!({"type": "object", "properties": {"url": {"type": "string"}}});
    let notes = native(&hub, "notes", LifecycleMode::Idle, &[("share", &["file.share@1", "link.open@1"], json!({"type": "object",
        "properties": {"files": {"type": "array", "items": {"type": "string"}}, "url": {"type": "string"}}}))]);
    let n = notes.clone();
    eventually("notes 休眠", move || n.state().status == StateStatus::Dormant).await;
    eventually("休眠快照", || has_tool(&hub, "notes.share", Availability::Dormant)).await;
    let chat = native(&hub, "chat", LifecycleMode::Persistent, &[
        ("post", &["message.send@1"], send_schema()),
        ("bad", &["message.send@1"], json!({"type": "object", "properties": {"to": {"type": "string"}}})),
        ("ping", &["chat.ping@1"], json!({"type": "object"})),
        ("open", &["link.open@1"], link_schema),
    ]);
    eventually("chat 注册工具", || has_tool(&hub, "chat.open", Availability::Available)).await;

    // 全部：词表 6 个 + 未知的 chat.ping@1，按（动词, 版本）排序
    let v = intents(&hub, json!({})).await.unwrap();
    assert_eq!(
        ids(&v),
        ["calendar.create@1", "chat.ping@1", "file.share@1", "link.open@1", "media.play@1", "message.send@1", "navigation.start@1"],
        "{v}"
    );
    // 机主默认在前；隐藏的 vault 与 mail.admin.blast 不出现；清单（未连接）照常列出
    let send = entry(&v, "message.send@1");
    assert_eq!(implementers(send), [("chat.post", true), ("mail.compose.send", false)], "{send}");
    assert_eq!(send["implementations"][0]["availability"], "available");
    assert_eq!(send["implementations"][1]["availability"], "disconnected");
    assert_eq!(send["implementations"][1]["appId"], "mail");
    assert!(send["description"].as_str().is_some_and(|d| !d.is_empty()) && send["known"] == true);
    // 不兼容：不列为实现者，列出原因
    assert_eq!(send["incompatible"].as_array().unwrap().len(), 1, "{send}");
    assert_eq!(send["incompatible"][0]["tool"], "chat.bad");
    assert!(send["incompatible"][0]["reason"].as_str().unwrap().contains("缺少必填参数 text"));
    // 休眠快照与页面目录（页面目录中被 hide 的 mail.files.secret 不出现）
    assert_eq!(implementers(entry(&v, "file.share@1")), [("mail.files.share", false), ("notes.share", false)]);
    assert_eq!(entry(&v, "file.share@1")["implementations"][1]["availability"], "dormant");
    assert_eq!(implementers(entry(&v, "link.open@1")), [("chat.open", false), ("notes.share", false)]);
    // 未知动词：known false，照常列出
    let ping = entry(&v, "chat.ping@1");
    assert_eq!((ping["known"].as_bool(), ping.get("description")), (Some(false), None));
    assert_eq!(implementers(ping), [("chat.ping", false)]);
    assert!(entry(&v, "media.play@1")["implementations"].as_array().unwrap().is_empty());
    assert!(v["message"].as_str().is_some_and(|m| m.contains("按工具全名")), "{v}");

    // intent 过滤：不带版本 = 任意版本；带版本只那一个；未知版本 known false
    let v = intents(&hub, json!({"intent": "message.send"})).await.unwrap();
    assert_eq!(ids(&v), ["message.send@1"]);
    let v = intents(&hub, json!({"intent": "link.open@1"})).await.unwrap();
    assert_eq!(ids(&v), ["link.open@1"]);
    let v = intents(&hub, json!({"intent": "message.send@2"})).await.unwrap();
    assert_eq!((ids(&v), entry(&v, "message.send@2")["known"].as_bool()), (vec!["message.send@2"], Some(false)));
    assert!(v["message"].as_str().unwrap().contains("没有 App 声明实现"), "{v}");
    let v = intents(&hub, json!({"intent": "nobody.does"})).await.unwrap();
    assert!(ids(&v).is_empty());
    for bad in [json!({"intent": "message"}), json!({"intent": "message.send@0"}), json!({"intent": 3}), json!({"x": 1})] {
        assert_eq!(intents(&hub, bad.clone()).await.unwrap_err().kind, ErrorKind::InvalidInput, "{bad}");
    }

    // 默认表替换：带版本的键优先；不合法时保留之前的表
    hub.set_intent_defaults(BTreeMap::from([("message.send@1".to_owned(), "mail.compose.send".to_owned())])).unwrap();
    let v = intents(&hub, json!({"intent": "message.send@1"})).await.unwrap();
    assert_eq!(implementers(entry(&v, "message.send@1")), [("mail.compose.send", true), ("chat.post", false)]);
    let err = hub.set_intent_defaults(BTreeMap::from([("message.send".to_owned(), "bad".to_owned())])).unwrap_err();
    assert_eq!(err.0.kind, ErrorKind::InvalidInput);
    let st = hub.intents();
    assert_eq!(st.defaults, BTreeMap::from([("message.send@1".to_owned(), "mail.compose.send".to_owned())]));
    assert!(st.last_error.is_some());
    assert_eq!(hub.status().intents.unwrap().defaults.len(), 1);

    // apps.tools 与 apps.search 带出 implements
    let v = call(&hub, None, "apps.tools", json!({"appId": "chat"})).await.unwrap();
    let post = v["tools"].as_array().unwrap().iter().find(|t| t["name"] == "chat.post").unwrap();
    assert_eq!(post["implements"], json!(["message.send@1"]));
    let compose = hub.tools(&ToolFilter { include_builtin: false, ..Default::default() }).into_iter().find(|t| t.name == "mail.compose.send");
    assert_eq!(compose.unwrap().implements, ["message.send@1"], "HubTool.implements（清单工具）");
    let v = call(&hub, None, "apps.search", json!({"query": "写信"})).await.unwrap();
    assert_eq!(v["results"][0]["implements"], json!(["message.send@1"]), "{v}");

    assert_eq!(*waker.0.lock().unwrap(), 0, "列出实现者不唤醒");
    assert_eq!(notes.state().status, StateStatus::Dormant);
    chat.stop();
    notes.stop();
    hub.shutdown().await;
}

/// 渐进暴露生效时，列为实现者的 App 随后出现在调用方的工具列表中；不兼容 / 未列出的 App 与其他调用方不受影响。
#[tokio::test(flavor = "multi_thread")]
async fn implementers_are_exposed_progressively() {
    let hub = Hub::start(HubConfig { tool_exposure: ToolExposure::Progressive, ..config() }).await.unwrap();
    let chat = native(&hub, "chat", LifecycleMode::Persistent, &[("post", &["message.send@1"], send_schema())]);
    let other = native(&hub, "other", LifecycleMode::Persistent, &[("bad", &["message.send@1"], json!({"type": "object"}))]);
    eventually("工具注册", || {
        let all = hub.tools(&ToolFilter { apps: Some(vec!["chat".into(), "other".into()]), ..Default::default() });
        all.iter().any(|t| t.name == "chat.post") && all.iter().any(|t| t.name == "other.bad")
    })
    .await;
    let listed = |session: &str| -> Vec<String> {
        hub.tools(&ToolFilter { session: Some(session.into()), include_builtin: false, ..Default::default() })
            .into_iter()
            .map(|t| t.name)
            .collect()
    };
    assert!(listed("s1").is_empty());
    let v = call(&hub, Some("s1"), "apps.intents", json!({"intent": "message.send"})).await.unwrap();
    assert_eq!(implementers(entry(&v, "message.send@1")), [("chat.post", true), ("mail.compose.send", false)]);
    let s1 = listed("s1");
    assert!(s1.contains(&"chat.post".to_owned()) && s1.contains(&"mail.compose.send".to_owned()), "{s1:?}");
    assert!(!s1.iter().any(|n| n.starts_with("other.")), "只不兼容的 App 不暴露：{s1:?}");
    assert!(listed("s2").is_empty(), "其他调用方不受影响");
    chat.stop();
    other.stop();
    hub.shutdown().await;
}
