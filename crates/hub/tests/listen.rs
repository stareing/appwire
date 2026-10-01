//! 合并端口与单实例（spec/protocol.md 1.3–1.7、spec/hub-api.md 3.6）：
//!
//! - 同一监听器上的 `/app`（WebSocket）、`/mcp`、`/healthz`，按路径的 Origin / 令牌规则；
//! - 根路径 `/` 的兼容；
//! - 握手结果与 `/healthz` 中的 Host 身份；
//! - `listen` 被占用时的备选地址；
//! - `run_dir`：单实例锁先于监听、登记文件的写入与删除。
//!
//! 所有 TCP 监听都绑定端口 0 并从监听器取实际地址。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use app_mcp_hub::{Health, HttpOptions, Hub, HubConfig};
use app_mcp_protocol::registry::{EndpointRegistry, LOCK_FILE, REGISTRY_FILE};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

const T: Duration = Duration::from_secs(10);

fn config() -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: None,
        ..Default::default()
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let n: u64 = rand::random();
    std::env::temp_dir().join(format!("app-mcp-listen-{tag}-{}-{n:x}", std::process::id()))
}

/// 发一个 HTTP/1.1 请求，返回 (状态码, 响应体)。
async fn http(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(T, s.read_to_end(&mut buf)).await.unwrap().unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_owned()).unwrap_or_default();
    (status, body)
}

