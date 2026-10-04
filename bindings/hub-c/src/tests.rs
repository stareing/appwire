//! 单元测试：全部经 FFI 函数调用（与 C 调用方看到的行为一致）。
//! App 侧用 `app-mcp-native` 在同进程内连接嵌入式 Hub。

use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use app_mcp_native::{
    CallHandle, CallResult, ContentAnnotations, NativeClient, NativeConfig, ReadHandle, ResourceReader, ResourceSpec,
    ResultStatus, Risk, ToolAnnotations, ToolHandler, ToolOptions, ToolSpec,
};
use hub::ErrorKind;
use serde_json::{Value, json};

use super::*;
use crate::ffi_util::FfiError;

const WAIT: Duration = Duration::from_secs(10);

fn c(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

fn last_error() -> String {
    // SAFETY: 总是返回有效的 C 字符串。
    unsafe { CStr::from_ptr(am_hub_last_error_message()) }
        .to_string_lossy()
        .into_owned()
}

/// 取走库分配的字符串并释放。
unsafe fn take(p: *mut c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: 由本库分配。
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    // SAFETY: 同上。
    unsafe { am_hub_string_free(p) };
    s
}

fn parse(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

/// user_data = `*const Sender<String>`（由测试持有，比 Hub 活得久）。
unsafe extern "C" fn on_result(ud: *mut c_void, json: *mut c_char) {
    // SAFETY: 测试传入的 Sender。
    let tx = unsafe { &*(ud as *const Sender<String>) };
    // SAFETY: 库分配的字符串。
    let _ = tx.send(unsafe { take(json) });
}

unsafe extern "C" fn on_event(ud: *mut c_void, json: *mut c_char) {
    // SAFETY: 同上。
    unsafe { on_result(ud, json) };
}

static FREED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn free_sender(ud: *mut c_void) {
    // SAFETY: Box::into_raw 得到的 Sender。
    drop(unsafe { Box::from_raw(ud as *mut Sender<String>) });
    FREED.fetch_add(1, Ordering::SeqCst);
}

/// 审批回调：user_data = `*const Sender<(String, usize)>`，把请求与句柄地址交给测试线程决定。
unsafe extern "C" fn on_approval(ud: *mut c_void, json: *mut c_char, h: *mut AmHubApproval) {
    // SAFETY: 测试传入的 Sender。
    let tx = unsafe { &*(ud as *const Sender<(String, usize)>) };
    // SAFETY: 库分配的字符串。
    let _ = tx.send((unsafe { take(json) }, h as usize));
}

unsafe extern "C" fn on_pairing(ud: *mut c_void, json: *mut c_char, h: *mut AmHubPairing) {
    // SAFETY: 同上。
    let tx = unsafe { &*(ud as *const Sender<(String, usize)>) };
    // SAFETY: 同上。
    let _ = tx.send((unsafe { take(json) }, h as usize));
}

fn ud<T>(v: &T) -> *mut c_void {
    v as *const T as *mut c_void
}

/// 启动 Hub。配置未写 `ipcEndpoint` 时关闭本地 IPC（不占用本机常驻 Host 的默认端点）。
fn start_hub(config: &str) -> *mut AmHub {
    let mut v: Value = serde_json::from_str(config).expect("测试配置");
    if let Some(o) = v.as_object_mut() {
        o.entry("ipcEndpoint").or_insert(Value::Null);
    }
    let cfg = c(&v.to_string());
    let mut hub = ptr::null_mut();
    // SAFETY: 有效参数。
    let st = unsafe { am_hub_start(cfg.as_ptr(), &mut hub) };
    assert_eq!(st, AmHubStatus::Ok, "{}", last_error());
    assert!(!hub.is_null());
    hub
}

fn query_json(f: impl FnOnce(*mut *mut c_char) -> AmHubStatus) -> Value {
    let mut out = ptr::null_mut();
    let st = f(&mut out);
    assert_eq!(st, AmHubStatus::Ok, "{}", last_error());
    // SAFETY: 库分配的字符串。
    parse(&unsafe { take(out) })
}

fn call(hub: *mut AmHub, req: Value, tx: &Sender<String>) -> String {
    let req = c(&req.to_string());
    let mut id = ptr::null_mut();
    // SAFETY: 有效参数；tx 比回调活得久。
    let st = unsafe { am_hub_call(hub, req.as_ptr(), Some(on_result), ud(tx), &mut id) };
    assert_eq!(st, AmHubStatus::Ok, "{}", last_error());
    // SAFETY: 库分配的字符串。
    unsafe { take(id) }
}

fn recv(rx: &Receiver<String>) -> Value {
    parse(&rx.recv_timeout(WAIT).unwrap_or_default())
}

/// 等到满足条件的事件。
fn wait_event(rx: &Receiver<String>, pred: impl Fn(&Value) -> bool) -> bool {
    let deadline = Instant::now() + WAIT;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(s) if pred(&parse(&s)) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

// ---------------------------------------------------------------------------
// App 侧（app-mcp-native）
// ---------------------------------------------------------------------------

struct Echo;

impl ToolHandler for Echo {
    fn invoke(&self, call: CallHandle) {
        let args = parse(&call.arguments_json());
        let data = json!({ "echo": args });
        let _ = call.complete(Some(&data.to_string()), vec!["notes.list".to_owned()]);
    }
}

/// 永不完成（用于取消与停止）。
struct Hang;

impl ToolHandler for Hang {
    fn invoke(&self, call: CallHandle) {
        std::mem::forget(call);
    }
}

struct Notes;

impl ResourceReader for Notes {
    fn read(&self, read: ReadHandle) {
        let _ = read.complete(r#"{"items":["买牛奶"]}"#);
    }
}

struct App {
    client: NativeClient,
    _handles: Vec<Box<dyn std::any::Any>>,
}

fn start_app(hub: *mut AmHub) -> App {
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    assert!(!addr.is_empty());
    let mut cfg = NativeConfig::new("notes", "笔记");
    cfg.host_url = format!("ws://{addr}/app");
    cfg.instance_id = Some("n1".into());
    cfg.overview = Some(app_mcp_native::AppOverview {
        summary: "测试用笔记 App".into(),
        body: None,
        locale: None,
    });
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let mut handles: Vec<Box<dyn std::any::Any>> = Vec::new();
    let mut add = ToolSpec::new("add", "添加笔记");
    add.input_schema_json = Some(
        json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]})
            .to_string(),
    );
    handles.push(Box::new(client.register_tool(add, Arc::new(Echo)).expect("注册")));
    let mut del = ToolSpec::new("delete", "删除笔记");
    del.risk = Risk::Destructive;
    handles.push(Box::new(client.register_tool(del, Arc::new(Echo)).expect("注册")));
    handles.push(Box::new(
        client
            .register_tool(ToolSpec::new("hang", "不返回"), Arc::new(Hang))
            .expect("注册"),
    ));
    let res = ResourceSpec {
        name: "notes.list".into(),
        description: "全部笔记".into(),
        mime_type: None,
    };
    handles.push(Box::new(
        client.register_resource(res, Arc::new(Notes)).expect("注册资源"),
    ));
    client.start();
    App {
        client,
        _handles: handles,
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[test]
fn version_errors_and_null_arguments() {
    // SAFETY: 静态字符串。
    let v = unsafe { CStr::from_ptr(am_hub_version()) };
    assert_eq!(v.to_str().ok(), Some(env!("CARGO_PKG_VERSION")));
    // SAFETY: NULL 参数是合法输入（报错）。
    unsafe {
        assert_eq!(am_hub_start(ptr::null(), ptr::null_mut()), AmHubStatus::InvalidArgument);
        assert!(last_error().contains("out_hub"));
        let mut out = ptr::null_mut();
        assert_eq!(am_hub_apps_json(ptr::null(), &mut out), AmHubStatus::InvalidArgument);
        assert!(out.is_null());
        assert_eq!(am_hub_status_json(ptr::null(), &mut out), AmHubStatus::InvalidArgument);
        assert!(out.is_null());
        assert_eq!(am_hub_approval_complete(ptr::null_mut(), true), AmHubStatus::InvalidArgument);
        am_hub_string_free(ptr::null_mut());
        am_hub_free(ptr::null_mut());
        assert!(am_hub_listen_addr(ptr::null()).is_null());
    }
    let bad = c(r#"{"nope": 1}"#);
    let mut hub = ptr::null_mut();
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_start(bad.as_ptr(), &mut hub) }, AmHubStatus::InvalidJson);
    assert!(hub.is_null());
}

#[test]
fn bind_failure_is_io_error() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0"}"#);
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let cfg = c(&json!({ "listen": addr, "ipcEndpoint": null }).to_string());
    let mut second = ptr::null_mut();
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_start(cfg.as_ptr(), &mut second) }, AmHubStatus::Io);
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

