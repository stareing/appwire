//! 撤销（第 15 项 X2，spec/hub-api.md 3.23）端到端：真实 WebSocket App 连接（[`fake_app`]，结果原样给出 `undo`），
//! 以 App 收到的 `tools/invoke` 计数判定逆调用是否转发。
//!
//! 覆盖：登记条件（done / partial 登记；pending / noop / 失败 / 不合法 undo / 缓存命中不登记；关闭时不登记、不列出）；缺省取最近、
//! 指定 callId、只能撤销一次；TTL 惰性过期；条数上限丢最早；其他调用方 → `TOOL_NOT_FOUND`（`data.callId`）；任务回收后记录消失；
//! 逆调用走正常路径（策略 deny、对象锁 LOCKED、结果缓存失效）、发往原实例；逆调用失败时 `data.undo`；逆调用结果带 undo 可"重做"；
//! `apps.tools` / `apps.search` 的 `undoable`。MCP 出口（`mcp-server`）：`_meta` 的 `dev.appwire/undo` / `undoOf` / `undoable`、
//! 任务句柄的记录归句柄任务。

use std::time::Duration;

use app_mcp_hub::{
    Availability, CallOutcome, CallRequest, ErrorKind, Hub, HubConfig, PolicyConfig, ToolFilter, UndoLimits,
};
use serde_json::{Value, json};

use crate::support::fake_app::{FakeApp, tool};

const T: Duration = Duration::from_secs(10);
const APP: &str = "todo";

fn tools() -> Value {
    let mut add = tool("add", "write", Value::Null);
    add["undoable"] = json!(true);
    json!([add, tool("remove", "write", Value::Null), tool("list", "read", json!({"ttlMs": 60_000}))])
}

fn config(undo: UndoLimits) -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: None,
        list_changed_debounce: Duration::from_millis(10),
        undo,
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

async fn connect(hub: &Hub, instance: &str) -> FakeApp {
    let app = FakeApp::connect(hub, APP, instance, tools(), json!([])).await;
    let ready = |hub: &Hub| {
        hub.tools(&ToolFilter::default())
            .into_iter()
            .any(|t| t.name == "todo.add" && t.availability == Availability::Available)
    };
    eventually("工具可用", || ready(hub)).await;
    app
}

async fn start_with(undo: UndoLimits) -> (Hub, FakeApp) {
    let hub = Hub::start(config(undo)).await.expect("hub");
    let app = connect(&hub, "todo-1").await;
    (hub, app)
}

async fn start() -> (Hub, FakeApp) {
    start_with(UndoLimits::default()).await
}

async fn call_req(hub: &Hub, req: CallRequest) -> CallOutcome {
    tokio::time::timeout(T, hub.call_tool(req)).await.expect("调用超时").expect("调用")
}

async fn call(hub: &Hub, name: &str, args: Value) -> CallOutcome {
    call_req(hub, CallRequest::new(name, args)).await
}

/// 下一次 `add` 的结果带撤销信息（逆工具 `remove`，参数 `{id}`）；`extra` 合并进结果。
fn script_add(app: &FakeApp, id: u64, extra: Value) {
    let mut result = json!({"data": {"id": id}, "undo": {"tool": "remove", "arguments": {"id": id}, "label": "删除刚添加的待办"}});
    if let Value::Object(m) = extra {
        for (k, v) in m {
            result[k] = v;
        }
    }
    app.script("add", json!({ "result": result }));
}

/// 撤销：成功时为逆调用的数据，失败时为 (类别, details)。
async fn undo(hub: &Hub, args: Value) -> Result<Value, (ErrorKind, Value)> {
    undo_as(hub, None, args).await
}

async fn undo_as(hub: &Hub, session: Option<&str>, args: Value) -> Result<Value, (ErrorKind, Value)> {
    let mut req = CallRequest::new("apps.undo", args);
    req.session = session.map(str::to_owned);
    call_req(hub, req).await.result.map_err(|e| (e.kind, e.details.unwrap_or_default()))
}

fn not_found(r: &Result<Value, (ErrorKind, Value)>) -> bool {
    matches!(r, Err((ErrorKind::ToolNotFound, _)))
}

