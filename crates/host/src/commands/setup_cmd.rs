//! setup / uninstall 子命令：一条命令安装与撤销（实现见 [`crate::setup`]）。

use std::process::ExitCode;

use anyhow::Context;

use crate::cli::{SetupArgs, UninstallArgs};
use crate::config::AppHome;
use crate::{service, setup};

/// 运行 Agent 命令的工作目录：配置目录（不含 `.mcp.json`，避免读到项目作用域的条目）；dry-run 且尚不存在时用临时目录。
fn agent_env(home: &AppHome) -> anyhow::Result<setup::agents::AgentEnv> {
    let work = if home.dir.is_dir() { home.dir.clone() } else { std::env::temp_dir() };
    setup::agents::AgentEnv::from_system(&work)
}

fn print_report<T: serde::Serialize>(json: bool, report: &T, text: impl FnOnce() -> String) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        print!("{}", text());
    }
    Ok(())
}

pub(crate) async fn setup_cmd(args: SetupArgs) -> anyhow::Result<ExitCode> {
    let home = AppHome::resolve(args.home.home.as_deref())?;
    if !args.dry_run {
        std::fs::create_dir_all(&home.dir).with_context(|| format!("创建目录 {} 失败", home.dir.display()))?;
    }
    let opts = setup::SetupOptions { home: home.clone(), agents: args.agents, force: args.force, dry_run: args.dry_run };
    let report = setup::setup(
        &opts,
        &setup::ops::SystemHost,
        &setup::ops::SystemRunner::default(),
        &agent_env(&home)?,
        &service::current_exe()?,
    )
    .await?;
    print_report(args.json, &report, || report.render())?;
    Ok(if report.ok() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

pub(crate) async fn uninstall_cmd(args: UninstallArgs) -> anyhow::Result<ExitCode> {
    let home = AppHome::resolve(args.home.home.as_deref())?;
    let opts = setup::UninstallOptions { home: home.clone(), purge: args.purge, dry_run: args.dry_run };
    let report = setup::uninstall(&opts, &setup::ops::SystemHost, &setup::ops::SystemRunner::default(), &agent_env(&home)?).await?;
    print_report(args.json, &report, || report.render())?;
    Ok(if report.ok() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
