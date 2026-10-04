//! 调用对象（第 16 项 P5，spec/hub-api.md 3.6「调用对象」）：进行中调用的阶段、进度、按调用方隔离的 `apps.calls` /
//! `apps.cancel`、`HubStatus.calls`，以及调用结束即释放。
//!
//! App 端是真实的 `app-mcp-native` 客户端。

use std::sync::Arc;
use std::time::Duration;

use app_mcp_hub::call_objects::{CallState, CallStatus};
use app_mcp_hub::{
    ApprovalHandler, ApprovalPolicy, ApprovalRequest, CallOutcome, CallRequest, ErrorKind, Hub, HubConfig, Risk, async_trait,
};
use app_mcp_protocol::Visibility;
use app_mcp_native::{CallHandle, NativeClient, NativeConfig, ToolHandler, ToolSpec};
use serde_json::{Value, json};
use tokio::sync::Notify;

const T: Duration = Duration::from_secs(10);

fn config() -> HubConfig {
    HubConfig { listen: Some("127.0.0.1:0".into()), ipc_endpoint: None, ..Default::default() }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + T;
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "等待超时：{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 报告一次进度后一直等到被取消（或 5 秒）再完成。
struct Slow;
impl ToolHandler for Slow {
    fn invoke(&self, call: CallHandle) {
        std::thread::spawn(move || {
            let _ = call.report_progress(1.0, Some(4.0), Some("第一步"));
            for _ in 0..500 {
                if call.is_cancelled() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = call.complete(Some("{}"), vec![]);
        });
    }
}

fn app(hub: &Hub) -> NativeClient {
    let mut c = NativeConfig::new("shop", "商城");
    c.host_url = format!("ws://{}/app", hub.listen_addr().unwrap());
    c.instance_id = Some("shop-1".to_owned());
    let client = NativeClient::new(c, None).unwrap();
    client.register_tool(ToolSpec::new("job.run", "长作业"), Arc::new(Slow)).unwrap();
    client.start();
    client
}

async fn connected(hub: &Hub) {
    eventually("App 注册工具", || hub.status().apps.iter().any(|a| a.app_id == "shop" && !a.tools.is_empty())).await;
}

fn request(name: &str, args: Value, session: &str) -> CallRequest {
    CallRequest { session: Some(session.to_owned()), ..CallRequest::new(name, args) }
}

fn spawn_call(hub: &Arc<Hub>, req: CallRequest) -> tokio::task::JoinHandle<CallOutcome> {
    let hub = hub.clone();
    tokio::spawn(async move { tokio::time::timeout(T, hub.call_tool(req)).await.expect("调用超时").unwrap() })
}

fn calls(hub: &Hub) -> Vec<CallStatus> {
    hub.status().calls.unwrap_or_default()
}

async fn own_calls(hub: &Hub, session: &str) -> Vec<Value> {
    let out = hub.call_tool(request("apps.calls", json!({}), session)).await.unwrap();
    out.result.unwrap()["calls"].as_array().cloned().unwrap_or_default()
}

/// 运行中的调用带实例与最近进度；`apps.calls` 只列出自己的（不含本次查询、不含调用方键）；取消他人的调用与不存在的相同；
/// 取消自己的调用后发起方得到 `CANCELLED`，对象随即释放。
#[tokio::test(flavor = "multi_thread")]
async fn own_calls_listed_with_progress_and_cancellable() {
    let hub = Arc::new(Hub::start(config()).await.unwrap());
    let client = app(&hub);
    connected(&hub).await;

    let job = spawn_call(&hub, request("shop.job.run", json!({}), "s1"));
    eventually("运行中并有进度", || calls(&hub).iter().any(|c| c.state == CallState::Running && c.progress.is_some())).await;
    let status = calls(&hub);
    let c = status.iter().find(|c| c.name == "shop.job.run").expect("调用对象");
    assert_eq!((c.caller.as_deref(), c.subject.as_str(), c.instance_id.as_deref()), (Some("api:s1"), "api", Some("shop-1")));
    assert_eq!(c.platform_state, Some(Visibility::Visible), "诊断：执行实例的可见性");
    let p = c.progress.clone().unwrap();
    assert_eq!((p.progress, p.total, p.message.as_deref()), (1.0, Some(4.0), Some("第一步")));
    let call_id = c.call_id.clone();

    let mine = own_calls(&hub, "s1").await;
    assert_eq!(mine.len(), 1, "只有作业，不含本次查询：{mine:?}");
    assert_eq!((mine[0]["callId"].as_str(), mine[0]["state"].as_str()), (Some(call_id.as_str()), Some("running")));
    assert!(mine[0].get("caller").is_none(), "apps.calls 不给调用方键：{}", mine[0]);
    assert!(own_calls(&hub, "s2").await.is_empty(), "其他调用方看不到");

    let other = hub.call_tool(request("apps.cancel", json!({ "callId": call_id }), "s2")).await.unwrap();
    assert_eq!(other.result.unwrap_err().kind, ErrorKind::ToolNotFound, "不能取消他人的调用");
    let missing = hub.call_tool(request("apps.cancel", json!({ "callId": "call-nope" }), "s2")).await.unwrap();
    assert_eq!(missing.result.unwrap_err().kind, ErrorKind::ToolNotFound);
    assert!(calls(&hub).iter().any(|c| c.call_id == call_id), "仍在运行");

    let r = hub.call_tool(request("apps.cancel", json!({ "callId": call_id }), "s1")).await.unwrap();
    assert_eq!(r.result.unwrap()["cancelled"], true);
    let out = job.await.unwrap();
    assert_eq!(out.result.unwrap_err().kind, ErrorKind::Cancelled);
    eventually("结束后释放", || calls(&hub).is_empty()).await;

    client.stop();
    if let Ok(hub) = Arc::try_unwrap(hub) {
        hub.shutdown().await;
    }
}

/// 等待用户确认时阶段为 `approving`。
struct Gate(Arc<Notify>);
#[async_trait]
impl ApprovalHandler for Gate {
    async fn approve(&self, _req: ApprovalRequest) -> bool {
        self.0.notified().await;
        true
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn approving_state_is_visible() {
    let cfg = HubConfig {
        approval: ApprovalPolicy { require_at_or_above: Some(Risk::Write), timeout: None },
        ..config()
    };
    let hub = Arc::new(Hub::start(cfg).await.unwrap());
    let gate = Arc::new(Notify::new());
    hub.set_approval_handler(Arc::new(Gate(gate.clone())));
    let client = app(&hub);
    connected(&hub).await;

    let job = spawn_call(&hub, request("shop.job.run", json!({}), "s1"));
    eventually("等待审批", || calls(&hub).iter().any(|c| c.state == CallState::Approving)).await;
    gate.notify_one();
    eventually("审批后运行", || calls(&hub).iter().any(|c| c.state == CallState::Running)).await;
    hub.cancel_call(&calls(&hub)[0].call_id);
    assert_eq!(job.await.unwrap().result.unwrap_err().kind, ErrorKind::Cancelled);
    eventually("结束后释放", || calls(&hub).is_empty()).await;

    client.stop();
    if let Ok(hub) = Arc::try_unwrap(hub) {
        hub.shutdown().await;
    }
}
