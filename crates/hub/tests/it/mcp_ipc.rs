//! MCP over IPC 与 `/status`（spec/protocol.md 1.3、10；spec/hub-api.md 3.6、3.8）。
//!
//! 这也是厂商 Agent 经本地 IPC 接入 Hub 的示例：
//!
//! - Unix：rmcp 自带的 [`rmcp::transport::UnixSocketHttpClient`] + `StreamableHttpClientTransport::with_client`
//!   就是一个完整的 MCP 客户端（initialize → tools/list → tools/call），URL 只用于 `Host` 头，固定 `http://localhost/mcp`；
//! - Windows：命名管道上同样是 HTTP/1.1，下面用 hyper 的连接级客户端直接发 `initialize`。
//!
//! IPC 上不需要令牌（对端用户已由操作系统核对）；客户端应自己核对监听方是同一用户（spec/protocol.md 1.4）。
//! TCP 上的 `/status` 必须携带令牌。

use std::sync::Arc;
use std::time::Duration;

use app_mcp_hub::{Hub, HubConfig, HubStatus, HttpOptions};
use app_mcp_native::{CallHandle, NativeClient, NativeConfig, ToolHandler, ToolSpec};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};

const T: Duration = Duration::from_secs(10);

fn endpoint(tag: &str) -> String {
    let pid = std::process::id();
    let n: u32 = rand::random();
    #[cfg(unix)]
    {
        let dir = std::env::temp_dir().join(format!("app-mcp-mcpipc-{pid}-{tag}-{n:x}"));
        format!("unix:{}", dir.join("hub.sock").display())
    }
    #[cfg(windows)]
    {
        format!(r"pipe:\\.\pipe\app-mcp-test-mcpipc-{pid}-{tag}-{n:x}")
    }
}

fn config(ep: &str) -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: Some(ep.to_owned()),
        mcp_http: true,
        http: HttpOptions {
            token: Some("secret".into()),
            require_token_without_origin: true,
            ..Default::default()
        },
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

