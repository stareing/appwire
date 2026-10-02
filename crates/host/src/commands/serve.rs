//! serve 子命令（常驻模式）：单实例检测、端口占用说明、按需启动的空闲退出与关闭信号。

use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use app_mcp_hub::{Health, HttpOptions, Hub, HubConfig};
use app_mcp_protocol::ConnectionErrorCode;
use app_mcp_protocol::registry::EndpointRegistry;

use super::hub_config::hub_config;
use crate::activation::{ActivationError, Inherited};
use crate::cli::ServeArgs;
use crate::config::{AppHome, AuthMode, Settings};
use crate::probe::{self, Probe};
use crate::{agents, doctor, load_file, log_notices, logging, policy, token};

/// 探测用地址：未指定地址（0.0.0.0 / ::）换成回环。
pub(crate) fn probe_addr(addr: &str) -> String {
    match addr.parse::<SocketAddr>() {
        Ok(a) if a.ip().is_unspecified() => {
            let ip = if a.is_ipv4() { "127.0.0.1" } else { "[::1]" };
            format!("{ip}:{}", a.port())
        }
        _ => addr.to_owned(),
    }
}

/// App 连接服务的监听位置（日志 / 错误信息用）。
pub(super) fn describe_app_endpoints(s: &Settings) -> String {
    match &s.ipc_endpoint {
        Some(ipc) => format!("{}（本地 IPC {ipc}）", s.listen),
        None => s.listen.clone(),
    }
}

/// 运行中实例的说明（登记文件或 `/healthz` 的内容）。
pub(super) fn describe_registry(r: &EndpointRegistry) -> String {
    let id = &r.identity;
    let listen = r.listen.as_deref().map(probe_addr);
    format!(
        "app-mcp-host 已在运行（pid {}，版本 {}）：MCP {}，App 连接 {}，本地 IPC {}",
        id.pid,
        id.version,
        listen.as_deref().map(|a| format!("http://{a}/mcp")).unwrap_or_else(|| "-".into()),
        listen.as_deref().map(|a| format!("ws://{a}/app")).unwrap_or_else(|| "-".into()),
        r.ipc_endpoint.as_deref().unwrap_or("未开启"),
    )
}

pub(super) fn describe_health(h: &Health) -> String {
    describe_registry(&EndpointRegistry {
        identity: h.identity.clone(),
        listen: h.listen.clone(),
        ipc_endpoint: h.ipc_endpoint.clone(),
        started_at_ms: 0,
    })
}

/// 本配置目录下运行中的实例：读登记文件，并经 `/healthz` 确认它确实在服务（进程号一致）。
/// 登记文件不存在、或其中的地址上不是该进程时返回 `None`。
pub(crate) async fn running_instance(home: &AppHome) -> Option<EndpointRegistry> {
    let reg = EndpointRegistry::read(&home.registry_file()).ok().flatten()?;
    let Some(listen) = &reg.listen else {
        // 没有 TCP 服务：只能以登记文件为准（单实例锁保证写它的进程仍持锁时才存在）。
        return Some(reg);
    };
    match probe::probe(&probe_addr(listen)).await {
        Probe::AppMcp(h) if h.identity.pid == reg.identity.pid => Some(reg),
        _ => None,
    }
}

/// 运行中的 Host 与访问它所需的本机令牌（`policy reload`、`agent add / remove` 推送变更）。
pub(crate) async fn running_host(home: &AppHome) -> Option<(EndpointRegistry, Option<String>)> {
    let reg = running_instance(home).await?;
    let token = std::fs::read_to_string(home.token_file()).ok().map(|t| t.trim().to_owned());
    Some((reg, token))
}

