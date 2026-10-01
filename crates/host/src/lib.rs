//! app-mcp-host：Hub（`app-mcp-hub`）之上的命令行程序。
//!
//! - `serve`：常驻模式（推荐）。一个进程在同一端口（默认 `127.0.0.1:7717`）提供 App 连接（`/app`，WebSocket）、
//!   MCP Streamable HTTP（`/mcp`）与 `/healthz`，另有本地 IPC；多个 MCP 客户端各自建立 HTTP 会话，
//!   共享同一组 App 连接（每个会话有独立的实例选择与总览附带状态）。单实例由 `<home>/run/hub.lock` 保证，
//!   实际监听位置写在 `<home>/run/endpoints.json`。
//! - `service install|uninstall|status|start|stop`：当前用户的登录自启服务（[`service`]）；`install` 先检查端口占用。
//! - `doctor`：逐项诊断（[`doctor`]）；`status`：一行状态摘要。
//! - `stdio` / 不带子命令：单客户端 stdio 模式（兼容旧用法）。
//!
//! stdout 在 stdio 模式下专用于 MCP 协议，所有日志写 stderr（常驻模式另写 `<home>/logs/`）。

pub mod cli;
pub mod config;
pub mod doctor;
pub mod logging;
pub mod ports;
pub mod probe;
pub mod service;
pub mod token;

use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use app_mcp_hub::{Health, HttpOptions, Hub, HubConfig, load_manifests};
use app_mcp_protocol::ConnectionErrorCode;
use app_mcp_protocol::registry::EndpointRegistry;
use clap::Parser;

use crate::cli::{Cli, Command, HomeArg, LegacyArgs, ServeArgs, ServiceAction};
use crate::config::{AppHome, AuthMode, FileConfig, Overrides, Settings};
use crate::probe::Probe;

/// `status`：未运行时的退出码（与 LSB `status` 约定一致）。
pub const EXIT_NOT_RUNNING: u8 = 3;

