//! 集成测试：`app-mcp-host doctor` 与 `app-mcp-host status`（真实进程，临时配置目录）。
//!
//! serve 的设置写在临时配置目录的 `config.json`（监听端口 0、临时 IPC 端点），doctor / status 读同一配置目录。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicUsize, Ordering};

use app_mcp_native::{CachePolicy, CallHandle, NativeClient, NativeConfig, ToolAnnotations, ToolHandler, ToolOptions, ToolSpec};
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use app_mcp_protocol::registry::{EndpointRegistry, REGISTRY_FILE, RUN_DIR};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_app-mcp-host");
const T: Duration = Duration::from_secs(15);

struct TempHome(PathBuf);
impl TempHome {
    fn new(tag: &str) -> Self {
        Self::with_config(tag, json!({}))
    }

    /// `extra`：合并进 `config.json` 的顶层字段（如 `resultCache`）。
    fn with_config(tag: &str, extra: Value) -> Self {
        let n: u64 = rand::random();
        let dir = std::env::temp_dir().join(format!("app-mcp-doctor-{tag}-{}-{n:x}", std::process::id()));
        std::fs::create_dir_all(dir.join("manifests")).unwrap();
        let ipc = ipc_endpoint(&dir);
        let mut config = json!({ "listen": "127.0.0.1:0", "ipcEndpoint": ipc, "log": { "file": false } });
        for (k, v) in extra.as_object().expect("extra 为对象") {
            config[k] = v.clone();
        }
        std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
        Self(dir)
    }
}
impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ipc_endpoint(home: &Path) -> String {
    #[cfg(unix)]
    {
        format!("unix:{}", home.join("run").join("hub.sock").display())
    }
    #[cfg(windows)]
    {
        let name = home.file_name().unwrap().to_string_lossy();
        format!(r"pipe:\\.\pipe\{name}")
    }
}

fn cmd(home: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(BIN);
    c.args(args).arg("--home").arg(home).env_remove("APP_MCP_HOME").stdin(Stdio::null());
    c
}

