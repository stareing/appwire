//! Windows 全链路 e2e（spec/naming.md 4.3；TASKS 4d-E 验收）：临时登记目录 + `exec` 激活 + 示例 App（`named_app`）。
//!
//! 发现不激活 → 调用触发激活冷启动 → 调用 → 宽限后关闭（Hub 句柄回到基线、App 进程退出、管道消失）→ 再次调用再激活；
//! 运行期间新增 / 删除登记即时生效（目录通知）；App 已有通道时拒绝（`CHANNEL_LIMIT`）；名字被其他程序抢注
//! （`PEER_IDENTITY_MISMATCH`）；登记的程序不存在（`APP_NOT_INSTALLED`）。
//!
//! 激活由 Hub 进程直接拉起 App，App 继承本测试进程的环境：因此在启动运行时之前设置 `APP_MCP_APP_ID` /
//! `APP_MCP_EVENT_LOG`（本文件只有一个测试）。appId 每次随机，管道名不会与其他运行或常驻 Host 冲突。
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use app_mcp_hub::connector::PipeConnector;
use app_mcp_hub::{CallRequest, ErrorKind, Hub, HubConfig, WakerConfig};
use app_mcp_protocol::naming::registration::{self, Activation, Registration, kinds};
use app_mcp_protocol::naming::{Address, pipe as names};
use serde_json::{Value, json};
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

const T: Duration = Duration::from_secs(20);

struct Ctx {
    app: String,
    log: PathBuf,
    apps_dir: PathBuf,
    pipe: String,
}

fn events(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_owned).collect()
}

fn count(log: &Path, kind: &str) -> usize {
    events(log).iter().filter(|l| l.starts_with(kind)).count()
}

fn handle_count() -> u32 {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    let mut n = 0u32;
    // SAFETY: 伪句柄总是有效；n 为输出参数。
    unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut n) };
    n
}

/// 管道是否存在（不连接：用 `WaitNamedPipeW` 探测，0 = 默认超时；不存在时立即失败）。
fn pipe_exists(name: &str) -> bool {
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: wide 以 0 结尾。
    let ok = unsafe { WaitNamedPipeW(wide.as_ptr(), 1) } != 0;
    // 存在但忙（ERROR_SEM_TIMEOUT = 121）也算存在。
    ok || std::io::Error::last_os_error().raw_os_error() == Some(121)
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + T;
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "等待超时：{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn manifest(app: &str) -> app_mcp_manifest::Manifest {
    app_mcp_manifest::parse(
        &json!({
            "manifestVersion": 1, "appId": app, "name": "按名寻址 Windows e2e",
            "tools": [
                {"name": "echo", "description": "原样返回参数", "inputSchema": {"type": "object"}},
                {"name": "pid", "description": "返回进程号", "inputSchema": {"type": "object"}}
            ]
        })
        .to_string(),
    )
    .expect("清单")
}