/// 单实例锁已被持有：等待持锁实例写好登记文件（可能正在启动），打印其信息后以退出码 0 结束。
async fn already_running(home: &AppHome, err: &std::io::Error) -> anyhow::Result<ExitCode> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(Some(reg)) = EndpointRegistry::read(&home.registry_file()) {
            let msg = describe_registry(&reg);
            tracing::info!("{msg}；本进程退出");
            println!("{msg}");
            return Ok(ExitCode::SUCCESS);
        }
        if tokio::time::Instant::now() >= deadline {
            // 锁被持有但一直没有登记文件：持锁进程卡在启动阶段，属于异常，如实报告。
            anyhow::bail!("{err}；5 秒内未写出登记文件 {}", home.registry_file().display());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 端口 / IPC 端点被占用（单实例锁已取得，因此不是同一配置目录的 Host）：说明占用者。
async fn occupied_error(s: &Settings, e: std::io::Error) -> anyhow::Error {
    let addr = probe_addr(&s.listen);
    let st = doctor::port_state(&addr, None).await;
    let code = if st == doctor::PortState::Free {
        ConnectionErrorCode::IpcEndpointBusy
    } else {
        ConnectionErrorCode::PortBusy
    };
    let who = match st {
        doctor::PortState::Free => "监听地址空闲，被占用的是本地 IPC 端点".to_owned(),
        other => doctor::describe_port_state(&addr, &other),
    };
    anyhow::anyhow!(
        "[{code}] 启动 App 连接服务 {} 失败：{e}；{who}。建议：{}",
        describe_app_endpoints(s),
        code.hint()
    )
}

pub(crate) async fn serve(args: ServeArgs, inherited: Result<Option<Inherited>, ActivationError>) -> anyhow::Result<ExitCode> {
    let home = AppHome::resolve(args.hub.home.home.as_deref())?;
    let file = load_file(&home, args.hub.config.as_deref(), true)?;
    let s = Settings::resolve(&file, &args.overrides()?, &home)?;
    let log_path = logging::init(logging::LogOptions {
        level: &s.log_level,
        file: s
            .log_file
            .then(|| (home.log_dir(), s.log_max_bytes, s.log_keep)),
    })?;
    tracing::info!(
        pid = std::process::id(),
        version = env!("CARGO_PKG_VERSION"),
        home = %home.dir.display(),
        log = ?log_path,
        "app-mcp-host serve 启动"
    );
    log_notices(&s);
    let inherited = inherited?;
    let idle_exit = on_demand_idle(inherited.as_ref(), s.idle_exit_ms);
    let prebound = match inherited {
        Some(i) => i.listeners,
        None => app_mcp_hub::PreboundListeners::default(),
    };

    let token = match s.auth {
        AuthMode::Off => None,
        _ => Some(token::load_or_create(&home.token_file())?),
    };
    let options = HttpOptions {
        allow_remote: s.http_allow_remote,
        token,
        require_token_without_origin: s.auth == AuthMode::All,
    };
    let mut config = HubConfig {
        http: options.clone(),
        mcp_http: true,
        policy: policy::load(&home)?,
        agents: agents::load(&home)?,
        ..hub_config(&s, &home)
    };
    if !prebound.is_empty() {
        without_unhanded_listeners(&mut config, &prebound);
    }
    let activated = !prebound.is_empty();
    let hub = match Hub::start_with(config, prebound).await {
        Ok(h) => h,
        Err(e) if e.kind() == std::io::ErrorKind::ResourceBusy && activated => {
            // @why 以 0 退出时服务管理器看到套接字上仍有未接受的连接，会立即再次启动本进程（循环）；报错让它记为失败。
            anyhow::bail!(
                "[{}] {e}：同一配置目录已有不经服务管理器运行的 Host，按需启动的实例无法接管；请停止手动运行的 serve",
                ConnectionErrorCode::LockHeld
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::ResourceBusy => {
            tracing::info!(code = ConnectionErrorCode::LockHeld.as_str(), "{e}");
            return already_running(&home, &e).await;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return Err(occupied_error(&s, e).await),
        Err(e) => {
            return Err(e).with_context(|| format!("启动 App 连接服务 {} 失败", describe_app_endpoints(&s)));
        }
    };
    if let Some(addr) = &s.compat_http_addr
        && let Err(e) = hub.serve_http_with(addr, options).await
    {
        hub.shutdown().await;
        return Err(e).with_context(|| format!("启动兼容期 MCP 端口 {addr}（http.addr / --http）失败"));
    }
    let listen = hub.listen_addr().map(|a| a.to_string()).unwrap_or_default();
    tracing::info!(
        "app-mcp-host 已就绪：MCP http://{listen}/mcp，App 连接 ws://{listen}/app，本地 IPC {}，令牌策略 {:?}，登记文件 {}",
        hub.ipc_endpoint().unwrap_or("未开启"),
        s.auth,
        home.registry_file().display(),
    );
    match idle_exit {
        Some(idle) => tokio::select! {
            () = shutdown_signal() => tracing::info!("收到退出信号，停止"),
            () = hub.wait_idle(idle) => tracing::info!("空闲 {} 秒，退出（下一个连接由服务管理器再次启动）", idle.as_secs_f32()),
        },
        None => {
            shutdown_signal().await;
            tracing::info!("收到退出信号，停止");
        }
    }
    hub.shutdown().await;
    Ok(ExitCode::SUCCESS)
}

/// 按需启动时只服务交来的监听器：没有交来的那一个不自己绑定。
///
/// @why 自己绑定的端口 / 套接字在空闲退出后无人监听，客户端连不上也不会触发再次启动；监听位置归服务管理器的单元 / plist。
fn without_unhanded_listeners(config: &mut HubConfig, prebound: &app_mcp_hub::PreboundListeners) {
    if prebound.tcp.is_none() && config.listen.take().is_some() {
        tracing::warn!("服务管理器没有交来 TCP 监听套接字：本次不开 HTTP 服务（/app、/mcp）；检查单元的 ListenStream / plist 的 Sockets");
    }
    #[cfg(unix)]
    if prebound.ipc.is_none() && config.ipc_endpoint.take().is_some() {
        tracing::info!("服务管理器没有交来本地 IPC 套接字：本次不开本地 IPC");
    }
}

/// 按需启动时的空闲退出时间：只在监听套接字由服务管理器交来时生效（否则退出后没有谁再启动 Host），`0` = 不退出。
fn on_demand_idle(inherited: Option<&Inherited>, idle_exit_ms: u64) -> Option<Duration> {
    let i = inherited?;
    tracing::info!(
        "按需启动（{}）：监听 {}；{}",
        i.source,
        i.described.join("、"),
        if idle_exit_ms == 0 { "不空闲退出（idleExitMs = 0）".to_owned() } else { format!("空闲 {idle_exit_ms} ms 后退出") }
    );
    (idle_exit_ms > 0).then(|| Duration::from_millis(idle_exit_ms))
}

/// Ctrl+C，或 Unix 上的 SIGTERM（systemd / launchd 停止服务）。
pub(super) async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
