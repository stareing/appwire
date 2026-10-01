//! 集成测试：NativeClient 连接模拟 Host。

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_native::{
    CallHandle, CancelListener, CancelReason, ErrorKind, NativeClient, NativeConfig, NativeError,
    ReadHandle, ResourceReader, ResourceSpec, StateStatus, ToolHandler, ToolSpec,
};
use app_mcp_protocol::method;
use common::{MockHost, Recorder, WAIT, eventually};
use serde_json::{Value, json};

fn config(host: &MockHost) -> NativeConfig {
    let mut c = NativeConfig::new("test-app", "测试应用");
    c.host_url = host.url();
    c.instance_id = Some("inst-1".into());
    c
}

/// 把收到的 CallHandle 交给测试线程。
struct Forward(Mutex<Sender<CallHandle>>);
impl ToolHandler for Forward {
    fn invoke(&self, call: CallHandle) {
        assert_eq!(std::thread::current().name(), Some("app-mcp-dispatch"));
        self.0.lock().unwrap().send(call).unwrap();
    }
}

fn forward() -> (Arc<Forward>, std::sync::mpsc::Receiver<CallHandle>) {
    let (tx, rx) = channel();
    (Arc::new(Forward(Mutex::new(tx))), rx)
}

/// 同步返回参数的 handler。
struct Echo;
impl ToolHandler for Echo {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap();
        call.complete(Some(&json!({ "echo": args }).to_string()), vec![])
            .unwrap();
    }
}

