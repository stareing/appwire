//! 集成测试：上游 MCP 服务器聚合、Streamable HTTP 传输。

use std::collections::BTreeMap;
use std::time::Duration;

use app_mcp_hub::{Hub as Host, HubConfig as HostConfig, UpstreamConfig};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ReadResourceRequestParams, ResourceContents,
};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

const T: Duration = Duration::from_secs(10);
const UPSTREAM_BIN: &str = env!("CARGO_BIN_EXE_app-mcp-test-upstream");

type Client = RunningService<RoleClient, ()>;

fn upstreams(names: &[&str]) -> BTreeMap<String, UpstreamConfig> {
    names
        .iter()
        .map(|n| {
            (
                n.to_string(),
                UpstreamConfig {
                    command: UPSTREAM_BIN.into(),
                    ..Default::default()
                },
            )
        })
        .collect()
}

fn config() -> HostConfig {
    HostConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        ..Default::default()
    }
}

async fn duplex_client(host: &Host) -> Client {
    let (c, s) = tokio::io::duplex(1 << 20);
    let session = host.mcp_session();
    tokio::spawn(async move {
        if let Ok(svc) = session.serve(s).await {
            let _ = svc.waiting().await;
        }
    });
    ().serve(c).await.expect("mcp initialize")
}

async fn http_client(addr: std::net::SocketAddr) -> Client {
    let transport = StreamableHttpClientTransport::from_uri(format!("http://{addr}/mcp"));
    ().serve(transport).await.expect("http mcp initialize")
}

async fn tool_names(client: &Client) -> Vec<String> {
    client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect()
}