/// 登记条件：done / partial 登记；pending / noop、App 失败、不合法的 undo 不登记（结果照常）。
#[tokio::test(flavor = "multi_thread")]
async fn registers_only_completed_results_with_valid_undo() {
    let (hub, app) = start().await;
    for (status, registered) in [("done", true), ("partial", true), ("pending", false), ("noop", false)] {
        script_add(&app, 1, json!({"status": status}));
        let o = call(&hub, "todo.add", json!({})).await;
        assert_eq!(o.result.as_ref().expect("调用成功"), &json!({"id": 1}), "{status}");
        let before = app.invokes("remove");
        let r = undo(&hub, json!({})).await;
        assert_eq!(r.is_ok(), registered, "{status}: {r:?}");
        assert_eq!(app.invokes("remove"), before + usize::from(registered), "{status}");
    }
    for bad in [json!(42), json!({"tool": "bad name"}), json!({"tool": "remove", "arguments": [1]}), json!({"tool": "remove", "label": " "})] {
        app.script("add", json!({"result": {"data": {"id": 2}, "undo": bad}}));
        let o = call(&hub, "todo.add", json!({})).await;
        assert_eq!(o.result.as_ref().expect("不合法的 undo 不影响结果"), &json!({"id": 2}), "{bad}");
        assert!(not_found(&undo(&hub, json!({})).await), "不合法的 undo 不登记：{bad}");
    }
    app.script("add", json!({"error": {"code": -32006, "message": "失败", "data": {"kind": "HANDLER_ERROR"}}}));
    assert!(call(&hub, "todo.add", json!({})).await.result.is_err());
    assert!(not_found(&undo(&hub, json!({})).await), "失败的调用不登记");
    // 内置工具不登记
    call(&hub, "apps.list", json!({})).await;
    assert!(not_found(&undo(&hub, json!({})).await));
    hub.shutdown().await;
}

/// 缓存命中（spec/hub-api.md 3.20）不经 App、不登记。
#[tokio::test(flavor = "multi_thread")]
async fn cache_hit_does_not_register() {
    let (hub, app) = start().await;
    app.script("list", json!({"result": {"data": [], "undo": {"tool": "remove"}}}));
    call(&hub, "todo.list", json!({})).await;
    let hit = call(&hub, "todo.list", json!({})).await;
    assert!(hit.cached_age_ms.is_some(), "命中缓存：{hit:?}");
    assert!(not_found(&undo(&hub, json!({"callId": hit.call_id})).await), "命中不登记");
    assert!(undo(&hub, json!({})).await.is_ok(), "未命中的那次已登记");
    assert!(not_found(&undo(&hub, json!({})).await), "只有一条");
    hub.shutdown().await;
}

/// 关闭（`max_per_task = 0`）：不列出 `apps.undo`，调用为 `TOOL_NOT_FOUND`，结果带 undo 也不登记。
#[tokio::test(flavor = "multi_thread")]
async fn disabled_hides_tool_and_skips_registration() {
    let (hub, app) = start_with(UndoLimits { max_per_task: 0, ..UndoLimits::default() }).await;
    assert!(!hub.tools(&ToolFilter::default()).iter().any(|t| t.name == "apps.undo"), "关闭时不列出");
    script_add(&app, 1, json!({}));
    call(&hub, "todo.add", json!({})).await;
    let off = call(&hub, "apps.undo", json!({})).await.result.expect_err("关闭时不可调用");
    assert!(off.kind == ErrorKind::ToolNotFound && off.message.contains("未启用撤销"), "{off:?}");
    assert_eq!(app.invokes("remove"), 0);
    hub.shutdown().await;

    let (hub, _app) = start().await;
    assert!(hub.tools(&ToolFilter::default()).iter().any(|t| t.name == "apps.undo"), "默认开启并列出");
    hub.shutdown().await;
}

/// 缺省取最近一条、指定 callId、只能撤销一次；逆调用参数原样转交；结果为逆调用的结果。
#[tokio::test(flavor = "multi_thread")]
async fn latest_by_id_and_only_once() {
    let (hub, app) = start().await;
    let mut ids = Vec::new();
    for id in 1..=3 {
        script_add(&app, id, json!({}));
        ids.push(call(&hub, "todo.add", json!({})).await.call_id);
    }
    app.script("remove", json!({"result": {"data": {"removed": 3}}}));
    assert_eq!(undo(&hub, json!({})).await.expect("最近一条"), json!({"removed": 3}), "逆调用的结果原样返回");
    app.script("remove", json!({"result": {"data": {"removed": 1}}}));
    assert_eq!(undo(&hub, json!({"callId": ids[0]})).await.expect("指定 callId"), json!({"removed": 1}));
    let again = undo(&hub, json!({"callId": ids[0]})).await;
    let Err((ErrorKind::ToolNotFound, details)) = &again else { panic!("只能撤销一次：{again:?}") };
    assert_eq!(details["callId"], json!(ids[0]), "data.callId");
    assert!(undo(&hub, json!({"callId": ids[1]})).await.is_ok());
    let none = undo(&hub, json!({})).await;
    assert!(matches!(&none, Err((ErrorKind::ToolNotFound, d)) if d["callId"].is_null()), "{none:?}");
    assert_eq!(app.invokes("remove"), 3);
    hub.shutdown().await;
}

