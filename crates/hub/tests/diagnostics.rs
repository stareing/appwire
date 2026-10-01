//! 诊断（spec/protocol.md 第 10 节）：连接 ID、拒绝错误码、`app/diagnostic` 上报与 `Hub::status`。

use std::time::Duration;

use app_mcp_hub::{AppState, Hub, HubConfig, HubEvent};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

const T: Duration = Duration::from_secs(10);

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn hub() -> Hub {
    Hub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        listen_alternates: Vec::new(),
        ipc_endpoint: None,
        ..Default::default()
    })
    .await
    .expect("hub")
}

/// 连接并发 `app/hello`，返回 (连接, 握手结果)。
async fn hello(hub: &Hub, app_id: &str, origin: Option<&str>, protocol: &str) -> (Ws, Value) {
    let addr = hub.listen_addr().expect("listen");
    let mut req = format!("ws://{addr}/app").into_client_request().expect("req");
    if let Some(o) = origin {
        req.headers_mut().insert("origin", o.parse().expect("origin"));
    }
    let (mut ws, _) = tokio::time::timeout(T, tokio_tungstenite::connect_async(req)).await.expect("t").expect("ws");
    let hello = json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": {
        "appId": app_id, "appName": "诊断", "protocolVersion": protocol, "sdkVersion": "t",
        "clientKind": "web", "instanceId": format!("{app_id}-1")
    }});
    ws.send(WsMessage::text(hello.to_string())).await.expect("send");
    loop {
        let msg = tokio::time::timeout(T, ws.next()).await.expect("t").expect("eof").expect("msg");
        if let WsMessage::Text(t) = msg {
            let v: Value = serde_json::from_str(t.as_str()).expect("json");
            if v["id"] == 1 {
                return (ws, v["result"].clone());
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_id_and_reports() {
    let hub = hub().await;
    let mut events = hub.events();
    let (mut ws, r) = hello(&hub, "page", Some("http://localhost:5173"), "1").await;
    assert_eq!(r["status"], "paired");
    let cid = r["connectionId"].as_str().expect("connectionId").to_owned();
    let (tag, seq) = cid.split_once('-').expect("tag-seq");
    assert_eq!(tag.len(), 6);
    assert!(seq.parse::<u64>().is_ok());
    assert!(r.get("code").is_none());

    // 每条连接的 ID 不同，同一 Hub 的启动标记相同
    let (_ws2, r2) = hello(&hub, "page2", None, "1").await;
    let cid2 = r2["connectionId"].as_str().expect("cid2");
    assert_ne!(cid2, cid);
    assert!(cid2.starts_with(&format!("{tag}-")));

    // 实例信息带连接 ID
    let deadline = tokio::time::Instant::now() + T;
    while !hub.apps().iter().any(|a| a.app_id == "page" && a.connected) {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let apps = hub.apps();
    let page = apps.iter().find(|a| a.app_id == "page").expect("page");
    assert_eq!(page.instances[0].connection_id.as_deref(), Some(cid.as_str()));

    // 上报此前的浏览器拦截
    let n = json!({"jsonrpc": "2.0", "method": "app/diagnostic", "params": {
        "code": "BLOCKED_LOCAL_NETWORK_ACCESS", "message": "本地网络访问未授权", "count": 3
    }});
    ws.send(WsMessage::text(n.to_string())).await.expect("send");
    let ev = tokio::time::timeout(T, async {
        loop {
            if let Ok(e @ HubEvent::AppDiagnostic { .. }) = events.recv().await {
                return e;
            }
        }
    })
    .await
    .expect("diagnostic event");
    assert_eq!(
        ev,
        HubEvent::AppDiagnostic {
            app_id: "page".into(),
            instance_id: "page-1".into(),
            code: "BLOCKED_LOCAL_NETWORK_ACCESS".into(),
            message: "本地网络访问未授权".into(),
            count: 3,
        }
    );
    let st = hub.status();
    assert_eq!(st.reports.len(), 1);
    let rep = &st.reports[0];
    assert_eq!((rep.app_id.as_str(), rep.connection_id.as_str(), rep.count), ("page", cid.as_str(), 3));
    assert!(rep.received_at_ms > 0);
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejection_codes_and_last_error() {
    let hub = hub().await;
    let (_ws, r) = hello(&hub, "evil", Some("https://evil.example"), "1").await;
    assert_eq!(r["status"], "rejected");
    assert_eq!(r["code"], "ORIGIN_NOT_ALLOWED");
    assert!(r["connectionId"].is_string());
    let (_ws, r) = hello(&hub, "old", None, "0").await;
    assert_eq!(r["code"], "PROTOCOL_INCOMPATIBLE");
    let (_ws, r) = hello(&hub, "Bad App", None, "1").await;
    assert_eq!(r["code"], "INVALID_HELLO");

    // 被拒的 App 记为最近错误（appId 不合法的不记录）
    let st = hub.status();
    let evil = st.apps.iter().find(|a| a.app_id == "evil").expect("被拒的 App 也列出");
    assert_eq!(evil.state, AppState::Disconnected);
    let err = evil.last_error.as_ref().expect("last error");
    assert_eq!(err.code.as_deref(), Some("ORIGIN_NOT_ALLOWED"));
    assert!(err.at_ms > 0 && err.message.contains("evil.example"), "{err:?}");
    let json = serde_json::to_value(&st).expect("json");
    assert!(json["apps"].as_array().expect("apps").iter().all(|a| a["appId"] != "Bad App"));
    hub.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn status_states() {
    let hub = hub().await;
    let st = hub.status();
    assert!(st.apps.is_empty());
    assert!(st.listen.is_some() && st.started_at_ms > 0);
    assert!(!st.mcp_http && !st.auth.token_configured);
    let (ws, _) = hello(&hub, "page", None, "1").await;
    let deadline = tokio::time::Instant::now() + T;
    loop {
        if hub.status().apps.iter().any(|a| a.app_id == "page" && a.state == AppState::Connected) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(ws);
    hub.shutdown().await;
}