struct Serve(Child);
impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn start_serve(home: &TempHome) -> (Serve, EndpointRegistry) {
    let child = cmd(&home.0, &["serve"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("serve");
    let pid = child.id();
    let serve = Serve(child);
    let path = home.0.join(RUN_DIR).join(REGISTRY_FILE);
    let deadline = Instant::now() + T;
    loop {
        if let Ok(Some(reg)) = EndpointRegistry::read(&path)
            && reg.identity.pid == pid
        {
            return (serve, reg);
        }
        assert!(Instant::now() < deadline, "serve 未写出登记文件");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

struct Add;
impl ToolHandler for Add {
    fn invoke(&self, call: CallHandle) {
        let _ = call.complete(Some("{}"), vec![]);
    }
}

fn run(c: &mut Command) -> (i32, String) {
    let out = c.output().expect("run");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned())
}

fn doctor_json(home: &Path) -> (i32, Value) {
    let (code, out) = run(&mut cmd(home, &["doctor", "--json"]));
    (code, serde_json::from_str(&out).unwrap_or_else(|e| panic!("doctor --json 输出无法解析：{e}\n{out}")))
}

fn check<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("缺少检查 {id}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_and_status_with_running_host() {
    let home = TempHome::new("run");
    let (serve, reg) = start_serve(&home).await;

    let mut c = NativeConfig::new("calc", "计算器");
    c.host_url = reg.ipc_endpoint.clone().expect("ipc");
    c.instance_id = Some("calc-1".into());
    c.launch_token = Some(String::new());
    let app = NativeClient::new(c, None).unwrap();
    app.register_tool(ToolSpec::new("noop", "无操作"), Arc::new(Add)).unwrap();
    app.start();

    // 等 App 出现在 status 中
    let deadline = Instant::now() + T;
    let line = loop {
        let (code, out) = run(&mut cmd(&home.0, &["status"]));
        assert_eq!(code, 0, "{out}");
        if out.contains("App 在线 1") {
            break out;
        }
        assert!(Instant::now() < deadline, "status 未显示在线 App：{out}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(line.trim().lines().count(), 1, "{line}");
    assert!(line.contains(&format!("pid {}", reg.identity.pid)), "{line}");
    assert!(line.contains("listen 流 0 个、Agent 任务 0 个"), "{line}");
    assert!(line.contains("结果缓存 0 条（0.0 KiB），命中 0、未命中 0、淘汰 0"), "第 16 项 O3：{line}");

    let (code, report) = doctor_json(&home.0);
    assert_eq!(check(&report, "host")["status"], "ok", "{report:#}");
    assert_eq!(check(&report, "ipc")["status"], "ok", "{report:#}");
    assert_eq!(check(&report, "ports")["status"], "ok", "{report:#}");
    assert_eq!(check(&report, "run_dir")["status"], "ok", "{report:#}");
    assert_eq!(check(&report, "cache")["status"], "ok", "没有可缓存的请求：{report:#}");
    assert_eq!(check(&report, "cache")["details"]["cache"]["entries"], 0, "{report:#}");
    #[cfg(target_os = "linux")]
    assert_eq!(check(&report, "lock")["status"], "ok", "{report:#}");
    let apps = check(&report, "apps");
    assert_eq!(apps["status"], "ok", "{report:#}");
    let calc = apps["details"].as_array().unwrap().iter().find(|a| a["appId"] == "calc").expect("calc");
    assert_eq!(calc["state"], "connected");
    assert!(calc["instances"][0]["connectionId"].is_string());
    assert_eq!(check(&report, "reports")["status"], "ok");
    assert_eq!(code, 0, "{report:#}");

    // 人类可读输出
    let (_, text) = run(&mut cmd(&home.0, &["doctor"]));
    assert!(text.contains("结论：") && text.contains("Host 运行状态"), "{text}");

    app.stop();
    drop(serve);
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_and_status_without_host() {
    let home = TempHome::new("none");
    let (code, out) = run(&mut cmd(&home.0, &["status"]));
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("未运行"), "{out}");

    let (code, report) = doctor_json(&home.0);
    assert_eq!(code, 1);
    let host = check(&report, "host");
    assert_eq!(host["status"], "error");
    assert_eq!(host["code"], "HOST_NOT_RUNNING");
    assert!(host["hint"].as_str().unwrap().contains("app-mcp-host serve"));
    assert_eq!(check(&report, "apps")["status"], "skip");
}

// ---------------------------------------------------------------------------
// 结果缓存上限（第 16 项 O3 二期，配置 `resultCache`）
// ---------------------------------------------------------------------------

/// 只读、声明了 `cache` 的工具；记录实际执行次数。
struct Now(Arc<AtomicUsize>);
impl ToolHandler for Now {
    fn invoke(&self, call: CallHandle) {
        let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = call.complete(Some(&json!({ "n": n }).to_string()), vec![]);
    }
}

/// 以 `extra` 配置启动 serve，同一 MCP 会话以相同参数调用缓存工具两次；返回（App 实际执行次数，doctor 的 cache 检查）。
async fn call_cached_twice(tag: &str, extra: Value) -> (usize, Value) {
    let home = TempHome::with_config(tag, extra);
    let (serve, reg) = start_serve(&home).await;
    let mut c = NativeConfig::new("clock", "时钟");
    c.host_url = reg.ipc_endpoint.clone().expect("ipc");
    c.launch_token = Some(String::new());
    let app = NativeClient::new(c, None).unwrap();
    let options = ToolOptions {
        annotations: Some(ToolAnnotations { read_only_hint: Some(true), ..Default::default() }),
        cache: Some(CachePolicy { ttl_ms: 60_000, scope: Default::default() }),
        ..Default::default()
    };
    let invoked = Arc::new(AtomicUsize::new(0));
    app.register_tool_with(ToolSpec::new("now", "当前读数"), options, Arc::new(Now(invoked.clone()))).unwrap();
    app.start();

    let addr = reg.listen.clone().expect("listen");
    let mcp = ().serve(crate::support::mcp_http::transport(format!("http://{addr}/mcp"), None)).await.expect("MCP");
    let deadline = Instant::now() + T;
    while !mcp.list_all_tools().await.unwrap().iter().any(|t| t.name == "clock.now") {
        assert!(Instant::now() < deadline, "没有等到工具 clock.now");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for _ in 0..2 {
        let r = mcp.call_tool(CallToolRequestParams::new("clock.now")).await.unwrap();
        assert_ne!(r.is_error, Some(true), "{r:?}");
    }
    let (_, report) = doctor_json(&home.0);
    let cache = check(&report, "cache").clone();
    let _ = mcp.cancel().await;
    app.stop();
    drop(serve);
    (invoked.load(Ordering::SeqCst), cache)
}

#[tokio::test(flavor = "multi_thread")]
async fn result_cache_limits_from_config() {
    // 缺省上限：第二次命中，App 只执行一次；doctor 显示生效上限
    let (invoked, cache) = call_cached_twice("cache-default", json!({})).await;
    assert_eq!(invoked, 1, "{cache:#}");
    assert_eq!(cache["details"]["cache"]["hits"], 1, "{cache:#}");
    assert_eq!(cache["details"]["limits"], json!({"maxEntries": 1024, "maxBytes": 8_388_608, "maxEntryBytes": 65_536}));
    assert!(cache["summary"].as_str().unwrap().contains("上限 1024 条、8.0 MiB、单条 64.0 KiB"), "{cache:#}");

    // maxEntries 0：关闭，不查不存
    let (invoked, cache) = call_cached_twice("cache-off", json!({"resultCache": {"maxEntries": 0}})).await;
    assert_eq!(invoked, 2, "{cache:#}");
    assert_eq!((cache["details"]["cache"]["hits"].as_u64(), cache["details"]["cache"]["misses"].as_u64()), (Some(0), Some(0)));
    assert!(cache["summary"].as_str().unwrap().starts_with("已关闭"), "{cache:#}");

    // 单条上限小于结果：不存，两次都未命中
    let (invoked, cache) = call_cached_twice("cache-entry", json!({"resultCache": {"maxEntryBytes": 1}})).await;
    assert_eq!(invoked, 2, "{cache:#}");
    assert_eq!(cache["details"]["cache"]["misses"], 2, "{cache:#}");
    assert_eq!(cache["details"]["limits"]["maxEntryBytes"], 1);
}

#[test]
fn invalid_result_cache_config_fails_to_start() {
    let home = TempHome::with_config("cache-bad", json!({"resultCache": {"maxBytes": 100, "maxEntryBytes": 200}}));
    let (code, _, err) = crate::serve::run_to_exit(cmd(&home.0, &["serve"]));
    assert_ne!(code, 0, "{err}");
    assert!(err.contains("resultCache.maxEntryBytes（200）不能大于 resultCache.maxBytes（100）"), "{err}");
    let home = TempHome::with_config("cache-typo", json!({"resultCache": {"maxEntry": 1}}));
    let (code, _, err) = crate::serve::run_to_exit(cmd(&home.0, &["serve"]));
    assert_ne!(code, 0, "{err}");
    assert!(err.contains("maxEntry"), "未知字段指出名字：{err}");
}
