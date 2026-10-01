//! 多路复用连接（spec/protocol.md 第 9 节）：一条 WebSocket 承载多个实例。
//!
//! 手写客户端：`app/mux` 协商 → 每个通道 `open` + 独立握手 → 调用路由到正确的通道 →
//! 关闭一个通道只断开该实例 → 关闭连接断开全部实例；以及单实例连接不受影响、通道数上限。

use std::collections::HashMap;
use std::time::Duration;

use app_mcp_hub::{CallRequest, Hub, HubConfig};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as Ws;

const T: Duration = Duration::from_secs(10);

fn config() -> HubConfig {
    HubConfig {
        ws_addr: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        list_changed_debounce: Duration::from_millis(10),
        ..Default::default()
    }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + T;
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "等待超时：{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn instances(hub: &Hub, app: &str) -> Vec<String> {
    hub.apps()
        .into_iter()
        .find(|a| a.app_id == app)
        .map(|a| a.instances.into_iter().map(|i| i.instance_id).collect())
        .unwrap_or_default()
}

/// 多路复用客户端：后台任务按通道分发收到的帧，并自动回复 Host 的 `ping` 与 `tools/invoke`
/// （结果为 `{"instance": <实例 ID>}`，便于核对路由）。
struct Mux {
    out: mpsc::UnboundedSender<Option<String>>,
    /// 通道号 → 收到的 `close` 帧（原因）。
    closed: mpsc::UnboundedReceiver<(u32, Option<String>)>,
}

impl Mux {
    async fn connect(hub: &Hub) -> (Mux, Value) {
        let url = format!("ws://{}", hub.ws_addr().unwrap());
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let (mut sink, mut stream) = ws.split();
        sink.send(Ws::text(json!({"jsonrpc": "2.0", "id": 0, "method": "app/mux", "params": {"version": 1}}).to_string()))
            .await
            .unwrap();
        let Some(Ok(Ws::Text(t))) = timeout(T, stream.next()).await.unwrap() else { panic!("协商时断开") };
        let reply: Value = serde_json::from_str(t.as_str()).unwrap();

        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Option<String>>();
        let (closed_tx, closed_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut ids: HashMap<u64, String> = HashMap::new();
            loop {
                tokio::select! {
                    m = out_rx.recv() => match m {
                        Some(Some(t)) => {
                            // 记录各通道的 instanceId（来自 app/hello）
                            let v: Value = serde_json::from_str(&t).unwrap();
                            if v["msg"]["method"] == "app/hello" {
                                ids.insert(v["ch"].as_u64().unwrap(), v["msg"]["params"]["instanceId"].as_str().unwrap().to_owned());
                            }
                            let _ = sink.send(Ws::text(t)).await;
                        }
                        _ => { let _ = sink.close().await; break; }
                    },
                    m = stream.next() => {
                        let Some(Ok(Ws::Text(t))) = m else { break };
                        let f: Value = serde_json::from_str(t.as_str()).unwrap();
                        let ch = f["ch"].as_u64().unwrap();
                        match f["type"].as_str() {
                            Some("close") => { let _ = closed_tx.send((ch as u32, f["reason"].as_str().map(str::to_owned))); }
                            Some("msg") => {
                                let m = &f["msg"];
                                let reply = match m["method"].as_str() {
                                    Some("ping") => Some(json!({})),
                                    Some("tools/invoke") => Some(json!({"data": {"instance": ids.get(&ch).cloned()}})),
                                    _ => None,
                                };
                                if let Some(result) = reply {
                                    let r = json!({"type": "msg", "ch": ch, "msg": {"jsonrpc": "2.0", "id": m["id"], "result": result}});
                                    let _ = sink.send(Ws::text(r.to_string())).await;
                                }
                            }
                            other => panic!("未知帧类型 {other:?}"),
                        }
                    }
                }
            }
        });
        (Mux { out: out_tx, closed: closed_rx }, reply)
    }

    fn frame(&self, v: Value) {
        let _ = self.out.send(Some(v.to_string()));
    }

    fn msg(&self, ch: u32, msg: Value) {
        self.frame(json!({"type": "msg", "ch": ch, "msg": msg}));
    }

    /// 打开通道并完成握手、同步一个工具 `who`。
    fn open_instance(&self, ch: u32, instance_id: &str) {
        self.frame(json!({"type": "open", "ch": ch}));
        self.msg(ch, json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": {
            "appId": "tabs", "appName": "Tabs", "protocolVersion": "1", "sdkVersion": "0",
            "clientKind": "web", "instanceId": instance_id
        }}));
        let note = |m: &str, p: Value| json!({"jsonrpc": "2.0", "method": m, "params": p});
        self.msg(ch, note("tools/sync", json!({"tools": [{"name": "who", "description": "实例", "inputSchema": {"type": "object"}, "risk": "read"}]})));
        self.msg(ch, note("resources/sync", json!({"resources": []})));
        self.msg(ch, note("app/visibility", json!({"visibility": "visible", "focused": true})));
        self.msg(ch, note("app/ready", json!({})));
    }

    async fn expect_closed(&mut self) -> (u32, Option<String>) {
        timeout(T, self.closed.recv()).await.expect("没有等到 close 帧").expect("连接已关闭")
    }
}

async fn call_who(hub: &Hub, instance: &str) -> Value {
    let mut req = CallRequest::new("tabs.who", json!({}));
    req.instance_id = Some(instance.to_owned());
    let out = hub.call_tool(req).await.unwrap();
    out.result.unwrap()
}

