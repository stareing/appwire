//! 工具演进（第 16 项 O4，spec/hub-api.md 3.21）端到端：真实 WebSocket App 连接（[`FakeApp`]，声明原样发送）。
//!
//! 覆盖：`schemaHash` 出现在 `HubTool` / `apps.tools` / `apps.search`（内置工具不带）并随 schema 变化；弃用呈现
//! （`HubTool.deprecated`、`apps.tools` / `apps.search` 条目、搜索降权）；参数不符时 `INVALID_INPUT` 的提示与 `schemaHash`
//! （已连接与休眠两条路径）；运行时告警（`tools/sync` 回连以破坏性变化重新同步、`tools/changed` 可能破坏的变化、
//! 声明不变不记录，均照常发工具列表变化）；MCP 出口（`mcp-server`）：`tools/list` 的描述前缀与 `_meta`、调用结果的
//! `dev.appwire/deprecated`。

// @why 参数校验用例只在 `schema-validation` 下编译，其辅助项在其他 feature 组合下未被使用。
#![cfg_attr(not(feature = "schema-validation"), allow(unused_imports, dead_code))]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{
    Availability, CallOutcome, CallRequest, ChangeLevel, ErrorKind, Hub, HubConfig, HubError, HubEvent, HubTool, ToolFilter,
    WakeRequest, Waker, async_trait,
};
use serde_json::{Value, json};

use crate::support::fake_app::FakeApp;

const T: Duration = Duration::from_secs(10);
const APP: &str = "chat";
const INSTANCE: &str = "chat-1";

fn send_schema() -> Value {
    json!({"type": "object", "properties": {"to": {"type": "string"}}, "required": ["to"]})
}