struct Reader;
impl ResourceReader for Reader {
    fn read(&self, read: ReadHandle) {
        if read.resource_name() == "bad" {
            read.fail(ErrorKind::HandlerError, "读取失败").unwrap();
            assert_eq!(read.complete("{}"), Err(NativeError::AlreadyCompleted));
        } else {
            assert!(matches!(
                read.complete("{"),
                Err(NativeError::InvalidJson(_))
            ));
            read.complete(r#"{"items":[1,2]}"#).unwrap();
        }
    }
}

fn names(list: &Value, key: &str) -> Vec<String> {
    let mut v: Vec<String> = list[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    v.sort();
    v
}

/// 启动客户端并完成握手，返回 tools/sync 参数。
fn connect(host: &MockHost, client: &NativeClient) -> Value {
    client.start();
    host.wait_connected();
    let hello = host.wait_request(method::HELLO);
    assert_eq!(hello["appId"], "test-app");
    assert_eq!(hello["clientKind"], "native");
    let sync = host.wait_notification(method::TOOLS_SYNC);
    host.wait_ready();
    eventually("Connected", || {
        client.state().status == StateStatus::Connected
    });
    sync
}

#[test]
fn handshake_sync_and_listener() {
    let host = MockHost::start();
    let rec = Arc::new(Recorder::default());
    let client = NativeClient::new(config(&host), Some(rec.clone())).unwrap();
    assert_eq!(client.state().status, StateStatus::Idle);
    client
        .register_tool(ToolSpec::new("cart.add", "加入购物车"), Arc::new(Echo))
        .unwrap();
    let mut disabled = ToolSpec::new("cart.hidden", "禁用");
    disabled.enabled = false;
    client.register_tool(disabled, Arc::new(Echo)).unwrap();

    let sync = connect(&host, &client);
    assert_eq!(names(&sync, "tools"), vec!["cart.add"]);
    assert_eq!(client.token().as_deref(), Some("tok-1"));
    eventually("on_paired", || {
        rec.tokens.lock().unwrap().as_slice() == ["tok-1"]
    });
    eventually("状态回调", || {
        rec.statuses().last() == Some(&StateStatus::Connected)
    });
    assert_eq!(
        rec.statuses(),
        vec![
            StateStatus::Connecting,
            StateStatus::Handshaking,
            StateStatus::Connected
        ]
    );
    assert!(
        rec.threads
            .lock()
            .unwrap()
            .iter()
            .all(|t| t == "app-mcp-dispatch")
    );

    // 连接 ID（spec/protocol.md 10.3）：可查询，连接后的日志带 [cid] 前缀
    let cid = client.connection_id().expect("连接 ID");
    assert!(cid.starts_with("mock-"), "{cid}");
    eventually("连接日志", || {
        rec.logs.lock().unwrap().iter().any(|(_, m)| m.starts_with(&format!("[{cid}] 已连接 Host")))
    });

    // start 重复调用无效果
    client.start();

    // 连接后注册 → tools/changed
    let late = client
        .register_tool(ToolSpec::new("late", "后注册"), Arc::new(Echo))
        .unwrap();
    let changed = host.wait_notification(method::TOOLS_CHANGED);
    assert_eq!(names(&changed, "upserted"), vec!["late"]);
    late.set_enabled(false).unwrap();
    let changed = host.wait_notification(method::TOOLS_CHANGED);
    assert_eq!(changed["removed"], json!(["late"]));

    // 同步 handler
    let id = host.invoke("c1", "cart.add", json!({ "n": 1 }));
    assert_eq!(
        host.wait_response(&id).unwrap(),
        json!({ "data": { "echo": { "n": 1 } } })
    );

    // 可见性
    client.set_visibility(app_mcp_native::Visibility::Hidden, false);
    let vis = host.wait_notification(method::VISIBILITY);
    assert_eq!(vis, json!({ "visibility": "hidden", "focused": false }));
}

#[test]
fn complete_and_fail_from_other_threads() {
    let host = MockHost::start();
    let client = NativeClient::new(config(&host), None).unwrap();
    let (handler, calls) = forward();
    let mut spec = ToolSpec::new("work", "异步工作");
    spec.input_schema_json =
        Some(r#"{"type":"object","properties":{"x":{"type":"number"}}}"#.into());
    client.register_tool(spec, handler).unwrap();
    connect(&host, &client);

    let id = host.invoke("c1", "work", json!({ "x": 2 }));
    let call = calls.recv_timeout(WAIT).unwrap();
    assert_eq!(call.call_id(), "c1");
    assert_eq!(call.tool_name(), "work");
    assert_eq!(
        serde_json::from_str::<Value>(&call.arguments_json()).unwrap(),
        json!({ "x": 2 })
    );
    assert!(!call.is_cancelled());
    let c2 = call.clone();
    std::thread::spawn(move || {
        assert!(matches!(
            c2.complete(Some("{nope"), vec![]),
            Err(NativeError::InvalidJson(_))
        ));
        c2.complete(Some(r#"{"ok":true}"#), vec!["cart.state".into()])
            .unwrap();
    })
    .join()
    .unwrap();
    assert_eq!(
        call.complete(None, vec![]),
        Err(NativeError::AlreadyCompleted)
    );
    assert_eq!(
        call.fail(ErrorKind::HandlerError, "x"),
        Err(NativeError::AlreadyCompleted)
    );
    assert_eq!(
        host.wait_response(&id).unwrap(),
        json!({ "data": { "ok": true }, "stateHints": ["cart.state"] })
    );

    // fail 带 kind
    let id = host.invoke("c2", "work", json!({}));
    let call = calls.recv_timeout(WAIT).unwrap();
    std::thread::spawn(move || {
        call.fail(ErrorKind::UserRejected, "用户取消了操作")
            .unwrap()
    })
    .join()
    .unwrap();
    let err = host.wait_response(&id).unwrap_err();
    assert_eq!(err.kind(), Some(ErrorKind::UserRejected));
    assert_eq!(err.message, "用户取消了操作");

    // complete(None) → data: null
    let id = host.invoke("c3", "work", json!({}));
    calls
        .recv_timeout(WAIT)
        .unwrap()
        .complete(None, vec![])
        .unwrap();
    assert_eq!(host.wait_response(&id).unwrap(), json!({ "data": null }));

    // 未知工具
    let id = host.invoke("c4", "missing", json!({}));
    assert_eq!(
        host.wait_response(&id).unwrap_err().kind(),
        Some(ErrorKind::ToolNotFound)
    );
}

struct CancelRec(Mutex<Vec<(CancelReason, Option<String>)>>);
impl CancelListener for CancelRec {
    fn on_cancel(&self, reason: CancelReason) {
        let t = std::thread::current().name().map(str::to_owned);
        self.0.lock().unwrap().push((reason, t));
    }
}

#[test]
fn host_cancel_notifies_listener() {
    let host = MockHost::start();
    let client = NativeClient::new(config(&host), None).unwrap();
    let (handler, calls) = forward();
    client
        .register_tool(ToolSpec::new("slow", "慢"), handler)
        .unwrap();
    connect(&host, &client);

    let id = host.invoke("c1", "slow", json!({}));
    let call = calls.recv_timeout(WAIT).unwrap();
    let rec = Arc::new(CancelRec(Mutex::new(Vec::new())));
    call.set_cancel_listener(rec.clone());
    host.send(app_mcp_protocol::Message::notification(
        method::TOOLS_CANCEL,
        json!({ "callId": "c1", "reason": "user" }),
    ));
    eventually("on_cancel", || !rec.0.lock().unwrap().is_empty());
    assert_eq!(
        rec.0.lock().unwrap()[0],
        (CancelReason::Requested, Some("app-mcp-dispatch".into()))
    );
    assert!(call.is_cancelled());
    assert_eq!(
        call.complete(None, vec![]),
        Err(NativeError::AlreadyCompleted)
    );
    assert_eq!(
        host.wait_response(&id).unwrap_err().kind(),
        Some(ErrorKind::Cancelled)
    );

    // 已取消后设置监听：立即在当前线程回调
    let late = Arc::new(CancelRec(Mutex::new(Vec::new())));
    call.set_cancel_listener(late.clone());
    let me = std::thread::current().name().map(str::to_owned);
    assert_eq!(
        late.0.lock().unwrap().as_slice(),
        &[(CancelReason::Requested, me)]
    );
}

/// 进度（spec/protocol.md 3.3）与去重（同一 callId 重放首次结果）经原生运行时。
#[test]
fn progress_and_dedup() {
    let host = MockHost::start();
    let client = NativeClient::new(config(&host), None).unwrap();
    let (handler, calls) = forward();
    client.register_tool(ToolSpec::new("export", "导出"), handler).unwrap();
    connect(&host, &client);

    let id = host.invoke("c1", "export", json!({}));
    let call = calls.recv_timeout(WAIT).unwrap();
    call.report_progress(1.0, Some(3.0), Some("第 1 页")).unwrap();
    assert_eq!(
        host.wait_notification(method::TOOLS_PROGRESS),
        json!({ "callId": "c1", "progress": 1.0, "total": 3.0, "message": "第 1 页" })
    );
    call.complete(Some("7"), vec![]).unwrap();
    assert_eq!(host.wait_response(&id).unwrap(), json!({ "data": 7 }));
    assert_eq!(call.report_progress(2.0, None, None), Err(NativeError::AlreadyCompleted));

    // 同一 callId 再次到达：不再调用 handler，重放首次结果
    let id = host.invoke("c1", "export", json!({}));
    assert_eq!(host.wait_response(&id).unwrap(), json!({ "data": 7 }));
    assert!(calls.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn resource_read() {
    let host = MockHost::start();
    let client = NativeClient::new(config(&host), None).unwrap();
    let spec = |n: &str| ResourceSpec {
        name: n.into(),
        description: "d".into(),
        mime_type: None,
    };
    let res = client
        .register_resource(spec("cart.state"), Arc::new(Reader))
        .unwrap();
    client
        .register_resource(spec("bad"), Arc::new(Reader))
        .unwrap();
    client.start();
    let sync = host.wait_notification(method::RESOURCES_SYNC);
    assert_eq!(names(&sync, "resources"), vec!["bad", "cart.state"]);
    host.wait_ready();

    let id = host.request(method::RESOURCES_READ, json!({ "name": "cart.state" }));
    assert_eq!(
        host.wait_response(&id).unwrap()["contents"],
        json!({ "items": [1, 2] })
    );
    let id = host.request(method::RESOURCES_READ, json!({ "name": "bad" }));
    assert_eq!(
        host.wait_response(&id).unwrap_err().kind(),
        Some(ErrorKind::HandlerError)
    );

    // 订阅后通知变化 → resources/updated
    let id = host.request(method::RESOURCES_SUBSCRIBE, json!({ "name": "cart.state" }));
    host.wait_response(&id).unwrap();
    res.notify_changed().unwrap();
    assert_eq!(
        host.wait_notification(method::RESOURCES_UPDATED),
        json!({ "name": "cart.state" })
    );

    res.dispose();
    let changed = host.wait_notification(method::RESOURCES_CHANGED);
    assert_eq!(changed["removed"], json!(["cart.state"]));
    let id = host.request(method::RESOURCES_READ, json!({ "name": "cart.state" }));
    assert_eq!(
        host.wait_response(&id).unwrap_err().kind(),
        Some(ErrorKind::ResourceNotFound)
    );
}

#[test]
fn scope_dispose_unregisters_everything() {
    let host = MockHost::start();
    let client = NativeClient::new(config(&host), None).unwrap();
    client
        .register_tool(ToolSpec::new("root", "根"), Arc::new(Echo))
        .unwrap();
    connect(&host, &client);

    let scope = client.create_scope("page").unwrap();
    let child = scope.create_scope("dialog").unwrap();
    scope
        .register_tool(ToolSpec::new("page.a", "a"), Arc::new(Echo))
        .unwrap();
    child
        .register_tool(ToolSpec::new("dialog.b", "b"), Arc::new(Echo))
        .unwrap();
    child
        .register_resource(
            ResourceSpec {
                name: "dialog.r".into(),
                description: "r".into(),
                mime_type: None,
            },
            Arc::new(Reader),
        )
        .unwrap();
    let changed = host.wait_notification(method::TOOLS_CHANGED);
    assert_eq!(names(&changed, "upserted"), vec!["dialog.b", "page.a"]);
    host.wait_notification(method::RESOURCES_CHANGED);

    scope.dispose();
    scope.dispose();
    let changed = host.wait_notification(method::TOOLS_CHANGED);
    let mut removed: Vec<String> = serde_json::from_value(changed["removed"].clone()).unwrap();
    removed.sort();
    assert_eq!(removed, vec!["dialog.b", "page.a"]);
    let changed = host.wait_notification(method::RESOURCES_CHANGED);
    assert_eq!(changed["removed"], json!(["dialog.r"]));

    let id = host.invoke("c1", "page.a", json!({}));
    assert_eq!(
        host.wait_response(&id).unwrap_err().kind(),
        Some(ErrorKind::ToolNotFound)
    );
    assert!(matches!(
        child.register_tool(ToolSpec::new("x", "x"), Arc::new(Echo)),
        Err(NativeError::Disposed)
    ));
    child.dispose();
    let id = host.invoke("c2", "root", json!({}));
    assert!(host.wait_response(&id).is_ok());
}

#[test]
fn stop_cancels_calls_and_disconnects() {
    let host = MockHost::start();
    let rec = Arc::new(Recorder::default());
    let client = NativeClient::new(config(&host), Some(rec.clone())).unwrap();
    let (handler, calls) = forward();
    let tool = client
        .register_tool(ToolSpec::new("slow", "慢"), handler)
        .unwrap();
    connect(&host, &client);

    host.invoke("c1", "slow", json!({}));
    let call = calls.recv_timeout(WAIT).unwrap();
    let cancel = Arc::new(CancelRec(Mutex::new(Vec::new())));
    call.set_cancel_listener(cancel.clone());

    client.stop();
    host.wait_closed();
    eventually("on_cancel", || !cancel.0.lock().unwrap().is_empty());
    assert_eq!(cancel.0.lock().unwrap()[0].0, CancelReason::Stopped);
    assert_eq!(
        call.complete(None, vec![]),
        Err(NativeError::AlreadyCompleted)
    );
    assert_eq!(client.state().status, StateStatus::Stopped);
    eventually("Stopped 回调", || {
        rec.statuses().last() == Some(&StateStatus::Stopped)
    });
    assert_eq!(
        client
            .register_tool(ToolSpec::new("x", "x"), Arc::new(Echo))
            .unwrap_err(),
        NativeError::Stopped
    );
    assert_eq!(client.create_scope("s").unwrap_err(), NativeError::Stopped);
    assert_eq!(tool.set_enabled(false), Err(NativeError::Stopped));
    tool.dispose();

    // 不再重连
    assert!(host.next(Duration::from_millis(800)).is_none());
}

#[test]
fn reconnects_after_host_disconnect() {
    let host = MockHost::start();
    let rec = Arc::new(Recorder::default());
    let client = NativeClient::new(config(&host), Some(rec.clone())).unwrap();
    let (handler, calls) = forward();
    client
        .register_tool(ToolSpec::new("slow", "慢"), handler)
        .unwrap();
    connect(&host, &client);

    host.invoke("c1", "slow", json!({}));
    let call = calls.recv_timeout(WAIT).unwrap();
    let cancel = Arc::new(CancelRec(Mutex::new(Vec::new())));
    call.set_cancel_listener(cancel.clone());

    host.close();
    host.wait_closed();
    eventually("on_cancel", || !cancel.0.lock().unwrap().is_empty());
    assert_eq!(cancel.0.lock().unwrap()[0].0, CancelReason::Disconnected);
    eventually("Backoff", || {
        rec.states
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.status == StateStatus::Backoff)
    });
    let backoff = rec
        .states
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.status == StateStatus::Backoff)
        .cloned()
        .unwrap();
    assert!(backoff.retry_in_ms.is_some_and(|ms| ms <= 500));
    // 对端正常关闭：带 CONNECTION_CLOSED（spec/protocol.md 10.1）
    assert_eq!(backoff.code.as_deref(), Some("CONNECTION_CLOSED"), "{backoff:?}");
    assert!(backoff.reason.as_deref().is_some_and(|r| r.contains("关闭")), "{backoff:?}");

    // 重连：hello 带上第一次配对得到的 token，并重新全量同步
    assert_eq!(host.wait_connected(), 2);
    let hello = host.wait_request(method::HELLO);
    assert_eq!(hello["token"], "tok-1");
    let sync = host.wait_notification(method::TOOLS_SYNC);
    assert_eq!(names(&sync, "tools"), vec!["slow"]);
    host.wait_ready();
    eventually("Connected", || {
        client.state().status == StateStatus::Connected
    });
    assert_eq!(client.token().as_deref(), Some("tok-2"));

    let id = host.invoke("c2", "slow", json!({}));
    calls
        .recv_timeout(WAIT)
        .unwrap()
        .complete(Some("1"), vec![])
        .unwrap();
    assert_eq!(host.wait_response(&id).unwrap(), json!({ "data": 1 }));
}

#[test]
fn abrupt_drop_backs_off_with_connection_lost() {
    let host = MockHost::start();
    let rec = Arc::new(Recorder::default());
    let client = NativeClient::new(config(&host), Some(rec.clone())).unwrap();
    connect(&host, &client);

    host.abort();
    host.wait_closed();
    eventually("Backoff", || rec.statuses().contains(&StateStatus::Backoff));
    let backoff = rec
        .states
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.status == StateStatus::Backoff)
        .cloned()
        .unwrap();
    // 未经关闭握手的中断：CONNECTION_LOST
    assert_eq!(backoff.code.as_deref(), Some("CONNECTION_LOST"), "{backoff:?}");
    assert!(backoff.reason.as_deref().is_some_and(|r| r.contains("中断")), "{backoff:?}");
    // 断开日志带断开前的连接 ID
    assert!(
        rec.logs.lock().unwrap().iter().any(|(_, m)| m.starts_with("[mock-1] [CONNECTION_LOST]")),
        "{:?}",
        rec.logs.lock().unwrap()
    );

    // 之后照常重连
    assert_eq!(host.wait_connected(), 2);
    host.wait_ready();
    eventually("Connected", || client.state().status == StateStatus::Connected);
}

#[test]
fn connect_failure_backs_off() {
    // 没人监听的临时 IPC 端点（不用"先绑定再释放"的 TCP 端口：释放后可能被其他进程占用）。
    let unique = format!("app-mcp-missing-{}-{}", std::process::id(), rand_suffix());
    let mut c = NativeConfig::new("test-app", "测试");
    c.host_url = if cfg!(windows) {
        format!(r"pipe:\\.\pipe\{unique}")
    } else {
        format!("unix:{}", std::env::temp_dir().join(format!("{unique}.sock")).display())
    };
    let rec = Arc::new(Recorder::default());
    let client = NativeClient::new(c, Some(rec.clone())).unwrap();
    client.start();
    eventually("Backoff", || client.state().status == StateStatus::Backoff);
    let state = client.state();
    assert!(state.retry_in_ms.is_some());
    // 连接失败按系统错误归类（spec/protocol.md 10.1）：端点不存在 → HOST_NOT_RUNNING
    assert_eq!(state.code.as_deref(), Some("HOST_NOT_RUNNING"), "{state:?}");
    assert!(state.reason.as_deref().is_some_and(|r| r.contains("失败")), "{state:?}");
    assert_eq!(client.connection_id(), None);
    eventually("再次尝试", || {
        rec.statuses()
            .iter()
            .filter(|s| **s == StateStatus::Connecting)
            .count()
            >= 2
    });
}

// 命名管道端点只在 Windows 上被配置接受（其他平台在配置校验时即拒绝）。
#[cfg(windows)]
#[test]
fn overlong_pipe_name_backs_off_with_path_too_long() {
    // 超过 MAX_PIPE_NAME_CHARS 的管道名：连接前即拒绝，错误码 IPC_PATH_TOO_LONG（spec/protocol.md 10.1）。
    let name = format!(r"\\.\pipe\{}", "a".repeat(app_mcp_protocol::endpoint::MAX_PIPE_NAME_CHARS));
    let mut c = NativeConfig::new("test-app", "测试");
    c.host_url = format!("pipe:{name}");
    let client = NativeClient::new(c, None).unwrap();
    client.start();
    eventually("Backoff", || client.state().status == StateStatus::Backoff);
    let state = client.state();
    assert_eq!(state.code.as_deref(), Some("IPC_PATH_TOO_LONG"), "{state:?}");
}

struct DropCounter(Arc<AtomicUsize>);
impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct CountingHandler(#[allow(dead_code)] DropCounter);
impl ToolHandler for CountingHandler {
    fn invoke(&self, call: CallHandle) {
        let _ = call.complete(None, vec![]);
    }
}

#[test]
fn drop_stops_and_joins_threads() {
    let host = MockHost::start();
    let rec = Arc::new(Recorder::default());
    let dropped = Arc::new(AtomicUsize::new(0));
    let client = NativeClient::new(config(&host), Some(rec.clone())).unwrap();
    let tool = client
        .register_tool(
            ToolSpec::new("t", "t"),
            Arc::new(CountingHandler(DropCounter(dropped.clone()))),
        )
        .unwrap();
    connect(&host, &client);
    let clone = client.clone();
    drop(client);
    assert_eq!(clone.state().status, StateStatus::Connected);
    drop(clone);
    // 停止时释放 handler。
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    // drop 返回时分发线程已执行完剩余回调（包括 Stopped 状态）并被 join。
    assert_eq!(rec.statuses().last(), Some(&StateStatus::Stopped));
    host.wait_closed();
    // 仍存活的句柄可以安全使用。
    assert_eq!(tool.set_enabled(false), Err(NativeError::Stopped));
    // 句柄释放后没有任何线程再持有共享状态（listener 只剩测试持有的引用）。
    drop(tool);
    assert_eq!(Arc::strong_count(&rec), 1);
}

/// 最后一个 NativeClient 克隆在分发线程（用户回调）里被丢弃，不能死锁。
struct DropsClient(Mutex<Option<NativeClient>>, Mutex<Sender<()>>);
impl ToolHandler for DropsClient {
    fn invoke(&self, call: CallHandle) {
        call.complete(None, vec![]).unwrap();
        let client = self.0.lock().unwrap().take();
        drop(client);
        self.1.lock().unwrap().send(()).unwrap();
    }
}

#[test]
fn last_clone_dropped_on_dispatch_thread() {
    let host = MockHost::start();
    let client = NativeClient::new(config(&host), None).unwrap();
    let (tx, rx) = channel();
    let handler = Arc::new(DropsClient(
        Mutex::new(Some(client.clone())),
        Mutex::new(tx),
    ));
    client
        .register_tool(ToolSpec::new("t", "t"), handler)
        .unwrap();
    connect(&host, &client);
    drop(client);
    // 注意：核心的 stop 会丢弃尚未取走的 Send 事件，紧接着 drop 的调用结果不一定能发出，这里不检查响应。
    host.invoke("c1", "t", json!({}));
    rx.recv_timeout(WAIT).unwrap();
    host.wait_closed();
}

/// 测试用的随机后缀。
fn rand_suffix() -> u64 {
    use std::hash::{BuildHasher, RandomState};
    RandomState::new().hash_one(std::time::SystemTime::now())
}