#[test]
fn ipc_endpoint_config_and_single_instance() {
    #[cfg(unix)]
    let (endpoint, dir) = {
        let dir = std::env::temp_dir().join(format!("app-mcp-hub-c-ipc-{}", std::process::id()));
        (format!("unix:{}", dir.join("hub.sock").display()), Some(dir))
    };
    #[cfg(windows)]
    let (endpoint, dir) = (
        format!(r"pipe:\\.\pipe\app-mcp-hub-c-test-{}", std::process::id()),
        None::<std::path::PathBuf>,
    );
    let config = json!({ "listen": null, "ipcEndpoint": endpoint }).to_string();
    let hub = start_hub(&config);
    // SAFETY: 有效句柄。
    assert_eq!(unsafe { take(am_hub_ipc_endpoint(hub)) }, endpoint);
    // 同一端点第二个 Hub：已有 Hub 在监听 → IO 错误。
    let cfg = c(&config);
    let mut second = ptr::null_mut();
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_start(cfg.as_ptr(), &mut second) }, AmHubStatus::Io);
    assert!(second.is_null());
    // SAFETY: 有效句柄；停止后返回 NULL。
    unsafe {
        am_hub_shutdown(hub);
        assert!(am_hub_ipc_endpoint(hub).is_null());
        am_hub_free(hub);
    }
    // 关闭时的默认值：未开启 → NULL。
    let off = start_hub(r#"{"listen":null}"#);
    // SAFETY: 有效句柄。
    unsafe {
        assert!(am_hub_ipc_endpoint(off).is_null());
        am_hub_free(off);
    }
    if let Some(d) = dir {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn static_manifest_queries_export_and_errors() {
    let hub = start_hub(
        &json!({
            "listen": null,
            "manifests": [{
                "manifestVersion": 1, "appId": "shop", "name": "商城",
                "overview": { "summary": "演示商城" },
                "tools": [{ "name": "cart.add", "description": "加购",
                    "inputSchema": { "type": "object", "properties": { "sku": { "type": "string" } } },
                    "risk": "payment" }],
                "resources": [{ "name": "cart.state", "description": "购物车" }]
            }]
        })
        .to_string(),
    );
    // SAFETY: 以下均为有效参数。
    unsafe {
        assert!(am_hub_listen_addr(hub).is_null());
        let apps = query_json(|o| am_hub_apps_json(hub, o));
        assert_eq!(apps[0]["appId"], "shop");
        assert_eq!(apps[0]["connected"], false);

        // 运行状态：身份、监听、令牌策略、静态清单 App 为 disconnected
        let st = query_json(|o| am_hub_status_json(hub, o));
        assert_eq!(st["service"], "app-mcp", "{st}");
        assert_eq!(st["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(st["pid"], std::process::id());
        assert!(st.get("listen").is_none() && st.get("ipcEndpoint").is_none(), "{st}");
        assert!(st["startedAtMs"].as_u64().is_some_and(|t| t > 0));
        assert!(st["mcpHttp"].is_boolean());
        assert_eq!(st["auth"]["tokenConfigured"], false);
        assert_eq!(st["auth"]["tokenRequiredWithoutOrigin"], false);
        assert_eq!(st["mcpSessions"], 0);
        assert_eq!(st["apps"][0]["appId"], "shop");
        assert_eq!(st["apps"][0]["kind"], "app");
        assert_eq!(st["apps"][0]["state"], "disconnected");
        assert_eq!(st["apps"][0]["instances"], json!([]));
        assert_eq!(st["reports"], json!([]));

        let filter = c(r#"{"includeBuiltin": false}"#);
        let tools = query_json(|o| am_hub_tools_json(hub, filter.as_ptr(), o));
        assert_eq!(tools.as_array().map(Vec::len), Some(1));
        assert_eq!(tools[0]["name"], "shop.cart.add");
        assert_eq!(tools[0]["availability"], "disconnected");
        let all = query_json(|o| am_hub_tools_json(hub, ptr::null(), o));
        assert!(all.as_array().map(Vec::len).unwrap_or(0) > 1, "默认包含内置工具");
        let low = c(r#"{"maxRisk": "write", "includeBuiltin": false}"#);
        assert_eq!(query_json(|o| am_hub_tools_json(hub, low.as_ptr(), o)), json!([]));

        let res = query_json(|o| am_hub_resources_json(hub, o));
        assert_eq!(res[0]["uri"], "app-mcp://shop/cart.state");
        let shop = c("shop");
        let ov = query_json(|o| am_hub_overview_json(hub, shop.as_ptr(), o));
        assert_eq!(ov["summary"], "演示商城");
        let nope = c("nope");
        assert_eq!(query_json(|o| am_hub_overview_json(hub, nope.as_ptr(), o)), Value::Null);

        // 各格式导出
        let a = query_json(|o| am_hub_export_tools(hub, 3, filter.as_ptr(), o));
        assert_eq!(a[0]["name"], "shop__cart__add");
        assert!(a[0]["input_schema"].is_object());
        let oc = query_json(|o| am_hub_export_tools(hub, 1, filter.as_ptr(), o));
        assert_eq!(oc[0]["type"], "function");
        let or = query_json(|o| am_hub_export_tools(hub, 2, filter.as_ptr(), o));
        assert_eq!(or[0]["name"], "shop__cart__add");
        let g = query_json(|o| am_hub_export_tools(hub, 4, filter.as_ptr(), o));
        assert!(g["functionDeclarations"].is_array());
        let m = query_json(|o| am_hub_export_tools(hub, 0, filter.as_ptr(), o));
        assert!(m[0]["inputSchema"].is_object());
        let mut out = ptr::null_mut();
        assert_eq!(am_hub_export_tools(hub, 9, ptr::null(), &mut out), AmHubStatus::InvalidArgument);
        let bad = c("{");
        assert_eq!(am_hub_tools_json(hub, bad.as_ptr(), &mut out), AmHubStatus::InvalidJson);

        // 调用：静态工具未连接 → APP_DISCONNECTED；未知 App → TOOL_NOT_FOUND（仍为 CallOutcome）
        let (tx, rx) = mpsc::channel();
        let id = call(hub, json!({"name": "shop.cart.add", "arguments": {"sku": "A"}}), &tx);
        assert!(id.starts_with("am-hub-"));
        let o = recv(&rx);
        assert_eq!(o["callId"], id.as_str(), "{o}");
        assert_eq!(o["result"]["error"]["kind"], "APP_DISCONNECTED");
        call(hub, json!({"name": "ghost.x", "callId": "mine"}), &tx);
        let o = recv(&rx);
        assert_eq!(o["callId"], "mine");
        assert_eq!(o["result"]["error"]["kind"], "TOOL_NOT_FOUND");
        assert_eq!((o["durationMs"].clone(), o["woke"].clone()), (json!(0), json!(false)), "{o}");
        let bad_req = c("[1]");
        let mut id = ptr::null_mut();
        assert_eq!(
            am_hub_call(hub, bad_req.as_ptr(), Some(on_result), ud(&tx), &mut id),
            AmHubStatus::InvalidJson
        );
        assert!(id.is_null());

        // dispatch：Anthropic 错误结果
        let tu = c(r#"{"type":"tool_use","id":"toolu_1","name":"shop__cart__add","input":{}}"#);
        assert_eq!(
            am_hub_dispatch(hub, 3, tu.as_ptr(), ptr::null(), Some(on_result), ud(&tx)),
            AmHubStatus::Ok
        );
        let r = recv(&rx);
        assert_eq!(r["type"], "tool_result");
        assert_eq!(r["tool_use_id"], "toolu_1");
        assert_eq!(r["is_error"], true);

        // 读资源：未连接
        let uri = c("app-mcp://shop/cart.state");
        assert_eq!(am_hub_read_resource(hub, uri.as_ptr(), Some(on_result), ud(&tx)), AmHubStatus::Ok);
        assert!(recv(&rx)["error"]["kind"].is_string());

        // 订阅非法 URI → AM_HUB_ERR_HUB
        let unknown = c("not-a-uri");
        assert_eq!(am_hub_subscribe(hub, unknown.as_ptr()), AmHubStatus::Hub);
        assert!(last_error().contains("RESOURCE_NOT_FOUND"), "{}", last_error());

        // 停止后：操作返回 STOPPED，回调不调用
        am_hub_shutdown(hub);
        am_hub_shutdown(hub);
        let req = c(r#"{"name":"shop.cart.add"}"#);
        assert_eq!(
            am_hub_call(hub, req.as_ptr(), Some(on_result), ud(&tx), ptr::null_mut()),
            AmHubStatus::Stopped
        );
        assert_eq!(am_hub_apps_json(hub, &mut out), AmHubStatus::Stopped);
        assert_eq!(am_hub_status_json(hub, &mut out), AmHubStatus::Stopped);
        assert!(out.is_null());
        am_hub_free(hub);
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn app_round_trip_events_approval_and_shutdown() {
    let hub = start_hub(
        r#"{"listen":"127.0.0.1:0","approval":{"requireAtOrAbove":"destructive","timeout":3000}}"#,
    );
    let freed_before = FREED.load(Ordering::SeqCst);
    let (ev_tx, ev_rx) = mpsc::channel::<String>();
    let ev_tx = Box::into_raw(Box::new(ev_tx));
    // SAFETY: user_data 归库所有，由 free_sender 释放。
    let st = unsafe { am_hub_set_event_cb(hub, Some(on_event), ev_tx.cast(), Some(free_sender)) };
    assert_eq!(st, AmHubStatus::Ok);
    let (ap_tx, ap_rx) = mpsc::channel::<(String, usize)>();
    // SAFETY: ap_tx 比 Hub 活得久。
    assert_eq!(
        unsafe { am_hub_set_approval_cb(hub, Some(on_approval), ud(&ap_tx), None) },
        AmHubStatus::Ok
    );

    let app = start_app(hub);
    assert!(wait_event(&ev_rx, |e| e["type"] == "appConnected" && e["appId"] == "notes"));
    // 等工具同步完成
    let deadline = Instant::now() + WAIT;
    let filter = c(r#"{"apps":["notes"],"onlyAvailable":true,"includeBuiltin":false}"#);
    loop {
        // SAFETY: 有效参数。
        let t = query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) });
        if t.as_array().map(Vec::len) == Some(3) {
            break;
        }
        assert!(Instant::now() < deadline, "工具未同步：{t}");
        std::thread::sleep(Duration::from_millis(20));
    }

    let (tx, rx) = mpsc::channel();
    // 普通调用：成功 + stateHints + 首次附带总览
    call(hub, json!({"name":"notes.add","arguments":{"text":"买牛奶"},"session":"s1"}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["ok"]["echo"]["text"], "买牛奶", "{o}");
    assert_eq!(o["stateHints"], json!(["notes.list"]));
    assert_eq!(o["instanceId"], "n1");
    assert_eq!(o["overview"]["appId"], "notes");
    // v15：Hub API 会话的 Agent 任务出现在 status 中
    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    let task = st["tasks"].as_array().and_then(|t| t.iter().find(|t| t["caller"] == "api:s1")).cloned();
    let task = task.unwrap_or_else(|| panic!("缺少 api:s1 任务：{st}"));
    assert_eq!(task["kind"], "api");
    // v16：listen 流数（没有 MCP 客户端时为 0）
    assert_eq!(st["mcpListenStreams"], 0, "{st}");
    assert!(task["id"].as_str().is_some_and(|id| id.starts_with("task-")), "{task}");
    // 参数不合法
    call(hub, json!({"name":"notes.add","arguments":{},"session":"s1"}), &tx);
    assert_eq!(recv(&rx)["result"]["error"]["kind"], "INVALID_INPUT");

    // 审批：拒绝 → USER_REJECTED；同意 → 成功（在分发线程外完成句柄）
    call(hub, json!({"name":"notes.delete"}), &tx);
    let (req, h) = ap_rx.recv_timeout(WAIT).expect("审批请求");
    let req = parse(&req);
    assert_eq!(req["tool"], "delete");
    assert_eq!(req["risk"], "destructive");
    // v15：经 am_hub_call 发起的审批不带 principal / clientName
    assert!(req.get("principal").is_none() && req.get("clientName").is_none(), "{req}");
    // SAFETY: 回调交出的句柄。
    assert_eq!(unsafe { am_hub_approval_complete(h as *mut AmHubApproval, false) }, AmHubStatus::Ok);
    assert_eq!(recv(&rx)["result"]["error"]["kind"], "USER_REJECTED");
    call(hub, json!({"name":"notes.delete"}), &tx);
    let (_, h) = ap_rx.recv_timeout(WAIT).expect("审批请求");
    // SAFETY: 同上。
    assert_eq!(unsafe { am_hub_approval_complete(h as *mut AmHubApproval, true) }, AmHubStatus::Ok);
    assert!(recv(&rx)["result"]["ok"].is_object());

    // 审批等待中取消 → 句柄完成返回 ALREADY_COMPLETED
    let id = call(hub, json!({"name":"notes.delete"}), &tx);
    let (_, h) = ap_rx.recv_timeout(WAIT).expect("审批请求");
    let cid = c(&id);
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_cancel_call(hub, cid.as_ptr()) }, AmHubStatus::Ok);
    assert_eq!(recv(&rx)["result"]["error"]["kind"], "CANCELLED");
    // SAFETY: 同上。
    assert_eq!(
        unsafe { am_hub_approval_complete(h as *mut AmHubApproval, true) },
        AmHubStatus::AlreadyCompleted
    );

    // dispatch（Anthropic）：导出名 → tool_result
    let tu = c(r#"{"type":"tool_use","id":"toolu_9","name":"notes__add","input":{"text":"x"}}"#);
    // SAFETY: 有效参数。
    let st = unsafe { am_hub_dispatch(hub, 3, tu.as_ptr(), ptr::null(), Some(on_result), ud(&tx)) };
    assert_eq!(st, AmHubStatus::Ok);
    let r = recv(&rx);
    assert_eq!(r["tool_use_id"], "toolu_9");
    assert!(r.get("is_error").is_none_or(|v| v == false), "{r}");
    assert!(r["content"].to_string().contains("echo"));

    // 资源：读取 + 订阅
    let uri = c("app-mcp://notes/notes.list");
    // SAFETY: 有效参数。
    unsafe {
        assert_eq!(am_hub_read_resource(hub, uri.as_ptr(), Some(on_result), ud(&tx)), AmHubStatus::Ok);
        assert_eq!(am_hub_subscribe(hub, uri.as_ptr()), AmHubStatus::Ok);
        assert_eq!(am_hub_unsubscribe(hub, uri.as_ptr()), AmHubStatus::Ok);
    }
    let r = recv(&rx);
    assert!(r["ok"]["text"].as_str().unwrap_or_default().contains("买牛奶"), "{r}");

    // select_instance / reset_session
    let (app_id, inst) = (c("notes"), c("n1"));
    // SAFETY: 有效参数。
    unsafe {
        assert_eq!(am_hub_select_instance(hub, app_id.as_ptr(), inst.as_ptr()), AmHubStatus::Ok);
        let apps = query_json(|o| am_hub_apps_json(hub, o));
        assert_eq!(apps[0]["selectedInstance"], "n1");
        assert_eq!(am_hub_select_instance(hub, app_id.as_ptr(), ptr::null()), AmHubStatus::Ok);
        let s1 = c("s1");
        assert_eq!(am_hub_reset_session(hub, s1.as_ptr()), AmHubStatus::Ok);
    }
    call(hub, json!({"name":"notes.add","arguments":{"text":"再来"},"session":"s1"}), &tx);
    assert!(recv(&rx)["overview"].is_object(), "重置会话后再次附带总览");

    // 停止时进行中的调用以 CANCELLED 兜底结果回调
    call(hub, json!({"name":"notes.hang"}), &tx);
    std::thread::sleep(Duration::from_millis(100));
    // SAFETY: 有效句柄。
    unsafe { am_hub_shutdown(hub) };
    let o = recv(&rx);
    assert_eq!(o["result"]["error"]["kind"], "CANCELLED", "{o}");
    app.client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
    assert_eq!(FREED.load(Ordering::SeqCst), freed_before + 1, "事件回调的 user_data 被释放");
}

/// 已连接的实例在 apps / status 中带同一连接 ID（spec/hub-api.md 3.9）。
#[test]
fn status_lists_connected_instance_with_connection_id() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0"}"#);
    let app = start_app(hub);
    let deadline = Instant::now() + WAIT;
    let st = loop {
        // SAFETY: 有效参数。
        let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
        if st["apps"][0]["state"] == "connected" {
            break st;
        }
        assert!(Instant::now() < deadline, "App 未连接：{st}");
        std::thread::sleep(Duration::from_millis(20));
    };
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    assert_eq!(st["listen"], addr.as_str());
    let app_st = &st["apps"][0];
    assert_eq!(app_st["appId"], "notes");
    let inst = &app_st["instances"][0];
    assert_eq!(inst["instanceId"], "n1");
    assert_eq!(inst["state"], "connected");
    let cid = inst["connectionId"].as_str().unwrap_or_default().to_owned();
    assert!(cid.contains('-'), "连接 ID 形如 <标记>-<序号>：{inst}");
    // SAFETY: 有效参数。
    let apps = query_json(|o| unsafe { am_hub_apps_json(hub, o) });
    assert_eq!(apps[0]["instances"][0]["connectionId"], cid.as_str());

    app.client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

#[test]
fn approval_without_callback_and_timeout_reject() {
    let hub = start_hub(
        r#"{"listen":"127.0.0.1:0","approval":{"requireAtOrAbove":"write","timeout":200}}"#,
    );
    let app = start_app(hub);
    let deadline = Instant::now() + WAIT;
    let filter = c(r#"{"apps":["notes"],"onlyAvailable":true,"includeBuiltin":false}"#);
    // SAFETY: 有效参数。
    while query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) })
        .as_array()
        .map(Vec::len)
        != Some(3)
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let (tx, rx) = mpsc::channel();
    // 未设置审批回调 → USER_REJECTED
    call(hub, json!({"name":"notes.add","arguments":{"text":"a"}}), &tx);
    assert_eq!(recv(&rx)["result"]["error"]["kind"], "USER_REJECTED");
    // 回调不完成句柄 → 超时拒绝，之后完成返回 ALREADY_COMPLETED
    let (ap_tx, ap_rx) = mpsc::channel::<(String, usize)>();
    // SAFETY: ap_tx 比 Hub 活得久。
    unsafe { am_hub_set_approval_cb(hub, Some(on_approval), ud(&ap_tx), None) };
    call(hub, json!({"name":"notes.add","arguments":{"text":"a"}}), &tx);
    let (_, h) = ap_rx.recv_timeout(WAIT).expect("审批请求");
    assert_eq!(recv(&rx)["result"]["error"]["kind"], "USER_REJECTED");
    // SAFETY: 回调交出的句柄。
    assert_eq!(
        unsafe { am_hub_approval_complete(h as *mut AmHubApproval, true) },
        AmHubStatus::AlreadyCompleted
    );
    app.client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

#[test]
fn pairing_callback_accepts_and_rejects() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0"}"#);
    let (tx, rx) = mpsc::channel::<(String, usize)>();
    // SAFETY: tx 比 Hub 活得久。
    assert_eq!(
        unsafe { am_hub_set_pairing_cb(hub, Some(on_pairing), ud(&tx), None) },
        AmHubStatus::Ok
    );
    let app = start_app(hub);
    let (req, h) = rx.recv_timeout(WAIT).expect("配对请求");
    let req = parse(&req);
    assert_eq!(req["appId"], "notes");
    assert_eq!(req["clientKind"], "native");
    // SAFETY: 回调交出的句柄。
    assert_eq!(unsafe { am_hub_pairing_complete(h as *mut AmHubPairing, true) }, AmHubStatus::Ok);
    let deadline = Instant::now() + WAIT;
    loop {
        // SAFETY: 有效参数。
        let apps = query_json(|o| unsafe { am_hub_apps_json(hub, o) });
        if apps[0]["connected"] == true {
            break;
        }
        assert!(Instant::now() < deadline, "配对后未连接：{apps}");
        std::thread::sleep(Duration::from_millis(20));
    }
    app.client.stop();

    // 拒绝：另一个 App
    let mut cfg = NativeConfig::new("other", "另一个");
    // SAFETY: 有效句柄。
    cfg.host_url = format!("ws://{}/app", unsafe { take(am_hub_listen_addr(hub)) });
    let other = NativeClient::new(cfg, None).expect("创建 App");
    other.start();
    let (req, h) = rx.recv_timeout(WAIT).expect("配对请求");
    assert_eq!(parse(&req)["appId"], "other");
    // SAFETY: 回调交出的句柄。
    assert_eq!(unsafe { am_hub_pairing_complete(h as *mut AmHubPairing, false) }, AmHubStatus::Ok);
    let deadline = Instant::now() + WAIT;
    while other.state().status != app_mcp_native::StateStatus::Rejected {
        assert!(Instant::now() < deadline, "未被拒绝：{:?}", other.state());
        std::thread::sleep(Duration::from_millis(20));
    }
    other.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

/// 在回调（分发线程）中释放 Hub 不死锁。
#[test]
fn free_from_callback() {
    unsafe extern "C" fn free_in_cb(ud: *mut c_void, json: *mut c_char) {
        // SAFETY: 测试传入的 (Hub 指针, Sender)。
        let (hub, tx) = unsafe { &*(ud as *const (usize, Sender<String>)) };
        // SAFETY: 有效句柄；在回调中释放是允许的。
        unsafe { am_hub_free(*hub as *mut AmHub) };
        // SAFETY: 库分配的字符串。
        let _ = tx.send(unsafe { take(json) });
    }
    let hub = start_hub(r#"{"listen":null}"#);
    let (tx, rx) = mpsc::channel();
    let ctx = (hub as usize, tx);
    let req = c(r#"{"name":"ghost.x"}"#);
    // SAFETY: ctx 比回调活得久（recv 等待回调结束）。
    let st = unsafe { am_hub_call(hub, req.as_ptr(), Some(free_in_cb), ud(&ctx), ptr::null_mut()) };
    assert_eq!(st, AmHubStatus::Ok);
    assert_eq!(recv(&rx)["result"]["error"]["kind"], "TOOL_NOT_FOUND");
}

/// 头文件与实现一致：函数名、状态码、格式枚举。
#[test]
fn header_matches_implementation() {
    let h = include_str!("../include/app_mcp_hub.h");
    assert!(h.contains(&format!("#define AM_HUB_API_VERSION {AM_HUB_API_VERSION}")));
    for (name, v) in [
        ("AM_HUB_OK", AmHubStatus::Ok),
        ("AM_HUB_ERR_INVALID_ARGUMENT", AmHubStatus::InvalidArgument),
        ("AM_HUB_ERR_INVALID_JSON", AmHubStatus::InvalidJson),
        ("AM_HUB_ERR_INVALID_CONFIG", AmHubStatus::InvalidConfig),
        ("AM_HUB_ERR_IO", AmHubStatus::Io),
        ("AM_HUB_ERR_HUB", AmHubStatus::Hub),
        ("AM_HUB_ERR_ALREADY_COMPLETED", AmHubStatus::AlreadyCompleted),
        ("AM_HUB_ERR_STOPPED", AmHubStatus::Stopped),
        ("AM_HUB_ERR_INTERNAL", AmHubStatus::Internal),
        ("AM_HUB_ERR_PANIC", AmHubStatus::Panic),
        ("AM_HUB_ERR_UNSUPPORTED", AmHubStatus::Unsupported),
    ] {
        assert!(h.contains(&format!("{name} = {}", v as i32)), "{name}");
    }
    for f in [
        "am_hub_version", "am_hub_last_error_message", "am_hub_string_free", "am_hub_start",
        "am_hub_shutdown", "am_hub_free", "am_hub_listen_addr", "am_hub_ipc_endpoint", "am_hub_serve_http",
        "am_hub_apps_json", "am_hub_tools_json", "am_hub_resources_json", "am_hub_overview_json",
        "am_hub_status_json", "am_hub_call", "am_hub_cancel_call", "am_hub_read_resource", "am_hub_subscribe",
        "am_hub_unsubscribe", "am_hub_select_instance", "am_hub_reset_session",
        "am_hub_export_tools", "am_hub_dispatch", "am_hub_set_event_cb", "am_hub_set_approval_cb",
        "am_hub_approval_complete", "am_hub_set_pairing_cb", "am_hub_pairing_complete",
        "am_hub_set_waker_cb", "am_hub_waker_complete", "am_hub_set_policy", "am_hub_set_agents", "am_hub_call_with_progress",
        "am_hub_set_app_event_cb", "am_hub_set_intent_defaults", "am_hub_intents_json",
    ] {
        assert!(h.contains(&format!("{f}(")), "{f}");
    }
    let exported: usize = [
        include_str!("lib.rs"),
        include_str!("lifecycle.rs"),
        include_str!("query.rs"),
        include_str!("calls.rs"),
        include_str!("callbacks.rs"),
        include_str!("app_events.rs"),
        include_str!("intents.rs"),
    ]
    .iter()
    .map(|src| src.matches("#[unsafe(no_mangle)]").count())
    .sum();
    assert_eq!(exported, 36, "导出函数数量与头文件清单一致");
}

#[test]
fn serve_http_on_loopback() {
    let hub = start_hub(r#"{"listen":null}"#);
    let addr = c("127.0.0.1:0");
    let mut out = ptr::null_mut();
    // SAFETY: 有效参数。
    unsafe {
        assert_eq!(am_hub_serve_http(hub, addr.as_ptr(), false, &mut out), AmHubStatus::Ok);
        assert!(take(out).starts_with("127.0.0.1:"));
        let remote = c("0.0.0.0:0");
        assert_eq!(am_hub_serve_http(hub, remote.as_ptr(), false, &mut out), AmHubStatus::Io);
        am_hub_free(hub);
    }
}

mod agents;
mod events;
mod intents;
mod lifecycle;
mod locks;
