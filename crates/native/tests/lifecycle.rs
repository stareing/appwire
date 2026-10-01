//! 生命周期集成测试：休眠释放运行时、唤醒快速恢复、on-demand、连接超时、handler 持有与错误详情。

mod common;

use std::io::{BufRead, BufReader, Lines};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_native::{
    CallHandle, ClientListener, ErrorKind, LifecycleMode, LogLevel, NativeClient, NativeConfig,
    NativeError, ReadHandle, Residency, ResourceOptions, ResourceReader, ResourceSpec, StateInfo,
    StateStatus, ToolHandler, ToolSpec, Visibility,
};
use app_mcp_protocol::method;
use common::{HostEvent, MockHost, eventually};
use serde_json::{Value, json};

struct Add;
impl ToolHandler for Add {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or(Value::Null);
        let sum = args["a"].as_i64().unwrap_or(0) + args["b"].as_i64().unwrap_or(0);
        std::thread::spawn(move || {
            call.complete(Some(&json!({ "sum": sum }).to_string()), vec![])
                .unwrap()
        });
    }
}

#[derive(Default)]
struct Listener {
    statuses: Mutex<Vec<StateStatus>>,
    idle_exits: AtomicUsize,
    logs: Mutex<Vec<String>>,
}

impl ClientListener for Listener {
    fn on_state_changed(&self, state: StateInfo) {
        self.statuses.lock().unwrap().push(state.status);
    }
    fn on_paired(&self, _token: String) {}
    fn on_log(&self, _level: LogLevel, message: String) {
        self.logs.lock().unwrap().push(message);
    }
    fn on_idle_exit(&self) {
        self.idle_exits.fetch_add(1, Ordering::SeqCst);
    }
}

fn fake_host_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.parent()?;
    let path = dir
        .join("examples")
        .join(format!("fake_host{}", std::env::consts::EXE_SUFFIX));
    path.exists().then_some(path)
}

fn spawn_fake_host(args: &[&str]) -> Option<(Child, Lines<BufReader<ChildStdout>>, String)> {
    let Some(bin) = fake_host_path() else {
        eprintln!("未找到 fake_host 可执行文件，跳过（先运行 cargo build --example fake_host）");
        return None;
    };
    let mut child = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let first = lines.next().unwrap().unwrap();
    let addr = first
        .strip_prefix("LISTENING ")
        .expect("LISTENING 行")
        .to_owned();
    Some((child, lines, addr))
}

fn next_json(lines: &mut Lines<BufReader<ChildStdout>>) -> Value {
    let line = lines.next().expect("fake_host 提前结束").unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn idle_sleep_wake_resume_and_sleep_again() {
    let Some((mut child, mut lines, addr)) = spawn_fake_host(&[
        "--invoke",
        "math.add",
        "--args",
        r#"{"a":1,"b":2}"#,
        "--await-sleep",
        "--wake",
        "--invoke",
        "math.add",
        "--args",
        r#"{"a":20,"b":22}"#,
        "--await-sleep",
        "--lease-ms",
        "100",
        "--timeout-ms",
        "15000",
    ]) else {
        return;
    };

    let mut config = NativeConfig::new("fake-test", "Fake");
    config.host_url = format!("ws://{addr}");
    config.lifecycle.mode = LifecycleMode::Idle;
    config.lifecycle.idle_timeout_ms = 200;
    config.lifecycle.residency = Residency::ExitAlways;
    let listener = Arc::new(Listener::default());
    let client = NativeClient::new(config, Some(listener.clone())).unwrap();
    client
        .register_tool(ToolSpec::new("math.add", "加法"), Arc::new(Add))
        .unwrap();
    client.start();

    assert_eq!(
        next_json(&mut lines),
        json!({ "type": "tools", "tools": ["math.add"], "resources": [] })
    );
    assert_eq!(next_json(&mut lines)["result"]["data"]["sum"], 3);
    let sleep = next_json(&mut lines);
    assert_eq!(sleep["type"], "sleep");
    assert_eq!(sleep["accepted"], true);
    assert_eq!(sleep["reason"], "idle");
    assert_eq!(sleep["toolsHash"], json!(client.tools_hash()));

    // 休眠：状态 Dormant、运行时已销毁、连接已关闭
    eventually("进入休眠并释放运行时", || {
        client.state().status == StateStatus::Dormant && !client.runtime_active()
    });
    eventually("IdleExit 回调", || listener.idle_exits.load(Ordering::SeqCst) == 1);

    // 唤醒
    let wake = next_json(&mut lines);
    assert_eq!(wake["type"], "wake");
    assert!(!client.handle_wake("--not-ours"));
    assert!(client.handle_wake(wake["arg"].as_str().unwrap()));
    let hello = next_json(&mut lines);
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["launchToken"], wake["token"]);
    assert_eq!(hello["resumeToken"], "resume-1");
    assert_eq!(hello["wakeReason"], "os-activation");
    assert_eq!(hello["toolsCurrent"], true);
    let tools = next_json(&mut lines);
    assert_eq!(tools["tools"], json!(["math.add"]), "Host 沿用休眠前快照");
    assert_eq!(tools["synced"], false, "toolsCurrent 时跳过 tools/sync");
    assert_eq!(next_json(&mut lines)["result"]["data"]["sum"], 42);
    let sleep = next_json(&mut lines);
    assert_eq!(sleep["accepted"], true);

    let status = child.wait().unwrap();
    assert!(status.success(), "fake_host 退出码 {status:?}");
    eventually("再次休眠", || {
        client.state().status == StateStatus::Dormant && !client.runtime_active()
    });
    eventually("第二次 IdleExit", || listener.idle_exits.load(Ordering::SeqCst) == 2);
    let statuses = listener.statuses.lock().unwrap().clone();
    assert!(statuses.contains(&StateStatus::Waking), "{statuses:?}");
    drop(client);
}

