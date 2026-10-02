//! 命令行定义。

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::setup::agents::{AgentSelection, parse_selection};

mod admin_commands;
mod hub_args;

pub use admin_commands::{AgentCommand, AppAction, AppInstallArgs, AppTargetArgs, AppUninstallArgs, PolicyCommand, PolicyRuleArgs};
pub use hub_args::{HubArgs, LegacyArgs, ServeArgs, ServiceInstallArgs, StdioArgs};

/// 本地 MCP Host：聚合本机各 App 的工具并以 MCP 暴露给模型。
///
/// 推荐：`app-mcp-host service install`（登录自启）或 `app-mcp-host serve`（前台常驻），
/// MCP 客户端连接 http://127.0.0.1:7717/mcp（App 连接同一端口的 /app）。不带子命令时为 stdio 模式（兼容旧用法）。
#[derive(Debug, Parser)]
#[command(
    name = "app-mcp-host",
    version,
    about,
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// 旧用法（不带子命令）：stdio 模式，可加 --http 同时提供 HTTP。
    #[command(flatten)]
    pub legacy: LegacyArgs,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 常驻模式（推荐）：同一端口提供 App 连接（/app，WebSocket）、MCP Streamable HTTP（/mcp）与 /healthz，
    /// 另有本地 IPC；多个 MCP 客户端共享同一组 App 连接。已有实例在运行时（单实例锁）打印其信息并退出（退出码 0）。
    Serve(ServeArgs),
    /// stdio 模式：单个 MCP 客户端以子进程方式启动（测试 / 无法安装服务的环境）。
    Stdio(StdioArgs),
    /// 当前用户的登录自启服务（systemd --user / launchd / Windows 登录启动项）。
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// 诊断：逐项检查 Host 运行、单实例锁、本地 IPC、端口占用（含占用进程）、Windows 排除端口段、令牌、
    /// 各 App 实例状态与最近错误、网页拦截上报、adb reverse，每项给出结论与修复建议。有错误时退出码 1。
    Doctor {
        #[command(flatten)]
        home: HomeArg,
        /// 输出 JSON（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 一行状态摘要（运行中的 Host、App 在线 / 休眠数）；未运行时退出码 3。
    Status(HomeArg),
    /// 一条命令完成安装（幂等）：从 npx / uvx 等包管理器目录运行时把二进制复制到 <home>/bin → 登录自启服务 →
    /// 等待 /healthz → 把 Host 写入已安装 MCP Agent 的配置（已有不同的同名条目时需 --force；写前备份、写后校验、
    /// 失败回滚）→ doctor 自检。改动记录在 <home>/setup.json，供 uninstall 撤销。有失败 / 冲突时退出码 1。
    Setup(SetupArgs),
    /// 撤销 setup：按 <home>/setup.json 只删除 setup 写入的 Agent 条目（文件自写入后未变化时整文件恢复备份）、
    /// 卸载 setup 安装的服务；--purge 同时删除 <home>/bin。
    Uninstall(UninstallArgs),
    /// 策略规则（<home>/policy.json）：hide 让 App / 工具对所有 Agent 不可见（调用按不存在），deny 拒绝调用 / 唤醒
    /// （POLICY_DENIED）。无规则时默认放行。按注解匹配等完整写法直接编辑文件后 reload（spec/hub-api.md 3.13）。
    Policy {
        #[command(subcommand)]
        action: PolicyCommand,
    },
    /// Agent 登记（<home>/agents.json）：每个 Agent 一个令牌，MCP 客户端以 Authorization: Bearer <令牌> 连接 /mcp 时
    /// 按 Agent 区分（任务、任务句柄、apps.select、租约分开）。未登记时所有请求为本机主体（spec/hub-api.md 3.6「Agent 身份」）。
    Agent {
        #[command(subcommand)]
        action: AgentCommand,
    },
    /// 按名寻址的 App 登记（spec/naming.md 4.1、4.3、4.4、5.3）：生成 App 登记文件（Linux 另生成 D-Bus 激活文件
    /// `$XDG_DATA_HOME/dbus-1/services/dev.appmcp.App.<appId>.service`；Windows 写 `%LOCALAPPDATA%\app-mcp\apps\<appId>.json`；
    /// macOS 另生成按需套接字的用户 LaunchAgent `~/Library/LaunchAgents/dev.appmcp.App.<appId>.plist` 并 `launchctl bootstrap`），
    /// Hub（`serve --name-service`）不启动 App 即可发现它，调用时按需激活。
    App {
        #[command(subcommand)]
        action: AppAction,
    },
    /// 打印本地访问令牌（不存在时生成），供 MCP 客户端配置 `Authorization: Bearer <令牌>`。
    Token {
        #[command(flatten)]
        home: HomeArg,
        /// 重新生成令牌（旧令牌立即失效；运行中的实例需重启）。
        #[arg(long)]
        regenerate: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ServiceAction {
    /// 安装并启动。给出的 serve 参数写入配置文件（`<home>/config.json`）。
    Install(Box<ServiceInstallArgs>),
    /// 停止并卸载。
    Uninstall(HomeArg),
    /// 显示安装与运行状态。
    Status(HomeArg),
    /// 启动。
    Start(HomeArg),
    /// 停止。
    Stop(HomeArg),
}

#[derive(Debug, Clone, Args)]
pub struct SetupArgs {
    #[command(flatten)]
    pub home: HomeArg,
    /// 要配置的 MCP Agent：all（默认，检测到的全部）/ none / 逗号分隔的列表：claude-code、codex、gemini、cursor、
    /// vscode、windsurf、claude-desktop（后两者只打印手动配置说明）。
    #[arg(long, value_name = "LIST|all|none", default_value = "all", value_parser = parse_selection)]
    pub agents: AgentSelection,
    /// 替换 Agent 配置中已有的不同的同名条目（app-mcp）。
    #[arg(long)]
    pub force: bool,
    /// 只列出将要执行的操作，不做任何修改。
    #[arg(long)]
    pub dry_run: bool,
    /// 输出 JSON（机器可读）。
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct UninstallArgs {
    #[command(flatten)]
    pub home: HomeArg,
    /// 同时删除 <home>/bin（setup 复制的二进制）。
    #[arg(long)]
    pub purge: bool,
    /// 只列出将要执行的操作，不做任何修改。
    #[arg(long)]
    pub dry_run: bool,
    /// 输出 JSON（机器可读）。
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Default, Args)]
pub struct HomeArg {
    /// 配置目录（config.json、token、logs/、manifests/），默认 $APP_MCP_HOME 或 ~/.app-mcp。
    #[arg(long, value_name = "DIR")]
    pub home: Option<PathBuf>,
}

#[cfg(test)]
mod tests;
