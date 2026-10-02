//! 宿主实现的名字服务（spec/naming.md 4.2 Android；`HostedConnector`）：用 socketpair 模拟 Android 的
//! `bindService` + `open() → ParcelFileDescriptor`——"App 进程"是同进程的 `NativeClient`，拨号时把 socketpair 一端交给它的
//! `accept_channel`（与 `ToolsService.open()` 相同），另一端交回 Hub。
//!
//! 覆盖：发现只读元数据（按清单列出工具、不拨号）→ 调用时拨号 → 宽限内合并 → 宽限后关闭并恰好释放一次租约（= unbindService），
//! App 回到休眠；安装 / 卸载事件；宿主错误码；对端 uid 不符；宿主拨号超时后迟到的通道被释放。
#![cfg(unix)]

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_mcp_hub::connector::{HostNameService, HostedChannel, HostedConnector, HostedName};
use app_mcp_hub::{CallRequest, ConnectorError, DiscoveredName, ErrorKind, Hub, HubConfig, ToolFilter, WakerConfig};
use app_mcp_native::{CallHandle, LifecycleMode, NativeClient, NativeConfig, StateStatus, ToolHandler, ToolSpec};
use app_mcp_protocol::naming::{Address, codes};
use serde_json::{Value, json};

const APP: &str = "hosted-app";
const T: Duration = Duration::from_secs(20);

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + T;
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "等待超时：{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn manifest(app_id: &str) -> app_mcp_manifest::Manifest {
    app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": app_id, "name": "宿主连接器测试",
            "tools": [{"name": "echo", "description": "原样返回参数", "inputSchema": {"type": "object"}}]
        })
        .to_string(),
    )
    .expect("清单")
}

fn hosted(app_id: &str, with_manifest: bool) -> HostedName {
    HostedName {
        name: DiscoveredName {
            address: Address::new(app_id, None).expect("地址"),
            activatable: true,
            running: false,
            detail: format!("dev.example.{app_id}/dev.appmcp.android.ToolsService"),
        },
        manifest: with_manifest.then(|| manifest(app_id)),
    }
}

struct Echo;
impl ToolHandler for Echo {
    fn invoke(&self, call: CallHandle) {
        let args: Value = serde_json::from_str(&call.arguments_json()).unwrap_or(Value::Null);
        let _ = call.complete(Some(&json!({ "echo": args }).to_string()), vec![]);
    }
}

/// 每个 appId 的拨号行为。
#[derive(Clone)]
enum Behavior {
    /// 正常：交给"App 进程"（首次拨号时创建，模拟冷启动）。
    Serve,
    /// 宿主报告失败（错误码字符串）。
    Fail(&'static str),
    /// 交回通道，但声称期望的对端 uid 是另一个（模拟包身份不符）。
    WrongUid,
    /// 阻塞超过超时后才交回通道。
    Late(Duration),
}

#[derive(Default)]
struct FakeAndroid {
    behaviors: Mutex<HashMap<String, Behavior>>,
    apps: Mutex<HashMap<String, NativeClient>>,
    dials: AtomicUsize,
    releases: Mutex<Vec<u64>>,
    next_lease: AtomicU64,
}

impl FakeAndroid {
    fn app(&self, app_id: &str) -> NativeClient {
        self.apps
            .lock()
            .unwrap()
            .entry(app_id.to_owned())
            .or_insert_with(|| {
                let dir = std::env::temp_dir();
                let mut cfg = NativeConfig::new(app_id, "宿主连接器测试");
                cfg.lifecycle.mode = LifecycleMode::OnDemand;
                cfg.host_url = format!("unix:{}", dir.join(format!("no-hub-{}.sock", std::process::id())).display());
                let client = NativeClient::new(cfg, None).expect("client");
                client.register_tool(ToolSpec::new("echo", "回显"), Arc::new(Echo)).expect("tool");
                client.start();
                client
            })
            .clone()
    }

