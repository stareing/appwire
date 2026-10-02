//! app-mcp-host：Hub（`app-mcp-hub`）之上的命令行程序。
//!
//! - `serve`：常驻模式（推荐）。一个进程在同一端口（默认 `127.0.0.1:7717`）提供 App 连接（`/app`，WebSocket）、
//!   MCP Streamable HTTP（`/mcp`）与 `/healthz`，另有本地 IPC；多个 MCP 客户端各自建立 HTTP 会话，
//!   共享同一组 App 连接（每个会话有独立的实例选择与总览附带状态）。单实例由 `<home>/run/hub.lock` 保证，
//!   实际监听位置写在 `<home>/run/endpoints.json`；休眠实例记录写在 `<home>/state/dormant/`（重启后读回，仍可唤醒）。
//! - `service install|uninstall|status|start|stop`：当前用户的登录自启服务（[`service`]）；`install` 先检查端口占用；
//!   `install --on-demand` 改为按需启动（systemd 套接字激活 / launchd `Sockets`，[`activation`]），空闲后退出。
//! - `doctor`：逐项诊断（[`doctor`]）；`status`：一行状态摘要。
//! - `setup` / `uninstall`：一条命令安装（二进制就位、自启、写入已装 Agent 的 MCP 配置、自检）与撤销（[`setup`]）。
//! - `policy show|validate|reload|hide|deny|remove`：策略规则 `<home>/policy.json`（[`policy`]）。
//! - `stdio` / 不带子命令：单客户端 stdio 模式（兼容旧用法）。
//!
//! stdout 在 stdio 模式下专用于 MCP 协议，所有日志写 stderr（常驻模式另写 `<home>/logs/`）。

pub mod activation;
pub mod app_install;
pub mod cli;
pub mod config;
pub mod data_home;
pub mod doctor;
pub mod logging;
pub mod agents;
pub mod policy;
pub mod ports;
pub mod probe;
pub mod service;
pub mod setup;
pub mod token;

mod commands;

use std::path::Path;
use std::process::ExitCode;

use clap::Parser;

use crate::activation::{ActivationError, Inherited};
use crate::cli::{Cli, Command, HomeArg, LegacyArgs};
use crate::commands::legacy::run_legacy;
use crate::commands::serve::serve;
use crate::commands::service_cmd::{service_cmd, status_line};
use crate::commands::setup_cmd::{setup_cmd, uninstall_cmd};
use crate::config::{AppHome, FileConfig, Settings};

pub(crate) use crate::commands::serve::{probe_addr, running_host, running_instance};
pub(crate) use crate::commands::service_cmd::{install_service, service_settings, uninstall_service};

/// `status`：未运行时的退出码（与 LSB `status` 约定一致）。
pub const EXIT_NOT_RUNNING: u8 = 3;

/// 程序入口（`app-mcp-host` 与 Windows 无窗口版 `app-mcp-hostw` 共用）。
pub fn main_entry() -> ExitCode {
    let cli = Cli::parse();
    // @why 先于创建运行时（其他线程）：取得交来的套接字时要清除 LISTEN_* 环境变量（activation::take_inherited 的约定）。
    let inherited = match &cli.command {
        Some(Command::Serve(_)) => activation::take_inherited(),
        _ => Ok(None),
    };
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
    match runtime.block_on(run(cli, inherited)) {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("{e:#}");
            eprintln!("错误：{e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli, inherited: Result<Option<Inherited>, ActivationError>) -> anyhow::Result<ExitCode> {
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
        Some(Command::Serve(args)) => serve(args, inherited).await,
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
        Some(Command::Setup(args)) => setup_cmd(args).await,
        Some(Command::Uninstall(args)) => uninstall_cmd(args).await,
        Some(Command::Policy { action }) => policy::cmd(action).await,
        Some(Command::Agent { action }) => agents::cmd(action).await,
        Some(Command::App { action }) => app_install::cmd(action).await,
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

/// 解析配置时产生的提示（弃用项等），在日志初始化之后记录。
fn log_notices(s: &Settings) {
    for n in &s.notices {
        tracing::warn!("{n}");
    }
}
