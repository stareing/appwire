//! 集成测试：按需启动（spec/protocol.md 1.9），按 systemd 套接字激活协议启动真实 `app-mcp-host serve` 进程。
//!
//! 测试进程扮演 systemd：自己持有监听套接字（TCP 端口 0、临时目录中的 Unix 套接字），以 fd 3 / 4 交给子进程，
//! 并设置 `LISTEN_PID`（经 `sh -c 'export LISTEN_PID=$$ …; exec …'`，exec 不改变进程号）/ `LISTEN_FDS` / `LISTEN_FDNAMES`。
//!
//! - Host 在交来的套接字上服务（不绑定 `--listen` / `--ipc-endpoint`），登记文件写交来的地址；
//! - 进行中的调用（比空闲时间长）不被空闲退出打断；
//! - App 在线时不退出；App 休眠、MCP 客户端断开后空闲满 `--idle-exit-ms` 退出（退出码 0、删除登记文件）；
//! - 退出后排在监听套接字上的连接，在"服务管理器"再次启动 Host 后得到服务；重启后的 Host 仍列出休眠 App 的工具
//!   （读回休眠记录），调用经唤醒器带回同一实例并完成。
#![cfg(target_os = "linux")]

use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_mcp_native::{
    CallHandle, LifecycleMode, NativeClient, NativeConfig, StateStatus, ToolHandler, ToolSpec, WakeDescriptor, WakeKind,
};
use app_mcp_protocol::registry::{EndpointRegistry, REGISTRY_FILE, RUN_DIR};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

use crate::support::mcp_http;

const BIN: &str = env!("CARGO_BIN_EXE_app-mcp-host");
const T: Duration = Duration::from_secs(15);
/// Host 的空闲退出时间（测试取短值）。
const IDLE_MS: u64 = 1500;
/// 慢工具的耗时：长于空闲时间，验证进行中的调用不被空闲退出打断。
const SLOW_MS: u64 = 2 * IDLE_MS;

