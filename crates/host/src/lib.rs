//! app-mcp-host：Hub（`app-mcp-hub`）之上的命令行程序。
//!
//! - `serve`：常驻模式（推荐）。一个进程同时提供 App 连接服务（WebSocket，默认 7717）与
//!   MCP Streamable HTTP（默认 `127.0.0.1:7718/mcp`）；多个 MCP 客户端各自建立 HTTP 会话，
//!   共享同一组 App 连接（每个会话有独立的实例选择与总览附带状态）。
//! - `service install|uninstall|status|start|stop`：当前用户的登录自启服务（[`service`]）。
//! - `stdio` / 不带子命令：单客户端 stdio 模式（兼容旧用法）。
//!
//! stdout 在 stdio 模式下专用于 MCP 协议，所有日志写 stderr（常驻模式另写 `<home>/logs/`）。

pub mod cli;
pub mod config;
pub mod logging;
pub mod probe;
pub mod service;
pub mod token;

use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use app_mcp_hub::{HttpOptions, Hub, HubConfig, load_manifests};
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

fn hub_config(s: &Settings) -> HubConfig {
    let mut manifests = Vec::new();
    for (dir, required) in &s.manifest_dirs {
        manifests.extend(load_manifests(&[], Some(dir), *required));
    }
    manifests.extend(load_manifests(&s.manifests, None, false));
    HubConfig {
        ws_addr: Some(s.ws_addr.clone()),
        manifests,
        allow_origins: s.allow_origins.clone(),
        upstreams: s.upstreams.clone(),
        lease_ttl: Duration::from_millis(s.lease_ms),
        wake_timeout: Duration::from_millis(s.wake_timeout_ms),
        wake_from_launch: s.wake_from_launch,
        ..Default::default()
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
    let hub = Hub::start(hub_config(&s))
        .await
        .with_context(|| format!("启动 App 连接服务 {} 失败", s.ws_addr))?;
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
fn probe_addr(addr: &str) -> String {
    match addr.parse::<SocketAddr>() {
        Ok(a) if a.ip().is_unspecified() => {
            let ip = if a.is_ipv4() { "127.0.0.1" } else { "[::1]" };
            format!("{ip}:{}", a.port())
        }
        _ => addr.to_owned(),
    }
}

fn describe_running(h: &app_mcp_hub::Health, http_addr: &str) -> String {
    format!(
        "app-mcp-host 已在运行（pid {}，版本 {}）：MCP http://{}{}，App 连接 ws://{}",
        h.pid,
        h.version,
        probe_addr(http_addr),
        h.mcp_path,
        h.ws_addr.as_deref().unwrap_or("-"),
    )
}

/// 端口被占用时：若 HTTP 端口上是健康的 app-mcp → 视为已在运行（退出码 0），否则报错。
async fn already_running_or(s: &Settings, what: String) -> anyhow::Result<ExitCode> {
    // 两个 serve 同时启动时，另一个可能刚绑定端口、尚未开始服务 HTTP：稍等重试一次。
    for attempt in 0..2 {
        match probe::probe(&probe_addr(&s.http_addr)).await {
            Probe::AppMcp(h) => {
                let msg = describe_running(&h, &s.http_addr);
                tracing::info!("{msg}；本进程退出");
                println!("{msg}");
                return Ok(ExitCode::SUCCESS);
            }
            Probe::Other(desc) => {
                anyhow::bail!("{what}；{} 上是其他程序（{desc}）", s.http_addr)
            }
            Probe::Free if attempt == 0 => tokio::time::sleep(Duration::from_millis(500)).await,
            Probe::Free => {}
        }
    }
    anyhow::bail!("{what}（可能是 stdio 模式的 app-mcp-host 或其他程序）")
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

    let token = match s.auth {
        AuthMode::Off => None,
        _ => Some(token::load_or_create(&home.token_file())?),
    };

    let hub = match Hub::start(hub_config(&s)).await {
        Ok(h) => h,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            return already_running_or(&s, format!("App 连接端口 {} 已被占用", s.ws_addr)).await;
        }
        Err(e) => return Err(e).with_context(|| format!("启动 App 连接服务 {} 失败", s.ws_addr)),
    };
    let options = HttpOptions {
        allow_remote: s.http_allow_remote,
        token,
        require_token_without_origin: s.auth == AuthMode::All,
    };
    let http = match hub.serve_http_with(&s.http_addr, options).await {
        Ok(a) => a,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            hub.shutdown().await;
            return already_running_or(&s, format!("MCP HTTP 端口 {} 已被占用", s.http_addr)).await;
        }
        Err(e) => {
            hub.shutdown().await;
            return Err(e).with_context(|| format!("启动 HTTP MCP 服务 {} 失败", s.http_addr));
        }
    };
    tracing::info!(
        "app-mcp-host 已就绪：MCP http://{http}/mcp，App 连接 ws://{}，令牌策略 {:?}",
        hub.ws_addr().map(|a| a.to_string()).unwrap_or_default(),
        s.auth
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

async fn wait_health(addr: &str, want_running: bool, timeout: Duration) -> Probe {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let p = probe::probe(addr).await;
        // 进程退出过程中端口可能短暂处于“接受连接但不响应”的状态（Other），继续等待。
        let done = match &p {
            Probe::AppMcp(_) => want_running,
            Probe::Free => !want_running,
            Probe::Other(_) => false,
        };
        if done || tokio::time::Instant::now() >= deadline {
            return p;
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

fn print_client_hint(s: &Settings) {
    let url = format!("http://{}/mcp", probe_addr(&s.http_addr));
    println!("MCP 端点：{url}");
    println!("  Claude Code：claude mcp add --transport http app-mcp {url}");
    if s.auth == AuthMode::All {
        println!("  已要求所有客户端携带令牌：请求头 Authorization: Bearer $(app-mcp-host token)");
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
            match wait_health(&probe_addr(&s.http_addr), true, Duration::from_secs(10)).await {
                Probe::AppMcp(h) => println!("{}", describe_running(&h, &s.http_addr)),
                Probe::Free => println!(
                    "服务尚未就绪；可用 `app-mcp-host service status` 查看，日志在 {}",
                    home.log_dir().display()
                ),
                Probe::Other(d) => println!("警告：{} 被其他程序占用（{d}）", s.http_addr),
            }
            print_client_hint(&s);
            Ok(ExitCode::SUCCESS)
        }
        ServiceAction::Uninstall(HomeArg { home }) => {
            let home = AppHome::resolve(home.as_deref())?;
            let s = service_settings(&home)?;
            let removed = service::uninstall()?;
            if !service::MANAGED_BY_OS {
                // Windows：登录启动项不管理进程，结束当前运行的实例。
                if let Probe::AppMcp(h) = probe::probe(&probe_addr(&s.http_addr)).await {
                    service::kill_pid(h.pid)?;
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
            match probe::probe(&probe_addr(&s.http_addr)).await {
                Probe::AppMcp(h) => {
                    println!("{}", describe_running(&h, &s.http_addr));
                    Ok(ExitCode::SUCCESS)
                }
                Probe::Free => {
                    println!("未运行（{} 无监听）", s.http_addr);
                    Ok(ExitCode::from(EXIT_NOT_RUNNING))
                }
                Probe::Other(d) => {
                    println!("未运行：{} 被其他程序占用（{d}）", s.http_addr);
                    Ok(ExitCode::from(EXIT_NOT_RUNNING))
                }
            }
        }
        ServiceAction::Start(HomeArg { home }) => {
            let home = AppHome::resolve(home.as_deref())?;
            let s = service_settings(&home)?;
            let addr = probe_addr(&s.http_addr);
            if let Probe::AppMcp(h) = probe::probe(&addr).await {
                println!("{}", describe_running(&h, &s.http_addr));
                return Ok(ExitCode::SUCCESS);
            }
            if !service::installed()? {
                anyhow::bail!(
                    "未安装登录自启服务：请先运行 `app-mcp-host service install`，或直接运行 `app-mcp-host serve`"
                );
            }
            service::start(&spec_for(&home)?)?;
            match wait_health(&addr, true, Duration::from_secs(10)).await {
                Probe::AppMcp(h) => {
                    println!("{}", describe_running(&h, &s.http_addr));
                    Ok(ExitCode::SUCCESS)
                }
                _ => anyhow::bail!("启动后 10 秒内未就绪；日志见 {}", home.log_dir().display()),
            }
        }
        ServiceAction::Stop(HomeArg { home }) => {
            let home = AppHome::resolve(home.as_deref())?;
            let s = service_settings(&home)?;
            let addr = probe_addr(&s.http_addr);
            if service::MANAGED_BY_OS && service::installed()? {
                service::stop()?;
            }
            // 仍在运行：Windows（登录启动项不管理进程）或手动运行的 serve → 按 pid 结束。
            if let Probe::AppMcp(h) = probe::probe(&addr).await {
                service::kill_pid(h.pid)?;
            }
            match wait_health(&addr, false, Duration::from_secs(10)).await {
                Probe::Free => {
                    println!("已停止");
                    Ok(ExitCode::SUCCESS)
                }
                Probe::AppMcp(h) => anyhow::bail!("实例（pid {}）仍在运行", h.pid),
                Probe::Other(d) => {
                    println!("app-mcp-host 未运行；{} 被其他程序占用（{d}）", s.http_addr);
                    Ok(ExitCode::SUCCESS)
                }
            }
        }
    }
}
