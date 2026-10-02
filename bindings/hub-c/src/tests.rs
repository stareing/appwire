//! 单元测试：全部经 FFI 函数调用（与 C 调用方看到的行为一致）。
//! App 侧用 `app-mcp-native` 在同进程内连接嵌入式 Hub。

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use app_mcp_native::{
    CallHandle, CallResult, ContentAnnotations, NativeClient, NativeConfig, ReadHandle, ResourceReader, ResourceSpec,
    ResultStatus, Risk, ToolAnnotations, ToolHandler, ToolOptions, ToolSpec,
};

use super::*;

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
        "am_hub_set_waker_cb", "am_hub_waker_complete", "am_hub_set_policy", "am_hub_call_with_progress",
    ] {
        assert!(h.contains(&format!("{f}(")), "{f}");
    }
    let src = include_str!("lib.rs");
    let exported = src.matches("#[unsafe(no_mangle)]").count();
    assert_eq!(exported, 32, "导出函数数量与头文件清单一致");
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

// ---------------------------------------------------------------------------
// 生命周期：休眠 → 列出 dormant → 调用触发自定义唤醒 → 成功
// ---------------------------------------------------------------------------

/// 与 `free_sender` 相同，但不计入 FREED（避免与并行测试的计数互相干扰）。
unsafe extern "C" fn free_sender_quiet(ud: *mut c_void) {
    // SAFETY: Box::into_raw 得到的 Sender。
    drop(unsafe { Box::from_raw(ud as *mut Sender<String>) });
}

unsafe extern "C" fn on_wake(ud: *mut c_void, json: *mut c_char, h: *mut AmHubWake) {
    // SAFETY: 测试传入的 Sender。
    let tx = unsafe { &*(ud as *const Sender<(String, usize)>) };
    // SAFETY: 库分配的字符串。
    let _ = tx.send((unsafe { take(json) }, h as usize));
}

fn start_idle_app(hub: *mut AmHub) -> (NativeClient, Box<dyn std::any::Any>) {
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("sleepy", "会睡觉的 App");
    cfg.host_url = format!("ws://{addr}/app");
    cfg.instance_id = Some("s1".into());
    cfg.lifecycle.mode = app_mcp_native::LifecycleMode::Idle;
    cfg.lifecycle.idle_timeout_ms = 300;
    cfg.lifecycle.wake = Some(app_mcp_native::WakeDescriptor {
        kind: app_mcp_native::WakeKind::AndroidIntent,
        target: Some("dev.example/.WakeReceiver".into()),
        background: true,
    });
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let h = client
        .register_tool(ToolSpec::new("ping", "回显"), Arc::new(Echo))
        .expect("注册");
    client.start();
    (client, Box::new(h))
}