struct TempHome(PathBuf);
impl TempHome {
    fn new() -> Self {
        let n: u64 = rand::random();
        let dir = std::env::temp_dir().join(format!("app-mcp-ondemand-{}-{n:x}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
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

/// 扮演 systemd：持有监听套接字（Host 退出后连接仍排在这里）。
struct Sockets {
    tcp: std::net::TcpListener,
    unix: std::os::unix::net::UnixListener,
    unix_path: PathBuf,
}

impl Sockets {
    fn bind(home: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = home.join("sock");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let unix_path = dir.join("hub.sock");
        Self {
            tcp: std::net::TcpListener::bind("127.0.0.1:0").unwrap(),
            unix: std::os::unix::net::UnixListener::bind(&unix_path).unwrap(),
            unix_path,
        }
    }
    fn addr(&self) -> std::net::SocketAddr {
        self.tcp.local_addr().unwrap()
    }
    fn ipc_endpoint(&self) -> String {
        format!("unix:{}", self.unix_path.display())
    }
}

/// 以套接字激活方式启动 serve：fd 3 = TCP、fd 4 = Unix。`--listen` / `--ipc-endpoint` 指向别处，确认 Host 没有自己绑定。
fn spawn_activated(home: &Path, sockets: &Sockets, waker: &str) -> Child {
    spawn_with_fds(home, vec![sockets.tcp.as_raw_fd(), sockets.unix.as_raw_fd()], waker)
}

/// 交来 `fds`（依次放到 3、4 …）启动 serve。
fn spawn_with_fds(home: &Path, fds: Vec<RawFd>, waker: &str) -> Child {
    let n = fds.len();
    let names = vec!["app-mcp-host"; n].join(":");
    let mut c = Command::new("sh");
    c.arg("-c")
        .arg(format!(r#"export LISTEN_PID=$$ LISTEN_FDS={n} LISTEN_FDNAMES={names}; exec "$0" "$@""#))
        .arg(BIN)
        .arg("serve")
        .arg("--home")
        .arg(home)
        .args(["--listen", "127.0.0.1:0"])
        .args(["--ipc-endpoint", &format!("unix:{}", home.join("not-used.sock").display())])
        .args(["--idle-exit-ms", &IDLE_MS.to_string()])
        .args(["--lease-ms", "0", "--waker", waker])
        .env_remove("APP_MCP_HOME")
        .env("RUST_LOG", "info")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // SAFETY: 只调用 async-signal-safe 的 fcntl / dup2 / close；先复制到高位再放到 3、4，避免源 fd 恰为 3 / 4 时互相覆盖。
    unsafe {
        c.pre_exec(move || {
            let mut high = [0; 8];
            let high = &mut high[..fds.len()];
            for (i, fd) in fds.iter().enumerate() {
                high[i] = libc::fcntl(*fd, libc::F_DUPFD, 100);
                if high[i] < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            for (i, fd) in high.iter().enumerate() {
                if libc::dup2(*fd, 3 + i as RawFd) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(*fd);
            }
            Ok(())
        });
    }
    c.spawn().expect("启动 serve")
}

fn stderr_of(child: &mut Child) -> String {
    use std::io::Read;
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    err
}

async fn wait_registered(home: &TempHome, child: &mut Child) -> EndpointRegistry {
    let deadline = Instant::now() + T;
    loop {
        if let Ok(Some(reg)) = EndpointRegistry::read(&home.registry())
            && reg.identity.pid == child.id()
        {
            return reg;
        }
        if let Ok(Some(status)) = child.try_wait() {
            panic!("serve 提前退出（{status}）：{}", stderr_of(child));
        }
        assert!(Instant::now() < deadline, "serve 未写出登记文件");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 等子进程退出（最多 `limit`），返回退出码。
async fn wait_exit(child: &mut Child, limit: Duration) -> Option<i32> {
    let deadline = Instant::now() + limit;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return status.code();
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

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

struct Slow;
impl ToolHandler for Slow {
    fn invoke(&self, call: CallHandle) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(SLOW_MS));
            let _ = call.complete(Some(&json!({ "done": true }).to_string()), vec![]);
        });
    }
}

/// 空闲 300 ms 后休眠、声明了唤醒描述的 App（休眠记录写入 `<home>/state/dormant/calc.json`）。
fn calc_app(endpoint: &str) -> NativeClient {
    let mut c = NativeConfig::new("calc", "计算器");
    c.host_url = endpoint.to_owned();
    c.instance_id = Some("calc-ondemand".into());
    c.lifecycle.mode = LifecycleMode::Idle;
    c.lifecycle.idle_timeout_ms = 300;
    c.lifecycle.wake = Some(WakeDescriptor { kind: WakeKind::Uri, target: Some("calc-app".into()), background: true });
    let app = NativeClient::new(c, None).unwrap();
    let schema = r#"{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}}}"#;
    let mut add = ToolSpec::new("math.add", "加法");
    add.input_schema_json = Some(schema.into());
    app.register_tool(add, Arc::new(Add)).unwrap();
    app.register_tool(ToolSpec::new("math.slow", "慢操作"), Arc::new(Slow)).unwrap();
    app.start();
    app
}

type Client = RunningService<RoleClient, ()>;

async fn mcp(addr: std::net::SocketAddr) -> Client {
    ().serve(mcp_http::transport(format!("http://{addr}/mcp"), None)).await.expect("MCP initialize")
}

async fn wait_tool(client: &Client, name: &str) {
    let deadline = Instant::now() + T;
    loop {
        if client.list_all_tools().await.unwrap().iter().any(|t| t.name == name) {
            return;
        }
        assert!(Instant::now() < deadline, "没有等到工具 {name}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn call(client: &Client, name: &str, args: Value) -> CallToolResult {
    client
        .call_tool(CallToolRequestParams::new(name.to_owned()).with_arguments(args.as_object().unwrap().clone()))
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn socket_activated_host_serves_idles_out_and_restarts_with_dormant_apps() {
    let home = TempHome::new();
    let sockets = Sockets::bind(&home.0);
    let wake_log = home.0.join("wake.jsonl");
    let waker = json!({"exec": ["sh", "-c", r#"cat >> "$0""#, wake_log.to_string_lossy()]}).to_string();

    // ---- 第一次激活：在交来的套接字上服务 ----
    let mut host = spawn_activated(&home.0, &sockets, &waker);
    let reg = wait_registered(&home, &mut host).await;
    assert_eq!(reg.listen.as_deref(), Some(sockets.addr().to_string().as_str()), "应在交来的 TCP 套接字上服务");
    assert_eq!(reg.ipc_endpoint.as_deref(), Some(sockets.ipc_endpoint().as_str()), "应在交来的 Unix 套接字上服务");

    let app = calc_app(&sockets.ipc_endpoint());
    let client = mcp(sockets.addr()).await;
    wait_tool(&client, "calc.math.add").await;
    let r = call(&client, "calc.math.add", json!({"a": 1, "b": 2})).await;
    assert_eq!(r.structured_content.as_ref().map(|v| v["sum"].clone()), Some(json!(3)));
    // 进行中的调用比空闲时间长：不被空闲退出打断
    let r = call(&client, "calc.math.slow", json!({})).await;
    assert_eq!(r.structured_content.as_ref().map(|v| v["done"].clone()), Some(json!(true)), "{r:?}");
    assert!(host.try_wait().unwrap().is_none(), "调用进行中 Host 不应退出");
    client.cancel().await.unwrap();

    // App 空闲休眠（写出带恢复令牌的休眠记录）→ 没有连接、会话与在线 App → 空闲满 IDLE_MS 退出
    let record = home.0.join("state").join("dormant").join("calc.json");
    let deadline = Instant::now() + T;
    while !(app.state().status == StateStatus::Dormant
        && std::fs::read_to_string(&record)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .is_some_and(|v| v["instances"][0]["resumeToken"].as_str().is_some_and(|t| !t.is_empty())))
    {
        assert!(Instant::now() < deadline, "App 没有休眠并写出休眠记录");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let dormant_at = Instant::now();
    let code = wait_exit(&mut host, Duration::from_millis(IDLE_MS) + T).await;
    let idle_for = dormant_at.elapsed();
    if code.is_none() {
        // 先结束再读 stderr（读到 EOF 才返回）：失败时立即报告而不是挂住。
        let _ = host.kill();
        let _ = host.wait();
    }
    let err = stderr_of(&mut host);
    assert_eq!(code, Some(0), "空闲退出应为退出码 0：{err}");
    assert!(idle_for >= Duration::from_millis(IDLE_MS) - Duration::from_millis(400), "退出过早：{idle_for:?}");
    assert!(err.contains("空闲") && err.contains("退出"), "{err}");
    assert!(err.contains("按需启动（systemd 套接字激活）"), "{err}");
    assert!(!home.registry().exists(), "退出时应删除登记文件");
    assert!(sockets.unix_path.exists(), "交来的套接字文件归服务管理器，Host 不删除");

    // ---- 连接先到、Host 后启动（systemd 收到连接才启动服务）：排队的连接得到服务 ----
    let queued = tokio::spawn(mcp(sockets.addr()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!queued.is_finished(), "Host 未运行时连接应在监听队列中等待");
    let mut host = spawn_activated(&home.0, &sockets, &waker);
    let client = tokio::time::timeout(T, queued).await.expect("排队的连接未得到服务").unwrap();
    wait_registered(&home, &mut host).await;

    // 休眠 App 的工具仍列出（读回休眠记录），调用经唤醒器带回同一实例
    wait_tool(&client, "calc.math.add").await;
    let forward = tokio::spawn({
        let (wake_log, app) = (wake_log.clone(), app.clone());
        async move {
            let deadline = Instant::now() + T;
            loop {
                if let Ok(text) = std::fs::read_to_string(&wake_log)
                    && let Some(line) = text.lines().next()
                {
                    let req: Value = serde_json::from_str(line).unwrap();
                    assert_eq!(req["instanceId"], "calc-ondemand");
                    assert!(app.handle_wake(req["activationArg"].as_str().unwrap()));
                    return;
                }
                assert!(Instant::now() < deadline, "唤醒器没有被调用");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    });
    let r = call(&client, "calc.math.add", json!({"a": 30, "b": 12})).await;
    assert_eq!(r.structured_content.as_ref().map(|v| v["sum"].clone()), Some(json!(42)), "{r:?}");
    forward.await.unwrap();
    client.cancel().await.unwrap();
    app.stop();
    let _ = host.kill();
    let _ = host.wait();
}

/// 没有交来套接字时（普通 serve）`--idle-exit-ms` 不生效：空闲后仍在运行（退出后没有谁再启动它）。
#[tokio::test(flavor = "multi_thread")]
async fn plain_serve_ignores_idle_exit() {
    let home = TempHome::new();
    let mut host = Command::new(BIN)
        .arg("serve")
        .arg("--home")
        .arg(&home.0)
        .args(["--listen", "127.0.0.1:0"])
        .args(["--ipc-endpoint", &format!("unix:{}", home.0.join("run").join("hub.sock").display())])
        .args(["--idle-exit-ms", "200"])
        .env_remove("APP_MCP_HOME")
        .env_remove("LISTEN_PID")
        .env_remove("LISTEN_FDS")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_registered(&home, &mut host).await;
    assert_eq!(wait_exit(&mut host, Duration::from_millis(1000)).await, None, "普通 serve 不应空闲退出");
    let _ = host.kill();
    let _ = host.wait();
}

/// `LISTEN_PID` 指向别的进程：变量不是给本进程的，按普通 serve 自己绑定（sd_listen_fds(3)）。
#[tokio::test(flavor = "multi_thread")]
async fn listen_env_for_other_pid_is_ignored() {
    let home = TempHome::new();
    let mut host = Command::new(BIN)
        .arg("serve")
        .arg("--home")
        .arg(&home.0)
        .args(["--listen", "127.0.0.1:0"])
        .args(["--ipc-endpoint", "none"])
        .env_remove("APP_MCP_HOME")
        .env("LISTEN_PID", "1")
        .env("LISTEN_FDS", "2")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let reg = wait_registered(&home, &mut host).await;
    assert!(reg.listen.as_deref().is_some_and(|a| !a.ends_with(":0")), "应自己绑定 --listen：{reg:?}");
    let _ = host.kill();
    let _ = host.wait();
}

/// `LISTEN_FDS` 不是数字：显式失败（不静默忽略配置错误）。
#[test]
fn malformed_listen_env_fails() {
    let home = TempHome::new();
    let out = Command::new("sh")
        .arg("-c")
        .arg(r#"export LISTEN_PID=$$ LISTEN_FDS=two; exec "$0" "$@""#)
        .arg(BIN)
        .arg("serve")
        .arg("--home")
        .arg(&home.0)
        .args(["--listen", "127.0.0.1:0", "--ipc-endpoint", "none", "--no-log-file"])
        .env_remove("APP_MCP_HOME")
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(err.contains("LISTEN_FDS 不是数字"), "{err}");
}

/// 只交来本地 IPC 套接字：Host 只在它上面服务，不自己绑定 `--listen`（自己绑定的端口在空闲退出后无人监听，也不会触发再次启动）。
#[tokio::test(flavor = "multi_thread")]
async fn only_handed_listeners_are_served() {
    let home = TempHome::new();
    let sockets = Sockets::bind(&home.0);
    let mut host = spawn_with_fds(&home.0, vec![sockets.unix.as_raw_fd()], "none");
    let reg = wait_registered(&home, &mut host).await;
    assert_eq!(reg.listen, None, "没有交来 TCP 套接字时不应自己绑定");
    assert_eq!(reg.ipc_endpoint.as_deref(), Some(sockets.ipc_endpoint().as_str()));
    let _ = host.kill();
    let _ = host.wait();
}
