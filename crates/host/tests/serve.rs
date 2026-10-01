//! 集成测试：`app-mcp-host serve` 常驻模式（真实进程）。
//!
//! - 一个 serve 进程，两个独立的 MCP HTTP 会话同时列出并调用同一 App（app-mcp-native 客户端）的工具，
//!   各自首次调用附带总览；
//! - 单实例：同一配置目录的第二个 serve（单实例锁）打印已运行实例并退出码 0；stdio 模式报错并给出 MCP 地址；
//!   端口被其他程序 / 其他配置目录的 Host 占用时报错并说明占用者；
//! - 合并端口：同一端口上的 `/app`、`/mcp`、`/healthz`；本地访问令牌：带 Origin 无令牌 → 401，
//!   Origin 不在允许列表 → 403。
//!
//! 每个测试用临时配置目录、临时 IPC 端点，监听端口 0；实际地址从登记文件 `<home>/run/endpoints.json` 读取
//! （不用"先绑定再释放"的端口，消除端口竞争）。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_mcp_host::probe::{Probe, probe};
use app_mcp_native::{AppOverview, CallHandle, NativeClient, NativeConfig, ToolHandler, ToolSpec};
use app_mcp_protocol::registry::{EndpointRegistry, REGISTRY_FILE, RUN_DIR};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_app-mcp-host");
const T: Duration = Duration::from_secs(15);

struct TempHome(PathBuf);
impl TempHome {
    fn new(tag: &str) -> Self {
        let n: u64 = rand::random();
        let dir = std::env::temp_dir().join(format!("app-mcp-serve-{tag}-{}-{n:x}", std::process::id()));
        std::fs::create_dir_all(dir.join("manifests")).unwrap();
        Self(dir)
    }
    fn registry(&self) -> PathBuf {
        self.0.join(RUN_DIR).join(REGISTRY_FILE)
    }
}
impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 每个临时配置目录一个本地 IPC 端点（不占用本机常驻 Host 的默认端点）。
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

