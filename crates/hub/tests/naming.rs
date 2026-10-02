//! Linux 全链路 e2e（spec/naming.md；TASKS 4d 验收）：私有 D-Bus 会话总线 + 激活文件 + 示例 App（`named_app`）。
//!
//! 发现不激活 → 调用触发激活冷启动 → 调用 → 宽限后关闭（Hub 零活引用、App 进程退出）→ 再次调用再激活。
//! 本文件只有一个测试：宽限后核对本进程（Hub 所在进程）的 fd / 线程数不高于调用前的基线。
#![cfg(all(target_os = "linux", not(target_env = "ohos"), feature = "dbus"))]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use app_mcp_hub::connector::DbusConnector;
use app_mcp_hub::{CallRequest, ErrorKind, Hub, HubConfig, WakerConfig};
use app_mcp_native::test_bus::PrivateBus;
use app_mcp_protocol::naming::{Address, dbus as names};
use serde_json::{Value, json};

const APP: &str = "named-e2e";
const T: Duration = Duration::from_secs(20);

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + T;
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "等待超时：{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn events(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_owned).collect()
}

fn count(log: &Path, kind: &str) -> usize {
    events(log).iter().filter(|l| l.starts_with(kind)).count()
}

fn fd_count() -> usize {
    std::fs::read_dir("/proc/self/fd").map(|d| d.count()).unwrap_or(0)
}

fn thread_count() -> usize {
    std::fs::read_dir("/proc/self/task").map(|d| d.count()).unwrap_or(0)
}

async fn bus_names(address: &str) -> Vec<String> {
    let conn = zbus::connection::Builder::address(address).expect("地址").build().await.expect("连接总线");
    let dbus = zbus::fdo::DBusProxy::new(&conn).await.expect("DBusProxy");
    dbus.list_names().await.expect("ListNames").into_iter().map(|n| n.to_string()).collect()
}

fn manifest() -> app_mcp_manifest::Manifest {
    app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": APP, "name": "按名寻址 e2e",
            "tools": [
                {"name": "echo", "description": "原样返回参数", "inputSchema": {"type": "object"}},
                {"name": "pid", "description": "返回进程号", "inputSchema": {"type": "object"}}
            ]
        })
        .to_string(),
    )
    .expect("清单")
}

/// `apps.list`（内置工具，与模型看到的一致）中本 App 的 `nameService`。
async fn named_entry(hub: &Hub) -> Option<Value> {
    let outcome = hub.call_tool(CallRequest::new("apps.list", json!({}))).await.ok()?;
    let value = outcome.result.ok()?;
    let apps = value.get("structuredContent").unwrap_or(&value)["apps"].as_array()?.clone();
    apps.into_iter().find(|a| a["appId"] == APP).map(|a| a["nameService"].clone())
}