/// 以 WebSocket 连接 `url` 并完成 `app/hello`，返回握手结果。
async fn hello(url: &str, origin: Option<&str>) -> Value {
    let mut req = url.into_client_request().unwrap();
    if let Some(o) = origin {
        req.headers_mut().insert("origin", o.parse().unwrap());
    }
    let (mut ws, _) = tokio::time::timeout(T, tokio_tungstenite::connect_async(req)).await.unwrap().unwrap();
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": {
        "appId": "probe", "appName": "探测", "protocolVersion": "1", "sdkVersion": "t",
        "clientKind": "native", "instanceId": "probe-1"
    }});
    ws.send(WsMessage::text(hello.to_string())).await.unwrap();
    loop {
        let msg = tokio::time::timeout(T, ws.next()).await.unwrap().unwrap().unwrap();
        if let WsMessage::Text(t) = msg {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["id"] == 1 {
                return v["result"].clone();
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn one_port_serves_app_mcp_and_healthz() {
    let hub = Hub::start(HubConfig {
        mcp_http: true,
        http: HttpOptions { token: Some("secret".into()), ..Default::default() },
        ..config()
    })
    .await
    .unwrap();
    let addr = hub.listen_addr().unwrap();
    assert_ne!(addr.port(), 0);

    // /app：WebSocket 握手，结果带 Host 身份
    let r = hello(&format!("ws://{addr}/app"), None).await;
    assert_eq!(r["status"], "paired");
    assert_eq!(r["service"], "app-mcp");
    assert_eq!(r["pid"], std::process::id());
    assert_eq!(r["user"].as_str(), app_mcp_protocol::identity::current_user().as_deref());
    assert_eq!(r["hostVersion"], hub.identity().version);

    // 根路径 /：兼容期内按 /app 处理
    let r = hello(&format!("ws://{addr}/"), Some("http://localhost:5173")).await;
    assert_eq!(r["status"], "paired");
    // 其他路径不是 App 连接
    assert!(tokio_tungstenite::connect_async(format!("ws://{addr}/other")).await.is_err());

    // /healthz：身份与监听信息，不需要令牌
    let (status, body) = http(addr, "GET", "/healthz", &[], "").await;
    assert_eq!(status, 200);
    let h: Health = serde_json::from_str(&body).unwrap();
    assert!(h.is_app_mcp());
    assert_eq!(&h.identity, hub.identity());
    assert_eq!(h.listen.as_deref(), Some(addr.to_string().as_str()));
    assert_eq!(h.app_path, "/app");
    assert_eq!(h.mcp_path.as_deref(), Some("/mcp"));
    assert!(h.token_required_for_browsers);
    // 不在允许列表的 Origin：/healthz、/mcp 403
    let evil = [("Origin", "https://evil.example")];
    assert_eq!(http(addr, "GET", "/healthz", &evil, "").await.0, 403);
    assert_eq!(http(addr, "POST", "/mcp", &evil, "{}").await.0, 403);

    // /mcp：浏览器来源必须带令牌；本地客户端可不带，带了就必须正确
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}
    }})
    .to_string();
    let mcp_headers = |extra: Vec<(&'static str, &'static str)>| {
        let mut h = vec![("Content-Type", "application/json"), ("Accept", "application/json, text/event-stream")];
        h.extend(extra);
        h
    };
    let (s, _) = http(addr, "POST", "/mcp", &mcp_headers(vec![("Origin", "http://localhost:5173")]), &init).await;
    assert_eq!(s, 401);
    let (s, _) = http(addr, "POST", "/mcp", &mcp_headers(vec![("Authorization", "Bearer nope")]), &init).await;
    assert_eq!(s, 401);
    let (s, body) = http(addr, "POST", "/mcp", &mcp_headers(vec![]), &init).await;
    assert_eq!(s, 200, "{body}");
    assert!(body.contains("serverInfo"), "{body}");

    // /app 不升级：426；未知路径：404
    assert_eq!(http(addr, "GET", "/app", &[], "").await.0, 426);
    assert_eq!(http(addr, "GET", "/nope", &[], "").await.0, 404);
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_is_off_unless_enabled_and_extra_listener_serves_it() {
    let hub = Hub::start(config()).await.unwrap();
    let addr = hub.listen_addr().unwrap();
    assert_eq!(http(addr, "POST", "/mcp", &[], "{}").await.0, 404);
    let (_, body) = http(addr, "GET", "/healthz", &[], "").await;
    let h: Health = serde_json::from_str(&body).unwrap();
    assert_eq!(h.mcp_path, None);

    // 额外监听器（如兼容期的旧 MCP 端口）：同一套路由，总是提供 /mcp
    let extra = hub.serve_http("127.0.0.1:0", false).await.unwrap();
    let (_, body) = http(extra, "GET", "/healthz", &[], "").await;
    let h: Health = serde_json::from_str(&body).unwrap();
    assert_eq!(h.mcp_path.as_deref(), Some("/mcp"));
    assert_eq!(h.listen.as_deref(), Some(addr.to_string().as_str()), "listen 为主服务地址");
    assert_eq!(hello(&format!("ws://{extra}/app"), None).await["status"], "paired");
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn busy_listen_uses_alternates() {
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = occupied.local_addr().unwrap().to_string();
    let hub = Hub::start(HubConfig {
        listen: Some(busy.clone()),
        listen_alternates: vec![busy.clone(), "127.0.0.1:0".into()],
        ..config()
    })
    .await
    .unwrap();
    let addr = hub.listen_addr().unwrap();
    assert_ne!(addr.to_string(), busy);
    assert_eq!(hello(&format!("ws://{addr}/app"), None).await["status"], "paired");
    hub.shutdown().await;

    // 没有备选：报 AddrInUse
    let err = Hub::start(HubConfig { listen: Some(busy.clone()), ..config() }).await.err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    // 备选也被占用：AddrInUse，消息列出尝试过的地址
    let err = Hub::start(HubConfig { listen: Some(busy.clone()), listen_alternates: vec![busy.clone()], ..config() })
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    assert!(err.to_string().contains("备选"), "{err}");
    drop(occupied);
}

#[tokio::test(flavor = "multi_thread")]
async fn run_dir_lock_and_registry() {
    let dir = temp_dir("run");
    let first = Hub::start(HubConfig { run_dir: Some(dir.clone()), ..config() }).await.unwrap();
    let addr = first.listen_addr().unwrap();
    assert!(dir.join(LOCK_FILE).exists());
    let reg = EndpointRegistry::read(&dir.join(REGISTRY_FILE)).unwrap().expect("登记文件");
    assert_eq!(reg, first.endpoint_registry());
    assert_eq!(reg.listen.as_deref(), Some(addr.to_string().as_str()));
    assert_eq!(reg.app_endpoint(), Some(format!("ws://{addr}/app")));
    assert_eq!(reg.identity.pid, std::process::id());
    assert!(reg.started_at_ms > 0);

    // 第二个 Hub：锁先于监听，返回 ResourceBusy（不会去绑定端口）
    let occupied_by_first = addr.to_string();
    let err = Hub::start(HubConfig {
        run_dir: Some(dir.clone()),
        listen: Some(occupied_by_first),
        ..config()
    })
    .await
    .err()
    .unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::ResourceBusy, "{err}");
    assert!(err.to_string().contains(&format!("pid {}", std::process::id())), "{err}");

    first.shutdown().await;
    assert!(!dir.join(REGISTRY_FILE).exists(), "停止时删除登记文件");
    // 锁已释放：可以再次启动（并行测试 fork 子进程时可能短暂持有句柄副本，稍等重试）
    let deadline = tokio::time::Instant::now() + T;
    let again = loop {
        match Hub::start(HubConfig { run_dir: Some(dir.clone()), ..config() }).await {
            Ok(h) => break h,
            Err(e) if e.kind() == std::io::ErrorKind::ResourceBusy && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("{e}"),
        }
    };
    drop(again);
    assert!(!dir.join(REGISTRY_FILE).exists(), "Drop 同样删除登记文件");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn non_loopback_listen_requires_allow_remote() {
    let err = Hub::start(HubConfig { listen: Some("0.0.0.0:0".into()), ..config() }).await.err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    let hub = Hub::start(HubConfig {
        listen: Some("0.0.0.0:0".into()),
        http: HttpOptions { allow_remote: true, ..Default::default() },
        ..config()
    })
    .await
    .unwrap();
    hub.shutdown().await;
}
