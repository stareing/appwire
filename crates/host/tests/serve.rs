//! 集成测试：`app-mcp-host serve` 常驻模式（真实进程）。
//!
//! - 一个 serve 进程，两个独立的 MCP HTTP 会话同时列出并调用同一 App（app-mcp-native 客户端）的工具，
//!   各自首次调用附带总览；
//! - 第二个 serve 探测到健康实例后退出码 0；端口被其他程序占用时报错；
//! - `/healthz`；本地访问令牌：带 Origin 无令牌 → 401，Origin 不在允许列表 → 403。
//!
//! 每个测试用临时配置目录与随机端口，不影响本机可能在运行的实例。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_mcp_host::probe::{Probe, probe};
use app_mcp_native::{AppOverview, CallHandle, NativeClient, NativeConfig, ToolHandler, ToolSpec};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_app-mcp-host");
const T: Duration = Duration::from_secs(15);

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct TempHome(PathBuf);
impl TempHome {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "app-mcp-serve-{tag}-{}-{}",
            std::process::id(),
            free_port()
        ));
        std::fs::create_dir_all(dir.join("manifests")).unwrap();
        Self(dir)
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

fn serve_cmd(home: &Path, ws: u16, http: u16, extra: &[&str]) -> Command {
    let mut c = Command::new(BIN);
    c.arg("serve")
        .arg("--home")
        .arg(home)
        .args(["--ws-addr", &format!("127.0.0.1:{ws}")])
        .args(["--ipc-endpoint", &ipc_endpoint(home)])
        .args(["--http", &format!("127.0.0.1:{http}")])
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
    http: SocketAddr,
    ws: SocketAddr,
    ipc: String,
}
impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn start_serve(home: &Path, extra: &[&str]) -> Serve {
    let (ws, http) = (free_port(), free_port());
    let child = serve_cmd(home, ws, http, extra)
        .spawn()
        .expect("启动 serve");
    let http: SocketAddr = format!("127.0.0.1:{http}").parse().unwrap();
    let mut s = Serve {
        child,
        http,
        ws: format!("127.0.0.1:{ws}").parse().unwrap(),
        ipc: ipc_endpoint(home),
    };
    let deadline = Instant::now() + T;
    loop {
        if let Probe::AppMcp(h) = probe(&http.to_string()).await {
            assert_eq!(h.pid, s.child.id());
            assert_eq!(h.ws_addr.as_deref(), Some(s.ws.to_string().as_str()));
            return s;
        }
        if let Ok(Some(status)) = s.child.try_wait() {
            let mut err = String::new();
            s.child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut err)
                .unwrap();
            panic!("serve 提前退出（{status}）：{err}");
        }
        assert!(Instant::now() < deadline, "serve 未就绪");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
    let serve = start_serve(&home.0, &[]).await;
    // 原生 App 经本地 IPC 连接常驻 Host。
    let app = calc_app(&serve.ipc);

    let a = mcp(serve.http, None).await;
    let b = mcp(serve.http, None).await;
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
    let c = mcp(serve.http, None).await;
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
async fn second_serve_exits_zero_when_healthy_instance_runs() {
    let home = TempHome::new("single");
    let serve = start_serve(&home.0, &[]).await;
    let (code, stdout, stderr) = run_to_exit(serve_cmd(
        &home.0,
        serve.ws.port(),
        serve.http.port(),
        &["--no-log-file"],
    ));
    assert_eq!(code, 0, "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("已在运行"), "{stdout}");
    assert!(
        stdout.contains(&format!("pid {}", serve.child.id())),
        "{stdout}"
    );
    // 第一个实例不受影响
    assert!(matches!(
        probe(&serve.http.to_string()).await,
        Probe::AppMcp(_)
    ));

    // 只有 HTTP 端口相同（WebSocket 端口空闲）时也识别为已在运行
    let (code, stdout, _) = run_to_exit(serve_cmd(
        &home.0,
        free_port(),
        serve.http.port(),
        &["--no-log-file"],
    ));
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("已在运行"));
}

#[tokio::test(flavor = "multi_thread")]
async fn port_taken_by_other_program_is_an_error() {
    let home = TempHome::new("taken");
    // 一个“其他程序”：接受连接并返回 404
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let other_port = other.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in other.incoming().flatten() {
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        }
    });

    // HTTP 端口被占用
    let (code, _, stderr) = run_to_exit(serve_cmd(
        &home.0,
        free_port(),
        other_port,
        &["--no-log-file"],
    ));
    assert_ne!(code, 0);
    assert!(stderr.contains("已被占用"), "{stderr}");
    assert!(stderr.contains("其他程序"), "{stderr}");

    // WebSocket 端口被占用（HTTP 端口空闲）
    let (code, _, stderr) = run_to_exit(serve_cmd(
        &home.0,
        other_port,
        free_port(),
        &["--no-log-file"],
    ));
    assert_ne!(code, 0);
    assert!(stderr.contains("App 连接端口"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn healthz_and_token_policy() {
    let home = TempHome::new("token");
    let serve = start_serve(&home.0, &["--no-log-file"]).await;
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
        http_status(serve.http, "POST", "/mcp", &refs, init)
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

    // /healthz 不需要令牌
    assert_eq!(http_status(serve.http, "GET", "/healthz", &[], ""), 200);
    drop(serve);

    // --auth all：本地客户端也必须带令牌
    let serve = start_serve(&home.0, &["--no-log-file", "--auth", "all"]).await;
    let status = |h: Vec<(&str, String)>| {
        let refs: Vec<(&str, &str)> = h.iter().map(|(k, v)| (*k, v.as_str())).collect();
        http_status(serve.http, "POST", "/mcp", &refs, init)
    };
    assert_eq!(status(with(&[])), 401);
    assert_eq!(status(with(&[("Authorization", bearer.clone())])), 200);
    // rmcp 客户端带令牌可正常使用
    let c = mcp(serve.http, Some(&token)).await;
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
    let (ws, http) = (free_port(), free_port());
    std::fs::write(
        home.0.join("config.json"),
        json!({
            "wsAddr": format!("127.0.0.1:{ws}"),
            "ipcEndpoint": ipc_endpoint(&home.0),
            "http": { "addr": format!("127.0.0.1:{http}"), "auth": "off" },
            "log": { "file": false }
        })
        .to_string(),
    )
    .unwrap();
    let mut child = Command::new(BIN)
        .args(["serve", "--home"])
        .arg(&home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let addr = format!("127.0.0.1:{http}");
    let deadline = Instant::now() + T;
    let health = loop {
        if let Probe::AppMcp(h) = probe(&addr).await {
            break h;
        }
        assert!(Instant::now() < deadline, "serve 未按配置文件监听");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(health.ws_addr, Some(format!("127.0.0.1:{ws}")));
    assert!(!health.token_required_for_browsers, "auth=off");
    assert!(!home.0.join("token").exists(), "auth=off 时不生成令牌");
    assert!(!home.0.join("logs").exists(), "log.file=false");
    #[cfg(unix)]
    assert!(home.0.join("run").join("hub.sock").exists(), "按配置文件监听本地 IPC");
    let _ = child.kill();
    let _ = child.wait();
}