#[test]
fn dormant_app_woken_by_custom_waker() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0","listChangedDebounceMs":20,"wakeTimeoutMs":8000,"leaseTtlMs":0}"#);
    let (etx, erx) = mpsc::channel::<String>();
    let (wtx, wrx) = mpsc::channel::<(String, usize)>();
    // SAFETY: 有效参数；Sender 归库所有（free_sender 释放）或由测试持有且比 Hub 活得久。
    unsafe {
        let st = am_hub_set_event_cb(hub, Some(on_event), Box::into_raw(Box::new(etx)).cast(), Some(free_sender_quiet));
        assert_eq!(st, AmHubStatus::Ok);
        assert_eq!(am_hub_set_waker_cb(hub, Some(on_wake), ud(&wtx), None), AmHubStatus::Ok);
    }
    let (client, _h) = start_idle_app(hub);

    assert!(
        wait_event(&erx, |e| e["type"] == "appDormant" && e["appId"] == "sleepy" && e["instanceId"] == "s1"),
        "应收到 appDormant"
    );
    // SAFETY: 有效参数。
    let apps = query_json(|o| unsafe { am_hub_apps_json(hub, o) });
    let app = apps
        .as_array()
        .and_then(|a| a.iter().find(|a| a["appId"] == "sleepy"))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(app["connected"], false, "{app}");
    assert_eq!(app["dormantInstances"][0]["instanceId"], "s1", "{app}");
    let filter = c(r#"{"apps":["sleepy"],"includeBuiltin":false}"#);
    // SAFETY: 有效参数。
    let tools = query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) });
    assert_eq!(tools[0]["availability"], "dormant", "{tools}");

    // 调用 → Hub 调用自定义 Waker；测试线程扮演厂商：让同进程 App handleWake(令牌) 后完成句柄。
    let (rtx, rrx) = mpsc::channel::<String>();
    call(hub, json!({"name": "sleepy.ping", "arguments": {"x": 1}}), &rtx);
    let (req, h) = wrx.recv_timeout(WAIT).expect("应调用 Waker");
    let req = parse(&req);
    assert_eq!(req["appId"], "sleepy");
    assert_eq!(req["instanceId"], "s1");
    assert_eq!(req["descriptor"]["kind"], "android-intent");
    let token = req["token"].as_str().unwrap_or_default().to_owned();
    assert_eq!(token.len(), 32);
    assert_eq!(req["activationArg"], format!("app-mcp-wake:{token}"));
    assert!(client.handle_wake(req["activationArg"].as_str().unwrap_or_default()));
    // SAFETY: 回调交出的句柄，只消费一次。
    assert_eq!(unsafe { am_hub_waker_complete(h as *mut AmHubWake, true, ptr::null(), ptr::null()) }, AmHubStatus::Ok);
    let out = recv(&rrx);
    assert_eq!(out["result"]["ok"]["echo"], json!({"x": 1}), "{out}");
    assert!(wait_event(&erx, |e| e["type"] == "appWaking" || e["type"] == "appConnected"));
    client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

#[test]
fn waker_failure_maps_error_kind() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0","listChangedDebounceMs":20,"leaseTtlMs":0}"#);
    let (etx, erx) = mpsc::channel::<String>();
    let (wtx, wrx) = mpsc::channel::<(String, usize)>();
    // SAFETY: 同上。
    unsafe {
        am_hub_set_event_cb(hub, Some(on_event), Box::into_raw(Box::new(etx)).cast(), Some(free_sender_quiet));
        am_hub_set_waker_cb(hub, Some(on_wake), ud(&wtx), None);
    }
    let (client, _h) = start_idle_app(hub);
    assert!(wait_event(&erx, |e| e["type"] == "appDormant"));
    let (rtx, rrx) = mpsc::channel::<String>();
    call(hub, json!({"name": "sleepy.ping"}), &rtx);
    let (_, h) = wrx.recv_timeout(WAIT).expect("应调用 Waker");
    let kind = c("APP_NOT_INSTALLED");
    let msg = c("没装");
    // SAFETY: 回调交出的句柄。
    let st = unsafe { am_hub_waker_complete(h as *mut AmHubWake, false, kind.as_ptr(), msg.as_ptr()) };
    assert_eq!(st, AmHubStatus::Ok);
    let out = recv(&rrx);
    assert_eq!(out["result"]["error"]["kind"], "APP_NOT_INSTALLED", "{out}");
    assert!(out["result"]["error"]["message"].as_str().unwrap_or_default().contains("没装"));
    // 未知类别 → LAUNCH_FAILED；清除回调恢复默认实现
    assert_eq!(parse_error_kind(Some("NOPE")), ErrorKind::LaunchFailed);
    assert_eq!(parse_error_kind(None), ErrorKind::LaunchFailed);
    assert_eq!(parse_error_kind(Some("TIMEOUT")), ErrorKind::Timeout);
    // SAFETY: 有效参数。
    unsafe {
        assert_eq!(am_hub_set_waker_cb(hub, None, ptr::null_mut(), None), AmHubStatus::Ok);
        assert_eq!(am_hub_waker_complete(ptr::null_mut(), true, ptr::null(), ptr::null()), AmHubStatus::InvalidArgument);
    }
    client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

