//! 一致性用例 runner（Rust native）：按 `conformance/cases/*.json` 的 `app` 部分注册工具与资源，连接 fake_host
//! （`--case` 模式，核对在 fake_host 内完成），汇总各用例结论。格式与约定见 conformance/README.md。
//!
//! 只跑部分用例：`APP_MCP_CONFORMANCE_CASES=handshake,errors cargo test -p app-mcp-native --test conformance`。

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use app_mcp_native::{
    BusyPolicy, CallDedupPolicy, CallHandle, CallResult, ErrorKind, EventInfo, NativeClient, NativeConfig, NavigateHandle, NavigationHandler,
    ReadHandle, ResourceOptions, ResourceReader, ResourceSpec, ToolHandle, ToolHandler, ToolOptions, ToolSpec,
    Visibility,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[path = "../src/test_support.rs"]
mod test_support;

const SDK: &str = "rust";
/// 本 runner 支持的用例能力（`requires`），见 conformance/README.md。
const FEATURES: &[&str] = &[
    "toolOptions", "mutate", "lifecycle", "wake", "richResult", "userAction", "progress", "resourceOptions",
    "readFailure", "surface", "navigation", "backgroundTool", "backgroundNavigation", "idempotencyKey", "callScheduling",
    "busy", "events",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// JSON 字段 → SDK 类型（枚举、注解等都有 serde 定义，与协议同名）。
fn parse<T: DeserializeOwned>(v: &Value) -> Option<T> {
    (!v.is_null()).then(|| serde_json::from_value(v.clone()).ok()).flatten()
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

/// 工具声明（用例 JSON）→ `ToolSpec` + `ToolOptions`。
fn tool_spec(decl: &Value) -> (ToolSpec, ToolOptions) {
    let mut spec = ToolSpec::new(decl["name"].as_str().unwrap_or_default(), decl["description"].as_str().unwrap_or_default());
    spec.input_schema_json = (!decl["inputSchema"].is_null()).then(|| decl["inputSchema"].to_string());
    spec.risk = parse(&decl["risk"]).unwrap_or_default();
    spec.activation = parse(&decl["activation"]);
    spec.title = text(&decl["title"]);
    spec.enabled = decl["enabled"].as_bool().unwrap_or(true);
    let options = ToolOptions {
        annotations: parse(&decl["annotations"]),
        output_schema_json: (!decl["outputSchema"].is_null()).then(|| decl["outputSchema"].to_string()),
        surface: parse(&decl["surface"]).unwrap_or_default(),
        page: text(&decl["page"]),
        background_tool: text(&decl["backgroundTool"]),
        implements: parse(&decl["implements"]).unwrap_or_default(),
        concurrency: decl["concurrency"].as_u64().map_or(0, |n| n as u32),
        exclusive: text(&decl["exclusive"]),
    };
    (spec, options)
}

/// 一个用例的 App：客户端与已注册工具（mutate 用）。
struct App {
    client: NativeClient,
    tools: Mutex<HashMap<String, (ToolHandle, Value)>>,
}

impl App {
    fn register_tool(self: &Arc<Self>, decl: &Value) {
        let (spec, options) = tool_spec(decl);
        let handler = Arc::new(CaseTool { app: Arc::downgrade(self), spec: decl["handler"].clone(), runs: AtomicU64::new(0) });
        let name = spec.name.clone();
        let handle = self.client.register_tool_with(spec, options, handler).expect("注册工具");
        self.tools.lock().unwrap().insert(name, (handle, decl.clone()));
    }

    fn register_resource(&self, decl: &Value) {
        let spec = ResourceSpec {
            name: decl["name"].as_str().unwrap_or_default().to_owned(),
            description: decl["description"].as_str().unwrap_or_default().to_owned(),
            mime_type: text(&decl["mimeType"]),
        };
        let options = ResourceOptions { realtime: decl["realtime"].as_bool().unwrap_or(false), annotations: parse(&decl["annotations"]) };
        let reader = Arc::new(CaseResource(decl["read"].clone()));
        self.client.register_resource_with(spec, options, reader).expect("注册资源");
    }

    /// 事件声明（conformance/README.md 2.2）。
    fn declare_event(&self, decl: &Value) {
        let info: EventInfo = serde_json::from_value(decl.clone()).expect("事件声明");
        self.client.declare_event(info).expect("声明事件");
    }

    /// handler 的 `emit`：每项的结果为 `true` / `false`（已发送 / 未连接丢弃），本地错误为 `"error"`。
    fn emit_events(&self, list: &[Value]) -> Value {
        let outcomes = list.iter().map(|e| {
            let payload = e.get("payload").map(Value::to_string);
            match self.client.emit_event(e["name"].as_str().unwrap_or_default(), payload.as_deref()) {
                Ok(sent) => json!(sent),
                Err(_) => json!("error"),
            }
        });
        Value::Array(outcomes.collect())
    }

    /// handler 的 `mutate` 操作（conformance/README.md）。
    fn mutate(self: &Arc<Self>, op: &Value) {
        let name = op["name"].as_str().unwrap_or_default();
        match op["op"].as_str().unwrap_or_default() {
            "register" => self.register_tool(&op["tool"]),
            "update" => {
                let mut tools = self.tools.lock().unwrap();
                let Some((handle, decl)) = tools.get_mut(name) else { panic!("update 未知工具 {name}") };
                let Some(fields) = decl.as_object_mut() else { panic!("工具声明应为对象") };
                for (k, v) in op["set"].as_object().into_iter().flatten() {
                    match v {
                        Value::Null => fields.remove(k),
                        _ => fields.insert(k.clone(), v.clone()),
                    };
                }
                let (spec, options) = tool_spec(decl);
                handle.update_with(spec, options).expect("更新工具");
            }
            "remove" => {
                if let Some((handle, _)) = self.tools.lock().unwrap().remove(name) {
                    handle.dispose();
                }
            }
            "busy" => self.client.set_busy(op["value"].as_bool().expect("busy 的 value 应为布尔")),
            "declareEvent" => self.declare_event(&op["event"]),
            "removeEvent" => {
                self.client.remove_event(name);
            }
            op_name @ ("enable" | "disable") => {
                let tools = self.tools.lock().unwrap();
                tools[name].0.set_enabled(op_name == "enable").expect("启用 / 禁用");
            }
            other => panic!("未知的 mutate 操作 {other}"),
        }
    }
}

/// `app.navigation`（conformance/README.md 2.4）：按页面名查行为；未列出的页面以失败回复。
struct CaseNavigation {
    app: Weak<App>,
    pages: Value,
}

impl NavigationHandler for CaseNavigation {
    fn navigate(&self, request: NavigateHandle) {
        let page = request.page();
        let Some(spec) = self.pages.get(&page).cloned() else {
            let _ = request.fail(&format!("未知页面：{page}"));
            return;
        };
        if let Some(msg) = spec["throw"].as_str() {
            panic!("{msg}");
        }
        let app = self.app.clone();
        std::thread::spawn(move || {
            if let (Some(ops), Some(app)) = (spec["mutate"].as_array(), app.upgrade()) {
                for op in ops {
                    app.mutate(op);
                }
            }
            let _ = if let Some(msg) = spec["deny"].as_str() {
                request.deny(msg)
            } else if let Some(msg) = spec["fail"].as_str() {
                request.fail(msg)
            } else if let Some(u) = spec["userAction"].as_object() {
                let text = |k: &str| u.get(k).and_then(Value::as_str);
                request.fail_user_action(text("message").unwrap_or_default(), text("reason"), text("uri"))
            } else if spec["failParams"].as_bool() == Some(true) {
                request.fail(&request.params_json().unwrap_or_default())
            } else {
                request.complete()
            };
        });
    }
}

struct CaseTool {
    app: Weak<App>,
    spec: Value,
    runs: AtomicU64,
}

impl ToolHandler for CaseTool {
    fn invoke(&self, call: CallHandle) {
        let spec = self.spec.clone();
        let count = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        let app = self.app.clone();
        std::thread::spawn(move || run_handler(&spec, count, app, call));
    }
}

/// 按 handler 描述执行（顺序：progress → delayMs → mutate → emit → 结果，见 conformance/README.md）。
fn run_handler(spec: &Value, count: u64, app: Weak<App>, call: CallHandle) {
    for p in spec["progress"].as_array().into_iter().flatten() {
        let _ = call.report_progress(p["progress"].as_f64().unwrap_or(0.0), p["total"].as_f64(), p["message"].as_str());
    }
    if let Some(ms) = spec["delayMs"].as_u64() {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until && !call.is_cancelled() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    if let (Some(ops), Some(app)) = (spec["mutate"].as_array(), app.upgrade()) {
        for op in ops {
            app.mutate(op);
        }
    }
    let emitted = match (spec["emit"].as_array(), app.upgrade()) {
        (Some(list), Some(app)) => Some(app.emit_events(list)),
        _ => None,
    };
    // 调用已被取消 / 超时：完成会返回 AlreadyCompleted，忽略。
    let _ = complete(spec, count, emitted, &call);
}

fn complete(spec: &Value, count: u64, emitted: Option<Value>, call: &CallHandle) -> Result<(), app_mcp_native::NativeError> {
    if let Some(msg) = spec["throw"].as_str() {
        return call.fail(ErrorKind::HandlerError, msg);
    }
    if !spec["userAction"].is_null() {
        let u = &spec["userAction"];
        return call.fail_user_action(u["message"].as_str().unwrap_or_default(), u["reason"].as_str(), u["uri"].as_str());
    }
    if let Some(r) = spec["result"].as_object() {
        let r = Value::Object(r.clone());
        return call.complete_with(CallResult {
            data_json: (!r["data"].is_null()).then(|| r["data"].to_string()),
            state_hints: parse(&r["stateHints"]).unwrap_or_default(),
            status: parse(&r["status"]).unwrap_or_default(),
            state_resource: text(&r["stateResource"]),
            summary: text(&r["summary"]),
            annotations: parse(&r["annotations"]),
        });
    }
    if let Some(v) = spec.get("return") {
        return call.complete(Some(&v.to_string()), vec![]);
    }
    if spec["echo"].as_bool() == Some(true) {
        return call.complete(Some(&call.arguments_json()), vec![]);
    }
    if spec["returnIdempotencyKey"].as_bool() == Some(true) {
        return call.complete(Some(&json!({ "idempotencyKey": call.idempotency_key() }).to_string()), vec![]);
    }
    if spec["counter"].as_bool() == Some(true) {
        return call.complete(Some(&json!({ "count": count }).to_string()), vec![]);
    }
    if let Some(emitted) = emitted {
        return call.complete(Some(&json!({ "emitted": emitted }).to_string()), vec![]);
    }
    // returnNothing（以及未声明结果）：Rust 的"无返回值"即 data_json = None。
    call.complete(None, vec![])
}

struct CaseResource(Value);

impl ResourceReader for CaseResource {
    fn read(&self, read: ReadHandle) {
        let spec = &self.0;
        let _ = if let Some(v) = spec.get("return") {
            read.complete(&v.to_string())
        } else if !spec["fail"].is_null() {
            let f = &spec["fail"];
            let kind = parse(&f["kind"]).unwrap_or(ErrorKind::HandlerError);
            let details = (!f["details"].is_null()).then(|| f["details"].to_string());
            read.fail_with_details(kind, f["message"].as_str().unwrap_or_default(), details.as_deref())
        } else if !spec["userAction"].is_null() {
            let u = &spec["userAction"];
            read.fail_user_action(u["message"].as_str().unwrap_or_default(), u["reason"].as_str(), u["uri"].as_str())
        } else {
            read.fail(ErrorKind::HandlerError, spec["throw"].as_str().unwrap_or("读取失败"))
        };
    }
}

fn config(addr: &str, case: &Value) -> NativeConfig {
    let mut cfg = NativeConfig::new("conf", "Conformance");
    cfg.host_url = if addr.contains(':') && !addr.starts_with("unix:") && !addr.starts_with("pipe:") {
        format!("ws://{addr}/app")
    } else {
        addr.to_owned()
    };
    let c = &case["app"]["config"];
    let l = &c["lifecycle"];
    if let Some(mode) = parse(&l["mode"]) {
        cfg.lifecycle.mode = mode;
    }
    if let Some(ms) = l["idleTimeoutMs"].as_u64() {
        cfg.lifecycle.idle_timeout_ms = ms;
    }
    if let Some(ms) = l["graceMs"].as_u64() {
        cfg.lifecycle.grace_ms = ms;
    }
    if let Some(ms) = l["mergeWindowMs"].as_u64() {
        cfg.lifecycle.merge_window_ms = ms;
    }
    if let Some(d) = c["callDedup"].as_object() {
        cfg.call_dedup = CallDedupPolicy {
            ttl_ms: d.get("ttlMs").and_then(Value::as_u64).unwrap_or(cfg.call_dedup.ttl_ms),
            max_entries: d.get("maxEntries").and_then(Value::as_u64).map_or(cfg.call_dedup.max_entries, |n| n as usize),
        };
    }
    if let Some(n) = c["maxConcurrentCalls"].as_u64() {
        cfg.max_concurrent_calls = n as u32;
    }
    if let Some(n) = c["maxQueuedCalls"].as_u64() {
        cfg.max_queued_calls = n as u32;
    }
    match c["busyPolicy"].as_str() {
        Some("queue") => cfg.busy_policy = BusyPolicy::Queue,
        Some("reject") => cfg.busy_policy = BusyPolicy::Reject,
        Some(other) => panic!("未知的 busyPolicy {other}"),
        None => {}
    }
    cfg
}

/// 跑一个用例，返回 fake_host 给出的结论行。
fn run_case(bin: &Path, path: &Path, report_dir: &Path) -> Value {
    let case: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let missing: Vec<&str> = case["requires"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|f| !FEATURES.contains(f))
        .collect();
    let mut cmd = Command::new(bin);
    cmd.arg("--case").arg(path).args(["--sdk", SDK]).arg("--report-dir").arg(report_dir);
    if !missing.is_empty() {
        cmd.args(["--skip", &format!("runner 不支持：{}", missing.join(", "))]);
    }
    let mut child = cmd.stdout(Stdio::piped()).spawn().expect("启动 fake_host");
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut app: Option<Arc<App>> = None;
    let mut verdict = Value::Null;
    for line in lines.by_ref() {
        let line = line.unwrap();
        if let Some(addr) = line.strip_prefix("LISTENING ") {
            let client = NativeClient::new(config(addr, &case), None).expect("创建客户端");
            let a = Arc::new(App { client, tools: Mutex::new(HashMap::new()) });
            for t in case["app"]["tools"].as_array().into_iter().flatten() {
                a.register_tool(t);
            }
            for r in case["app"]["resources"].as_array().into_iter().flatten() {
                a.register_resource(r);
            }
            for e in case["app"]["events"].as_array().into_iter().flatten() {
                a.declare_event(e);
            }
            if case["app"]["navigation"].is_object() {
                let nav = CaseNavigation { app: Arc::downgrade(&a), pages: case["app"]["navigation"].clone() };
                a.client.set_navigation_handler(Some(Arc::new(nav)));
            }
            if let Some(b) = case["app"]["config"]["navigateInBackground"].as_bool() {
                a.client.set_navigate_in_background(b);
            }
            if let Some(b) = case["app"]["busy"].as_bool() {
                a.client.set_busy(b);
            }
            if let Some(v) = parse::<Visibility>(&case["app"]["visibility"]) {
                a.client.set_visibility(v, false);
            }
            a.client.start();
            app = Some(a);
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str() {
            Some("wake") => {
                if let Some(a) = &app {
                    a.client.handle_wake(v["arg"].as_str().unwrap_or_default());
                }
            }
            Some("verdict") => verdict = v,
            _ => {}
        }
    }
    let _ = child.wait();
    if let Some(a) = app {
        a.client.stop();
    }
    verdict
}

#[test]
fn conformance_cases() {
    let bin = test_support::fake_host_path().unwrap_or_else(|e| panic!("{e}"));
    let root = repo_root();
    let report_dir = root.join("target/conformance");
    let only: Option<Vec<String>> = std::env::var("APP_MCP_CONFORMANCE_CASES")
        .ok()
        .map(|s| s.split(',').map(str::to_owned).collect());
    let mut cases: Vec<PathBuf> = std::fs::read_dir(root.join("conformance/cases"))
        .expect("conformance/cases")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter(|p| {
            let id = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            only.as_ref().is_none_or(|o| o.iter().any(|x| x == id))
        })
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "没有找到用例");
    let mut failed = Vec::new();
    for path in &cases {
        let v = run_case(&bin, path, &report_dir);
        let status = v["status"].as_str().unwrap_or("error");
        eprintln!("[{SDK}] {:<24} {status}", path.file_stem().unwrap().to_string_lossy());
        if !matches!(status, "pass" | "xfail" | "xpass" | "skip") {
            failed.push(format!("{}: {}", path.display(), v["failures"]));
        }
    }
    assert!(failed.is_empty(), "一致性用例失败：\n{}", failed.join("\n"));
}
