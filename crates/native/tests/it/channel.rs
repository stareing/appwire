//! 接受现成通道（spec/naming.md 4.2，Android `ToolsService.open()` 的原生入口 `NativeClient::accept_channel`）：
//! 用 socketpair 模拟 Hub 一端。休眠时没有运行时；交来通道即重建运行时，SDK 先发 `app/hello`（os-activation），
//! 调用可用；Hub 关闭后回到休眠并再次释放运行时；忙 / 未启动 / 非套接字被拒绝。
#![cfg(unix)]

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use app_mcp_native::{
    CallHandle, ChannelRefusal, LifecycleMode, NativeClient, NativeConfig, StateStatus, ToolHandler, ToolSpec,
};
use crate::common::eventually;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as WsMessage;

const APP: &str = "channel-test";

struct Echo;
impl ToolHandler for Echo {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or(Value::Null);
        let _ = call.complete(Some(&json!({ "echo": args }).to_string()), vec![]);
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>;

/// 休眠（on-demand、已 start）的客户端；Host 端点指向不存在的套接字，证明通道不经端点。
fn dormant_client(dir: &std::path::Path) -> NativeClient {
    let mut cfg = NativeConfig::new(APP, "通道测试");
    cfg.instance_id = Some("inst-c1".into());
    cfg.lifecycle.mode = LifecycleMode::OnDemand;
    cfg.host_url = format!("unix:{}", dir.join("no-hub.sock").display());
    let client = NativeClient::new(cfg, None).expect("client");
    client.register_tool(ToolSpec::new("echo", "回显"), Arc::new(Echo)).expect("tool");
    client.start();
    client
}

/// Hub 一端：在 socketpair 的另一端上作为 WebSocket 服务端完成升级。
async fn serve(hub_end: UnixStream) -> Ws {
    hub_end.set_nonblocking(true).expect("nonblocking");
    let stream = tokio::net::UnixStream::from_std(hub_end).expect("tokio stream");
    tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::accept_async(stream))
        .await
        .expect("升级超时")
        .expect("升级失败")
}

async fn recv(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("超时").expect("流结束").expect("读失败");
        if let WsMessage::Text(t) = msg {
            return serde_json::from_str(t.as_str()).expect("json");
        }
    }
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(WsMessage::text(v.to_string())).await.expect("发送");
}

async fn handshake(ws: &mut Ws) -> Value {
    let hello = recv(ws).await;
    assert_eq!(hello["method"], "app/hello");
    send(ws, json!({"jsonrpc": "2.0", "id": hello["id"], "result": {
        "status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "test", "service": "app-mcp"}}))
    .await;
    loop {
        if recv(ws).await["method"] == "app/ready" {
            break;
        }
    }
    hello["params"].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn accepted_channel_runs_core_and_returns_to_dormant() {
    let dir = tempdir();
    let client = dormant_client(&dir);
    eventually("休眠且无运行时", || client.state().status == StateStatus::Dormant && !client.runtime_active());

    let (app_end, hub_end) = UnixStream::pair().expect("socketpair");
    client.accept_channel(app_end).expect("接受通道");
    let mut ws = serve(hub_end).await;
    let hello = handshake(&mut ws).await;
    assert_eq!(hello["appId"], APP);
    assert_eq!(hello["instanceId"], "inst-c1");
    assert_eq!(hello["wakeReason"], "os-activation");
    assert_eq!(hello["heartbeatMs"], 0, "通道上不发心跳");
    eventually("已连接", || client.state().status == StateStatus::Connected);

    // 已有通道：第二条被拒绝（CHANNEL_LIMIT）。
    let (busy_end, _busy_hub) = UnixStream::pair().expect("socketpair");
    assert_eq!(client.accept_channel(busy_end), Err(ChannelRefusal::Busy));

    send(&mut ws, json!({"jsonrpc": "2.0", "id": 7, "method": "tools/invoke",
        "params": {"callId": "c1", "name": "echo", "arguments": {"x": 1}}}))
    .await;
    let result = loop {
        let m = recv(&mut ws).await;
        if m["id"] == 7 {
            break m;
        }
    };
    assert_eq!(result["result"]["data"]["echo"]["x"], 1, "{result}");

    // Hub 关闭（宽限到期 / 解绑）：回到休眠、不重连，运行时再次释放（spec/naming.md 7.1）。
    ws.close(None).await.expect("关闭");
    drop(ws);
    eventually("回到休眠并释放运行时", || client.state().status == StateStatus::Dormant && !client.runtime_active());

    // 对端直接消失（Hub 进程死亡 = EOF）同样回到休眠。
    let (app_end, hub_end) = UnixStream::pair().expect("socketpair");
    client.accept_channel(app_end).expect("再次接受");
    let mut ws = serve(hub_end).await;
    let _ = handshake(&mut ws).await;
    drop(ws);
    eventually("EOF 后回到休眠", || client.state().status == StateStatus::Dormant && !client.runtime_active());
    client.stop();
}

#[test]
fn refuses_before_start_after_stop_and_non_sockets() {
    let dir = tempdir();
    let mut cfg = NativeConfig::new(APP, "x");
    cfg.lifecycle.mode = LifecycleMode::OnDemand;
    cfg.host_url = format!("unix:{}", dir.join("no-hub.sock").display());
    let client = NativeClient::new(cfg, None).expect("client");

    let (a, _b) = UnixStream::pair().expect("socketpair");
    assert_eq!(client.accept_channel(a), Err(ChannelRefusal::Stopped), "尚未 start");

    client.start();
    // 管道不是套接字：在进入核心之前拒绝。
    let (reader, _writer) = std::io::pipe().expect("pipe");
    let not_socket = UnixStream::from(std::os::fd::OwnedFd::from(reader));
    assert!(matches!(client.accept_channel(not_socket), Err(ChannelRefusal::Invalid(_))));
    assert_eq!(client.state().status, StateStatus::Dormant, "拒绝不改变状态");

    client.stop();
    let (a, _b) = UnixStream::pair().expect("socketpair");
    assert_eq!(client.accept_channel(a), Err(ChannelRefusal::Stopped), "已停止");
}

fn tempdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("app-mcp-channel-{}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&dir).expect("临时目录");
    dir
}

fn rand_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or_default()
}
