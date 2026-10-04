//! 手写 App（WebSocket 直连 `/app`）：声明原样发送（`cache`、`deprecated` 等不依赖各语言 SDK 的注册选项），
//! 自动回答 `tools/invoke` / `resources/read` 并计数——计数不变即 Hub 没有转发（命中缓存）。
//! 结果缓存（`result_cache`）与工具演进（`schema_evolution`）的集成测试共用。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::Hub;
use app_mcp_protocol::{ResourcesSyncParams, ToolsSyncParams, tools_hash};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as Ws;

const T: Duration = Duration::from_secs(10);

/// 计数与预设回答（App 连接任务与测试共享）。
#[derive(Default)]
struct Book {
    /// 工具局部名 → 收到的 `tools/invoke` 数。
    invokes: HashMap<String, usize>,
    /// 资源名 → 收到的 `resources/read` 数。
    reads: HashMap<String, usize>,
    /// 工具局部名 → 依次使用的预设回答（`{"result": …}` 或 `{"error": …}`）；用完后回默认结果。
    scripted: HashMap<String, VecDeque<Value>>,
}

pub struct FakeApp {
    out: mpsc::UnboundedSender<Option<String>>,
    /// App 收到的其他消息（响应、`app/lease` 等）。
    inbox: mpsc::UnboundedReceiver<Value>,
    book: Arc<Mutex<Book>>,
    pub hash: String,
    next_id: i64,
}

/// 工具声明：`risk` 为 read / write；`cache` 为 `null` 时不声明。
pub fn tool(name: &str, risk: &str, cache: Value) -> Value {
    let mut t = json!({"name": name, "description": name, "inputSchema": {"type": "object"}, "risk": risk});
    if !cache.is_null() {
        t["cache"] = cache;
    }
    t
}

/// 资源声明。
pub fn resource(name: &str, cache: Value) -> Value {
    let mut r = json!({"name": name, "description": name});
    if !cache.is_null() {
        r["cache"] = cache;
    }
    r
}

impl FakeApp {
    /// 连接并完成握手（`tools/sync`、`resources/sync`、可见、`app/ready`）。
    pub async fn connect(hub: &Hub, app_id: &str, instance_id: &str, tools: Value, resources: Value) -> FakeApp {
        let url = format!("ws://{}/app", hub.listen_addr().expect("listen"));
        let (ws, _) = tokio_tungstenite::connect_async(url).await.expect("ws");
        let (mut sink, mut stream) = ws.split();
        let hello = json!({
            "appId": app_id, "appName": app_id, "protocolVersion": "1", "sdkVersion": "0",
            "clientKind": "native", "instanceId": instance_id
        });
        sink.send(Ws::text(json!({"jsonrpc": "2.0", "id": 0, "method": "app/hello", "params": hello}).to_string()))
            .await
            .expect("hello");
        loop {
            let Some(Ok(Ws::Text(t))) = timeout(T, stream.next()).await.expect("握手超时") else { panic!("握手时断开") };
            let v: Value = serde_json::from_str(t.as_str()).expect("json");
            if v["id"] == 0 {
                assert!(v.get("error").is_none(), "握手失败：{v}");
                break;
            }
        }
        let note = |m: &str, p: Value| Ws::text(json!({"jsonrpc": "2.0", "method": m, "params": p}).to_string());
        for msg in [
            note("tools/sync", json!({ "tools": tools })),
            note("resources/sync", json!({ "resources": resources })),
            note("app/visibility", json!({"visibility": "visible", "focused": true})),
            note("app/ready", json!({})),
        ] {
            sink.send(msg).await.expect("send");
        }
        let t: ToolsSyncParams = serde_json::from_value(json!({ "tools": tools })).expect("tools");
        let r: ResourcesSyncParams = serde_json::from_value(json!({ "resources": resources })).expect("resources");
        let hash = tools_hash(&t, &r);

        let book = Arc::new(Mutex::new(Book::default()));
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Option<String>>();
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let shared = book.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    m = out_rx.recv() => match m {
                        Some(Some(t)) => { let _ = sink.send(Ws::text(t)).await; }
                        _ => { let _ = sink.close().await; break; }
                    },
                    m = stream.next() => {
                        let Some(Ok(Ws::Text(t))) = m else { break };
                        let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else { continue };
                        match answer(&shared, &v) {
                            Some(reply) => { let _ = sink.send(Ws::text(reply.to_string())).await; }
                            None => { let _ = in_tx.send(v); }
                        }
                    }
                }
            }
        });
        FakeApp { out: out_tx, inbox: in_rx, book, hash, next_id: 100 }
    }

    pub fn invokes(&self, tool: &str) -> usize {
        self.book.lock().unwrap().invokes.get(tool).copied().unwrap_or(0)
    }

    pub fn reads(&self, resource: &str) -> usize {
        self.book.lock().unwrap().reads.get(resource).copied().unwrap_or(0)
    }

    /// 下一次调用 `tool` 时用 `reply`（`{"result": …}` 或 `{"error": …}`）回答。
    pub fn script(&self, tool: &str, reply: Value) {
        self.book.lock().unwrap().scripted.entry(tool.to_owned()).or_default().push_back(reply);
    }

    /// 发通知（`resources/updated`、`tools/changed` 等）。
    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.out.send(Some(json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()));
    }

    /// 带唤醒描述请求休眠，等到 Hub 接受后断开。
    pub async fn sleep_and_close(mut self) {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({"jsonrpc": "2.0", "id": id, "method": "app/sleep",
            "params": {"reason": "idle", "toolsHash": self.hash, "wake": {"kind": "uri", "target": "fake-app"}}});
        let _ = self.out.send(Some(req.to_string()));
        let reply = timeout(T, async {
            loop {
                let v = self.inbox.recv().await.expect("连接已关闭");
                if v["id"] == id {
                    return v;
                }
            }
        })
        .await
        .expect("没有等到 app/sleep 的回复");
        assert_eq!(reply["result"]["accepted"], true, "{reply}");
        let _ = self.out.send(None);
    }
}

/// Hub 发来的请求的自动回答；其他消息返回 `None`（转入收件箱）。
fn answer(book: &Mutex<Book>, v: &Value) -> Option<Value> {
    let id = v.get("id")?.clone();
    let mut b = book.lock().unwrap();
    let reply = match v["method"].as_str()? {
        "ping" => json!({"result": {}}),
        "tools/invoke" => {
            let name = v["params"]["name"].as_str().unwrap_or_default().to_owned();
            let n = {
                let c = b.invokes.entry(name.clone()).or_default();
                *c += 1;
                *c
            };
            match b.scripted.get_mut(&name).and_then(VecDeque::pop_front) {
                Some(r) => r,
                None => json!({"result": {"data": {"tool": name, "n": n}}}),
            }
        }
        "resources/read" => {
            let name = v["params"]["name"].as_str().unwrap_or_default().to_owned();
            let c = b.reads.entry(name.clone()).or_default();
            *c += 1;
            json!({"result": {"contents": {"resource": name, "n": *c}}})
        }
        _ => json!({"result": {}}),
    };
    let mut out = json!({"jsonrpc": "2.0", "id": id});
    if let Value::Object(m) = reply {
        for (k, val) in m {
            out[k] = val;
        }
    }
    Some(out)
}
