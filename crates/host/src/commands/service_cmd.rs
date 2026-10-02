//! service 与 status 子命令：登录自启服务的安装 / 卸载 / 状态 / 启停，以及一行状态摘要。

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use app_mcp_protocol::ConnectionErrorCode;
use app_mcp_protocol::registry::EndpointRegistry;

use super::serve::{describe_health, describe_registry, probe_addr, running_instance};
use crate::cli::{HomeArg, ServeArgs, ServiceAction};
use crate::config::{AppHome, AuthMode, FileConfig, Overrides, Settings};
use crate::probe::{self, Probe};
use crate::{EXIT_NOT_RUNNING, doctor, service, token};

/// 读取 `<home>/config.json` 合并后的设置（服务命令用来确定探测地址）。
pub(crate) fn service_settings(home: &AppHome) -> anyhow::Result<Settings> {
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
        on_demand: None,
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
pub(crate) async fn status_line(home: &Option<std::path::PathBuf>) -> anyhow::Result<ExitCode> {
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
                        "App 在线 {}、休眠 {}、唤醒中 {}，{}{}",
                        n(app_mcp_hub::AppState::Connected),
                        n(app_mcp_hub::AppState::Dormant),
                        n(app_mcp_hub::AppState::Waking),
                        doctor::callers_text(&st),
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

/// `service install` 的结果（`setup` 复用；输出由调用方决定，`setup --json` 时不打印）。
pub(crate) struct ServiceInstalled {
    pub home: AppHome,
    pub settings: Settings,
    /// 安装前的提示（配置弃用项、端口检查），已带前缀，按顺序输出。
    pub messages: Vec<String>,
    /// 服务文件 / 注册表项位置。
    pub location: String,
    /// 等待 `/healthz` 就绪的结果：运行中实例的登记信息，或未就绪的原因。
    pub registry: Result<EndpointRegistry, String>,
}

/// 写入 `<home>/config.json`、端口预检、生成令牌、以 `exe` 安装并启动登录自启服务，等待最多 10 秒就绪。
///
/// @input exe 服务运行的可执行文件（Windows 上为同目录的无窗口版，见 [`service::background_exe_for`]）。
/// @input on_demand 按需启动（spec/protocol.md 1.9）：服务管理器代为监听预检选定的地址；就绪检查经 `/healthz` 连接即触发启动。
/// @error 不支持 `--config`；监听地址均被占用时不安装（PORT_BUSY）；按需启动时监听地址不是 `IP:端口` 或 IPC 端点不是 `unix:`。
pub(crate) async fn install_service(args: &ServeArgs, exe: std::path::PathBuf, on_demand: bool) -> anyhow::Result<ServiceInstalled> {
    if args.hub.config.is_some() {
        anyhow::bail!("service install 使用 <home>/config.json，不支持 --config；可用 --home 指定配置目录");
    }
    let home = AppHome::resolve(args.hub.home.home.as_deref())?;
    std::fs::create_dir_all(&home.dir).with_context(|| format!("创建目录 {} 失败", home.dir.display()))?;
    let mut file = FileConfig::load(&home.config_file(), false)?;
    file.apply(&args.overrides()?)?;
    file.save(&home.config_file())?;
    let s = Settings::resolve(&file, &Overrides::default(), &home)?;
    let mut messages: Vec<String> = s.notices.iter().map(|n| format!("提示：{n}")).collect();
    // 按需启动的监听地址：预检选定的地址（缺省端口被占用时为备选端口），本配置目录的实例在运行时为它的地址。
    let mut on_demand_listen = s.listen.clone();
    let running = running_instance(&home).await;
    if on_demand && let Some(reg) = &running {
        // 端口 / 套接字要交给服务管理器：停下现有实例（登录自启或手动运行的 serve）。
        on_demand_listen = reg.listen.clone().unwrap_or(on_demand_listen);
        if service::MANAGED_BY_OS && service::installed()? {
            let _ = service::stop();
        }
        if running_instance(&home).await.is_some() {
            service::kill_pid(reg.identity.pid)?;
        }
        messages.push(format!("已停止运行中的实例（pid {}），监听交给服务管理器", reg.identity.pid));
    }
    // 端口预检（本配置目录的实例已在运行时跳过：端口由它占用）。
    if running.is_none() {
        let plan = doctor::port_preflight(&s).await;
        messages.extend(plan.busy.iter().map(|(_, why)| format!("端口检查：{why}")));
        match &plan.chosen {
            None => anyhow::bail!(
                "[{code}] 监听地址均被占用，未安装服务。建议：{}",
                ConnectionErrorCode::PortBusy.hint(),
                code = ConnectionErrorCode::PortBusy
            ),
            Some(addr) if !plan.busy.is_empty() => {
                on_demand_listen = addr.clone();
                messages.push(format!(
                    "提示：默认端口被占用，Host 将改用 {addr}（网页 SDK 会依次尝试 7717、7737、7757）；建议停止占用者以使用默认端口"
                ))
            }
            Some(_) => {}
        }
    }
    if s.auth != AuthMode::Off {
        token::load_or_create(&home.token_file())?;
    }
    let on_demand = match on_demand {
        true => Some(service::OnDemandSockets::from_settings(&probe_addr(&on_demand_listen), s.ipc_endpoint.as_deref())?),
        false => None,
    };
    let spec = service::ServiceSpec { exe, home: home.dir.clone(), on_demand };
    let location = service::install(&spec)?;
    if cfg!(windows) {
        service::start(&spec)?;
    }
    if let Some(sockets) = &spec.on_demand {
        // 连接即启动：就绪检查之前先连一次服务管理器代为监听的地址（可能是备选端口，与配置的 listen 不同）。
        let _ = probe::probe(&sockets.listen.to_string()).await;
    }
    let registry = match wait_status(&home, &s, true, Duration::from_secs(10)).await {
        Status::Running(reg) => Ok(reg),
        Status::NotRunning(why) => Err(why),
    };
    Ok(ServiceInstalled { home, settings: s, messages, location, registry })
}

/// 卸载登录自启服务；Windows 上（登录启动项不管理进程）同时结束本配置目录运行中的实例。返回是否曾安装。
pub(crate) async fn uninstall_service(home: &AppHome) -> anyhow::Result<bool> {
    let removed = service::uninstall()?;
    if !service::MANAGED_BY_OS
        && let Some(reg) = running_instance(home).await
    {
        service::kill_pid(reg.identity.pid)?;
    }
    Ok(removed)
}

pub(crate) async fn service_cmd(action: ServiceAction) -> anyhow::Result<ExitCode> {
    match action {
        ServiceAction::Install(args) => {
            let out = install_service(&args.serve, service::service_exe()?, args.on_demand).await?;
            for line in &out.messages {
                println!("{line}");
            }
            println!(
                "已安装{}：{}",
                if args.on_demand { "按需启动服务（首个连接时启动，空闲后退出）" } else { "登录自启服务" },
                out.location
            );
            println!("配置文件：{}", out.home.config_file().display());
            let listen = match &out.registry {
                Ok(reg) => {
                    println!("{}", describe_registry(reg));
                    reg.listen.clone().unwrap_or_else(|| out.settings.listen.clone())
                }
                Err(why) => {
                    println!(
                        "服务尚未就绪（{why}）；可用 `app-mcp-host service status` 查看，日志在 {}",
                        out.home.log_dir().display()
                    );
                    out.settings.listen.clone()
                }
            };
            print_client_hint(&listen, out.settings.auth);
            Ok(ExitCode::SUCCESS)
        }
        ServiceAction::Uninstall(HomeArg { home }) => {
            let home = AppHome::resolve(home.as_deref())?;
            let removed = uninstall_service(&home).await?;
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