/// 一个经 IPC 连接的原生 App（`calc`，工具 `math.add`）。
fn start_app(ep: &str) -> NativeClient {
    let mut c = NativeConfig::new("calc", "计算器");
    c.host_url = ep.to_owned();
    c.instance_id = Some("calc-1".into());
    c.launch_token = Some(String::new());
    let client = NativeClient::new(c, None).expect("native client");
    let mut spec = ToolSpec::new("math.add", "加法");
    spec.input_schema_json =
        Some(r#"{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}}}"#.into());
    client.register_tool(spec, Arc::new(Add)).expect("register");
    client.start();
    client
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

/// 打开到 IPC 端点的连接（Unix 域套接字 / 命名管道）。
async fn open_ipc(ep: &str) -> Box<dyn Io> {
    #[cfg(unix)]
    {
        let path = ep.strip_prefix("unix:").expect("unix endpoint");
        Box::new(tokio::net::UnixStream::connect(path).await.expect("connect unix socket"))
    }
    #[cfg(windows)]
    {
        let name = ep.strip_prefix("pipe:").expect("pipe endpoint");
        Box::new(
            tokio::net::windows::named_pipe::ClientOptions::new()
                .open(name)
                .expect("open named pipe"),
        )
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// 经一条新连接发一个 HTTP/1.1 请求，返回 (状态码, 响应头, 响应体)。
async fn send(
    io: Box<dyn Io>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, http::HeaderMap, String) {
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await.expect("handshake");
    tokio::spawn(conn);
    let mut req = http::Request::builder().method(method).uri(path).header(http::header::HOST, "localhost");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let req = req.body(Full::new(Bytes::from(body.to_owned()))).expect("request");
    let resp = tokio::time::timeout(T, sender.send_request(req)).await.expect("timeout").expect("send");
    let (parts, body) = resp.into_parts();
    let bytes = if parts.headers.get(http::header::CONTENT_TYPE).is_some_and(|v| v.as_bytes().starts_with(b"text/event-stream")) {
        // SSE：读到第一个带 JSON-RPC 消息的事件为止（之前可能有空的启动事件），流本身不会结束。
        let mut body = body;
        let mut buf = Vec::new();
        while let Some(frame) = tokio::time::timeout(T, body.frame()).await.expect("sse timeout") {
            if let Ok(data) = frame.expect("frame").into_data() {
                buf.extend_from_slice(&data);
            }
            let text = String::from_utf8_lossy(&buf);
            if text.find("\"jsonrpc\"").is_some_and(|i| text[i..].contains("\n\n")) {
                break;
            }
        }
        buf
    } else {
        body.collect().await.expect("body").to_bytes().to_vec()
    };
    (parts.status.as_u16(), parts.headers, String::from_utf8_lossy(&bytes).into_owned())
}

async fn status_over_ipc(ep: &str) -> HubStatus {
    let (code, _, body) = send(open_ipc(ep).await, "GET", "/status", &[], "").await;
    assert_eq!(code, 200, "{body}");
    serde_json::from_str(&body).expect("status json")
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn rmcp_client_over_unix_socket() {
    use rmcp::ServiceExt;
    use rmcp::model::CallToolRequestParams;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    use rmcp::transport::{StreamableHttpClientTransport, UnixSocketHttpClient};

    let ep = endpoint("rmcp");
    let hub = Hub::start(config(&ep)).await.expect("hub");
    let app = start_app(&ep);
    eventually("App 经 IPC 连上", || hub.apps().iter().any(|a| a.app_id == "calc" && a.connected)).await;

    // 厂商客户端：IPC 上不带令牌（TCP 上 --auth all 的同一 Hub 会要求令牌）。
    let path = ep.strip_prefix("unix:").expect("unix endpoint");
    let url = "http://localhost/mcp";
    let transport = StreamableHttpClientTransport::with_client(
        UnixSocketHttpClient::new(path, url),
        StreamableHttpClientTransportConfig::with_uri(url),
    );
    let client = ().serve(transport).await.expect("initialize over IPC");
    let names: Vec<String> = client
        .list_all_tools()
        .await
        .expect("tools/list")
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    assert!(names.iter().any(|n| n == "calc.math.add"), "{names:?}");
    let mut args = serde_json::Map::new();
    args.insert("a".into(), json!(40));
    args.insert("b".into(), json!(2));
    let r = client
        .call_tool(CallToolRequestParams::new("calc.math.add").with_arguments(args))
        .await
        .expect("tools/call");
    let text: String = r.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect();
    assert!(text.contains("42"), "{text}");

    eventually("MCP 会话计入 /status", || hub.status().mcp_sessions >= 1).await;
    let _ = client.cancel().await;
    app.stop();
    hub.shutdown().await;
}

/// 第 12 项 S7：MCP 2026-07-28 客户端经 IPC 上的 Streamable HTTP（rmcp 无状态路径）：`server/discover` 协商 2026-07-28、
/// 列工具、调用、`subscriptions/listen` 在 App 上线 / 下线后收到 `tools/list_changed`；不产生 MCP 会话。
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn modern_rmcp_client_over_unix_socket_with_listen() {
    use rmcp::ClientServiceExt;
    use rmcp::model::{CallToolRequestParams, GetMeta, ProtocolVersion, SubscriptionFilter};
    use rmcp::service::{ClientCacheConfig, ClientLifecycleMode};
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
    use rmcp::transport::{StreamableHttpClientTransport, UnixSocketHttpClient};

    let ep = endpoint("modern");
    let hub = Hub::start(config(&ep)).await.expect("hub");
    let path = ep.strip_prefix("unix:").expect("unix endpoint");
    let url = "http://localhost/mcp";
    let transport = StreamableHttpClientTransport::with_client(
        UnixSocketHttpClient::new(path, url),
        StreamableHttpClientTransportConfig::with_uri(url),
    );
    let lifecycle = ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] };
    let client = ().serve_with_lifecycle(transport, lifecycle).await.expect("server/discover over IPC");
    client.peer().set_response_cache_config(ClientCacheConfig::disabled()).await;
    assert_eq!(client.peer().peer_info().map(|i| i.protocol_version.clone()), Some(ProtocolVersion::V_2026_07_28));
    let names = || async {
        client.list_all_tools().await.expect("tools/list").into_iter().map(|t| t.name.to_string()).collect::<Vec<_>>()
    };
    assert!(!names().await.iter().any(|n| n == "calc.math.add"));

    let mut sub = client
        .peer()
        .listen(SubscriptionFilter::builder().tools_list_changed().build())
        .await
        .expect("subscriptions/listen");
    eventually("listen 流计入 /status", || hub.status().mcp_listen_streams == Some(1)).await;
    let next_method = |n: rmcp::model::ServerNotification| {
        assert!(n.get_meta().subscription_id().is_some(), "带 subscriptionId");
        serde_json::to_value(&n).expect("json")["method"].as_str().unwrap_or_default().to_owned()
    };

    let app = start_app(&ep);
    let n = tokio::time::timeout(T, sub.next()).await.expect("等待 list_changed").expect("listen").expect("通知");
    assert_eq!(next_method(n), "notifications/tools/list_changed");
    assert!(names().await.iter().any(|n| n == "calc.math.add"));
    let mut args = serde_json::Map::new();
    args.insert("a".into(), json!(40));
    args.insert("b".into(), json!(2));
    let r = client
        .call_tool(CallToolRequestParams::new("calc.math.add").with_arguments(args))
        .await
        .expect("tools/call");
    let text: String = r.content.iter().filter_map(|c| c.as_text().map(|t| t.text.clone())).collect();
    assert!(text.contains("42"), "{text}");

    app.stop();
    let n = tokio::time::timeout(T, sub.next()).await.expect("等待 list_changed").expect("listen").expect("通知");
    assert_eq!(next_method(n), "notifications/tools/list_changed");
    assert_eq!(hub.status().mcp_sessions, 0, "无会话请求不产生 MCP 会话");

    // 客户端关闭流（HTTP 断开）：Hub 移除订阅方
    sub.cancel().await.expect("cancel");
    eventually("listen 流移除", || hub.status().mcp_listen_streams == Some(0)).await;
    let _ = client.cancel().await;
    hub.shutdown().await;
}

/// 第 12 项 S7：直接发一个 2026-07-28 请求（不经 rmcp 客户端）——响应不带 `Mcp-Session-Id`，结果带 `resultType: complete`。
#[tokio::test(flavor = "multi_thread")]
async fn raw_modern_request_over_ipc_has_no_session() {
    let ep = endpoint("raw-modern");
    let hub = Hub::start(config(&ep)).await.expect("hub");
    let req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/list",
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "vendor-agent", "version": "1"}
        }}
    })
    .to_string();
    let (code, headers, body) = send(
        open_ipc(&ep).await,
        "POST",
        "/mcp",
        &[
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", "tools/list"),
        ],
        &req,
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert!(headers.get("mcp-session-id").is_none(), "{headers:?}");
    assert!(body.contains(r#""resultType":"complete""#) && body.contains("apps.list"), "{body}");
    hub.shutdown().await;
}

/// 命名管道 / Unix 套接字上直接发 MCP `initialize`（不依赖 rmcp 客户端）。
#[tokio::test(flavor = "multi_thread")]
async fn raw_initialize_over_ipc() {
    let ep = endpoint("raw");
    let hub = Hub::start(config(&ep)).await.expect("hub");
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "vendor-agent", "version": "1"}}
    })
    .to_string();
    let (code, headers, body) = send(
        open_ipc(&ep).await,
        "POST",
        "/mcp",
        &[("content-type", "application/json"), ("accept", "application/json, text/event-stream")],
        &init,
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert!(headers.get("mcp-session-id").is_some(), "{headers:?}");
    assert!(body.contains("app-mcp-host"), "{body}");
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn status_over_ipc_and_tcp_auth() {
    let ep = endpoint("status");
    let hub = Hub::start(config(&ep)).await.expect("hub");
    let app = start_app(&ep);
    eventually("App 经 IPC 连上", || hub.apps().iter().any(|a| a.app_id == "calc" && a.connected)).await;

    let st = status_over_ipc(&ep).await;
    assert_eq!(st.identity.pid, std::process::id());
    assert_eq!(st.ipc_endpoint.as_deref(), Some(ep.as_str()));
    assert!(st.started_at_ms > 0 && st.mcp_http);
    assert!(st.auth.token_configured && st.auth.token_required_without_origin);
    let calc = st.apps.iter().find(|a| a.app_id == "calc").expect("calc");
    assert_eq!(calc.state, app_mcp_hub::AppState::Connected);
    assert_eq!(calc.instances[0].state, app_mcp_hub::InstanceState::Connected);
    let cid = calc.instances[0].info.connection_id.clone().expect("connection id");
    assert!(cid.contains('-'), "{cid}");
    assert_eq!(calc.instances[0].info.pid, Some(std::process::id()));
    // /status 与 Hub::status 同源。实例的活跃时间、可见性会随 App 的消息变化，只比较稳定的部分。
    let api = hub.status();
    assert_eq!((&st.identity, &st.listen, &st.ipc_endpoint, st.started_at_ms), (&api.identity, &api.listen, &api.ipc_endpoint, api.started_at_ms));
    let ids = |s: &app_mcp_hub::HubStatus| -> Vec<(String, Vec<Option<String>>)> {
        s.apps.iter().map(|a| (a.app_id.clone(), a.instances.iter().map(|i| i.info.connection_id.clone()).collect())).collect()
    };
    assert_eq!(ids(&st), ids(&api));

    // TCP：没有令牌 401，令牌正确 200。
    let addr = hub.listen_addr().expect("listen");
    let tcp = || async { Box::new(tokio::net::TcpStream::connect(addr).await.expect("tcp")) as Box<dyn Io> };
    let (code, _, _) = send(tcp().await, "GET", "/status", &[], "").await;
    assert_eq!(code, 401);
    let (code, _, _) = send(tcp().await, "GET", "/status", &[("authorization", "Bearer nope")], "").await;
    assert_eq!(code, 401);
    let (code, _, body) = send(tcp().await, "GET", "/status", &[("authorization", "Bearer secret")], "").await;
    assert_eq!(code, 200, "{body}");
    let (code, _, _) = send(tcp().await, "POST", "/status", &[("authorization", "Bearer secret")], "").await;
    assert_eq!(code, 405);
    app.stop();
    hub.shutdown().await;
}

/// 未配置令牌时 TCP 上的 `/status` 一律 403（只能经 IPC）。
#[tokio::test(flavor = "multi_thread")]
async fn status_tcp_without_token_is_forbidden() {
    let ep = endpoint("notoken");
    let hub = Hub::start(HubConfig { http: HttpOptions::default(), ..config(&ep) }).await.expect("hub");
    let addr = hub.listen_addr().expect("listen");
    let io = Box::new(tokio::net::TcpStream::connect(addr).await.expect("tcp"));
    let (code, _, body) = send(io, "GET", "/status", &[], "").await;
    assert_eq!(code, 403);
    assert!(body.contains("IPC"), "{body}");
    assert!(!status_over_ipc(&ep).await.auth.token_configured);
    hub.shutdown().await;
}