#[test]
fn rejected_sleep_is_retried() {
    let Some((mut child, mut lines, addr)) =
        spawn_fake_host(&["--await-sleep", "--reject-sleep", "100", "--timeout-ms", "10000"])
    else {
        return;
    };
    let mut config = NativeConfig::new("fake-test", "Fake");
    config.host_url = format!("ws://{addr}");
    config.lifecycle.mode = LifecycleMode::Idle;
    config.lifecycle.idle_timeout_ms = 100;
    let client = NativeClient::new(config, None).unwrap();
    client.start();
    assert_eq!(next_json(&mut lines)["type"], "tools");
    assert_eq!(next_json(&mut lines)["accepted"], false);
    assert_eq!(next_json(&mut lines)["accepted"], true);
    assert!(child.wait().unwrap().success());
    eventually("休眠", || client.state().status == StateStatus::Dormant);
}

#[test]
fn on_demand_does_not_connect_until_asked() {
    let host = MockHost::start();
    let mut config = NativeConfig::new("test-app", "测试应用");
    config.host_url = host.url();
    config.instance_id = Some("inst-od".into());
    config.lifecycle.mode = LifecycleMode::OnDemand;
    let client = NativeClient::new(config, None).unwrap();
    client.start();
    eventually("Dormant 且运行时释放", || {
        client.state().status == StateStatus::Dormant && !client.runtime_active()
    });
    assert!(
        host.next(Duration::from_millis(300)).is_none(),
        "on-demand 启动不连接"
    );
    // 休眠期间注册工具不唤醒
    client
        .register_tool(ToolSpec::new("a", "a"), Arc::new(Add))
        .unwrap();
    assert!(host.next(Duration::from_millis(200)).is_none());
    assert_eq!(client.state().status, StateStatus::Dormant);

    assert!(client.connect_now());
    host.wait_connected();
    let hello = host.wait_request(method::HELLO);
    assert_eq!(hello["wakeReason"], "app");
    host.wait_ready();
    assert_eq!(client.state().status, StateStatus::Connected);
    assert!(client.runtime_active());
}

#[test]
fn explicit_sleep_against_host_without_support_keeps_connection() {
    // MockHost 对 app/sleep 不回复：显式休眠保持等待，连接不受影响
    let host = MockHost::start();
    let mut config = NativeConfig::new("test-app", "测试应用");
    config.host_url = host.url();
    let client = NativeClient::new(config, None).unwrap();
    client.start();
    host.wait_ready();
    assert!(client.sleep());
    host.wait_request(method::SLEEP);
    assert_eq!(client.state().status, StateStatus::Connected);
}

#[test]
fn connect_timeout_enters_backoff() {
    // 只监听不 accept 的端口：TCP 连接能建立，WebSocket 握手永远不完成。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let mut config = NativeConfig::new("test-app", "测试应用");
    config.host_url = format!("ws://{addr}");
    config.connect_timeout_ms = 150;
    let rec = Arc::new(Listener::default());
    let client = NativeClient::new(config, Some(rec.clone())).unwrap();
    client.start();
    eventually("连接超时后进入退避", || {
        client.state().status == StateStatus::Backoff
    });
    eventually("超时日志", || {
        rec.logs
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains("内未能建立连接"))
    });
    drop(listener);
}

/// 把 CallHandle 交给测试线程。
struct Forward(Mutex<std::sync::mpsc::Sender<CallHandle>>);
impl ToolHandler for Forward {
    fn invoke(&self, call: CallHandle) {
        self.0.lock().unwrap().send(call).unwrap();
    }
}

