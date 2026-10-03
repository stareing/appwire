//! 测试：
//! - 桥接（与 Tauri 无关）：真实 `NativeClient` 经本地 IPC 连接嵌入式 Hub，用记录事件的 [`PageSink`] 充当页面；
//! - 插件：Tauri `MockRuntime`，经真实 IPC 命令 `plugin:app-mcp|op`（含 ACL）登记到同一个 Hub。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{CallRequest, Hub, HubConfig, ToolFilter};
use serde_json::{Value, json};

use super::bridge::{Bridge, PageSink, Sessions};
use super::*;

mod bridge_sessions;
mod bridge_tools;
mod mock;
mod navigation;

const T: Duration = Duration::from_secs(10);

fn endpoint(tag: &str) -> String {
    let pid = std::process::id();
    #[cfg(unix)]
    {
        let dir = std::env::temp_dir().join(format!("app-mcp-tauri-{pid}-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        format!("unix:{}", dir.join("hub.sock").display())
    }
    #[cfg(windows)]
    {
        format!(r"pipe:\\.\pipe\app-mcp-tauri-test-{pid}-{tag}")
    }
}

async fn start_hub(tag: &str) -> (Arc<Hub>, String) {
    let ep = endpoint(tag);
    let hub = Hub::start(HubConfig {
        listen: None,
        ipc_endpoint: Some(ep.clone()),
        ..Default::default()
    })
    .await
    .expect("启动 Hub");
    (Arc::new(hub), ep)
}

fn native_config(endpoint: &str, app_id: &str) -> NativeConfig {
    let mut c = NativeConfig::new(app_id, "Tauri 测试");
    c.host_url = endpoint.to_owned();
    c.launch_token = Some(String::new());
    c.client_kind = ClientKind::Hybrid;
    c
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

fn tool_names(hub: &Hub, app_id: &str) -> Vec<String> {
    let mut names: Vec<String> = hub
        .tools(&ToolFilter::default())
        .into_iter()
        .filter(|t| t.app_id == app_id)
        .map(|t| t.tool)
        .collect();
    names.sort();
    names
}

/// 记录事件的页面。
#[derive(Default)]
struct FakePage {
    events: Mutex<Vec<Value>>,
    gone: AtomicBool,
    /// [`PageSink::raise`] 的调用次数。
    raised: std::sync::atomic::AtomicUsize,
}

impl FakePage {
    fn take(&self, ty: &str) -> Vec<Value> {
        let mut events = self.events.lock().unwrap_or_else(|p| p.into_inner());
        let (hit, rest): (Vec<Value>, Vec<Value>) = events.drain(..).partition(|e| e["type"] == ty);
        *events = rest;
        hit
    }

    fn all(&self) -> Vec<Value> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

struct Sink(Arc<FakePage>);

impl PageSink for Sink {
    fn deliver(&self, event: &Value) -> bool {
        if self.0.gone.load(Ordering::SeqCst) {
            return false;
        }
        self.0
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(event.clone());
        true
    }

    fn raise(&self) {
        self.0.raised.fetch_add(1, Ordering::SeqCst);
    }
}

async fn shutdown(hub: Arc<Hub>) {
    if let Ok(hub) = Arc::try_unwrap(hub) {
        hub.shutdown().await;
    }
}

struct Fixture {
    hub: Arc<Hub>,
    bridge: Bridge,
    sessions: Arc<Sessions>,
}

impl Fixture {
    async fn new(tag: &str, accept: Option<Arc<bridge::AcceptFn>>) -> Self {
        let (hub, ep) = start_hub(tag).await;
        let sessions = Arc::new(Sessions::new());
        let listener = PluginListener {
            sessions: sessions.clone(),
            user: None,
            idle_exit: None,
        };
        let client = NativeClient::new(native_config(&ep, tag), Some(Arc::new(listener)))
            .expect("创建客户端");
        let bridge = Bridge::new(client, sessions.clone(), accept);
        Self {
            hub,
            bridge,
            sessions,
        }
    }

    fn op(&self, page: &Arc<FakePage>, label: &str, window: &str, op: Value) -> Value {
        let page = page.clone();
        self.bridge
            .handle(label, window, move || Arc::new(Sink(page)), op)
    }

    async fn connected(&self, app_id: &str) {
        self.bridge.client().start();
        eventually("App 连上 Hub", || {
            self.hub
                .apps()
                .iter()
                .any(|a| a.app_id == app_id && a.connected)
        })
        .await;
    }
}

fn register(id: u64, name: &str) -> Value {
    json!({
        "op": "tool.register", "id": id, "name": name,
        "spec": { "description": "页面工具", "risk": "read",
                  "inputSchema": { "type": "object", "properties": { "a": { "type": "integer" } } } }
    })
}

async fn wait_event(page: &FakePage, ty: &str) -> Value {
    let mut found = None;
    eventually(&format!("页面收到 {ty}"), || {
        found = page.take(ty).into_iter().next();
        found.is_some()
    })
    .await;
    found.unwrap_or(Value::Null)
}

#[test]
fn state_json_matches_web_connection_state() {
    let info = |status, retry_in_ms, reason: Option<&str>| StateInfo {
        status,
        retry_in_ms,
        reason: reason.map(str::to_owned),
        code: None,
    };
    assert_eq!(
        bridge::state_json(&info(StateStatus::PendingPairing, None, None)),
        json!({ "status": "pending-pairing" })
    );
    assert_eq!(
        bridge::state_json(&info(StateStatus::HostMismatch, None, Some("不是 app-mcp"))),
        json!({ "status": "host-mismatch", "reason": "不是 app-mcp", "code": "HOST_NOT_APP_MCP" })
    );
    assert_eq!(
        bridge::state_json(&info(StateStatus::Rejected, None, None)),
        json!({ "status": "rejected", "reason": "", "code": "REJECTED" })
    );
    let failed = StateInfo {
        code: Some("CONNECT_FAILED".into()),
        ..info(StateStatus::Backoff, Some(10), Some("连不上"))
    };
    let failed = bridge::state_json(&failed);
    assert_eq!(
        (failed["reason"].as_str(), failed["code"].as_str()),
        (Some("连不上"), Some("CONNECT_FAILED"))
    );
    let backoff = bridge::state_json(&info(StateStatus::Backoff, Some(1500), None));
    assert_eq!(backoff["status"], "backoff");
    assert!(backoff["retryAt"].as_u64().unwrap_or(0) > 1_500);
    for (status, name) in [
        (StateStatus::Idle, "idle"),
        (StateStatus::Connecting, "connecting"),
        (StateStatus::Handshaking, "handshaking"),
        (StateStatus::Connected, "connected"),
        (StateStatus::Stopped, "stopped"),
        (StateStatus::Dormant, "dormant"),
        (StateStatus::Waking, "waking"),
    ] {
        assert_eq!(
            bridge::state_json(&info(status, None, None)),
            json!({ "status": name })
        );
    }
}

#[test]
fn bridge_script_matches_protocol() {
    assert!(BRIDGE_SCRIPT.contains("version: 1"));
    assert_eq!(BRIDGE_VERSION, 1);
    assert!(BRIDGE_SCRIPT.contains("'plugin:app-mcp|op'"));
    assert!(BRIDGE_SCRIPT.contains("__APP_MCP_TAURI_DISPATCH__"));
    assert!(DISPATCH_FN.ends_with("__APP_MCP_TAURI_DISPATCH__"));
}
