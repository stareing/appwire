//! 本地 IPC 传输（spec/protocol.md 1.2）：真实 native 客户端经 Unix 域套接字 / 命名管道连接嵌入式 Hub。
//!
//! 覆盖：往返调用与对端进程号、单实例（同一端点第二个 Hub 返回 `AddrInUse`）、停止后可重新绑定、
//! Unix 上的残留套接字清理与拒绝覆盖普通文件。

use std::sync::Arc;
use std::time::Duration;

use app_mcp_hub::{CallRequest, Hub, HubConfig};
use app_mcp_native::{CallHandle, NativeClient, NativeConfig, ToolHandler, ToolSpec};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);

/// 每个测试独立的端点。
fn endpoint(tag: &str) -> String {
    let pid = std::process::id();
    #[cfg(unix)]
    {
        let dir = std::env::temp_dir().join(format!("app-mcp-ipc-{pid}-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        format!("unix:{}", dir.join("hub.sock").display())
    }
    #[cfg(windows)]
    {
        format!(r"pipe:\\.\pipe\app-mcp-test-{pid}-{tag}")
    }
}

fn config(endpoint: &str) -> HubConfig {
    HubConfig {
        listen: None,
        ipc_endpoint: Some(endpoint.to_owned()),
        ..Default::default()
    }
}

struct Add;
impl ToolHandler for Add {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or(Value::Null);
        let sum = args["a"].as_i64().unwrap_or(0) + args["b"].as_i64().unwrap_or(0);
        let _ = call.complete(Some(&json!({ "sum": sum }).to_string()), vec![]);
    }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    tokio::time::timeout(T, async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("等待超时：{what}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn native_roundtrip_over_ipc() {
    let ep = endpoint("roundtrip");
    let hub = Hub::start(config(&ep)).await.unwrap();
    assert_eq!(hub.ipc_endpoint(), Some(ep.as_str()));
    assert_eq!(hub.listen_addr(), None);

    let mut c = NativeConfig::new("calc", "计算器");
    c.host_url = ep.clone();
    c.instance_id = Some("calc-ipc".into());
    c.launch_token = Some(String::new());
    let client = NativeClient::new(c, None).unwrap();
    let mut spec = ToolSpec::new("math.add", "加法");
    spec.input_schema_json =
        Some(r#"{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}}}"#.into());
    client.register_tool(spec, Arc::new(Add)).unwrap();
    client.start();

    eventually("App 经 IPC 连上", || {
        hub.apps().iter().any(|a| a.app_id == "calc" && a.connected)
    })
    .await;
    let out = hub
        .call_tool(CallRequest::new("calc.math.add", json!({"a": 40, "b": 2})))
        .await
        .unwrap();
    assert_eq!(out.result.unwrap()["sum"], 42);

    // 对端进程号由操作系统提供（本测试进程）。
    let apps = hub.apps();
    let app = apps.iter().find(|a| a.app_id == "calc").unwrap();
    assert_eq!(app.instances[0].pid, Some(std::process::id()));

    client.stop();
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn single_instance_per_endpoint() {
    let ep = endpoint("single");
    let first = Hub::start(config(&ep)).await.unwrap();
    let second = Hub::start(config(&ep)).await;
    assert_eq!(
        second.err().map(|e| e.kind()),
        Some(std::io::ErrorKind::AddrInUse)
    );
    first.shutdown().await;
    // 停止后端点释放，可以重新绑定。
    tokio::time::sleep(Duration::from_millis(50)).await;
    let again = Hub::start(config(&ep)).await.unwrap();
    again.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_non_ipc_endpoint() {
    for bad in ["ws://127.0.0.1:0", "relative.sock"] {
        let r = Hub::start(config(bad)).await;
        assert_eq!(
            r.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::InvalidInput),
            "{bad}"
        );
    }
}

/// 套接字路径超过 `sun_path` 上限：启动失败，错误带 `IPC_PATH_TOO_LONG` 与建议，且不创建目录；
/// 原生 SDK 连接同一路径时进入带同一错误码的 `backoff`。
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn rejects_too_long_unix_socket_path() {
    use app_mcp_protocol::endpoint::MAX_UNIX_SOCKET_PATH_BYTES;
    use app_mcp_protocol::{ConnectionErrorCode, ConnectionIssue};

    let base = std::env::temp_dir().join(format!("app-mcp-ipc-{}-long", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let pad = MAX_UNIX_SOCKET_PATH_BYTES.saturating_sub(base.as_os_str().len()) + 1;
    let dir = base.join("d".repeat(pad.min(200)));
    let path = dir.join("hub.sock");
    assert!(path.as_os_str().len() > MAX_UNIX_SOCKET_PATH_BYTES);
    let ep = format!("unix:{}", path.display());

    let err = Hub::start(config(&ep)).await.err().expect("超长路径应启动失败");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    let issue = err.get_ref().and_then(|e| e.downcast_ref::<ConnectionIssue>()).expect("错误内含 ConnectionIssue");
    assert_eq!(issue.code, ConnectionErrorCode::IpcPathTooLong);
    let text = err.to_string();
    assert!(text.starts_with("[IPC_PATH_TOO_LONG]") && text.contains("--ipc-endpoint"), "{text}");
    assert!(!base.exists(), "超长路径不应创建目录");

    let mut c = NativeConfig::new("long-path", "超长路径");
    c.host_url = ep;
    let client = NativeClient::new(c, None).unwrap();
    client.start();
    eventually("backoff", || client.state().code.as_deref() == Some("IPC_PATH_TOO_LONG")).await;
    client.stop();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn unix_socket_file_lifecycle() {
    use std::os::unix::fs::PermissionsExt;

    let ep = endpoint("unixfile");
    let path = std::path::PathBuf::from(ep.strip_prefix("unix:").unwrap());
    let dir = path.parent().unwrap().to_path_buf();

    // 新建的目录为 0700、套接字为 0600；停止后删除套接字。
    let hub = Hub::start(config(&ep)).await.unwrap();
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    hub.shutdown().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!path.exists(), "停止后应删除套接字文件");

    // 残留的套接字文件（无人监听）：删除后重新绑定。
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    let hub = Hub::start(config(&ep)).await.unwrap();
    hub.shutdown().await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 路径上是普通文件：拒绝覆盖。
    std::fs::write(&path, b"not a socket").unwrap();
    let r = Hub::start(config(&ep)).await;
    assert_eq!(r.err().map(|e| e.kind()), Some(std::io::ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(&path).unwrap(), b"not a socket");

    // 目录对其他用户可写：拒绝。
    std::fs::remove_file(&path).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    let r = Hub::start(config(&ep)).await;
    assert_eq!(r.err().map(|e| e.kind()), Some(std::io::ErrorKind::PermissionDenied));

    let _ = std::fs::remove_dir_all(&dir);
}