#[test]
fn call_hold_and_fail_with_details() {
    let host = MockHost::start();
    let mut config = NativeConfig::new("test-app", "测试应用");
    config.host_url = host.url();
    config.lifecycle.mode = LifecycleMode::Idle;
    config.lifecycle.idle_timeout_ms = 100;
    let client = NativeClient::new(config, None).unwrap();
    let (tx, rx) = channel();
    client
        .register_tool(ToolSpec::new("t", "t"), Arc::new(Forward(Mutex::new(tx))))
        .unwrap();
    // 持有期间不休眠
    let app_hold = client.hold();
    client.start();
    host.wait_ready();

    let id = host.invoke("c1", "t", json!({}));
    let call = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let call_hold = call.hold().unwrap();
    assert!(matches!(
        call.fail_with_details(ErrorKind::InvalidInput, "bad", Some("{")),
        Err(NativeError::InvalidJson(_))
    ));
    call.fail_with_details(
        ErrorKind::InvalidInput,
        "参数错误",
        Some(r#"{"issues":[{"path":["a"]}]}"#),
    )
    .unwrap();
    let err = host.wait_response(&id).unwrap_err();
    assert_eq!(err.data.as_ref().unwrap()["kind"], "INVALID_INPUT");
    // 对象详情按协议合并进 data
    assert_eq!(err.data.as_ref().unwrap()["issues"][0]["path"][0], "a");
    assert!(matches!(call.hold(), Err(NativeError::AlreadyCompleted)));

    // 两个持有都在：不发 app/sleep
    app_hold.release();
    app_hold.release();
    assert!(
        !saw_sleep(&host, Duration::from_millis(400)),
        "调用持有仍然有效"
    );
    // 丢弃最后一个克隆即释放
    drop(call_hold);
    assert!(saw_sleep(&host, Duration::from_secs(3)));
}

fn saw_sleep(host: &MockHost, within: Duration) -> bool {
    let deadline = std::time::Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        match host.next(left) {
            Some(HostEvent::Msg(app_mcp_protocol::Message::Request(r))) if r.method == method::SLEEP => {
                return true;
            }
            Some(_) => {}
            None => return false,
        }
    }
}

#[test]
fn wss_uses_tls_and_fails_cleanly_against_plain_server() {
    // 明文 WebSocket 服务器上走 wss://：TLS 握手失败 → 按连接失败进入退避（不 panic）。
    let host = MockHost::start();
    let mut config = NativeConfig::new("test-app", "测试应用");
    config.host_url = format!("wss://{}", host.addr);
    config.connect_timeout_ms = 2_000;
    let rec = Arc::new(Listener::default());
    let client = NativeClient::new(config, Some(rec.clone())).unwrap();
    client.start();
    eventually("TLS 失败后进入退避", || {
        rec.statuses.lock().unwrap().contains(&StateStatus::Backoff)
    });
    assert!(
        rec.logs.lock().unwrap().iter().any(|m| m.contains("连接")),
        "{:?}",
        rec.logs.lock().unwrap()
    );
}

#[test]
fn relay_without_host_counts_as_host_absent_and_goes_dormant() {
    // 模拟 adb reverse 远端没有 Hub：接受 TCP 连接后立即关闭（WebSocket 握手未完成）。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    {
        let accepted = accepted.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                accepted.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
        });
    }
    let mut config = NativeConfig::new("test-app", "测试应用");
    config.host_url = format!("ws://{addr}");
    config.lifecycle.mode = LifecycleMode::Idle;
    let rec = Arc::new(Listener::default());
    let client = NativeClient::new(config, Some(rec.clone())).unwrap();
    client.start();
    eventually("连续 3 次 Host 不在后进入休眠", || client.state().status == StateStatus::Dormant);
    assert!(rec.logs.lock().unwrap().iter().any(|m| m.contains("HOST_NOT_RUNNING")), "{:?}", rec.logs.lock().unwrap());
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(accepted.load(Ordering::SeqCst), 3, "休眠后不再发起连接");
    assert_eq!(client.state().status, StateStatus::Dormant);
}

struct Order;
impl ResourceReader for Order {
    fn read(&self, read: ReadHandle) {
        let _ = read.complete(&json!({ "status": "paid" }).to_string());
    }
}

#[test]
fn realtime_resource_and_background_sleep_reach_the_wire() {
    // 4e 第二部分（spec/lifecycle.md 第 13 节）：realtime 随 resources/sync 上报；进入后台立即以 background 休眠
    let host = MockHost::start();
    let mut config = NativeConfig::new("test-app", "测试应用");
    config.host_url = host.url();
    config.lifecycle.mode = LifecycleMode::Idle;
    config.lifecycle.sleep_on_background = true;
    config.lifecycle.merge_window_ms = 500;
    let client = NativeClient::new(config, None).unwrap();
    let spec = ResourceSpec { name: "order".into(), description: "订单".into(), mime_type: None };
    let _order = client
        .register_resource_with(spec, ResourceOptions { realtime: true }, Arc::new(Order))
        .unwrap();
    client.start();
    let sync = host.wait_notification(method::RESOURCES_SYNC);
    assert_eq!(sync["resources"][0]["realtime"], true, "{sync}");
    host.wait_ready();
    client.set_visibility(Visibility::Hidden, false);
    let sleep = host.wait_request(method::SLEEP);
    assert_eq!(sleep["reason"], "background");
}