async fn call(hub: &Hub, tool: &str, args: Value) -> Result<Value, app_mcp_hub::ToolError> {
    let out = hub.call_tool(CallRequest::new(format!("{APP}.{tool}"), args)).await.expect("call_tool");
    out.result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activation_call_grace_and_reactivation_over_private_bus() {
    let scratch = std::env::temp_dir().join(format!("app-mcp-naming-{:032x}", rand::random::<u128>()));
    std::fs::create_dir_all(&scratch).expect("临时目录");
    let log = scratch.join("events.log");
    let env = [("APP_MCP_EVENT_LOG", log.as_os_str()), ("APP_MCP_APP_ID", std::ffi::OsStr::new(APP))];
    let Some(bus) = PrivateBus::start_with_env(&env).expect("启动私有总线") else {
        eprintln!("跳过：本机没有 dbus-daemon");
        return;
    };
    let app = app_mcp_native::test_support::example_path("named_app").expect("构建 named_app");
    // 激活文件：与 `app-mcp-host app install` 相同的生成函数与位置（$XDG_DATA_HOME/dbus-1/services）。
    let service = bus.services_dir().join(names::service_file_name(APP));
    std::fs::write(&service, names::service_file(APP, &app.display().to_string())).expect("写激活文件");
    // 写入后通知总线重新读取激活目录（spec/naming.md 4.1、U-05）。
    {
        let conn = zbus::connection::Builder::address(bus.address()).expect("地址").build().await.expect("连接");
        zbus::fdo::DBusProxy::new(&conn).await.expect("proxy").reload_config().await.expect("ReloadConfig");
    }

    let grace = Duration::from_millis(400);
    let hub = Hub::start(HubConfig {
        listen: None,
        ipc_endpoint: None,
        manifests: vec![manifest()],
        // 不用唤醒描述：证明激活只经名字服务。
        waker: WakerConfig::None,
        connectors: vec![Arc::new(DbusConnector::new(Some(bus.address().to_owned())))],
        channel_grace: grace,
        // 关闭租约：通道关闭时刻只由宽限决定。
        lease_ttl: Duration::ZERO,
        wake_timeout: Duration::from_secs(15),
        list_changed_debounce: Duration::from_millis(10),
        ..Default::default()
    })
    .await
    .expect("Hub 启动");

    // 1. 发现不激活：发现记录出现（可激活、未运行），App 进程从未启动，总线上没有该名字的所有者。
    let deadline = tokio::time::Instant::now() + T;
    let entry = loop {
        if let Some(e) = named_entry(&hub).await.filter(|e| !e.is_null()) {
            break e;
        }
        assert!(tokio::time::Instant::now() < deadline, "等待发现记录超时");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(entry["activatable"], true, "{entry}");
    assert_eq!(entry["running"], false, "{entry}");
    assert_eq!(entry["source"], "dbus");
    assert_eq!(entry["name"], names::bus_name(&Address::new(APP, None).expect("地址")));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(count(&log, "start"), 0, "发现不得启动 App：{:?}", events(&log));
    assert!(!bus_names(bus.address()).await.iter().any(|n| n.starts_with("dev.appmcp.App.")));
    let apps = hub.apps();
    let info = apps.iter().find(|a| a.app_id == APP).expect("apps 中应有该 App");
    assert!(!info.connected);

    let base_fds = fd_count();
    let base_threads = thread_count();

    // 2. 调用触发激活冷启动 → 调用。
    let r = call(&hub, "echo", json!({"x": 1})).await.expect("第一次调用");
    assert_eq!(r["echo"]["x"], 1, "{r}");
    assert_eq!(count(&log, "start"), 1, "{:?}", events(&log));
    let first_pid = call(&hub, "pid", json!({})).await.expect("pid")["pid"].as_u64();
    assert_eq!(count(&log, "start"), 1, "宽限内的调用合并进同一通道，不再激活");
    assert!(hub.apps().iter().any(|a| a.app_id == APP && a.connected));

    // 3. 宽限后关闭：Hub 不再持有连接（实例转为休眠快照），App 收到 EOF 后（激活启动）退出、名字消失。
    eventually("通道关闭、实例转为休眠", || {
        hub.apps().iter().any(|a| a.app_id == APP && !a.connected && a.dormant_instances.len() == 1)
    })
    .await;
    eventually("App 进程退出", || count(&log, "exit") == 1).await;
    let deadline = tokio::time::Instant::now() + T;
    while bus_names(bus.address()).await.iter().any(|n| n.starts_with("dev.appmcp.App.")) {
        assert!(tokio::time::Instant::now() < deadline, "名字应随 App 退出消失");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // @invariant 宽限后零活引用（spec/naming.md 7.1、7.7）：通道 fd 已关闭、没有为该 App 留下线程。
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(fd_count() <= base_fds, "宽限后 fd 应回到基线：{} > {base_fds}", fd_count());
    assert!(thread_count() <= base_threads, "宽限后线程数应回到基线：{} > {base_threads}", thread_count());
    let entry = named_entry(&hub).await.expect("发现记录保留");
    assert_eq!(entry["running"], false, "名字消失后记录保留、运行状态为否：{entry}");

    // 4. 再次调用再激活（新进程）。
    let r = call(&hub, "pid", json!({})).await.expect("再次调用");
    assert_eq!(count(&log, "start"), 2, "{:?}", events(&log));
    assert_ne!(r["pid"].as_u64(), first_pid, "应是重新激活的新进程");
    eventually("第二次宽限后关闭", || count(&log, "exit") == 2).await;
    eventually("Hub 不再持有连接", || hub.apps().iter().any(|a| a.app_id == APP && !a.connected)).await;

    // 5. 激活文件指向的程序不存在 → APP_NOT_INSTALLED（data.code = NAME_NOT_FOUND），发现记录随之移除。
    std::fs::write(&service, names::service_file(APP, "/nonexistent/app-mcp-named")).expect("改写激活文件");
    {
        let conn = zbus::connection::Builder::address(bus.address()).expect("地址").build().await.expect("连接");
        zbus::fdo::DBusProxy::new(&conn).await.expect("proxy").reload_config().await.expect("ReloadConfig");
    }
    let err = call(&hub, "echo", json!({})).await.expect_err("程序不存在应失败");
    assert_eq!(err.kind, ErrorKind::AppNotInstalled, "{err:?}");
    assert_eq!(err.details.as_ref().and_then(|d| d["code"].as_str()), Some("NAME_NOT_FOUND"), "{err:?}");

    hub.shutdown().await;
    drop(bus);
    let _ = std::fs::remove_dir_all(&scratch);
}
