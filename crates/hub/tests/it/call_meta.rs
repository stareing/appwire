//! 调用元信息（第 19 项 R4，spec/hub-api.md 3.15 键表）：Hub API `CallOutcome.duration_ms` / `woke`，MCP 结果 `_meta` 的
//! `dev.appwire/callId`、`instanceId`、`durationMs`、`woke`；callId 与 App handler 看到的 callId 相同。
//!
//! App 端是真实的 `app-mcp-native` 客户端（idle 模式），唤醒用进程内的假 [`Waker`]。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{CallRequest, Hub, HubConfig, HubError, WakeRequest, Waker, async_trait};
use app_mcp_native::{
    CallHandle, LifecycleMode, NativeClient, NativeConfig, StateStatus, ToolHandler, ToolSpec, WakeDescriptor as NativeWake,
    WakeKind as NativeWakeKind,
};
use serde_json::json;

const T: Duration = Duration::from_secs(10);
/// handler 的固定耗时：`durationMs` 至少为此值。
const HANDLER_DELAY_MS: u64 = 60;
const INSTANCE: &str = "calc-1";

fn config() -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        list_changed_debounce: Duration::from_millis(10),
        wake_timeout: Duration::from_secs(5),
        lease_ttl: Duration::from_secs(5),
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

/// 等待固定时长后返回 handler 看到的 callId。
struct EchoCallId;
impl ToolHandler for EchoCallId {
    fn invoke(&self, call: CallHandle) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(HANDLER_DELAY_MS));
            let out = json!({ "callId": call.call_id() });
            let _ = call.complete(Some(&out.to_string()), vec![]);
        });
    }
}

/// 假 Waker：把激活参数交给客户端的 `handle_wake`（模拟 OS 激活）。
#[derive(Default)]
struct FakeWaker {
    client: Mutex<Option<NativeClient>>,
    count: Mutex<usize>,
}

#[async_trait]
impl Waker for FakeWaker {
    async fn wake(&self, req: WakeRequest) -> Result<(), HubError> {
        *self.count.lock().unwrap() += 1;
        if let Some(c) = self.client.lock().unwrap().clone() {
            assert!(c.handle_wake(&req.activation_arg));
        }
        Ok(())
    }
}

/// 启动 Hub 与一个已进入休眠的 App（`calc.echo`）。
async fn dormant_app() -> (Hub, NativeClient, Arc<FakeWaker>) {
    let hub = Hub::start(config()).await.unwrap();
    let waker = Arc::new(FakeWaker::default());
    hub.set_waker(waker.clone());
    let mut c = NativeConfig::new("calc", "计算器");
    c.host_url = format!("ws://{}/app", hub.listen_addr().unwrap());
    c.instance_id = Some(INSTANCE.to_owned());
    c.launch_token = Some(String::new());
    c.lifecycle.mode = LifecycleMode::Idle;
    c.lifecycle.idle_timeout_ms = 150;
    c.lifecycle.wake = Some(NativeWake { kind: NativeWakeKind::Uri, target: Some("calc-app".into()), background: true });
    let client = NativeClient::new(c, None).unwrap();
    client.register_tool(ToolSpec::new("echo", "回显 callId"), Arc::new(EchoCallId)).unwrap();
    *waker.client.lock().unwrap() = Some(client.clone());
    client.start();
    let c = client.clone();
    eventually("App 休眠", move || c.state().status == StateStatus::Dormant).await;
    (hub, client, waker)
}

