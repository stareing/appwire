//! 集成测试：`app-mcp-host doctor` 与 `app-mcp-host status`（真实进程，临时配置目录）。
//!
//! serve 的设置写在临时配置目录的 `config.json`（监听端口 0、临时 IPC 端点），doctor / status 读同一配置目录。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_mcp_native::{CallHandle, NativeClient, NativeConfig, ToolHandler, ToolSpec};
use app_mcp_protocol::registry::{EndpointRegistry, REGISTRY_FILE, RUN_DIR};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_app-mcp-host");
const T: Duration = Duration::from_secs(15);

struct TempHome(PathBuf);
impl TempHome {
    fn new(tag: &str) -> Self {
        let n: u64 = rand::random();
        let dir = std::env::temp_dir().join(format!("app-mcp-doctor-{tag}-{}-{n:x}", std::process::id()));
        std::fs::create_dir_all(dir.join("manifests")).unwrap();
        let ipc = ipc_endpoint(&dir);
        let config = json!({ "listen": "127.0.0.1:0", "ipcEndpoint": ipc, "log": { "file": false } });
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

    let (code, report) = doctor_json(&home.0);
    assert_eq!(check(&report, "host")["status"], "ok", "{report:#}");
    assert_eq!(check(&report, "ipc")["status"], "ok", "{report:#}");
    assert_eq!(check(&report, "ports")["status"], "ok", "{report:#}");
    assert_eq!(check(&report, "run_dir")["status"], "ok", "{report:#}");
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
