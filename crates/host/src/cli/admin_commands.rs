//! `policy`、`agent`、`app` 子命令的参数定义。

use std::path::PathBuf;

use clap::{Args, Subcommand};

use super::HomeArg;

#[derive(Debug, Subcommand)]
pub enum PolicyCommand {
    /// 显示规则文件与运行中 Host 生效的规则（含命中次数、最近一次重载错误）。
    Show {
        #[command(flatten)]
        home: HomeArg,
        /// 输出 JSON。
        #[arg(long)]
        json: bool,
    },
    /// 校验规则文件（默认 <home>/policy.json）；不合法时退出码 1。
    Validate {
        #[command(flatten)]
        home: HomeArg,
        /// 要校验的文件。
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,
    },
    /// 让运行中的 Host 重新加载规则文件；不合法时 Host 保留之前的规则（退出码 1）。Host 未运行时退出码 3。
    Reload(HomeArg),
    /// 添加 hide 规则：App（或其中的工具）对所有 Agent 不可见，调用按不存在处理。
    Hide(PolicyRuleArgs),
    /// 添加 deny 规则：调用返回 POLICY_DENIED（工具仍可见）。
    Deny {
        #[command(flatten)]
        rule: PolicyRuleArgs,
        /// 同时禁止唤醒（App 未运行 / 休眠时不启动它）。
        #[arg(long)]
        wake: bool,
        /// 只拒绝该 Agent（agent add 登记的名字，末尾可用 *）；省略时对所有调用方生效。
        #[arg(long, value_name = "NAME")]
        agent: Option<String>,
    },
    /// 按 id 删除规则。
    Remove {
        #[command(flatten)]
        home: HomeArg,
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// 登记 Agent 并生成令牌（令牌打印到 stdout）；Host 在运行时随即生效。
    Add {
        #[command(flatten)]
        home: HomeArg,
        /// Agent 名：字母、数字、-、_、.，以字母或数字开头，至多 64 个字符。
        name: String,
        /// 为已登记的 Agent 换新令牌（旧令牌随即失效）。
        #[arg(long)]
        rotate: bool,
    },
    /// 删除 Agent（其令牌随即失效）。
    Remove {
        #[command(flatten)]
        home: HomeArg,
        name: String,
    },
    /// 让运行中的 Host 重新加载登记文件（手工编辑后）；不合法时 Host 保留之前的登记（退出码 1）。Host 未运行时退出码 3。
    Reload(HomeArg),
    /// 打印 Agent 的令牌。
    Token {
        #[command(flatten)]
        home: HomeArg,
        name: String,
    },
    /// 列出登记文件与运行中 Host 的 Agent（只列名字）。
    List {
        #[command(flatten)]
        home: HomeArg,
        /// 输出 JSON。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Args)]
pub struct PolicyRuleArgs {
    #[command(flatten)]
    pub home: HomeArg,
    /// appId（或上游名）；末尾可用 * 通配，如 shop*；* 表示全部。
    pub app: String,
    /// 工具局部名（不含 appId），末尾可用 * 通配，如 cart.*；省略时作用于整个 App。
    #[arg(long, value_name = "NAME")]
    pub tool: Option<String>,
    /// 规则 id，默认由动作与目标生成（如 hide-shop-cart.add）。
    #[arg(long)]
    pub id: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum AppAction {
    /// 登记（幂等，覆盖同一 appId 的旧登记）：写激活文件、App 登记文件；给出 --manifest 时复制到 <home>/manifests。
    Install(AppInstallArgs),
    /// 撤销登记：删除 install 写入的文件。
    Uninstall(AppUninstallArgs),
}

#[derive(Debug, Clone, Args)]
pub struct AppInstallArgs {
    /// App 的 appId（`[a-z][a-z0-9-]{0,62}`）。
    #[arg(long, value_name = "ID")]
    pub app_id: String,
    /// 被激活时运行的程序（绝对路径；激活时追加参数 --app-mcp-activation）。
    #[arg(long, value_name = "PROGRAM")]
    pub exec: PathBuf,
    /// 显示名称（缺省取清单的 name，再缺省为 appId）。
    #[arg(long)]
    pub name: Option<String>,
    /// 静态清单 app-mcp.json：复制到 <home>/manifests/<appId>.json（Host 启动时加载，未运行也能列出工具）。
    #[arg(long, value_name = "FILE")]
    pub manifest: Option<PathBuf>,
    #[command(flatten)]
    pub target: AppTargetArgs,
}

#[derive(Debug, Clone, Args)]
pub struct AppUninstallArgs {
    #[arg(long, value_name = "ID")]
    pub app_id: String,
    #[command(flatten)]
    pub target: AppTargetArgs,
}

#[derive(Debug, Clone, Default, Args)]
pub struct AppTargetArgs {
    #[command(flatten)]
    pub home: HomeArg,
    /// 数据目录（激活文件与登记文件的根），默认 Linux $XDG_DATA_HOME 或 ~/.local/share、Windows %LOCALAPPDATA%、
    /// macOS ~/Library/Application Support（LaunchAgent 写入其上级目录的 LaunchAgents）。
    #[arg(long, value_name = "DIR")]
    pub data_home: Option<PathBuf>,
    /// 写入后不通知系统：Linux 不调用 D-Bus ReloadConfig（默认调用：dbus-broker 是否自动发现新文件未确认，spec/naming.md U-05）；
    /// macOS 不执行 launchctl bootstrap / bootout（下次登录时由 launchd 载入）；Windows 无效。
    #[arg(long)]
    pub no_reload: bool,
}