#[tokio::test]
async fn channels_are_independent_instances() {
    let hub = Hub::start(config()).await.unwrap();
    let (mut mux, reply) = Mux::connect(&hub).await;
    assert_eq!(reply["result"], json!({"version": 1, "maxChannels": 64}));

    mux.open_instance(1, "tab-a");
    mux.open_instance(2, "tab-b");
    eventually("两个实例都已连接", || instances(&hub, "tabs").len() == 2).await;

    // 调用按实例路由到各自的通道
    assert_eq!(call_who(&hub, "tab-a").await, json!({"instance": "tab-a"}));
    assert_eq!(call_who(&hub, "tab-b").await, json!({"instance": "tab-b"}));

    // 客户端关闭通道 1：只断开 tab-a；Host 回送 close（已关闭通道的帧，客户端忽略）
    mux.frame(json!({"type": "close", "ch": 1}));
    eventually("tab-a 断开", || instances(&hub, "tabs") == vec!["tab-b".to_owned()]).await;
    assert_eq!(mux.expect_closed().await.0, 1);
    // 发往已关闭通道的消息被丢弃，不影响其他通道
    mux.msg(1, json!({"jsonrpc": "2.0", "method": "app/ready", "params": {}}));
    assert_eq!(call_who(&hub, "tab-b").await, json!({"instance": "tab-b"}));

    // 同一 instanceId 在新通道上重新连接：替换旧通道，Host 关闭旧通道
    mux.open_instance(3, "tab-b");
    assert_eq!(mux.expect_closed().await.0, 2);
    eventually("tab-b 改由通道 3 承载", || instances(&hub, "tabs") == vec!["tab-b".to_owned()]).await;
    assert_eq!(call_who(&hub, "tab-b").await, json!({"instance": "tab-b"}));

    // 关闭整条连接：全部实例断开
    let _ = mux.out.send(None);
    eventually("全部实例断开", || instances(&hub, "tabs").is_empty()).await;
    hub.shutdown().await;
}

#[tokio::test]
async fn rejected_hello_closes_only_that_channel() {
    let hub = Hub::start(config()).await.unwrap();
    let (mut mux, _) = Mux::connect(&hub).await;
    mux.open_instance(1, "ok");
    // 协议版本不兼容 → rejected → Host 关闭该通道
    mux.frame(json!({"type": "open", "ch": 2}));
    mux.msg(2, json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": {
        "appId": "tabs", "appName": "Tabs", "protocolVersion": "99", "sdkVersion": "0",
        "clientKind": "web", "instanceId": "bad"
    }}));
    assert_eq!(mux.expect_closed().await.0, 2);
    eventually("ok 仍然连接", || instances(&hub, "tabs") == vec!["ok".to_owned()]).await;
    assert_eq!(call_who(&hub, "ok").await, json!({"instance": "ok"}));
    hub.shutdown().await;
}

#[tokio::test]
async fn channel_limit_and_invalid_frames() {
    let hub = Hub::start(config()).await.unwrap();
    let (mut mux, _) = Mux::connect(&hub).await;
    // 无效帧与通道 0 被忽略，连接不断开
    mux.frame(json!({"type": "bogus", "ch": 1}));
    mux.frame(json!({"type": "open", "ch": 0}));
    for ch in 1..=64 {
        mux.frame(json!({"type": "open", "ch": ch}));
    }
    mux.frame(json!({"type": "open", "ch": 65}));
    let (ch, reason) = mux.expect_closed().await;
    assert_eq!(ch, 65);
    assert!(reason.unwrap_or_default().contains("上限"));
    // 已打开的通道照常握手
    mux.msg(1, json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": {
        "appId": "tabs", "appName": "Tabs", "protocolVersion": "1", "sdkVersion": "0",
        "clientKind": "web", "instanceId": "one"
    }}));
    mux.msg(1, json!({"jsonrpc": "2.0", "method": "app/ready", "params": {}}));
    eventually("通道 1 已握手", || instances(&hub, "tabs") == vec!["one".to_owned()]).await;
    hub.shutdown().await;
}

#[tokio::test]
async fn plain_connection_still_works_and_mux_is_first_message_only() {
    let hub = Hub::start(config()).await.unwrap();
    let url = format!("ws://{}", hub.ws_addr().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // 单实例连接：第一条是 app/hello
    ws.send(Ws::text(json!({"jsonrpc": "2.0", "id": 1, "method": "app/hello", "params": {
        "appId": "plain", "appName": "Plain", "protocolVersion": "1", "sdkVersion": "0",
        "clientKind": "web", "instanceId": "p1"
    }}).to_string()))
    .await
    .unwrap();
    let Some(Ok(Ws::Text(t))) = timeout(T, ws.next()).await.unwrap() else { panic!("断开") };
    let v: Value = serde_json::from_str(t.as_str()).unwrap();
    assert_eq!(v["result"]["status"], "paired");
    // 握手后再发 app/mux：不是第一条消息，按普通未知请求处理
    ws.send(Ws::text(json!({"jsonrpc": "2.0", "id": 2, "method": "app/mux", "params": {"version": 1}}).to_string()))
        .await
        .unwrap();
    let v = loop {
        let Some(Ok(Ws::Text(t))) = timeout(T, ws.next()).await.unwrap() else { panic!("断开") };
        let v: Value = serde_json::from_str(t.as_str()).unwrap();
        if v["id"] == 2 {
            break v;
        }
    };
    assert_eq!(v["error"]["code"], -32601);
    hub.shutdown().await;
}