/// 程序入口（`app-mcp-host` 与 Windows 无窗口版 `app-mcp-hostw` 共用）。
pub fn main_entry() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("错误：无法创建运行时：{e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("{e:#}");
            eprintln!("错误：{e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    match cli.command {
        None => run_legacy(cli.legacy).await,
        Some(Command::Stdio(args)) => {
            run_legacy(LegacyArgs {
                stdio: true,
                hub: args.hub,
                ..Default::default()
            })
            .await
        }
        Some(Command::Serve(args)) => serve(args).await,
        Some(Command::Service { action }) => service_cmd(action).await,
        Some(Command::Doctor { home, json }) => {
            let home = AppHome::resolve(home.home.as_deref())?;
            let s = service_settings(&home)?;
            let report = doctor::run(&home, &s).await;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", report.render());
            }
            Ok(if report.has_errors() { ExitCode::FAILURE } else { ExitCode::SUCCESS })
        }
        Some(Command::Status(HomeArg { home })) => status_line(&home).await,
        Some(Command::Token { home, regenerate }) => {
            let home = AppHome::resolve(home.home.as_deref())?;
            let t = if regenerate {
                token::write_new(&home.token_file())?
            } else {
                token::load_or_create(&home.token_file())?
            };
            println!("{t}");
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// 读取配置文件：显式 `--config` 必须存在；默认 `<home>/config.json` 可不存在。
fn load_file(
    home: &AppHome,
    explicit: Option<&Path>,
    use_default: bool,
) -> anyhow::Result<FileConfig> {
    match explicit {
        Some(p) => FileConfig::load(p, true),
        None if use_default => FileConfig::load(&home.config_file(), false),
        None => Ok(FileConfig::default()),
    }
}

fn hub_config(s: &Settings, home: &AppHome) -> HubConfig {
    let mut manifests = Vec::new();
    for (dir, required) in &s.manifest_dirs {
        manifests.extend(load_manifests(&[], Some(dir), *required));
    }
    manifests.extend(load_manifests(&s.manifests, None, false));
    let defaults = HubConfig::default();
    HubConfig {
        listen: Some(s.listen.clone()),
        // 显式指定的地址只绑定它本身；缺省地址被占用时依次尝试备选端口（与网页 SDK 一致）。
        listen_alternates: if s.listen_explicit { Vec::new() } else { defaults.listen_alternates },
        ipc_endpoint: s.ipc_endpoint.clone(),
        run_dir: Some(home.run_dir()),
        manifests,
        allow_origins: s.allow_origins.clone(),
        upstreams: s.upstreams.clone(),
        lease_ttl: Duration::from_millis(s.lease_ms),
        wake_timeout: Duration::from_millis(s.wake_timeout_ms),
        wake_from_launch: s.wake_from_launch,
        waker: s.waker.clone(),
        tool_exposure: s.tool_exposure,
        tool_exposure_threshold: s.tool_exposure_threshold,
        ..defaults
    }
}

/// 解析配置时产生的提示（弃用项等），在日志初始化之后记录。
fn log_notices(s: &Settings) {
    for n in &s.notices {
        tracing::warn!("{n}");
    }
}

// ---------------------------------------------------------------------------
// stdio（旧用法）
// ---------------------------------------------------------------------------

async fn run_legacy(args: LegacyArgs) -> anyhow::Result<ExitCode> {
    let home = AppHome::resolve(args.hub.home.home.as_deref())?;
    // stdio 模式只在显式 --config 时读取配置文件（与旧行为一致，不受常驻服务配置影响）。
    let file = load_file(&home, args.hub.config.as_deref(), false)?;
    let s = Settings::resolve(&file, &args.hub.overrides()?, &home)?;
    logging::init(logging::LogOptions {
        level: &s.log_level,
        file: None,
    })?;
    log_notices(&s);
    let hub = match Hub::start(hub_config(&s, &home)).await {
        Ok(h) => h,
        Err(e) if e.kind() == std::io::ErrorKind::ResourceBusy => {
            let hint = running_instance(&home)
                .await
                .and_then(|r| r.mcp_url())
                .map(|url| format!("；请让 MCP 客户端直接连接 {url}"))
                .unwrap_or_default();
            anyhow::bail!("[{}] {e}{hint}", ConnectionErrorCode::LockHeld);
        }
        Err(e) => {
            return Err(e).with_context(|| format!("启动 App 连接服务 {} 失败", describe_app_endpoints(&s)));
        }
    };
    if let Some(addr) = &args.http {
        hub.serve_http(addr, args.http_allow_remote)
            .await
            .with_context(|| format!("启动 HTTP MCP 服务 {addr} 失败"))?;
    }
    if args.http.is_none() || args.stdio {
        hub.serve_stdio().await?;
        tracing::info!("MCP 客户端已断开，退出");
    } else {
        shutdown_signal().await;
        tracing::info!("收到退出信号");
    }
    hub.shutdown().await;
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------
// serve（常驻）
// ---------------------------------------------------------------------------

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
fn describe_app_endpoints(s: &Settings) -> String {
    match &s.ipc_endpoint {
        Some(ipc) => format!("{}（本地 IPC {ipc}）", s.listen),
        None => s.listen.clone(),
    }
}

/// 运行中实例的说明（登记文件或 `/healthz` 的内容）。
fn describe_registry(r: &EndpointRegistry) -> String {
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

fn describe_health(h: &Health) -> String {
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

async fn serve(args: ServeArgs) -> anyhow::Result<ExitCode> {
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

    let token = match s.auth {
        AuthMode::Off => None,
        _ => Some(token::load_or_create(&home.token_file())?),
    };
    let options = HttpOptions {
        allow_remote: s.http_allow_remote,
        token,
        require_token_without_origin: s.auth == AuthMode::All,
    };
    let config = HubConfig {
        http: options.clone(),
        mcp_http: true,
        ..hub_config(&s, &home)
    };
    let hub = match Hub::start(config).await {
        Ok(h) => h,
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
    shutdown_signal().await;
    tracing::info!("收到退出信号，停止");
    hub.shutdown().await;
    Ok(ExitCode::SUCCESS)
}

/// Ctrl+C，或 Unix 上的 SIGTERM（systemd / launchd 停止服务）。
async fn shutdown_signal() {
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

// ---------------------------------------------------------------------------
// service
// ---------------------------------------------------------------------------

/// 读取 `<home>/config.json` 合并后的设置（服务命令用来确定探测地址）。
fn service_settings(home: &AppHome) -> anyhow::Result<Settings> {
    let file = FileConfig::load(&home.config_file(), false)?;
    Settings::resolve(&file, &Overrides::default(), home)
}

/// 服务命令看到的运行状态。
enum Status {
    /// 本配置目录的实例在运行（登记文件 + `/healthz` 确认）。
    Running(EndpointRegistry),
    /// 未运行；附带说明（监听地址空闲 / 被其他程序占用）。
    NotRunning(String),
}

async fn status_of(home: &AppHome, s: &Settings) -> Status {
    if let Some(reg) = running_instance(home).await {
        return Status::Running(reg);
    }
    if probe_addr(&s.listen).ends_with(":0") {
        return Status::NotRunning(format!("本配置目录没有运行中的实例（监听地址 {} 为随机端口，无法探测）", s.listen));
    }
    match probe::probe(&probe_addr(&s.listen)).await {
        // 端口上是 app-mcp，但不是本配置目录登记的实例（如手动以其他 --home 运行的 serve）。
        Probe::AppMcp(h) => Status::NotRunning(format!(
            "本配置目录没有登记运行中的实例；{} 上是另一个 app-mcp Host（{}）",
            s.listen,
            describe_health(&h)
        )),
        Probe::Free => Status::NotRunning(format!("{} 无监听", s.listen)),
        Probe::Other(d) => {
            let addr = probe_addr(&s.listen);
            let st = doctor::PortState::Other { description: d, owner: doctor::port_owner(&addr) };
            Status::NotRunning(format!("[{}] {}", ConnectionErrorCode::PortBusy, doctor::describe_port_state(&addr, &st)))
        }
    }
}

async fn wait_status(home: &AppHome, s: &Settings, want_running: bool, timeout: Duration) -> Status {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let st = status_of(home, s).await;
        if matches!(st, Status::Running(_)) == want_running || tokio::time::Instant::now() >= deadline {
            return st;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn spec_for(home: &AppHome) -> anyhow::Result<service::ServiceSpec> {
    Ok(service::ServiceSpec {
        exe: service::service_exe()?,
        home: home.dir.clone(),
    })
}

fn print_client_hint(listen: &str, auth: AuthMode) {
    let url = format!("http://{}/mcp", probe_addr(listen));
    println!("MCP 端点：{url}");
    println!("  Claude Code：claude mcp add --transport http app-mcp {url}");
    if auth == AuthMode::All {
        println!("  已要求所有客户端携带令牌：请求头 Authorization: Bearer $(app-mcp-host token)");
    }
}

/// `app-mcp-host status`：一行摘要。
async fn status_line(home: &Option<std::path::PathBuf>) -> anyhow::Result<ExitCode> {
    let home = AppHome::resolve(home.as_deref())?;
    let s = service_settings(&home)?;
    match status_of(&home, &s).await {
        Status::Running(reg) => {
            let id = &reg.identity;
            let token = std::fs::read_to_string(home.token_file()).ok().map(|t| t.trim().to_owned());
            let listen = reg.listen.as_deref().map(probe_addr);
            let apps = match probe::fetch_status(reg.ipc_endpoint.as_deref(), listen.as_deref(), token.as_deref()).await {
                Ok(st) => {
                    let n = |want: app_mcp_hub::AppState| {
                        st.apps.iter().filter(|a| a.kind == app_mcp_hub::AppKind::App && a.state == want).count()
                    };
                    let errors = st.apps.iter().filter(|a| a.last_error.is_some()).count();
                    format!(
                        "App 在线 {}、休眠 {}、唤醒中 {}，MCP 会话 {}{}",
                        n(app_mcp_hub::AppState::Connected),
                        n(app_mcp_hub::AppState::Dormant),
                        n(app_mcp_hub::AppState::Waking),
                        st.mcp_sessions,
                        if errors > 0 { format!("，{errors} 个 App 有最近错误（app-mcp-host doctor 查看）") } else { String::new() }
                    )
                }
                Err(e) => format!("状态不可读（{e}）"),
            };
            println!(
                "app-mcp-host 运行中：pid {}，版本 {}，HTTP {}，IPC {}；{apps}",
                id.pid,
                id.version,
                listen.as_deref().unwrap_or("未开启"),
                reg.ipc_endpoint.as_deref().unwrap_or("未开启"),
            );
            Ok(ExitCode::SUCCESS)
        }
        Status::NotRunning(why) => {
            println!("app-mcp-host 未运行（{why}）；运行 app-mcp-host doctor 查看原因");
            Ok(ExitCode::from(EXIT_NOT_RUNNING))
        }
    }
}

async fn service_cmd(action: ServiceAction) -> anyhow::Result<ExitCode> {
    match action {
        ServiceAction::Install(args) => {
            if args.hub.config.is_some() {
                anyhow::bail!(
                    "service install 使用 <home>/config.json，不支持 --config；可用 --home 指定配置目录"
                );
            }
            let home = AppHome::resolve(args.hub.home.home.as_deref())?;
            std::fs::create_dir_all(&home.dir)
                .with_context(|| format!("创建目录 {} 失败", home.dir.display()))?;
            let mut file = FileConfig::load(&home.config_file(), false)?;
            file.apply(&args.overrides()?)?;
            file.save(&home.config_file())?;
            let s = Settings::resolve(&file, &Overrides::default(), &home)?;
            for n in &s.notices {
                println!("提示：{n}");
            }
            // 端口预检（本配置目录的实例已在运行时跳过：端口由它占用）。
            if running_instance(&home).await.is_none() {
                let plan = doctor::port_preflight(&s).await;
                for (_, why) in &plan.busy {
                    println!("端口检查：{why}");
                }
                match &plan.chosen {
                    None => anyhow::bail!(
                        "[{code}] 监听地址均被占用，未安装服务。建议：{}",
                        ConnectionErrorCode::PortBusy.hint(),
                        code = ConnectionErrorCode::PortBusy
                    ),
                    Some(addr) if !plan.busy.is_empty() => println!(
                        "提示：默认端口被占用，Host 将改用 {addr}（网页 SDK 会依次尝试 7717、7737、7757）；建议停止占用者以使用默认端口"
                    ),
                    Some(_) => {}
                }
            }
            if s.auth != AuthMode::Off {
                token::load_or_create(&home.token_file())?;
            }
            let spec = spec_for(&home)?;
            let location = service::install(&spec)?;
            println!("已安装登录自启服务：{location}");
            println!("配置文件：{}", home.config_file().display());
            if cfg!(windows) {
                service::start(&spec)?;
            }
            let listen = match wait_status(&home, &s, true, Duration::from_secs(10)).await {
                Status::Running(reg) => {
                    println!("{}", describe_registry(&reg));
                    reg.listen.unwrap_or_else(|| s.listen.clone())
                }
                Status::NotRunning(why) => {
                    println!(
                        "服务尚未就绪（{why}）；可用 `app-mcp-host service status` 查看，日志在 {}",
                        home.log_dir().display()
                    );
                    s.listen.clone()
                }
            };
            print_client_hint(&listen, s.auth);
            Ok(ExitCode::SUCCESS)
        }
        ServiceAction::Uninstall(HomeArg { home }) => {
            let home = AppHome::resolve(home.as_deref())?;
            let removed = service::uninstall()?;
            if !service::MANAGED_BY_OS {
                // Windows：登录启动项不管理进程，结束当前运行的实例。
                if let Some(reg) = running_instance(&home).await {
                    service::kill_pid(reg.identity.pid)?;
                }
            }
            println!(
                "{}",
                if removed {
                    "已卸载登录自启服务"
                } else {
                    "未安装登录自启服务"
                }
            );
            Ok(ExitCode::SUCCESS)
        }
        ServiceAction::Status(HomeArg { home }) => {
            let home = AppHome::resolve(home.as_deref())?;
            let s = service_settings(&home)?;
            let installed = service::installed()?;
            println!(
                "登录自启：{}（{}）",
                if installed { "已安装" } else { "未安装" },
                service::location()?
            );
            if let Some(m) = service::manager_status() {
                println!("{m}");
            }
            println!("配置目录：{}", home.dir.display());
            match status_of(&home, &s).await {
                Status::Running(reg) => {
                    println!("{}", describe_registry(&reg));
                    Ok(ExitCode::SUCCESS)
                }
                Status::NotRunning(why) => {
                    println!("未运行（{why}）");
                    Ok(ExitCode::from(EXIT_NOT_RUNNING))
                }
            }
        }
        ServiceAction::Start(HomeArg { home }) => {
            let home = AppHome::resolve(home.as_deref())?;
            let s = service_settings(&home)?;
            if let Some(reg) = running_instance(&home).await {
                println!("{}", describe_registry(&reg));
                return Ok(ExitCode::SUCCESS);
            }
            if !service::installed()? {
                anyhow::bail!(
                    "未安装登录自启服务：请先运行 `app-mcp-host service install`，或直接运行 `app-mcp-host serve`"
                );
            }
            service::start(&spec_for(&home)?)?;
            match wait_status(&home, &s, true, Duration::from_secs(10)).await {
                Status::Running(reg) => {
                    println!("{}", describe_registry(&reg));
                    Ok(ExitCode::SUCCESS)
                }
                Status::NotRunning(why) => {
                    anyhow::bail!("启动后 10 秒内未就绪（{why}）；日志见 {}", home.log_dir().display())
                }
            }
        }
        ServiceAction::Stop(HomeArg { home }) => {
            let home = AppHome::resolve(home.as_deref())?;
            let s = service_settings(&home)?;
            if service::MANAGED_BY_OS && service::installed()? {
                service::stop()?;
            }
            // 仍在运行：Windows（登录启动项不管理进程）或手动运行的 serve → 按 pid 结束。
            if let Some(reg) = running_instance(&home).await {
                service::kill_pid(reg.identity.pid)?;
            }
            match wait_status(&home, &s, false, Duration::from_secs(10)).await {
                Status::NotRunning(_) => {
                    println!("已停止");
                    Ok(ExitCode::SUCCESS)
                }
                Status::Running(reg) => anyhow::bail!("实例（pid {}）仍在运行", reg.identity.pid),
            }
        }
    }
}