    fn released(&self) -> Vec<u64> {
        self.releases.lock().unwrap().clone()
    }
}

impl HostNameService for FakeAndroid {
    fn discover(&self) -> Result<Vec<HostedName>, ConnectorError> {
        Ok(vec![hosted(APP, true)])
    }

    fn dial(&self, address: &Address, _timeout: Duration) -> Result<HostedChannel, ConnectorError> {
        self.dials.fetch_add(1, Ordering::SeqCst);
        let behavior = self.behaviors.lock().unwrap().get(&address.app_id).cloned().unwrap_or(Behavior::Serve);
        let me = app_mcp_protocol::endpoint::current_uid();
        let mut peer_uid = Some(me);
        match behavior {
            Behavior::Fail(code) => return Err(ConnectorError::new(app_mcp_hub::connector::naming_code(code), "宿主拒绝")),
            Behavior::WrongUid => peer_uid = Some(me.wrapping_add(1)),
            Behavior::Late(d) => std::thread::sleep(d),
            Behavior::Serve => {}
        }
        let (app_end, hub_end) = UnixStream::pair().map_err(|e| ConnectorError::new(codes::ACTIVATION_DENIED, e.to_string()))?;
        self.app(&address.app_id)
            .accept_channel(app_end)
            .map_err(|e| ConnectorError::new(codes::CHANNEL_LIMIT, e.to_string()))?;
        let lease = self.next_lease.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(HostedChannel { fd: hub_end.into(), lease, peer_uid })
    }