/// TTL 惰性过期（小 TTL）；条数上限丢最早。
#[tokio::test(flavor = "multi_thread")]
async fn ttl_and_cap() {
    let (hub, app) = start_with(UndoLimits { ttl: Duration::from_millis(200), max_per_task: 2 }).await;
    script_add(&app, 1, json!({}));
    let old = call(&hub, "todo.add", json!({})).await.call_id;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(not_found(&undo(&hub, json!({"callId": old})).await), "过期");
    hub.shutdown().await;

    let (hub, app) = start_with(UndoLimits { max_per_task: 2, ..UndoLimits::default() }).await;
    let mut ids = Vec::new();
    for id in 1..=3 {
        script_add(&app, id, json!({}));
        ids.push(call(&hub, "todo.add", json!({})).await.call_id);
    }
    assert!(not_found(&undo(&hub, json!({"callId": ids[0]})).await), "超过上限：最早的一条被丢弃");
    assert!(undo(&hub, json!({"callId": ids[1]})).await.is_ok());
    assert!(undo(&hub, json!({"callId": ids[2]})).await.is_ok());
    hub.shutdown().await;
}

/// 只查调用方自己任务的记录：其他会话 → `TOOL_NOT_FOUND`（与不存在相同）；任务回收（`reset_session`）后记录消失。
#[tokio::test(flavor = "multi_thread")]
async fn other_callers_and_reclaimed_tasks_see_nothing() {
    let (hub, app) = start().await;
    script_add(&app, 1, json!({}));
    let id = call(&hub, "todo.add", json!({})).await.call_id;
    let other = undo_as(&hub, Some("other"), json!({"callId": id})).await;
    assert!(matches!(&other, Err((ErrorKind::ToolNotFound, d)) if d["callId"] == json!(id)), "{other:?}");
    assert!(not_found(&undo_as(&hub, Some("other"), json!({})).await));
    assert_eq!(app.invokes("remove"), 0);

    script_add(&app, 2, json!({}));
    let mut req = CallRequest::new("todo.add", json!({}));
    req.session = Some("s".into());
    call_req(&hub, req).await;
    hub.reset_session(Some("s"));
    assert!(not_found(&undo_as(&hub, Some("s"), json!({})).await), "任务回收后记录消失");
    assert!(undo(&hub, json!({"callId": id})).await.is_ok(), "自己的记录不受影响");
    hub.shutdown().await;
}

/// 逆调用走正常路径：策略 deny 与他人的对象锁拦住它（记录已取出，`data.undo` 给出逆调用）；App 失败同样带 `data.undo`。
#[tokio::test(flavor = "multi_thread")]
async fn inverse_goes_through_policy_locks_and_reports_failure() {
    let (hub, app) = start().await;
    let rules = |v: Value| PolicyConfig::from_json(&v.to_string()).expect("规则合法");
    let expected_undo = json!({"tool": "todo.remove", "arguments": {"id": 1}});

    script_add(&app, 1, json!({}));
    let id = call(&hub, "todo.add", json!({})).await.call_id;
    hub.set_policy(rules(json!({"rules": [{"id": "d", "action": "deny", "app": APP, "tool": "remove"}]}))).unwrap();
    let denied = undo(&hub, json!({})).await;
    let Err((ErrorKind::PolicyDenied, d)) = &denied else { panic!("策略拦住逆调用：{denied:?}") };
    assert_eq!(d["undo"], expected_undo, "{d}");
    assert!(not_found(&undo(&hub, json!({"callId": id})).await), "记录已取出");
    hub.set_policy(PolicyConfig::default()).unwrap();

    script_add(&app, 1, json!({}));
    call(&hub, "todo.add", json!({})).await;
    let mut lock = CallRequest::new("apps.lock", json!({"appId": APP}));
    lock.session = Some("other".into());
    assert!(call_req(&hub, lock).await.result.is_ok(), "他人加锁");
    let locked = undo(&hub, json!({})).await;
    let Err((ErrorKind::Locked, d)) = &locked else { panic!("对象锁拦住逆调用：{locked:?}") };
    assert_eq!(d["undo"], expected_undo, "{d}");
    let mut unlock = CallRequest::new("apps.unlock", json!({"appId": APP}));
    unlock.session = Some("other".into());
    call_req(&hub, unlock).await;
    assert_eq!(app.invokes("remove"), 0, "被拦住的逆调用没有转发");

    script_add(&app, 1, json!({}));
    call(&hub, "todo.add", json!({})).await;
    app.script("remove", json!({"error": {"code": -32006, "message": "已被改动", "data": {"kind": "HANDLER_ERROR"}}}));
    let failed = undo(&hub, json!({})).await;
    let Err((ErrorKind::HandlerError, d)) = &failed else { panic!("App 失败原样返回：{failed:?}") };
    assert_eq!(d["undo"], expected_undo, "{d}");
    hub.shutdown().await;
}

