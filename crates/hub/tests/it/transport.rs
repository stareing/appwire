//! App 连接传输的资源设置（第 4f 项 d 测量结果）：WebSocket 读缓冲缩小后（8 KiB），接近第 14 项缺省上限的大消息
//! （参数约 0.9 MiB、结果约 3.5 MiB）仍能经 TCP 与本地 IPC 往返；真实 native 客户端扮演 App。

use std::sync::Arc;
use std::time::Duration;

use app_mcp_hub::{CallRequest, Hub, HubConfig, LimitPolicy};
use app_mcp_native::{CallHandle, NativeClient, NativeConfig, ToolHandler, ToolSpec};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(20);
/// 参数大小：低于缺省 `max_arguments_bytes`（1 MiB）。
const ARG_BYTES: usize = 900 * 1024;
/// 结果大小：低于缺省 `max_result_bytes`（4 MiB），远大于读缓冲。
const RESULT_BYTES: usize = 3584 * 1024;

/// `blob({text, size})`：返回参数长度与 `size` 字节的文本。
struct Blob;
impl ToolHandler for Blob {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or(Value::Null);
        let got = args["text"].as_str().map_or(0, str::len);
        let size = args["size"].as_u64().unwrap_or(0) as usize;
        let _ = call.complete(Some(&json!({ "received": got, "blob": "y".repeat(size) }).to_string()), vec![]);
    }
}

fn start_app(host_url: String) -> NativeClient {
    let mut c = NativeConfig::new("big", "大消息");
    c.host_url = host_url;
    c.instance_id = Some("big-1".into());
    c.launch_token = Some(String::new());
    let client = NativeClient::new(c, None).expect("native client");
    let mut spec = ToolSpec::new("blob", "大消息往返");
    spec.input_schema_json = Some(r#"{"type":"object"}"#.into());
    client.register_tool(spec, Arc::new(Blob)).expect("register");
    client.start();
    client
}

async fn wait_connected(hub: &Hub) {
    tokio::time::timeout(T, async {
        while !hub.apps().iter().any(|a| a.app_id == "big" && a.connected) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("App 没有连上");
}

async fn round_trip_large(hub: &Hub) {
    let args = json!({ "text": "x".repeat(ARG_BYTES), "size": RESULT_BYTES });
    let out = tokio::time::timeout(T, hub.call_tool(CallRequest::new("big.blob", args)))
        .await
        .expect("大消息调用超时")
        .expect("大消息调用");
    let result = out.result.expect("result");
    assert_eq!(result["received"], ARG_BYTES);
    assert_eq!(result["blob"].as_str().map(str::len), Some(RESULT_BYTES));
    // 之后的小调用照常（缓冲按需扩容后不影响后续消息）。
    let small = hub.call_tool(CallRequest::new("big.blob", json!({ "text": "a", "size": 1 }))).await.expect("small");
    assert_eq!(small.result.expect("result")["received"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn large_messages_over_tcp() {
    let hub = Hub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        ipc_endpoint: None,
        limits: LimitPolicy::default(),
        ..Default::default()
    })
    .await
    .expect("hub");
    let client = start_app(format!("ws://{}/app", hub.listen_addr().expect("listen addr")));
    wait_connected(&hub).await;
    round_trip_large(&hub).await;
    client.stop();
    hub.shutdown().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn large_messages_over_ipc() {
    let dir = std::env::temp_dir().join(format!("app-mcp-transport-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ep = format!("unix:{}", dir.join("hub.sock").display());
    let hub = Hub::start(HubConfig { listen: None, ipc_endpoint: Some(ep.clone()), ..Default::default() })
        .await
        .expect("hub");
    let client = start_app(ep);
    wait_connected(&hub).await;
    round_trip_large(&hub).await;
    client.stop();
    hub.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}