/// `send`（已弃用，改用 `send2`）、`send2`（同样匹配「send message」）、`peek`（带 outputSchema）。
fn tools() -> Value {
    json!([
        {"name": "send", "description": "send message", "inputSchema": send_schema(), "risk": "write",
         "deprecated": {"message": "旧版发送", "replacement": "send2"}},
        {"name": "send2", "description": "send message", "inputSchema": send_schema(), "risk": "write"},
        {"name": "peek", "description": "peek", "inputSchema": {"type": "object"}, "risk": "read",
         "outputSchema": {"type": "object", "properties": {"n": {"type": "integer"}}}},
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

fn hub_tool(hub: &Hub, name: &str) -> Option<HubTool> {
    hub.tools(&ToolFilter::default()).into_iter().find(|t| t.name == name)
}

fn availability(hub: &Hub, name: &str) -> Option<Availability> {
    hub_tool(hub, name).map(|t| t.availability)
}

async fn start_with(config: HubConfig, tools: Value) -> (Hub, FakeApp) {
    let hub = Hub::start(config).await.expect("hub");
    let app = FakeApp::connect(&hub, APP, INSTANCE, tools, json!([])).await;
    eventually("工具可用", || availability(&hub, "chat.peek") == Some(Availability::Available)).await;
    (hub, app)
}

async fn call(hub: &Hub, name: &str, args: Value) -> CallOutcome {
    tokio::time::timeout(T, hub.call_tool(CallRequest::new(name, args))).await.expect("调用超时").expect("调用")
}

/// 内置工具的结构化结果。
async fn builtin(hub: &Hub, name: &str, args: Value) -> Value {
    call(hub, name, args).await.result.expect("内置工具成功")
}

fn entry<'a>(list: &'a Value, name: &str) -> &'a Value {
    list.as_array().and_then(|a| a.iter().find(|t| t["name"] == name)).unwrap_or_else(|| panic!("没有 {name}：{list}"))
}

/// `schemaHash`：`HubTool`、`apps.tools`、`apps.search` 的 App 工具条目都带，与协议函数一致；内置工具不带；
/// schema 变化（`tools/changed`）后随之变化，描述变化不影响。
#[tokio::test(flavor = "multi_thread")]
async fn schema_hash_everywhere_and_tracks_schema() {
    let (hub, app) = start_with(config(), tools()).await;
    let send = hub_tool(&hub, "chat.send").expect("chat.send");
    let expected = app_mcp_protocol::schema_hash(&send_schema(), None);
    assert_eq!(send.schema_hash.as_deref(), Some(expected.as_str()));
    let peek = hub_tool(&hub, "chat.peek").expect("chat.peek");
    assert_ne!(peek.schema_hash, send.schema_hash, "outputSchema 参与哈希");
    assert!(hub_tool(&hub, "apps.list").expect("内置").schema_hash.is_none(), "内置工具不带");

    let listed = builtin(&hub, "apps.tools", json!({"appId": APP})).await;
    assert_eq!(entry(&listed["tools"], "chat.send")["schemaHash"], json!(expected), "{listed}");
    let found = builtin(&hub, "apps.search", json!({"query": "send message"})).await;
    assert_eq!(entry(&found["results"], "chat.send")["schemaHash"], json!(expected), "{found}");

    let described = json!({"name": "send2", "description": "新描述", "inputSchema": send_schema(), "risk": "write"});
    app.notify("tools/changed", json!({"upserted": [described], "removed": []}));
    eventually("描述更新", || hub_tool(&hub, "chat.send2").is_some_and(|t| t.description == "新描述")).await;
    assert_eq!(hub_tool(&hub, "chat.send2").unwrap().schema_hash.as_deref(), Some(expected.as_str()), "描述变化不影响");

    let widened = json!({"type": "object", "properties": {"to": {"type": "string"}, "cc": {"type": "string"}}, "required": ["to"]});
    let changed = json!({"name": "send2", "description": "新描述", "inputSchema": widened, "risk": "write"});
    app.notify("tools/changed", json!({"upserted": [changed], "removed": []}));
    let new_hash = app_mcp_protocol::schema_hash(&widened, None);
    eventually("schemaHash 随 schema 变化", || {
        hub_tool(&hub, "chat.send2").and_then(|t| t.schema_hash).as_deref() == Some(new_hash.as_str())
    })
    .await;
    assert_ne!(new_hash, expected);
    hub.shutdown().await;
}

/// 弃用呈现：`HubTool.deprecated`、`apps.tools` / `apps.search` 条目带原声明；搜索对弃用工具得分 −1（仍列出，排在同分的
/// 未弃用工具之后）；弃用工具照常可调用。
#[tokio::test(flavor = "multi_thread")]
async fn deprecation_presented_and_search_demoted() {
    let (hub, app) = start_with(config(), tools()).await;
    let declared = json!({"message": "旧版发送", "replacement": "send2"});
    let send = hub_tool(&hub, "chat.send").unwrap();
    assert_eq!(serde_json::to_value(&send.deprecated).unwrap(), declared);
    assert!(hub_tool(&hub, "chat.send2").unwrap().deprecated.is_none());

    let listed = builtin(&hub, "apps.tools", json!({"appId": APP})).await;
    assert_eq!(entry(&listed["tools"], "chat.send")["deprecated"], declared);
    assert!(entry(&listed["tools"], "chat.send2").get("deprecated").is_none(), "未弃用不带");

    let found = builtin(&hub, "apps.search", json!({"query": "send message"})).await;
    let results = found["results"].as_array().expect("results");
    let names: Vec<&str> = results.iter().filter_map(|r| r["name"].as_str()).collect();
    assert_eq!(&names[..2], ["chat.send2", "chat.send"], "弃用工具排在后面但仍列出：{found}");
    let score = |n: &str| entry(&found["results"], n)["score"].as_f64().unwrap();
    assert_eq!(score("chat.send2") - score("chat.send"), 1.0, "得分 −1");
    assert_eq!(entry(&found["results"], "chat.send")["deprecated"], declared);

    let o = call(&hub, "chat.send", json!({"to": "a"})).await;
    assert!(o.result.is_ok(), "弃用工具照常可调用：{o:?}");
    assert_eq!(app.invokes("send"), 1);
    hub.shutdown().await;
}

/// 计数的假 Waker（唤醒请求不回连）。
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

fn assert_refetch_hint(o: &CallOutcome, expected_hash: &str) {
    let e = o.result.as_ref().expect_err("参数不符应失败");
    assert_eq!(e.kind, ErrorKind::InvalidInput, "{e:?}");
    assert!(e.message.contains("工具定义可能已变化，请重新获取（apps.tools / tools/list）后再调用"), "{}", e.message);
    assert_eq!(e.details.as_ref().and_then(|d| d.get("schemaHash")), Some(&json!(expected_hash)), "{e:?}");
}

/// 参数不符 `inputSchema`：`INVALID_INPUT` 的消息追加重新获取的提示、`data` 带校验所用定义的 `schemaHash`；
/// 已连接（路由到实例）与休眠（按快照校验、不唤醒）两条路径相同。
#[cfg(feature = "schema-validation")]
#[tokio::test(flavor = "multi_thread")]
async fn invalid_input_carries_hint_and_schema_hash() {
    let (hub, app) = start_with(config(), tools()).await;
    let hash = app_mcp_protocol::schema_hash(&send_schema(), None);
    assert_refetch_hint(&call(&hub, "chat.send2", json!({"cc": "x"})).await, &hash);
    assert_eq!(app.invokes("send2"), 0, "未转发");

    let waker = Arc::new(CountingWaker::default());
    hub.set_waker(waker.clone());
    app.sleep_and_close().await;
    eventually("进入休眠", || availability(&hub, "chat.send2") == Some(Availability::Dormant)).await;
    assert_refetch_hint(&call(&hub, "chat.send2", json!({})).await, &hash);
    assert!(waker.requests.lock().unwrap().is_empty(), "参数不符：不唤醒");
    hub.shutdown().await;
}

/// 等到 Hub 发出工具列表变化事件。
async fn tools_changed(rx: &mut tokio::sync::broadcast::Receiver<HubEvent>) {
    tokio::time::timeout(T, async {
        loop {
            match rx.recv().await {
                Ok(HubEvent::ToolsChanged) => return,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(e) => panic!("事件通道关闭：{e}"),
            }
        }
    })
    .await
    .expect("没有等到 ToolsChanged");
}

/// 运行时告警：App 休眠后以破坏性变化重新同步（`send2` 新增必填参数）→ `schema_changes` 一条 breaking，照常发列表变化；
/// 声明不变的回连不记录；`tools/changed` 去掉 `outputSchema` → 一条 warning。
#[tokio::test(flavor = "multi_thread")]
async fn breaking_resync_and_changed_are_recorded() {
    let (hub, app) = start_with(config(), tools()).await;
    assert_eq!(hub.status().schema_changes, Some(Vec::new()), "启动时为空");

    // 声明不变的回连：不记录。
    app.sleep_and_close().await;
    eventually("进入休眠", || availability(&hub, "chat.send2") == Some(Availability::Dormant)).await;
    let app = FakeApp::connect(&hub, APP, INSTANCE, tools(), json!([])).await;
    eventually("回连可用", || availability(&hub, "chat.send2") == Some(Availability::Available)).await;
    assert_eq!(hub.status().schema_changes, Some(Vec::new()), "声明不变不记录");

    // 破坏性变化：新增必填参数。
    app.sleep_and_close().await;
    eventually("再次休眠", || availability(&hub, "chat.send2") == Some(Availability::Dormant)).await;
    let mut events = hub.events();
    let mut breaking = tools();
    breaking[1]["inputSchema"] = json!({"type": "object", "properties": {"to": {"type": "string"}, "subject": {"type": "string"}},
        "required": ["to", "subject"]});
    let app = FakeApp::connect(&hub, APP, INSTANCE, breaking, json!([])).await;
    tools_changed(&mut events).await;
    let records = hub.status().schema_changes.expect("schema_changes");
    assert_eq!(records.len(), 1, "{records:?}");
    let r = &records[0];
    assert_eq!((r.app_id.as_str(), r.tool.as_str(), r.level), (APP, "send2", ChangeLevel::Breaking), "{r:?}");
    assert!(r.changes.iter().any(|c| c.level == ChangeLevel::Breaking && c.path.contains("subject")), "{r:?}");
    assert!(r.at > 0);

    // tools/changed：去掉 outputSchema（可能破坏）。
    let mut events = hub.events();
    let plain_peek = json!({"name": "peek", "description": "peek", "inputSchema": {"type": "object"}, "risk": "read"});
    app.notify("tools/changed", json!({"upserted": [plain_peek], "removed": []}));
    tools_changed(&mut events).await;
    let records = hub.status().schema_changes.expect("schema_changes");
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!((records[1].tool.as_str(), records[1].level), ("peek", ChangeLevel::Warning), "新的在后：{records:?}");
    hub.shutdown().await;
}

/// MCP 出口：`tools/list` 的描述前缀（`[已弃用] …（改用 chat.send2）`）与 `_meta`（`schemaHash`、`deprecated`）；
/// 调用弃用工具的结果 `_meta` 带 `dev.appwire/deprecated`，未弃用的不带。
#[cfg(feature = "mcp-server")]
mod mcp {
    use app_mcp_hub::names::{META_DEPRECATED, META_SCHEMA_HASH};

    use super::*;
    use crate::support::mcp_http::{modern_call, modern_request};

    #[tokio::test(flavor = "multi_thread")]
    async fn tools_list_and_call_meta() {
        let mut c = config();
        c.mcp_http = true;
        let (hub, _app) = start_with(c, tools()).await;
        let addr = hub.listen_addr().expect("listen");
        let r = modern_request(addr, None, "tools/list", "", json!({})).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let list = r.json()["result"]["tools"].clone();
        let send = entry(&list, "chat.send");
        assert_eq!(send["description"], "[已弃用] 旧版发送（改用 chat.send2） send message");
        assert_eq!(send["_meta"][META_DEPRECATED], json!({"message": "旧版发送", "replacement": "send2"}));
        assert_eq!(send["_meta"][META_SCHEMA_HASH], json!(app_mcp_protocol::schema_hash(&send_schema(), None)));
        let send2 = entry(&list, "chat.send2");
        assert_eq!(send2["description"], "send message");
        assert!(send2["_meta"].get(META_DEPRECATED).is_none(), "{send2}");
        assert!(send2["_meta"][META_SCHEMA_HASH].is_string());
        assert!(entry(&list, "apps.list").get("_meta").is_none_or(|m| m.get(META_SCHEMA_HASH).is_none()), "内置工具不带");

        let called = modern_call(addr, None, "chat.send", json!({"to": "a"})).await.json()["result"].clone();
        assert_eq!(called["_meta"][META_DEPRECATED]["replacement"], "send2", "{called}");
        let plain = modern_call(addr, None, "chat.send2", json!({"to": "a"})).await.json()["result"].clone();
        assert!(plain["_meta"].get(META_DEPRECATED).is_none(), "{plain}");
        hub.shutdown().await;
    }
}