fn write_registration(dir: &Path, app: &str, executable: &str, kind: &str, target: &str) {
    let reg = Registration {
        registration_version: registration::REGISTRATION_VERSION,
        app_id: app.to_owned(),
        name: app.to_owned(),
        source: "manual".to_owned(),
        manifest: None,
        manifest_sha256: None,
        executable: Some(executable.to_owned()),
        activation: Activation { kind: kind.to_owned(), target: target.to_owned() },
    };
    std::fs::create_dir_all(dir).expect("登记目录");
    let tmp = dir.join(format!("{app}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&reg).expect("json")).expect("写登记");
    std::fs::rename(&tmp, dir.join(registration::file_name(app))).expect("改名");
}

async fn apps_list(hub: &Hub) -> Vec<Value> {
    let Ok(outcome) = hub.call_tool(CallRequest::new("apps.list", json!({}))).await else { return Vec::new() };
    let Ok(value) = outcome.result else { return Vec::new() };
    value.get("structuredContent").unwrap_or(&value)["apps"].as_array().cloned().unwrap_or_default()
}

async fn named_entry(hub: &Hub, app: &str) -> Option<Value> {
    apps_list(hub).await.into_iter().find(|a| a["appId"] == app).map(|a| a["nameService"].clone()).filter(|e| !e.is_null())
}

async fn call(hub: &Hub, app: &str, tool: &str, args: Value) -> Result<Value, app_mcp_hub::ToolError> {
    hub.call_tool(CallRequest::new(format!("{app}.{tool}"), args)).await.expect("call_tool").result
}

fn code(err: &app_mcp_hub::ToolError) -> Option<&str> {
    err.details.as_ref().and_then(|d| d["code"].as_str())
}

#[test]
fn pipe_activation_call_grace_and_reactivation() {
    let app = format!("pipe-e2e-{:08x}", rand::random::<u32>());
    let scratch = std::env::temp_dir().join(format!("app-mcp-pipe-e2e-{:032x}", rand::random::<u128>()));
    std::fs::create_dir_all(&scratch).expect("临时目录");
    let log = scratch.join("events.log");
    // SAFETY: 本测试二进制只有这一个测试，且在创建任何运行时 / 线程之前设置。
    unsafe {
        std::env::set_var("APP_MCP_EVENT_LOG", &log);
        std::env::set_var("APP_MCP_APP_ID", &app);
    }
    let sid = app_mcp_protocol::endpoint::win::current_user_sid().expect("SID");
    let pipe = names::pipe_name(&sid, &Address::new(&app, None).expect("地址"));
    let ctx = Ctx { app, log, apps_dir: scratch.join("apps"), pipe };
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("运行时");
    rt.block_on(scenario(&ctx));
    drop(rt);
    let _ = std::fs::remove_dir_all(&scratch);
}

async fn scenario(ctx: &Ctx) {
    let (app, log) = (ctx.app.as_str(), ctx.log.as_path());
    let exe = app_mcp_native::test_support::example_path("named_app").expect("构建 named_app");
    let exe_text = exe.display().to_string();
    write_registration(&ctx.apps_dir, app, &exe_text, kinds::EXEC, "");

    let grace = Duration::from_millis(400);
    let connector = PipeConnector::new(Some(ctx.apps_dir.clone())).expect("连接器");
    let hub = Hub::start(HubConfig {
        listen: None,
        ipc_endpoint: None,
        manifests: vec![manifest(app)],
        // 不用唤醒描述：证明激活只经名字服务。
        waker: WakerConfig::None,
        connectors: vec![Arc::new(connector)],
        channel_grace: grace,
        lease_ttl: Duration::ZERO,
        wake_timeout: Duration::from_secs(15),
        // 本测试有意多次拨号（拒绝、冒名、程序缺失），不受每分钟激活上限影响。
        wake_rate_limit: 0,
        list_changed_debounce: Duration::from_millis(10),
        ..Default::default()
    })
    .await
    .expect("Hub 启动");

    // 1. 发现不激活：记录可激活、未运行；App 从未启动、管道不存在。
    let deadline = tokio::time::Instant::now() + T;
    let entry = loop {
        if let Some(e) = named_entry(&hub, app).await {
            break e;
        }
        assert!(tokio::time::Instant::now() < deadline, "等待发现记录超时");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(entry["activatable"], true, "{entry}");
    assert_eq!(entry["running"], false, "{entry}");
    assert_eq!(entry["source"], "pipe");
    assert_eq!(entry["name"], ctx.pipe.as_str());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(count(log, "start"), 0, "发现不得启动 App：{:?}", events(log));
    assert!(!pipe_exists(&ctx.pipe));
    let base_handles = handle_count();

    // 2. 调用触发 exec 激活冷启动 → 调用；宽限内的调用合并进同一通道。
    let started = std::time::Instant::now();
    let r = call(&hub, app, "echo", json!({"x": 1})).await.expect("第一次调用");
    eprintln!("冷启动 + 调用：{} ms", started.elapsed().as_millis());
    assert_eq!(r["echo"]["x"], 1, "{r}");
    assert_eq!(count(log, "start"), 1, "{:?}", events(log));
    let started = std::time::Instant::now();
    let first_pid = call(&hub, app, "pid", json!({})).await.expect("pid")["pid"].as_u64();
    eprintln!("热调用：{} ms", started.elapsed().as_millis());
    assert_eq!(count(log, "start"), 1, "宽限内不再激活");
    assert!(hub.apps().iter().any(|a| a.app_id == app && a.connected));

    // 3. 宽限后关闭：实例转为休眠快照，App（激活启动）退出，管道消失；Hub 句柄回到基线（零活引用，7.1）。
    eventually("通道关闭、实例转为休眠", || {
        hub.apps().iter().any(|a| a.app_id == app && !a.connected && a.dormant_instances.len() == 1)
    })
    .await;
    eventually("App 进程退出", || count(log, "exit") == 1).await;
    eventually("管道随 App 退出消失", || !pipe_exists(&ctx.pipe)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    // 第一次激活会惰性创建一些进程级句柄（运行时的阻塞线程池、创建进程用到的系统对象）：以第一轮结束后为基线，
    // 第二轮激活 + 宽限后不得增长（每次激活不留句柄）。
    let warm_handles = handle_count();
    eprintln!("句柄：发现后 {base_handles}，第一轮宽限后 {warm_handles}");

    // 4. 再次调用再激活（新进程）。
    let started = std::time::Instant::now();
    let r = call(&hub, app, "pid", json!({})).await.expect("再次调用");
    eprintln!("再激活 + 调用：{} ms", started.elapsed().as_millis());
    assert_eq!(count(log, "start"), 2, "{:?}", events(log));
    assert_ne!(r["pid"].as_u64(), first_pid, "应是重新激活的新进程");
    eventually("第二次宽限后 App 退出", || count(log, "exit") == 2).await;
    eventually("Hub 不再持有连接", || hub.apps().iter().any(|a| a.app_id == app && !a.connected)).await;
    eventually("管道消失", || !pipe_exists(&ctx.pipe)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after = handle_count();
    eprintln!("句柄：第二轮宽限后 {after}");
    assert!(after <= warm_handles, "宽限后句柄应回到基线（零活引用，7.1）：{after} > {warm_handles}");

    // 5. App 已有通道时拒绝：用户直接运行的 App（常驻），本测试先占用它唯一的通道 → Hub 拨号得到 CHANNEL_LIMIT。
    let mut resident: Child = Command::new(&exe).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("运行 App");
    eventually("常驻 App 登记管道", || pipe_exists(&ctx.pipe)).await;
    let squatter = ClientOptions::new().open(&ctx.pipe).expect("占用通道");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let err = call(&hub, app, "echo", json!({})).await.expect_err("应被 App 拒绝");
    assert_eq!(err.kind, ErrorKind::LaunchFailed, "{err:?}");
    assert_eq!(code(&err), Some("CHANNEL_LIMIT"), "{err:?}");
    // 释放占用：App 的握手失败后回到可接受状态，之后照常接受 Hub 的通道（不激活新进程）。
    drop(squatter);
    let deadline = tokio::time::Instant::now() + T;
    let r = loop {
        match call(&hub, app, "pid", json!({})).await {
            Ok(r) => break r,
            Err(e) => {
                assert!(tokio::time::Instant::now() < deadline, "占用释放后应能调用：{e:?}");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };
    assert_eq!(r["pid"].as_u64(), Some(u64::from(resident.id())), "{r}");
    assert_eq!(count(log, "start"), 3, "常驻 App 只启动过一次：{:?}", events(log));
    let _ = resident.kill();
    let _ = resident.wait();
    eventually("常驻 App 的管道消失", || !pipe_exists(&ctx.pipe)).await;
    eventually("Hub 不再持有连接", || hub.apps().iter().any(|a| a.app_id == app && !a.connected)).await;

    // 6. 名字被其他程序抢注（本测试进程创建同名管道）→ PEER_IDENTITY_MISMATCH，不激活。
    let impostor = ServerOptions::new().first_pipe_instance(true).create(&ctx.pipe).expect("抢注管道");
    let err = call(&hub, app, "echo", json!({})).await.expect_err("应拒绝冒名者");
    assert_eq!(err.kind, ErrorKind::LaunchFailed, "{err:?}");
    assert_eq!(code(&err), Some("PEER_IDENTITY_MISMATCH"), "{err:?}");
    assert_eq!(count(log, "start"), 3, "管道存在时不激活");
    drop(impostor);

    // 7. 运行期间新增 / 删除登记即时生效（目录变更通知，不重启 Hub）。
    let other = format!("{app}-b");
    write_registration(&ctx.apps_dir, &other, &exe_text, kinds::EXEC, "");
    let deadline = tokio::time::Instant::now() + T;
    while named_entry(&hub, &other).await.is_none() {
        assert!(tokio::time::Instant::now() < deadline, "新登记应被发现");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    std::fs::remove_file(ctx.apps_dir.join(registration::file_name(&other))).expect("删除登记");
    let deadline = tokio::time::Instant::now() + T;
    while named_entry(&hub, &other).await.is_some() {
        assert!(tokio::time::Instant::now() < deadline, "删除的登记应被移除");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // 8. 激活程序不存在 → APP_NOT_INSTALLED（data.code = NAME_NOT_FOUND）。
    // （`executable` 本身不存在的登记在发现时即被移除，spec/naming.md 5.4；这里让 `exec` 的目标缺失。）
    let missing = ctx.apps_dir.join("missing.exe").display().to_string();
    write_registration(&ctx.apps_dir, app, &exe_text, kinds::EXEC, &missing);
    let err = call(&hub, app, "echo", json!({})).await.expect_err("程序不存在应失败");
    assert_eq!(err.kind, ErrorKind::AppNotInstalled, "{err:?}");
    assert_eq!(code(&err), Some("NAME_NOT_FOUND"), "{err:?}");

    hub.shutdown().await;
}