    fn release(&self, lease: u64) {
        self.releases.lock().unwrap().push(lease);
    }
}

async fn start_hub(connector: Arc<HostedConnector>, grace: Duration, wake_timeout: Duration) -> Hub {
    Hub::start(HubConfig {
        listen: None,
        ipc_endpoint: None,
        waker: WakerConfig::None,
        connectors: vec![connector],
        channel_grace: grace,
        lease_ttl: Duration::ZERO,
        wake_timeout,
        list_changed_debounce: Duration::from_millis(10),
        ..Default::default()
    })
    .await
    .expect("Hub 启动")
}

fn tool_names(hub: &Hub) -> Vec<String> {
    hub.tools(&ToolFilter::default()).into_iter().map(|t| t.name).collect()
}

async fn call(hub: &Hub, app_id: &str, args: Value) -> Result<Value, app_mcp_hub::ToolError> {
    hub.call_tool(CallRequest::new(format!("{app_id}.echo"), args)).await.expect("call_tool").result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discover_dial_grace_release_and_package_events() {
    let android = Arc::new(FakeAndroid::default());
    let connector = Arc::new(HostedConnector::new("android", android.clone()));
    let hub = start_hub(connector.clone(), Duration::from_millis(300), Duration::from_secs(10)).await;

    // 1. 发现只读元数据：按清单列出工具，不拨号、不创建"App 进程"。
    eventually("按清单列出工具", || tool_names(&hub).contains(&format!("{APP}.echo"))).await;
    assert_eq!(android.dials.load(Ordering::SeqCst), 0, "发现不得拨号");
    assert!(android.apps.lock().unwrap().is_empty());

    // 2. 调用 → 拨号（冷启动）→ 结果；宽限内第二次调用合并进同一通道。
    let r = call(&hub, APP, json!({"x": 1})).await.expect("第一次调用");
    assert_eq!(r["echo"]["x"], 1, "{r}");
    let r = call(&hub, APP, json!({"x": 2})).await.expect("第二次调用");
    assert_eq!(r["echo"]["x"], 2, "{r}");
    assert_eq!(android.dials.load(Ordering::SeqCst), 1, "宽限内不重新拨号");
    assert!(android.released().is_empty(), "宽限内不释放");

    // 3. 宽限后：Hub 关闭通道并恰好释放一次租约（= unbindService），App 回到休眠、释放运行时（spec/naming.md 7.1）。
    eventually("租约释放", || android.released() == vec![1]).await;
    let app = android.app(APP);
    eventually("App 回到休眠", || app.state().status == StateStatus::Dormant && !app.runtime_active()).await;
    eventually("Hub 不再持有连接", || hub.apps().iter().any(|a| a.app_id == APP && !a.connected)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(android.released(), vec![1], "不重复释放");

    // 4. 再次调用：重新拨号（新租约），宽限后同样释放。
    call(&hub, APP, json!({})).await.expect("再次调用");
    assert_eq!(android.dials.load(Ordering::SeqCst), 2);
    eventually("第二个租约释放", || android.released() == vec![1, 2]).await;

    // 5. 安装事件：带清单的新 App 出现在列表中（不拨号）；卸载事件移除记录与清单中的工具。
    connector.installed(hosted("late-app", true));
    eventually("安装后列出", || tool_names(&hub).contains(&"late-app.echo".to_owned())).await;
    assert_eq!(android.dials.load(Ordering::SeqCst), 2);
    connector.removed("late-app");
    eventually("卸载后移除", || !tool_names(&hub).contains(&"late-app.echo".to_owned())).await;
    assert!(!hub.apps().iter().any(|a| a.app_id == "late-app"), "卸载后发现记录应移除");

    // 6. 宿主报告的错误码原样进入调用错误的 details.code（App 拒绝了 Hub → HUB_NOT_TRUSTED）。
    android.behaviors.lock().unwrap().insert("picky-app".into(), Behavior::Fail("HUB_NOT_TRUSTED"));
    connector.installed(hosted("picky-app", true));
    eventually("picky 列出", || tool_names(&hub).contains(&"picky-app.echo".to_owned())).await;
    let err = call(&hub, "picky-app", json!({})).await.expect_err("应失败");
    assert_eq!(err.kind, ErrorKind::LaunchFailed, "{err:?}");
    assert_eq!(err.details.as_ref().and_then(|d| d["code"].as_str()), Some("HUB_NOT_TRUSTED"), "{err:?}");

    // 7. 对端 uid 与登记不符：PEER_IDENTITY_MISMATCH，已交回的通道随即释放租约。
    android.behaviors.lock().unwrap().insert("spoof-app".into(), Behavior::WrongUid);
    connector.installed(hosted("spoof-app", true));
    eventually("spoof 列出", || tool_names(&hub).contains(&"spoof-app.echo".to_owned())).await;
    let before = android.released().len();
    let err = call(&hub, "spoof-app", json!({})).await.expect_err("应失败");
    assert_eq!(err.details.as_ref().and_then(|d| d["code"].as_str()), Some("PEER_IDENTITY_MISMATCH"), "{err:?}");
    eventually("拒绝的通道释放租约", || android.released().len() == before + 1).await;

    hub.shutdown().await;
    for (_, c) in android.apps.lock().unwrap().drain() {
        c.stop();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_channel_after_timeout_is_released() {
    let android = Arc::new(FakeAndroid::default());
    let wake_timeout = Duration::from_millis(200);
    // 宿主在 Hub 的超时（200 ms + 兜底 2 s）之后才交回通道。
    android.behaviors.lock().unwrap().insert(APP.into(), Behavior::Late(Duration::from_millis(2_600)));
    let connector = Arc::new(HostedConnector::new("android", android.clone()));
    let hub = start_hub(connector, Duration::from_millis(300), wake_timeout).await;
    eventually("按清单列出工具", || tool_names(&hub).contains(&format!("{APP}.echo"))).await;

    // Hub 的等待（wake_timeout）先结束：调用以超时类错误返回，拨号 future 被放弃。
    let err = call(&hub, APP, json!({})).await.expect_err("应超时");
    assert!(matches!(err.kind, ErrorKind::AppNotResponding | ErrorKind::LaunchFailed), "{err:?}");
    // @invariant 迟到的通道不被任何人持有：宿主一返回就释放其租约（spec/naming.md 7.1）。
    eventually("迟到的租约被释放", || android.released() == vec![1]).await;
    hub.shutdown().await;
    for (_, c) in android.apps.lock().unwrap().drain() {
        c.stop();
    }
}