async fn wait_tools(client: &Client, pred: impl Fn(&[String]) -> bool) -> Vec<String> {
    timeout(T, async {
        loop {
            let names = tool_names(client).await;
            if pred(&names) {
                return names;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("工具列表没有达到预期")
}

fn has(n: &[String], name: &str) -> bool {
    n.iter().any(|x| x == name)
}

async fn call(
    client: &Client,
    name: &str,
    args: Value,
) -> Result<CallToolResult, rmcp::ServiceError> {
    let map = match args {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    client
        .call_tool(CallToolRequestParams::new(name.to_owned()).with_arguments(map))
        .await
}

fn texts(r: &CallToolResult) -> Vec<String> {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

#[tokio::test]
async fn upstream_is_aggregated_and_callable() {
    let host = Host::start(HostConfig {
        upstreams: upstreams(&["echo"]),
        ..config()
    })
    .await
    .unwrap();
    let client = duplex_client(&host).await;
    wait_tools(&client, |n| has(n, "echo.echo")).await;

    // 首次调用附带由上游 instructions 生成的总览；结果原样转发
    let r = call(&client, "echo.echo", json!({"text": "hi"}))
        .await
        .unwrap();
    let t = texts(&r);
    assert_eq!(t.len(), 2, "{t:?}");
    assert!(t[0].contains("<app-overview app=\"echo\""), "{}", t[0]);
    assert!(t[0].contains("这里是正文部分"));
    assert_eq!(t[1], "echo: hi");
    let r = call(&client, "echo.echo", json!({"text": "again"}))
        .await
        .unwrap();
    assert_eq!(texts(&r), ["echo: again"]);

    // 上游协议错误原样透传
    assert!(call(&client, "echo.nope", json!({})).await.is_err());

    // apps.list / apps.overview / instructions
    let list = call(&client, "apps.list", json!({}))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let up = list["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["appId"] == "echo")
        .unwrap()
        .clone();
    assert_eq!(up["kind"], "upstream");
    assert_eq!(up["connected"], true);
    assert_eq!(up["summary"].as_str().unwrap().chars().count(), 100);
    let ov = call(&client, "apps.overview", json!({"appId": "echo"}))
        .await
        .unwrap();
    assert_eq!(ov.structured_content.unwrap()["source"], "upstream");
    let client2 = duplex_client(&host).await;
    let instructions = client2.peer_info().unwrap().instructions.clone().unwrap();
    assert!(instructions.contains("- echo"), "{instructions}");
    assert!(instructions.contains("回显服务器"), "{instructions}");

    // 资源：列出 + 读取（URI 映射）
    let resources = client.list_all_resources().await.unwrap();
    let res = resources
        .iter()
        .find(|r| r.name == "echo.greeting")
        .expect("upstream resource");
    assert_eq!(res.uri, "app-mcp://echo/demo%3A%2F%2Fgreeting");
    let read = client
        .read_resource(ReadResourceRequestParams::new(res.uri.clone()))
        .await
        .unwrap();
    let ResourceContents::TextResourceContents { text, uri, .. } = &read.contents[0] else {
        panic!("text")
    };
    assert_eq!(text, "你好，上游");
    assert_eq!(uri, &res.uri);

    // 上游工具列表变化 → 本侧列表更新
    call(&client, "echo.add_tool", json!({"name": "dyn"}))
        .await
        .unwrap();
    wait_tools(&client, |n| has(n, "echo.dyn")).await;

    // 上游退出 → 标记未连接，退避后重启
    let _ = call(&client, "echo.crash", json!({})).await;
    let list = timeout(T, async {
        loop {
            let v = call(&client, "apps.list", json!({}))
                .await
                .unwrap()
                .structured_content
                .unwrap();
            let up = v["apps"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["appId"] == "echo")
                .unwrap()
                .clone();
            if up["restarts"].as_u64().unwrap_or(0) >= 1 && up["connected"] == true {
                return up;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("上游没有被重启");
    assert_eq!(list["connected"], true);
    let names = wait_tools(&client, |n| has(n, "echo.echo")).await;
    assert!(!has(&names, "echo.dyn"), "重启后是新进程");
    let r = call(&client, "echo.echo", json!({"text": "back"}))
        .await
        .unwrap();
    assert_eq!(texts(&r).last().unwrap(), "echo: back");
}

#[tokio::test]
async fn invalid_or_conflicting_upstream_names_are_skipped() {
    let manifest =
        app_mcp_manifest::parse(r#"{"manifestVersion":1,"appId":"shop","name":"Shop"}"#).unwrap();
    let mut ups = upstreams(&["apps", "shop", "Bad_Name", "good"]);
    ups.insert(
        "broken".into(),
        UpstreamConfig {
            command: "/nonexistent/app-mcp-upstream".into(),
            ..Default::default()
        },
    );
    let host = Host::start(HostConfig {
        upstreams: ups,
        manifests: vec![manifest],
        ..config()
    })
    .await
    .unwrap();
    let client = duplex_client(&host).await;
    wait_tools(&client, |n| has(n, "good.echo")).await;
    let list = call(&client, "apps.list", json!({}))
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let mut ids: Vec<(String, String)> = list["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["appId"].as_str().unwrap().to_owned(),
                a["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            ("broken".into(), "upstream".into()),
            ("good".into(), "upstream".into()),
            ("shop".into(), "app".into())
        ]
    );
    let broken = list["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["appId"] == "broken")
        .unwrap();
    assert_eq!(broken["connected"], false);
    assert!(broken["lastError"].as_str().is_some());
    let r = call(&client, "broken.x", json!({})).await.unwrap();
    assert_eq!(r.is_error, Some(true));
    assert!(texts(&r)[0].starts_with("APP_DISCONNECTED"));
}

#[tokio::test]
async fn sdk_cannot_take_upstream_name() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as Ws;
    let host = Host::start(HostConfig {
        upstreams: upstreams(&["echo"]),
        ..config()
    })
    .await
    .unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/app", host.listen_addr().expect("listen")))
        .await
        .unwrap();
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": {
        "appId": "echo", "appName": "x", "protocolVersion": "1", "sdkVersion": "0", "clientKind": "web", "instanceId": "i"
    }});
    ws.send(Ws::text(hello.to_string())).await.unwrap();
    let Some(Ok(Ws::Text(t))) = timeout(T, ws.next()).await.unwrap() else {
        panic!("no reply")
    };
    let v: Value = serde_json::from_str(t.as_str()).unwrap();
    assert_eq!(v["result"]["status"], "rejected");
}

#[tokio::test]
async fn streamable_http_sessions() {
    let manifest = app_mcp_manifest::parse(
        r#"{"manifestVersion":1,"appId":"shop","name":"Shop","overview":{"summary":"商城简介"},
            "tools":[{"name":"t","description":"静态","inputSchema":{"type":"object"}}]}"#,
    )
    .unwrap();
    let host = Host::start(HostConfig {
        manifests: vec![manifest],
        ..config()
    })
    .await
    .unwrap();
    let addr = host.serve_http("127.0.0.1:0", false).await.unwrap();

    let a = http_client(addr).await;
    assert!(
        a.peer_info()
            .unwrap()
            .instructions
            .clone()
            .unwrap()
            .contains("商城简介")
    );
    let names = tool_names(&a).await;
    assert!(
        has(&names, "apps.list") && has(&names, "shop.t"),
        "{names:?}"
    );

    // 首次接触按会话计算
    let r = call(&a, "shop.t", json!({})).await.unwrap();
    assert_eq!(texts(&r).len(), 2);
    let r = call(&a, "shop.t", json!({})).await.unwrap();
    assert_eq!(texts(&r).len(), 1);
    let b = http_client(addr).await;
    let r = call(&b, "shop.t", json!({})).await.unwrap();
    assert_eq!(texts(&r).len(), 2, "新的 HTTP 会话应再次附带总览");
    let _ = a.cancel().await;
    let _ = b.cancel().await;
}

async fn raw_post(addr: std::net::SocketAddr, path: &str, origin: Option<&str>) -> String {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"raw","version":"0"}}}"#;
    let origin = origin
        .map(|o| format!("Origin: {o}\r\n"))
        .unwrap_or_default();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{origin}Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        addr.port(),
        body.len()
    );
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 256];
    let n = timeout(T, s.read(&mut buf)).await.unwrap().unwrap();
    String::from_utf8_lossy(&buf[..n])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn http_origin_and_bind_checks() {
    let host = Host::start(HostConfig {
        allow_origins: vec!["https://app.example.com".into()],
        ..config()
    })
    .await
    .unwrap();
    let addr = host.serve_http("127.0.0.1:0", false).await.unwrap();
    assert!(
        raw_post(addr, "/mcp", Some("http://evil.example"))
            .await
            .contains(" 403 ")
    );
    assert!(
        raw_post(addr, "/mcp", Some("http://localhost:3000"))
            .await
            .contains(" 200 ")
    );
    assert!(
        raw_post(addr, "/mcp", Some("https://app.example.com"))
            .await
            .contains(" 200 ")
    );
    assert!(raw_post(addr, "/mcp", None).await.contains(" 200 "));
    assert!(raw_post(addr, "/other", None).await.contains(" 404 "));

    let err = host.serve_http("0.0.0.0:0", false).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(host.serve_http("0.0.0.0:0", true).await.is_ok());
}
