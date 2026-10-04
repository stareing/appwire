//! 第 16 项第一部分的集成测试：真实的 `app-mcp-native` 客户端扮演 App。
//!
//! - U1 / N7a：同一 callId 重复到达 App（调用方以同一 `call_id` 重试、执行中重复）只执行一次，得到首次结果。
//! - O2：MCP 请求带 `progressToken` 时，App 的进度经合并后成为 `notifications/progress`；
//!   MCP `notifications/cancelled` 一路传到 App handler 的取消监听。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::{CallRequest, Hub, HubConfig, ToolFilter};
use app_mcp_native::{CallHandle, CancelListener, CancelReason, NativeClient, NativeConfig, ToolHandler, ToolSpec};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::timeout;

const T: Duration = Duration::from_secs(10);

fn config(progress_interval: Duration) -> HubConfig {
    HubConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        list_changed_debounce: Duration::from_millis(10),
        progress_interval,
        ..Default::default()
    }
}

/// 计数的写工具：每次执行计数加一；`gate` 中有发送端时等它放行再完成（模拟执行中）。
struct Counter {
    runs: Arc<AtomicUsize>,
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl ToolHandler for Counter {
    fn invoke(&self, call: CallHandle) {
        let n = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        let gate = self.gate.lock().unwrap().take();
        std::thread::spawn(move || {
            if let Some(g) = gate {
                let _ = g.recv_timeout(T);
            }
            let _ = call.complete(Some(&json!({ "run": n }).to_string()), vec![]);
        });
    }
}

/// 长任务：报告若干进度后等待取消；取消原因发到 `cancelled`。
struct Export {
    steps: usize,
    cancelled: mpsc::UnboundedSender<CancelReason>,
}

struct OnCancel(mpsc::UnboundedSender<CancelReason>);
impl CancelListener for OnCancel {
    fn on_cancel(&self, reason: CancelReason) {
        let _ = self.0.send(reason);
    }
}

impl ToolHandler for Export {
    fn invoke(&self, call: CallHandle) {
        call.set_cancel_listener(Arc::new(OnCancel(self.cancelled.clone())));
        let steps = self.steps;
        std::thread::spawn(move || {
            for i in 1..=steps {
                let _ = call.report_progress(i as f64, Some(steps as f64), Some(&format!("第 {i} 步")));
            }
            // 不完成：等 Agent 取消
        });
    }
}

struct App {
    _client: NativeClient,
    runs: Arc<AtomicUsize>,
    gate: Arc<Counter>,
    cancelled: mpsc::UnboundedReceiver<CancelReason>,
}

async fn connect_app(hub: &Hub, steps: usize) -> App {
    let mut c = NativeConfig::new("shop", "商城");
    c.host_url = format!("ws://{}/app", hub.listen_addr().unwrap());
    c.instance_id = Some("i1".into());
    c.launch_token = Some(String::new());
    c.max_concurrent_calls = 4;
    let client = NativeClient::new(c, None).unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = Arc::new(Counter { runs: runs.clone(), gate: Mutex::new(None) });
    client.register_tool(ToolSpec::new("order.submit", "下单"), counter.clone()).unwrap();
    let (tx, cancelled) = mpsc::unbounded_channel();
    client.register_tool(ToolSpec::new("export", "导出"), Arc::new(Export { steps, cancelled: tx })).unwrap();
    client.start();
    let filter = ToolFilter { apps: Some(vec!["shop".into()]), ..Default::default() };
    timeout(T, async {
        while hub.tools(&filter).iter().filter(|t| t.app_id == "shop").count() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("工具没有出现");
    App { _client: client, runs, gate: counter, cancelled }
}

fn submit(call_id: &str) -> CallRequest {
    let mut req = CallRequest::new("shop.order.submit", json!({}));
    req.call_id = Some(call_id.to_owned());
    req
}

/// U1 复现：调用方（Hub API / 按 LLM tool_call id 分派）以同一 call_id 重试写调用，App 内只执行一次。
#[tokio::test]
async fn retried_call_id_executes_once() {
    let hub = Hub::start(config(Duration::from_millis(250))).await.unwrap();
    let app = connect_app(&hub, 0).await;

    let first = hub.call_tool(submit("order-1")).await.unwrap();
    assert_eq!(first.result.unwrap(), json!({ "run": 1 }));
    let again = hub.call_tool(submit("order-1")).await.unwrap();
    assert_eq!(again.result.unwrap(), json!({ "run": 1 }), "重放首次结果");
    assert_eq!(app.runs.load(Ordering::SeqCst), 1);
    // 不同 callId 照常执行
    assert_eq!(hub.call_tool(submit("order-2")).await.unwrap().result.unwrap(), json!({ "run": 2 }));

    // 执行中重复到达：挂到同一次执行上，两者得到同一结果
    let (open, gate) = std::sync::mpsc::channel();
    *app.gate.gate.lock().unwrap() = Some(gate);
    let (a, b) = tokio::join!(hub.call_tool(submit("order-3")), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let b = hub.call_tool(submit("order-3"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        open.send(()).unwrap();
        b.await
    });
    assert_eq!(a.unwrap().result.unwrap(), json!({ "run": 3 }));
    assert_eq!(b.unwrap().result.unwrap(), json!({ "run": 3 }));
    assert_eq!(app.runs.load(Ordering::SeqCst), 3);
    assert!(app.cancelled.is_empty(), "没有调用被取消");
}

#[cfg(feature = "mcp-server")]
mod mcp {
    use super::*;
    use rmcp::model::{
        CallToolRequest, CallToolRequestParams, ClientRequest, ProgressNotificationParam, ProgressToken,
    };
    use rmcp::service::{NotificationContext, PeerRequestOptions};
    use rmcp::{ClientHandler, RoleClient, ServiceExt};

    /// 假 MCP 客户端：记录收到的进度通知。
    #[derive(Clone)]
    struct Recorder(mpsc::UnboundedSender<ProgressNotificationParam>);

    impl ClientHandler for Recorder {
        async fn on_progress(&self, params: ProgressNotificationParam, _: NotificationContext<RoleClient>) {
            let _ = self.0.send(params);
        }
    }

    async fn mcp_client(
        hub: &Hub,
    ) -> (rmcp::service::RunningService<RoleClient, Recorder>, mpsc::UnboundedReceiver<ProgressNotificationParam>) {
        let (c, s) = tokio::io::duplex(1 << 20);
        let session = hub.mcp_session();
        tokio::spawn(async move {
            if let Ok(svc) = session.serve(s).await {
                let _ = svc.waiting().await;
            }
        });
        let (tx, rx) = mpsc::unbounded_channel();
        (Recorder(tx).serve(c).await.expect("mcp"), rx)
    }

    fn call_request(name: &str) -> ClientRequest {
        ClientRequest::CallToolRequest(CallToolRequest::new(CallToolRequestParams::new(name.to_owned())))
    }

    /// 进度经合并（间隔内只保留最新一条）转发为 `notifications/progress`；取消到达 App handler。
    #[tokio::test]
    async fn progress_is_forwarded_and_cancel_reaches_handler() {
        // 间隔足够长：App 一口气报告的 5 条进度合并为首条 + 最后一条
        let hub = Hub::start(config(Duration::from_millis(300))).await.unwrap();
        let mut app = connect_app(&hub, 5).await;
        let (client, mut progress) = mcp_client(&hub).await;

        let handle = client
            .peer()
            .send_cancellable_request(call_request("shop.export"), PeerRequestOptions::no_options())
            .await
            .unwrap();
        let token: ProgressToken = handle.progress_token.clone();
        let first = timeout(T, progress.recv()).await.expect("进度").unwrap();
        assert_eq!(first.progress_token, token);
        assert_eq!((first.progress, first.total, first.message.as_deref()), (1.0, Some(5.0), Some("第 1 步")));
        let last = timeout(T, progress.recv()).await.expect("合并后的最后一条").unwrap();
        assert_eq!((last.progress, last.message.as_deref()), (5.0, Some("第 5 步")), "间隔内只转发最新一条");
        assert!(timeout(Duration::from_millis(500), progress.recv()).await.is_err(), "没有更多进度");

        handle.cancel(Some("用户中止".into())).await.unwrap();
        let reason = timeout(T, app.cancelled.recv()).await.expect("handler 收到取消").unwrap();
        assert_eq!(reason, CancelReason::Requested);
        let _ = client.cancel().await;
    }

    /// 间隔为 0：每条递增的进度都转发。
    #[tokio::test]
    async fn zero_interval_forwards_every_update() {
        let hub = Hub::start(config(Duration::ZERO)).await.unwrap();
        let mut app = connect_app(&hub, 3).await;
        let (client, mut progress) = mcp_client(&hub).await;
        let handle = client
            .peer()
            .send_cancellable_request(call_request("shop.export"), PeerRequestOptions::no_options())
            .await
            .unwrap();
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(timeout(T, progress.recv()).await.expect("进度").unwrap().progress);
        }
        assert_eq!(seen, vec![1.0, 2.0, 3.0]);
        handle.cancel(None).await.unwrap();
        assert_eq!(timeout(T, app.cancelled.recv()).await.unwrap(), Some(CancelReason::Requested));
        let _ = client.cancel().await;
    }
}
