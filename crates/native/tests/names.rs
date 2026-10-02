//! 名字服务"被连接方"（spec/naming.md 第 3、4.1 节）：私有 D-Bus 会话总线上登记名字、`Open() → fd`、
//! 在 fd 上跑 WebSocket + 核心（SDK 先发 `app/hello`），Hub 关闭通道后回到休眠、名字保留；`stop` 后名字消失。
#![cfg(all(target_os = "linux", not(target_env = "ohos"), feature = "dbus"))]

mod common;
#[path = "../src/test_bus.rs"]
#[allow(dead_code)]
mod test_bus;

use std::sync::Arc;
use std::time::Duration;

use app_mcp_native::{CallHandle, LifecycleMode, NativeClient, NativeConfig, StateStatus, ToolHandler, ToolSpec};
use common::{Recorder, eventually};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as WsMessage;

const APP: &str = "named-test";
const BUS_NAME: &str = "dev.appmcp.App.named_test";

struct Echo;
impl ToolHandler for Echo {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or(Value::Null);
        let _ = call.complete(Some(&json!({ "echo": args }).to_string()), vec![]);
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>;

/// 扮演 Hub：经总线调用 `Open()`，在得到的 fd 上作为 WebSocket 服务端完成升级。
async fn open(conn: &zbus::Connection, name: &str, path: &str) -> zbus::Result<Ws> {
    let proxy = zbus::Proxy::new(conn, name.to_owned(), path.to_owned(), "dev.appmcp.App1").await?;
    let fd: zbus::zvariant::OwnedFd = proxy.call("Open", &()).await?;
    let std_stream = std::os::unix::net::UnixStream::from(std::os::fd::OwnedFd::from(fd));
    std_stream.set_nonblocking(true).map_err(|e| zbus::Error::InputOutput(Arc::new(e)))?;
    let stream = tokio::net::UnixStream::from_std(std_stream).map_err(|e| zbus::Error::InputOutput(Arc::new(e)))?;
    tokio_tungstenite::accept_async(stream).await.map_err(|e| zbus::Error::Failure(e.to_string()))
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

/// 完成握手（paired），返回 hello 参数。
async fn handshake(ws: &mut Ws) -> Value {
    let hello = recv(ws).await;
    assert_eq!(hello["method"], "app/hello");
    send(ws, json!({"jsonrpc": "2.0", "id": hello["id"], "result": {
        "status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "test", "service": "app-mcp"}}))
    .await;
    hello["params"].clone()
}

async fn bus_names(conn: &zbus::Connection) -> Vec<String> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await.expect("DBusProxy");
    dbus.list_names().await.expect("ListNames").into_iter().map(|n| n.to_string()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn registers_name_and_serves_channel_over_fd() {
    let Some(bus) = test_bus::PrivateBus::start().expect("启动私有总线") else {
        eprintln!("跳过：本机没有 dbus-daemon");
        return;
    };
    let mut cfg = NativeConfig::new(APP, "按名寻址测试");
    cfg.instance_id = Some("inst-n1".into());
    cfg.lifecycle.mode = LifecycleMode::OnDemand;
    cfg.register_name = true;
    cfg.name_instance = Some("w-2".into());
    cfg.name_service_address = Some(bus.address().to_owned());
    // 端点指向不存在的套接字：证明通道不经 Host 端点。
    cfg.host_url = format!("unix:{}", bus.dir().join("no-hub.sock").display());
    let rec = Arc::new(Recorder::default());
    let client = NativeClient::new(cfg, Some(rec.clone())).expect("client");
    client.register_tool(ToolSpec::new("echo", "回显"), Arc::new(Echo)).expect("tool");

    let hub = zbus::connection::Builder::address(bus.address()).expect("地址").build().await.expect("连接总线");
    assert!(!bus_names(&hub).await.iter().any(|n| n.starts_with("dev.appmcp.App.")), "start 之前不登记");
    client.start();
    for _ in 0..200 {
        if bus_names(&hub).await.iter().any(|n| n == BUS_NAME) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let names = bus_names(&hub).await;
    assert!(names.iter().any(|n| n == BUS_NAME), "{names:?}");
    assert!(names.iter().any(|n| n == "dev.appmcp.App.named_test.w_2"), "{names:?}");
    assert_eq!(client.state().status, StateStatus::Dormant);
    assert!(client.runtime_active(), "登记期间保持运行时（阻塞在总线连接上）");

    // 第一次拨入：SDK 先发 hello（os-activation、不发心跳），完整同步后可调用。
    let mut ws = open(&hub, BUS_NAME, "/dev/appmcp/App").await.expect("Open");
    let hello = handshake(&mut ws).await;
    assert_eq!(hello["appId"], APP);
    assert_eq!(hello["wakeReason"], "os-activation");
    assert_eq!(hello["heartbeatMs"], 0);
    let mut synced = false;
    for _ in 0..10 {
        let m = recv(&mut ws).await;
        if m["method"] == "tools/sync" {
            assert_eq!(m["params"]["tools"][0]["name"], "echo");
            synced = true;
        }
        if m["method"] == "app/ready" {
            break;
        }
    }
    assert!(synced);
    eventually("已连接", || client.state().status == StateStatus::Connected);

    // 已有通道时第二次拨入被拒绝（上限 1，CHANNEL_LIMIT）。
    let second = open(&hub, "dev.appmcp.App.named_test.w_2", "/dev/appmcp/App/w_2").await;
    match second {
        Err(zbus::Error::MethodError(name, Some(detail), _)) => {
            assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.LimitsExceeded");
            assert!(detail.starts_with("CHANNEL_LIMIT"), "{detail}");
        }
        other => panic!("应被拒绝：{:?}", other.map(|_| ())),
    }

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

    // Hub 关闭通道：回到休眠，不重连、不退出；名字保留，可再次拨入。
    ws.close(None).await.expect("关闭");
    drop(ws);
    eventually("回到休眠", || client.state().status == StateStatus::Dormant);
    assert!(bus_names(&hub).await.iter().any(|n| n == BUS_NAME));
    let mut ws = open(&hub, "dev.appmcp.App.named_test.w_2", "/dev/appmcp/App/w_2").await.expect("再次 Open");
    let hello = handshake(&mut ws).await;
    assert_eq!(hello["instanceId"], "inst-n1");
    drop(ws);
    eventually("对端 EOF 后回到休眠", || client.state().status == StateStatus::Dormant);

    // stop：注销名字。
    client.stop();
    for _ in 0..200 {
        if !bus_names(&hub).await.iter().any(|n| n.starts_with("dev.appmcp.App.")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let names = bus_names(&hub).await;
    assert!(!names.iter().any(|n| n.starts_with("dev.appmcp.App.")), "stop 后名字应消失：{names:?}");
    assert!(!rec.logs.lock().unwrap().iter().any(|(_, m)| m.contains("未能在名字服务登记")));
}

#[tokio::test(flavor = "multi_thread")]
async fn reserved_instance_name_is_config_error() {
    let mut cfg = NativeConfig::new(APP, "x");
    cfg.register_name = true;
    cfg.name_instance = Some("default".into());
    assert!(NativeClient::new(cfg, None).is_err(), "保留实例名应在配置时失败");
}