/// `listen`：监听地址；测试一律用端口 0（或被占用的端口），实际地址读登记文件。
fn serve_cmd(home: &Path, listen: &str, extra: &[&str]) -> Command {
    let mut c = Command::new(BIN);
    c.arg("serve")
        .arg("--home")
        .arg(home)
        .args(["--listen", listen])
        .args(["--ipc-endpoint", &ipc_endpoint(home)])
        .args(extra)
        .env_remove("APP_MCP_HOME")
        .env("RUST_LOG", "info")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

/// 运行中的 serve 进程；Drop 时结束。
struct Serve {
    child: Child,
    /// 实际监听地址（`/app`、`/mcp`、`/healthz`）。
    addr: SocketAddr,
    ipc: String,
}
impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn early_exit(child: &mut Child) -> Option<String> {
    let status = child.try_wait().ok().flatten()?;
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    Some(format!("serve 提前退出（{status}）：{err}"))
}

/// 等 `cmd` 启动的 serve 写出登记文件（进程号一致），返回实际地址。
async fn wait_registered(home: &TempHome, child: &mut Child) -> EndpointRegistry {
    let deadline = Instant::now() + T;
    loop {
        if let Ok(Some(reg)) = EndpointRegistry::read(&home.registry())
            && reg.identity.pid == child.id()
        {
            return reg;
        }
        if let Some(msg) = early_exit(child) {
            panic!("{msg}");
        }
        assert!(Instant::now() < deadline, "serve 未写出登记文件");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn start_serve(home: &TempHome, extra: &[&str]) -> Serve {
    let mut child = serve_cmd(&home.0, "127.0.0.1:0", extra).spawn().expect("启动 serve");
    let reg = wait_registered(home, &mut child).await;
    let addr: SocketAddr = reg.listen.as_deref().expect("listen").parse().unwrap();
    assert_ne!(addr.port(), 0);
    assert_eq!(reg.ipc_endpoint.as_deref(), Some(ipc_endpoint(&home.0).as_str()));
    // 登记文件写在绑定之后：/healthz 立即可用，且是同一个进程
    let Probe::AppMcp(h) = probe(&addr.to_string()).await else {
        panic!("登记后 /healthz 不可用");
    };
    assert_eq!(h.identity, reg.identity);
    assert_eq!(h.listen.as_deref(), Some(addr.to_string().as_str()));
    assert_eq!(h.mcp_path.as_deref(), Some("/mcp"));
    Serve { child, addr, ipc: ipc_endpoint(&home.0) }
}

/// 同步运行一个 serve，等它退出，返回（退出码，stdout，stderr）。
fn run_to_exit(mut cmd: Command) -> (i32, String, String) {
    let out = cmd.output().expect("运行 serve");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// 发一个原始 HTTP 请求，返回状态码。
fn http_status(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> u16 {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    s.write_all(req.as_bytes()).unwrap();
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).unwrap();
    let line = String::from_utf8_lossy(&buf[..n]);
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

// ---------------------------------------------------------------------------
// App：app-mcp-native 客户端
// ---------------------------------------------------------------------------

struct Add;
impl ToolHandler for Add {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or(Value::Null);
        let sum = args["a"].as_i64().unwrap_or(0) + args["b"].as_i64().unwrap_or(0);
        std::thread::spawn(move || {
            let _ = call.complete(Some(&json!({ "sum": sum }).to_string()), vec![]);
        });
    }
}

/// `endpoint`：`ws://…` 或本地 IPC 端点。
fn calc_app(endpoint: &str) -> NativeClient {
    let mut c = NativeConfig::new("calc", "计算器");
    c.host_url = endpoint.to_owned();
    c.launch_token = Some(String::new());
    c.overview = Some(AppOverview {
        summary: "做加法的计算器".into(),
        body: Some("## 能力\n两个整数相加。".into()),
        locale: None,
    });
    let client = NativeClient::new(c, None).unwrap();
    let mut spec = ToolSpec::new("math.add", "加法");
    spec.input_schema_json = Some(
        r#"{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}}}"#.into(),
    );
    client.register_tool(spec, Arc::new(Add)).unwrap();
    client.start();
    client
}

type Client = RunningService<RoleClient, ()>;

async fn mcp(addr: SocketAddr, token: Option<&str>) -> Client {
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    let mut cfg = StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"));
    if let Some(t) = token {
        cfg = cfg.auth_header(t.to_owned());
    }
    ().serve(StreamableHttpClientTransport::from_config(cfg))
        .await
        .expect("MCP initialize")
}

async fn wait_tool(client: &Client, name: &str) {
    let deadline = Instant::now() + T;
    loop {
        let tools = client.list_all_tools().await.unwrap();
        if tools.iter().any(|t| t.name == name) {
            return;
        }
        assert!(Instant::now() < deadline, "没有等到工具 {name}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn add(client: &Client, a: i64, b: i64) -> CallToolResult {
    let args = json!({ "a": a, "b": b });
    client
        .call_tool(
            CallToolRequestParams::new("calc.math.add")
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap()
}

fn texts(r: &CallToolResult) -> Vec<String> {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

fn has_overview(r: &CallToolResult) -> bool {
    texts(r)
        .iter()
        .any(|t| t.contains("<app-overview app=\"calc\""))
}

// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn two_http_sessions_share_one_app() {
    let home = TempHome::new("share");
    let serve = start_serve(&home, &[]).await;
    // 原生 App 经本地 IPC 连接常驻 Host。
    let app = calc_app(&serve.ipc);

    let a = mcp(serve.addr, None).await;
    let b = mcp(serve.addr, None).await;
    // 总览简介出现在两个会话的 instructions 中（App 已连接后建立的会话）
    tokio::join!(
        wait_tool(&a, "calc.math.add"),
        wait_tool(&b, "calc.math.add")
    );

    // 并发调用：各自首次附带总览
    let (ra, rb) = tokio::join!(add(&a, 1, 2), add(&b, 10, 20));
    assert!(has_overview(&ra), "{:?}", texts(&ra));
    assert!(has_overview(&rb), "{:?}", texts(&rb));
    assert_eq!(ra.structured_content.as_ref().unwrap()["sum"], 3);
    assert_eq!(rb.structured_content.as_ref().unwrap()["sum"], 30);
    // 同一会话不再重复
    let ra2 = add(&a, 2, 2).await;
    assert!(!has_overview(&ra2), "{:?}", texts(&ra2));
    assert_eq!(ra2.structured_content.as_ref().unwrap()["sum"], 4);
    // 新会话再附带一次
    let c = mcp(serve.addr, None).await;
    let instructions = c
        .peer_info()
        .unwrap()
        .instructions
        .clone()
        .unwrap_or_default();
    assert!(instructions.contains("做加法的计算器"), "{instructions}");
    assert!(has_overview(&add(&c, 0, 0).await));

    // apps.list：一个 App、一个实例（三个会话共享连接）
    let list = a
        .call_tool(CallToolRequestParams::new("apps.list"))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let calc = list["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["appId"] == "calc")
        .unwrap()
        .clone();
    assert_eq!(calc["connected"], true);
    assert_eq!(calc["instances"].as_array().unwrap().len(), 1);

    // 一个会话关闭不影响另一个
    a.cancel().await.unwrap();
    assert_eq!(add(&b, 5, 5).await.structured_content.unwrap()["sum"], 10);
    app.stop();

    // 常驻模式写日志文件
    let log = home.0.join("logs").join("app-mcp-host.log");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("app-mcp-host 已就绪"), "{text}");
    drop(serve);
}

#[tokio::test(flavor = "multi_thread")]
async fn second_serve_exits_zero_when_instance_holds_lock() {
    let home = TempHome::new("single");
    let serve = start_serve(&home, &[]).await;
    // 同一配置目录：单实例锁先于任何监听，与端口无关（端口 0、或第一个实例的端口都一样）
    for listen in ["127.0.0.1:0".to_owned(), serve.addr.to_string()] {
        let (code, stdout, stderr) = run_to_exit(serve_cmd(&home.0, &listen, &["--no-log-file"]));
        assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
        assert!(stdout.contains("已在运行"), "{stdout}");
        assert!(stdout.contains(&format!("pid {}", serve.child.id())), "{stdout}");
        assert!(stdout.contains(&format!("http://{}/mcp", serve.addr)), "{stdout}");
    }
    // stdio 模式：不能与常驻实例共存，报错并给出 MCP 地址
    let out = Command::new(BIN)
        .args(["stdio", "--home"])
        .arg(&home.0)
        .args(["--listen", "127.0.0.1:0", "--ipc-endpoint", "none"])
        .env_remove("APP_MCP_HOME")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(&format!("http://{}/mcp", serve.addr)), "{stderr}");
    // 第一个实例不受影响
    assert!(matches!(probe(&serve.addr.to_string()).await, Probe::AppMcp(_)));
    assert!(home.registry().exists());
    drop(serve);
}

#[tokio::test(flavor = "multi_thread")]
async fn registry_is_removed_on_sigterm() {
    let home = TempHome::new("term");
    let serve = start_serve(&home, &["--no-log-file"]).await;
    #[cfg(unix)]
    {
        let mut serve = serve;
        // SIGTERM：正常停止，删除登记文件
        let pid = serve.child.id();
        assert!(Command::new("kill").arg(pid.to_string()).status().unwrap().success());
        let status = serve.child.wait().unwrap();
        assert!(status.success(), "{status}");
        assert!(!home.registry().exists(), "退出时删除登记文件");
        // 锁随进程释放：可以再次启动
        let again = start_serve(&home, &["--no-log-file"]).await;
        drop(again);
    }
    #[cfg(not(unix))]
    drop(serve);
}

#[tokio::test(flavor = "multi_thread")]
async fn port_taken_by_other_program_is_an_error() {
    let home = TempHome::new("taken");
    // 一个“其他程序”：接受连接并返回 404
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let other_addr = other.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for mut s in other.incoming().flatten() {
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        }
    });
    // 显式指定的端口被占用：不换端口，报错并说明占用者
    let (code, _, stderr) = run_to_exit(serve_cmd(&home.0, &other_addr, &["--no-log-file"]));
    assert_ne!(code, 0);
    assert!(stderr.contains("其他程序"), "{stderr}");
    assert!(!home.registry().exists());

    // 端口被另一个配置目录的 app-mcp Host 占用
    let first_home = TempHome::new("taken-first");
    let first = start_serve(&first_home, &["--no-log-file"]).await;
    let (code, _, stderr) = run_to_exit(serve_cmd(&home.0, &first.addr.to_string(), &["--no-log-file"]));
    assert_ne!(code, 0);
    assert!(stderr.contains("另一个 app-mcp Host"), "{stderr}");
    assert!(stderr.contains(&format!("pid {}", first.child.id())), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn healthz_and_token_policy() {
    let home = TempHome::new("token");
    let serve = start_serve(&home, &["--no-log-file"]).await;
    let token = std::fs::read_to_string(home.0.join("token"))
        .unwrap()
        .trim()
        .to_owned();
    assert_eq!(token.len(), 64);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.0.join("token"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
    let base = [
        ("Content-Type", "application/json"),
        ("Accept", "application/json, text/event-stream"),
    ];
    let with = |extra: &[(&'static str, String)]| {
        let mut h: Vec<(&str, String)> = base.iter().map(|(k, v)| (*k, v.to_string())).collect();
        h.extend(extra.iter().cloned());
        h
    };
    let status = |h: Vec<(&str, String)>| {
        let refs: Vec<(&str, &str)> = h.iter().map(|(k, v)| (*k, v.as_str())).collect();
        http_status(serve.addr, "POST", "/mcp", &refs, init)
    };
    let bearer = format!("Bearer {token}");

    // 浏览器来源（允许列表内）无令牌 → 401；错误令牌 → 401；正确令牌 → 200
    assert_eq!(
        status(with(&[("Origin", "http://localhost:5173".into())])),
        401
    );
    assert_eq!(
        status(with(&[
            ("Origin", "http://localhost:5173".into()),
            ("Authorization", "Bearer wrong".into())
        ])),
        401
    );
    assert_eq!(
        status(with(&[
            ("Origin", "http://localhost:5173".into()),
            ("Authorization", bearer.clone())
        ])),
        200
    );
    // 不在允许列表的来源 → 403（即使带令牌）
    assert_eq!(
        status(with(&[
            ("Origin", "https://evil.example".into()),
            ("Authorization", bearer.clone())
        ])),
        403
    );
    // 本地客户端（无 Origin）默认可不带令牌；带了错误令牌 → 401
    assert_eq!(status(with(&[])), 200);
    assert_eq!(
        status(with(&[("Authorization", "Bearer wrong".into())])),
        401
    );

    // /healthz 不需要令牌；同一端口的 /app 是 App 连接（非升级请求 426）
    assert_eq!(http_status(serve.addr, "GET", "/healthz", &[], ""), 200);
    assert_eq!(http_status(serve.addr, "GET", "/app", &[], ""), 426);
    drop(serve);

    // --auth all：本地客户端也必须带令牌
    let serve = start_serve(&home, &["--no-log-file", "--auth", "all"]).await;
    let status = |h: Vec<(&str, String)>| {
        let refs: Vec<(&str, &str)> = h.iter().map(|(k, v)| (*k, v.as_str())).collect();
        http_status(serve.addr, "POST", "/mcp", &refs, init)
    };
    assert_eq!(status(with(&[])), 401);
    assert_eq!(status(with(&[("Authorization", bearer.clone())])), 200);
    // rmcp 客户端带令牌可正常使用
    let c = mcp(serve.addr, Some(&token)).await;
    assert!(
        c.list_all_tools()
            .await
            .unwrap()
            .iter()
            .any(|t| t.name == "apps.list")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn config_file_is_read() {
    let home = TempHome::new("config");
    std::fs::write(
        home.0.join("config.json"),
        json!({
            "listen": "127.0.0.1:0",
            "ipcEndpoint": ipc_endpoint(&home.0),
            // 已弃用的旧 MCP 端口：兼容期内显式配置才另开（这里用 localhost:0，与 listen 不同）
            "http": { "addr": "localhost:0", "auth": "off" },
            "log": { "file": false }
        })
        .to_string(),
    )
    .unwrap();
    let mut child = Command::new(BIN)
        .args(["serve", "--home"])
        .arg(&home.0)
        .env_remove("APP_MCP_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // 逐行读 stderr，等到“已就绪”（兼容端口在登记之后才开）
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let reg = wait_registered(&home, &mut child).await;
    let mut log = String::new();
    while !log.contains("app-mcp-host 已就绪") {
        let line = rx.recv_timeout(T).unwrap_or_else(|_| panic!("serve 未就绪：{log}"));
        log.push_str(&line);
        log.push('\n');
    }
    assert!(log.contains("http.addr / --http 已弃用"), "{log}");
    assert!(log.contains("额外的 HTTP 服务已启动"), "{log}");
    let addr = reg.listen.clone().unwrap();
    let Probe::AppMcp(health) = probe(&addr).await else { panic!("/healthz") };
    assert!(!health.token_required_for_browsers, "auth=off");
    assert!(!home.0.join("token").exists(), "auth=off 时不生成令牌");
    assert!(!home.0.join("logs").exists(), "log.file=false");
    #[cfg(unix)]
    assert!(home.0.join("run").join("hub.sock").exists(), "按配置文件监听本地 IPC");
    let _ = child.kill();
    let _ = child.wait();
}