/// 逆调用是写调用：整 App 的结果缓存失效（spec/hub-api.md 3.20）。
#[tokio::test(flavor = "multi_thread")]
async fn inverse_invalidates_result_cache() {
    let (hub, app) = start().await;
    script_add(&app, 1, json!({}));
    call(&hub, "todo.add", json!({})).await;
    call(&hub, "todo.list", json!({})).await;
    call(&hub, "todo.list", json!({})).await;
    assert_eq!(app.invokes("list"), 1, "命中");
    undo(&hub, json!({})).await.expect("撤销");
    call(&hub, "todo.list", json!({})).await;
    assert_eq!(app.invokes("list"), 2, "撤销后缓存失效");
    hub.shutdown().await;
}

/// 逆调用的结果带合法 undo 时照常登记（"重做"）；原实例仍在线时逆调用发往原实例。
#[tokio::test(flavor = "multi_thread")]
async fn redo_and_original_instance() {
    let (hub, app) = start().await;
    let app2 = connect(&hub, "todo-2").await;
    eventually("两个实例", || hub.status().apps.iter().any(|a| a.app_id == APP && a.instances.len() == 2)).await;
    // 原调用在 todo-2；随后 todo-1 处理了另一次调用（常规路由会优先最近活跃的 todo-1）。
    script_add(&app2, 1, json!({}));
    let mut req = CallRequest::new("todo.add", json!({}));
    req.instance_id = Some("todo-2".into());
    call_req(&hub, req).await;
    let mut other = CallRequest::new("todo.remove", json!({"id": 9}));
    other.instance_id = Some("todo-1".into());
    call_req(&hub, other).await;
    app2.script("remove", json!({"result": {"data": {"removed": 1}, "undo": {"tool": "add", "arguments": {"id": 1}}}}));
    let undone = call(&hub, "apps.undo", json!({})).await;
    assert_eq!(undone.instance_id.as_deref(), Some("todo-2"), "发往原实例");
    assert_eq!((app.invokes("remove"), app2.invokes("remove")), (1, 1));
    let redo = undo(&hub, json!({"callId": undone.call_id})).await;
    assert!(redo.is_ok(), "逆调用结果的 undo 照常登记（callId 为 apps.undo 的 callId）：{redo:?}");
    assert_eq!(app.invokes("add") + app2.invokes("add"), 2, "重做调用 add");
    hub.shutdown().await;
}

/// `apps.tools` / `apps.search` 条目：声明了 `undoable` 的带 `undoable: true`，其余不带。
#[tokio::test(flavor = "multi_thread")]
async fn undoable_in_listings() {
    let (hub, _app) = start().await;
    let tools = call(&hub, "apps.tools", json!({"appId": APP})).await.result.expect("apps.tools");
    let entry = |name: &str| tools["tools"].as_array().unwrap().iter().find(|t| t["name"] == name).cloned().unwrap();
    assert_eq!(entry("todo.add")["undoable"], json!(true), "{tools}");
    assert!(entry("todo.remove").get("undoable").is_none());
    let found = call(&hub, "apps.search", json!({"query": "add", "appId": APP})).await.result.expect("apps.search");
    let hit = found["results"].as_array().unwrap().iter().find(|t| t["name"] == "todo.add").cloned().expect("命中 add");
    assert_eq!(hit["undoable"], json!(true), "{found}");
    hub.shutdown().await;
}

/// MCP 出口（无会话请求，经 `/mcp`）：`_meta` 的 `dev.appwire/undo` / `undoOf`、`tools/list` 的 `dev.appwire/undoable`；
/// 任务句柄（`_meta` `dev.appwire/taskId`）的记录归句柄任务。
#[cfg(feature = "mcp-server")]
mod mcp {
    use std::net::SocketAddr;