/// Hub API：休眠唤醒的调用 `woke = true`，紧接着的调用（租约内仍连接）`woke = false`；`duration_ms` 不小于 handler 耗时；
/// callId（自动生成或调用方给定）与 handler 看到的相同。
#[tokio::test(flavor = "multi_thread")]
async fn hub_api_outcome_woke_duration_and_call_id() {
    let (hub, client, waker) = dormant_app().await;

    let o = tokio::time::timeout(T, hub.call_tool(CallRequest::new("calc.echo", json!({})))).await.unwrap().unwrap();
    let data = o.result.clone().expect("调用成功");
    assert!(o.woke, "休眠中的 App 被本次调用唤醒");
    assert_eq!(*waker.count.lock().unwrap(), 1);
    assert_eq!(o.instance_id.as_deref(), Some(INSTANCE));
    assert!(o.duration_ms >= HANDLER_DELAY_MS, "durationMs = {}", o.duration_ms);
    assert_eq!(data["callId"], json!(o.call_id), "callId 与 handler 看到的相同");

    let mut req = CallRequest::new("calc.echo", json!({}));
    req.call_id = Some("agent-call-1".into());
    let o = tokio::time::timeout(T, hub.call_tool(req)).await.unwrap().unwrap();
    assert!(!o.woke, "App 已连接，不经过唤醒");
    assert_eq!(*waker.count.lock().unwrap(), 1);
    assert_eq!(o.call_id, "agent-call-1");
    assert_eq!(o.result.unwrap()["callId"], "agent-call-1");
    assert!(o.duration_ms >= HANDLER_DELAY_MS, "durationMs = {}", o.duration_ms);

    // 内置工具：woke 恒为 false，duration 照常给出
    let o = hub.call_tool(CallRequest::new("apps.list", json!({}))).await.unwrap();
    assert!(!o.woke && o.instance_id.is_none());
    client.stop();
    hub.shutdown().await;
}

/// MCP 出口：结果 `_meta` 带 callId（等于 handler 看到的）、instanceId、durationMs、woke（先唤醒后热调用）。
#[cfg(feature = "mcp-server")]
mod mcp {
    use super::*;
    use serde_json::Value;
    use app_mcp_hub::names::{META_CALL_ID, META_DURATION_MS, META_INSTANCE_ID, META_WOKE};
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, CallToolResult};

    fn meta(r: &CallToolResult, key: &str) -> Value {
        r.meta.as_ref().and_then(|m| m.get(key).cloned()).unwrap_or(Value::Null)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mcp_result_meta_keys() {
        let (hub, client, waker) = dormant_app().await;
        let (c, s) = tokio::io::duplex(1 << 20);
        let session = hub.mcp_session();
        tokio::spawn(async move {
            if let Ok(svc) = session.serve(s).await {
                let _ = svc.waiting().await;
            }
        });
        let agent = ().serve(c).await.expect("mcp");
        let call = |name: &str| {
            let params = CallToolRequestParams::new(name.to_owned());
            let peer = agent.peer().clone();
            async move { tokio::time::timeout(T, peer.call_tool(params)).await.expect("MCP 调用超时").expect("call") }
        };

        for expect_woke in [true, false] {
            let r = call("calc.echo").await;
            assert_ne!(r.is_error, Some(true), "{r:?}");
            let seen = r.structured_content.as_ref().map(|v| v["callId"].clone()).unwrap_or(Value::Null);
            assert!(seen.as_str().is_some_and(|s| !s.is_empty()), "{r:?}");
            assert_eq!(meta(&r, META_CALL_ID), seen, "_meta callId 与 handler 看到的相同");
            assert_eq!(meta(&r, META_INSTANCE_ID), json!(INSTANCE));
            assert_eq!(meta(&r, META_WOKE), json!(expect_woke));
            let ms = meta(&r, META_DURATION_MS).as_u64().expect("durationMs 为非负整数");
            assert!(ms >= HANDLER_DELAY_MS, "durationMs = {ms}");
        }
        assert_eq!(*waker.count.lock().unwrap(), 1);

        // 内置工具：带 callId / durationMs，不带 woke / instanceId
        let r = call("apps.list").await;
        assert!(meta(&r, META_CALL_ID).is_string() && meta(&r, META_DURATION_MS).is_u64(), "{r:?}");
        assert!(meta(&r, META_WOKE).is_null() && meta(&r, META_INSTANCE_ID).is_null(), "{r:?}");

        let _ = agent.cancel().await;
        client.stop();
        hub.shutdown().await;
    }
}