/// 缺少 cargo feature 的错误（spec/hub-api.md 3.10）报 `AM_HUB_ERR_UNSUPPORTED`，其余 I/O 错误仍为 `AM_HUB_ERR_IO`。
#[test]
fn unsupported_io_error_maps_to_own_status() {
    let e = FfiError::io("启动 Hub 失败", &std::io::Error::new(std::io::ErrorKind::Unsupported, "缺少 `upstream`"));
    assert_eq!(e.status, AmHubStatus::Unsupported);
    assert!(e.message.contains("`upstream`"), "{}", e.message);
    let e = FfiError::io("启动 Hub 失败", &std::io::Error::new(std::io::ErrorKind::AddrInUse, "占用"));
    assert_eq!(e.status, AmHubStatus::Io);

    // 端到端：本库以 app-mcp-hub 默认（完整）能力构建时成功；缺少 feature 时为 UNSUPPORTED。
    let cfg = c(r#"{"listen":"127.0.0.1:0","ipcEndpoint":null,"mcpHttp":true}"#);
    let mut hub = ptr::null_mut();
    // SAFETY: 有效参数。
    let st = unsafe { am_hub_start(cfg.as_ptr(), &mut hub) };
    let want = if hub::features::MCP_SERVER { AmHubStatus::Ok } else { AmHubStatus::Unsupported };
    assert_eq!(st, want, "{}", last_error());
    if !hub.is_null() {
        // SAFETY: am_hub_start 成功返回的句柄。
        unsafe { am_hub_free(hub) };
    }
}

/// 以 pending + stateResource + summary + 内容注解完成（第 19 项 R1–R3）。
struct Submit;

impl ToolHandler for Submit {
    fn invoke(&self, call: CallHandle) {
        let _ = call.complete_with(CallResult {
            data_json: Some(r#"{"orderId":"o1"}"#.into()),
            status: ResultStatus::Pending,
            state_resource: Some("order.state".into()),
            summary: Some("已提交".into()),
            annotations: Some(ContentAnnotations { priority: Some(0.5), ..ContentAnnotations::default() }),
            ..CallResult::default()
        });
    }
}

/// 配置 limits / outputValidation（v9）→ 工具注解与 outputSchema、结构化结果、RATE_LIMITED、/status 新字段。
#[test]
fn limits_annotations_and_structured_result() {
    // 非法：限流时 burst = 0
    let bad = c(r#"{"listen":null,"ipcEndpoint":null,"limits":{"toolRateBurst":0}}"#);
    let mut out = ptr::null_mut();
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_start(bad.as_ptr(), &mut out) }, AmHubStatus::InvalidConfig);
    assert!(out.is_null());
    assert!(last_error().contains("toolRateBurst"), "{}", last_error());

    let hub = start_hub(
        r#"{"listen":"127.0.0.1:0","limits":{"toolRatePerMinute":1,"toolRateBurst":1},"outputValidation":"reject"}"#,
    );
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("shop", "商店");
    cfg.host_url = format!("ws://{addr}/app");
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let options = ToolOptions {
        annotations: Some(ToolAnnotations { idempotent_hint: Some(false), ..ToolAnnotations::default() }),
        output_schema_json: Some(r#"{"type":"object","properties":{"orderId":{"type":"string"}}}"#.into()),
        ..ToolOptions::default()
    };
    let _tool = client
        .register_tool_with(ToolSpec::new("order.submit", "下单"), options, Arc::new(Submit))
        .expect("注册");
    client.start();

    let deadline = Instant::now() + WAIT;
    let filter = c(r#"{"apps":["shop"],"onlyAvailable":true,"includeBuiltin":false}"#);
    let tools = loop {
        // SAFETY: 有效参数。
        let t = query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) });
        if t.as_array().map(Vec::len) == Some(1) {
            break t;
        }
        assert!(Instant::now() < deadline, "工具未同步：{t}");
        std::thread::sleep(Duration::from_millis(20));
    };
    // 声明的字段优先，缺少的按 risk（write）推导
    assert_eq!(tools[0]["annotations"]["idempotentHint"], false, "{tools}");
    assert_eq!(tools[0]["annotations"]["readOnlyHint"], false, "{tools}");
    assert_eq!(tools[0]["outputSchema"]["properties"]["orderId"]["type"], "string", "{tools}");

    let (tx, rx) = mpsc::channel();
    call(hub, json!({"name":"shop.order.submit"}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["ok"]["orderId"], "o1", "{o}");
    assert_eq!(o["status"], "pending", "{o}");
    assert_eq!(o["stateResource"], "app-mcp://shop/order.state", "{o}");
    assert_eq!(o["summary"], "已提交", "{o}");
    assert_eq!(o["annotations"], json!({"priority": 0.5}), "{o}");
    // 突发 1：第二次立即调用被限流
    call(hub, json!({"name":"shop.order.submit"}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["error"]["kind"], "RATE_LIMITED", "{o}");
    assert_eq!(o["result"]["error"]["details"]["scope"], "tool", "{o}");

    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    assert_eq!(st["limits"]["toolRatePerMinute"], 1, "{st}");
    assert_eq!(st["limits"]["appRatePerMinute"], 600, "{st}");
    assert_eq!(st["outputValidation"], "reject", "{st}");
    let app_st = &st["apps"][0];
    assert_eq!(app_st["rateLimited"], 1, "{st}");
    assert_eq!(app_st["tooLarge"], 0, "{st}");
    assert_eq!(app_st["tools"][0]["name"], "order.submit", "{st}");
    assert_eq!(app_st["tools"][0]["outputSchema"], true, "{st}");
    assert_eq!(app_st["tools"][0]["annotations"], json!({"idempotentHint": false}), "{st}");

    client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

fn set_policy(hub: *mut AmHub, policy: &str) -> AmHubStatus {
    let p = c(policy);
    // SAFETY: 有效参数。
    unsafe { am_hub_set_policy(hub, p.as_ptr()) }
}

fn tool_names(hub: *mut AmHub) -> Vec<String> {
    let filter = c(r#"{"apps":["notes"],"includeBuiltin":false}"#);
    // SAFETY: 有效参数。
    let t = query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) });
    let mut v: Vec<String> =
        t.as_array().into_iter().flatten().filter_map(|t| t["tool"].as_str().map(str::to_owned)).collect();
    v.sort();
    v
}

/// 配置 policy（v10）→ hide 的工具不在列表中且调用为 TOOL_NOT_FOUND；deny → POLICY_DENIED；am_hub_set_policy 替换，
/// 不合法时保留旧规则；HubStatus.policy 的命中计数与 lastError。
#[test]
fn policy_hide_deny_and_set_policy() {
    let bad = c(r#"{"listen":null,"ipcEndpoint":null,"policy":{"rules":[{"id":"x","action":"hide","app":"a*b"}]}}"#);
    let mut out = ptr::null_mut();
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_start(bad.as_ptr(), &mut out) }, AmHubStatus::InvalidConfig);
    assert!(out.is_null());
    assert!(last_error().contains("policy"), "{}", last_error());

    let hub = start_hub(
        r#"{"listen":"127.0.0.1:0","policy":{"rules":[
            {"id":"hide-delete","action":"hide","app":"notes","tool":"delete"},
            {"id":"deny-add","action":"deny","app":"notes","tool":"add"}]}}"#,
    );
    let app = start_app(hub);
    let deadline = Instant::now() + WAIT;
    while tool_names(hub).len() < 2 {
        assert!(Instant::now() < deadline, "工具未同步");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(tool_names(hub), ["add", "hang"], "hide 的工具不在列表中");

    let (tx, rx) = mpsc::channel();
    call(hub, json!({"name":"notes.delete"}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["error"]["kind"], "TOOL_NOT_FOUND", "{o}");
    call(hub, json!({"name":"notes.add","arguments":{"text":"x"}}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["error"]["kind"], "POLICY_DENIED", "{o}");
    assert_eq!(o["result"]["error"]["details"]["ruleId"], "deny-add", "{o}");
    assert_eq!(o["result"]["error"]["details"]["hook"], "call", "{o}");

    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    assert_eq!(st["policy"]["rules"][0]["id"], "hide-delete", "{st}");
    assert_eq!(st["policy"]["rules"][0]["hits"], 1, "{st}");
    assert_eq!(st["policy"]["rules"][1]["hits"], 1, "{st}");

    // 不是合法 JSON / 未知字段 / 规则不合法（hide 不能写 hooks）：报错，旧规则继续生效
    assert_eq!(set_policy(hub, "{"), AmHubStatus::InvalidJson);
    assert_eq!(set_policy(hub, r#"{"rules":[],"x":1}"#), AmHubStatus::InvalidJson);
    let invalid = r#"{"rules":[{"id":"h","action":"hide","app":"notes","hooks":["call"]}]}"#;
    assert_eq!(set_policy(hub, invalid), AmHubStatus::InvalidConfig);
    assert!(last_error().contains("hooks"), "{}", last_error());
    call(hub, json!({"name":"notes.add","arguments":{"text":"x"}}), &tx);
    assert_eq!(recv(&rx)["result"]["error"]["kind"], "POLICY_DENIED");
    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    assert!(st["policy"]["lastError"]["message"].is_string(), "{st}");

    // 清空：恢复原行为
    assert_eq!(set_policy(hub, "{}"), AmHubStatus::Ok, "{}", last_error());
    assert_eq!(tool_names(hub), ["add", "delete", "hang"]);
    call(hub, json!({"name":"notes.add","arguments":{"text":"x"}}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["ok"]["echo"]["text"], "x", "{o}");
    // SAFETY: NULL 参数。
    assert_eq!(unsafe { am_hub_set_policy(hub, ptr::null()) }, AmHubStatus::InvalidArgument);

    app.client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

// ---------------------------------------------------------------------------
// v11：调用进度
// ---------------------------------------------------------------------------

/// 报告两次进度（间隔超过 Hub 的合并间隔）后完成。
struct Progressing;

impl ToolHandler for Progressing {
    fn invoke(&self, call: CallHandle) {
        std::thread::spawn(move || {
            let _ = call.report_progress(1.0, Some(2.0), Some("第一步"));
            std::thread::sleep(Duration::from_millis(150));
            let _ = call.report_progress(2.0, None, None);
            std::thread::sleep(Duration::from_millis(150));
            let _ = call.complete(Some(r#"{"done":true}"#), Vec::new());
        });
    }
}

/// user_data = `*const Sender<String>`；进度以 `progress:` 前缀与结果区分。
unsafe extern "C" fn on_progress(ud: *mut c_void, json: *mut c_char) {
    // SAFETY: 测试传入的 Sender。
    let tx = unsafe { &*(ud as *const Sender<String>) };
    // SAFETY: 库分配的字符串。
    let _ = tx.send(format!("progress:{}", unsafe { take(json) }));
}

#[test]
fn call_with_progress_delivers_progress_before_result() {
    let hub = start_hub(r#"{"listen": "127.0.0.1:0", "progressIntervalMs": 10}"#);
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("steps", "步骤");
    cfg.host_url = format!("ws://{addr}/app");
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let _tool = client
        .register_tool(ToolSpec::new("run", "分步执行"), Arc::new(Progressing))
        .expect("注册");
    let _echo = client.register_tool(ToolSpec::new("echo", "回显"), Arc::new(Echo)).expect("注册");
    client.start();
    // 等工具同步完成
    let deadline = Instant::now() + WAIT;
    let filter = c(r#"{"apps":["steps"],"onlyAvailable":true,"includeBuiltin":false}"#);
    loop {
        // SAFETY: 有效参数。
        let t = query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) });
        if t.as_array().map(Vec::len) == Some(2) {
            break;
        }
        assert!(Instant::now() < deadline, "工具未同步：{t}");
        std::thread::sleep(Duration::from_millis(20));
    }

    let (tx, rx) = mpsc::channel::<String>();
    let req = c(&json!({"name": "steps.run", "arguments": {}, "callId": "p1"}).to_string());
    let mut id = ptr::null_mut();
    // SAFETY: 有效参数；tx 比回调活得久。
    let st = unsafe { am_hub_call_with_progress(hub, req.as_ptr(), Some(on_result), Some(on_progress), ud(&tx), &mut id) };
    assert_eq!(st, AmHubStatus::Ok, "{}", last_error());
    // SAFETY: 库分配的字符串。
    assert_eq!(unsafe { take(id) }, "p1");
    let mut got = Vec::new();
    while let Ok(s) = rx.recv_timeout(WAIT) {
        let done = !s.starts_with("progress:");
        got.push(s);
        if done {
            break;
        }
    }
    let (result, progress) = got.split_last().expect("收到结果");
    assert_eq!(parse(result)["result"]["ok"], json!({"done": true}), "{result}");
    let progress: Vec<Value> = progress.iter().map(|s| parse(&s["progress:".len()..])).collect();
    assert_eq!(
        progress,
        vec![
            json!({"callId": "p1", "progress": 1.0, "total": 2.0, "message": "第一步"}),
            json!({"callId": "p1", "progress": 2.0}),
        ]
    );

    // on_progress 为 NULL：等同 am_hub_call。
    let req = c(&json!({"name": "steps.echo", "arguments": {}}).to_string());
    // SAFETY: 有效参数。
    let st = unsafe { am_hub_call_with_progress(hub, req.as_ptr(), Some(on_result), None, ud(&tx), ptr::null_mut()) };
    assert_eq!(st, AmHubStatus::Ok);
    assert!(recv(&rx)["result"]["ok"].is_object());
    // 缺少 cb：参数错误。
    // SAFETY: 同上。
    let st = unsafe { am_hub_call_with_progress(hub, req.as_ptr(), None, Some(on_progress), ud(&tx), ptr::null_mut()) };
    assert_eq!(st, AmHubStatus::InvalidArgument);

    client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}


// ---------------------------------------------------------------------------
// 休眠记录持久化（stateDir）：重启后休眠的 App 仍可列出；HubStatus.dormantStore
// ---------------------------------------------------------------------------

#[test]
fn state_dir_persists_dormant_across_restart() {
    let dir = std::env::temp_dir().join(format!("app-mcp-hub-c-state-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = json!({
        "listen": "127.0.0.1:0", "listChangedDebounceMs": 20, "leaseTtlMs": 0, "stateDir": dir,
    })
    .to_string();

    // 未配置 stateDir：status 不含 dormantStore
    let plain = start_hub(r#"{"listen":null}"#);
    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(plain, o) });
    assert!(st.get("dormantStore").is_none(), "{st}");
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(plain) };

    let hub = start_hub(&config);
    let (etx, erx) = mpsc::channel::<String>();
    // SAFETY: 有效参数；Sender 归库所有。
    unsafe {
        let st = am_hub_set_event_cb(hub, Some(on_event), Box::into_raw(Box::new(etx)).cast(), Some(free_sender_quiet));
        assert_eq!(st, AmHubStatus::Ok);
    }
    let (client, _h) = start_idle_app(hub);
    assert!(wait_event(&erx, |e| e["type"] == "appDormant" && e["appId"] == "sleepy"), "应收到 appDormant");
    let deadline = Instant::now() + WAIT;
    loop {
        // SAFETY: 有效参数。
        let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
        let store = &st["dormantStore"];
        assert_eq!(store["dir"], dir.join("dormant").display().to_string(), "{st}");
        if store["writes"].as_u64().is_some_and(|w| w >= 1) {
            break;
        }
        assert!(Instant::now() < deadline, "休眠记录未写入：{st}");
        std::thread::sleep(Duration::from_millis(20));
    }
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
    client.stop();

    // 重启：从 stateDir 读回休眠实例
    let hub = start_hub(&config);
    // SAFETY: 有效参数。
    let apps = query_json(|o| unsafe { am_hub_apps_json(hub, o) });
    let app = apps
        .as_array()
        .and_then(|a| a.iter().find(|a| a["appId"] == "sleepy"))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(app["dormantInstances"][0]["instanceId"], "s1", "{apps}");
    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    assert_eq!(st["dormantStore"]["loadedInstances"], 1, "{st}");
    assert_eq!(st["dormantStore"]["issues"], json!([]), "{st}");
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
    let _ = std::fs::remove_dir_all(&dir);
}

/// 回显 Agent 给出的幂等键（spec/hub-api.md 3.15）。
struct KeyEcho;

impl ToolHandler for KeyEcho {
    fn invoke(&self, call: CallHandle) {
        let data = json!({ "key": call.idempotency_key() });
        let _ = call.complete(Some(&data.to_string()), Vec::new());
    }
}

/// v13：配置 navigateTimeoutMs；CallRequest.idempotencyKey 原样转交 App、不合法时 INVALID_INPUT；
/// HubTool.surface / page；内置工具 apps.activate / apps.release 与（有页面目录时）apps.page / apps.navigate。
#[test]
fn surface_page_idempotency_key_and_builtins() {
    let bad = c(r#"{"listen":null,"ipcEndpoint":null,"navigateTimeoutMs":-1}"#);
    let mut out = ptr::null_mut();
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_start(bad.as_ptr(), &mut out) }, AmHubStatus::InvalidJson);
    assert!(out.is_null());

    let hub = start_hub(r#"{"listen":"127.0.0.1:0","navigateTimeoutMs":800}"#);
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("shop", "商店");
    cfg.host_url = format!("ws://{addr}/app");
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let view = ToolOptions {
        surface: app_mcp_native::ToolSurface::View,
        page: Some("cart".into()),
        ..ToolOptions::default()
    };
    let _view = client
        .register_tool_with(ToolSpec::new("cart.checkout", "结算"), view, Arc::new(KeyEcho))
        .expect("注册");
    let _plain = client.register_tool(ToolSpec::new("order.submit", "下单"), Arc::new(KeyEcho)).expect("注册");
    client.start();

    let deadline = Instant::now() + WAIT;
    let filter = c(r#"{"apps":["shop"],"onlyAvailable":true,"includeBuiltin":false}"#);
    let tools = loop {
        // SAFETY: 有效参数。
        let t = query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) });
        if t.as_array().map(Vec::len) == Some(2) {
            break t;
        }
        assert!(Instant::now() < deadline, "工具未同步：{t}");
        std::thread::sleep(Duration::from_millis(20));
    };
    let by_name = |n: &str| tools.as_array().and_then(|a| a.iter().find(|t| t["name"] == n)).cloned();
    let checkout = by_name("shop.cart.checkout").expect("view 工具");
    assert_eq!((&checkout["surface"], &checkout["page"]), (&json!("view"), &json!("cart")), "{tools}");
    let submit = by_name("shop.order.submit").expect("app 工具");
    assert_eq!(submit["surface"], "app", "{tools}");
    assert!(submit.get("page").is_none(), "无页面时不出现：{tools}");

    // SAFETY: 有效参数（filter 为 NULL = 全部，含内置工具）。
    let all = query_json(|o| unsafe { am_hub_tools_json(hub, ptr::null(), o) });
    let names: Vec<&str> = all.as_array().into_iter().flatten().filter_map(|t| t["name"].as_str()).collect();
    for n in ["apps.list", "apps.select", "apps.overview", "apps.activate", "apps.release", "apps.page", "apps.navigate"] {
        assert!(names.contains(&n), "缺少内置工具 {n}：{names:?}");
    }
    for t in all.as_array().into_iter().flatten().filter(|t| t["name"].as_str().is_some_and(|n| n.starts_with("apps."))) {
        assert!(t.get("surface").is_none() && t.get("page").is_none(), "内置工具不带 surface / page：{t}");
    }

    let (tx, rx) = mpsc::channel();
    call(hub, json!({"name":"shop.order.submit","idempotencyKey":"order-7"}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["ok"]["key"], "order-7", "{o}");
    assert!(o.get("routedTo").is_none(), "未改调时不出现：{o}");
    // v14：调用元信息（App 已连接，不经过唤醒）
    assert!(o["durationMs"].is_u64() && o["woke"] == false, "{o}");
    call(hub, json!({"name":"shop.order.submit"}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["ok"]["key"], Value::Null, "{o}");
    call(hub, json!({"name":"shop.order.submit","idempotencyKey":""}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["error"]["kind"], "INVALID_INPUT", "{o}");

    client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}