    use app_mcp_hub::names::{META_CALL_ID, META_TASK_ID, META_UNDO, META_UNDO_OF, META_UNDOABLE};

    use super::*;
    use crate::support::mcp_http::modern_request;

    async fn start_mcp() -> (Hub, FakeApp, SocketAddr) {
        let mut c = config(UndoLimits::default());
        c.mcp_http = true;
        let hub = Hub::start(c).await.expect("hub");
        let app = connect(&hub, "todo-1").await;
        let addr = hub.listen_addr().expect("listen");
        (hub, app, addr)
    }

    async fn mcp_call(addr: SocketAddr, name: &str, args: Value, meta: Value) -> Value {
        let params = json!({"name": name, "arguments": args, "_meta": meta});
        let r = modern_request(addr, None, "tools/call", name, params).await;
        assert_eq!(r.status, 200, "{}", r.body);
        r.json()["result"].clone()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn meta_keys() {
        let (hub, app, addr) = start_mcp().await;
        let list = modern_request(addr, None, "tools/list", "", json!({})).await.json();
        let tools = list["result"]["tools"].as_array().cloned().unwrap_or_default();
        let meta = |name: &str| tools.iter().find(|t| t["name"] == name).map(|t| t["_meta"].clone()).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(meta("todo.add")[META_UNDOABLE], json!(true));
        assert!(meta("todo.remove").get(META_UNDOABLE).is_none());
        assert!(tools.iter().any(|t| t["name"] == "apps.undo"), "开启时列出 apps.undo");

        script_add(&app, 1, json!({}));
        let added = mcp_call(addr, "todo.add", json!({}), json!({})).await;
        assert_eq!(added["_meta"][META_UNDO], json!({"label": "删除刚添加的待办", "expiresInMs": 1_800_000}), "{added}");
        let original = added["_meta"][META_CALL_ID].clone();
        app.script("add", json!({"result": {"data": {"id": 2}}}));
        let plain = mcp_call(addr, "todo.add", json!({}), json!({})).await;
        assert!(plain["_meta"].get(META_UNDO).is_none(), "未登记不写：{plain}");

        let undone = mcp_call(addr, "apps.undo", json!({"callId": original}), json!({})).await;
        assert_eq!(undone["_meta"][META_UNDO_OF], original, "{undone}");
        assert_eq!(undone["structuredContent"], json!({"tool": "remove", "n": 1}), "逆调用的结果原样：{undone}");
        let again = mcp_call(addr, "apps.undo", json!({"callId": original}), json!({})).await;
        assert_eq!(again["isError"], true, "{again}");
        hub.shutdown().await;
    }

    /// 关闭时：结果不写 `dev.appwire/undo`，`tools/list` 不列出 `apps.undo`。
    #[tokio::test(flavor = "multi_thread")]
    async fn disabled_writes_no_meta() {
        let mut c = config(UndoLimits { max_per_task: 0, ..UndoLimits::default() });
        c.mcp_http = true;
        let hub = Hub::start(c).await.expect("hub");
        let app = connect(&hub, "todo-1").await;
        let addr = hub.listen_addr().expect("listen");
        let list = modern_request(addr, None, "tools/list", "", json!({})).await.json();
        let names: Vec<Value> = list["result"]["tools"].as_array().cloned().unwrap_or_default().iter().map(|t| t["name"].clone()).collect();
        assert!(names.contains(&json!("todo.add")) && !names.contains(&json!("apps.undo")), "{names:?}");
        script_add(&app, 1, json!({}));
        let added = mcp_call(addr, "todo.add", json!({}), json!({})).await;
        assert!(added["_meta"].get(META_UNDO).is_none(), "关闭时不登记：{added}");
        hub.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn task_handle_owns_its_records() {
        let (hub, app, addr) = start_mcp().await;
        let begun = mcp_call(addr, "apps.task.begin", json!({}), json!({})).await;
        let task = begun["structuredContent"]["taskId"].as_str().expect("taskId").to_owned();
        script_add(&app, 1, json!({}));
        mcp_call(addr, "todo.add", json!({}), json!({ META_TASK_ID: task })).await;
        let outside = mcp_call(addr, "apps.undo", json!({}), json!({})).await;
        assert_eq!(outside["isError"], true, "主体本身看不到句柄任务的记录：{outside}");
        let inside = mcp_call(addr, "apps.undo", json!({"taskId": task}), json!({})).await;
        assert!(inside["isError"] != json!(true), "{inside}");
        assert_eq!(app.invokes("remove"), 1);
        hub.shutdown().await;
    }
}
