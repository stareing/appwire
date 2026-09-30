//! 命令行定义。

use std::path::PathBuf;

use app_mcp_hub::upstream::parse_cli_spec;
use clap::{Args, Parser, Subcommand};

use crate::config::{AuthMode, Overrides};

/// 本地 MCP Host：聚合本机各 App 的工具并以 MCP 暴露给模型。
///
/// 推荐：`app-mcp-host service install`（登录自启）或 `app-mcp-host serve`（前台常驻），
/// MCP 客户端连接 http://127.0.0.1:7718/mcp。不带子命令时为 stdio 模式（兼容旧用法）。
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
    /// 常驻模式（推荐）：同时提供 App 连接服务（WebSocket）与 MCP Streamable HTTP，
    /// 多个 MCP 客户端共享同一组 App 连接。已有健康实例在运行时直接退出（退出码 0）。
    Serve(ServeArgs),
    /// stdio 模式：单个 MCP 客户端以子进程方式启动（测试 / 无法安装服务的环境）。
    Stdio(StdioArgs),
    /// 当前用户的登录自启服务（systemd --user / launchd / Windows 登录启动项）。
    Service {
        #[command(subcommand)]
        action: ServiceAction,
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
    Install(Box<ServeArgs>),
    /// 停止并卸载。
    Uninstall(HomeArg),
    /// 显示安装与运行状态。
    Status(HomeArg),
    /// 启动。
    Start(HomeArg),
    /// 停止。
    Stop(HomeArg),
}

#[derive(Debug, Clone, Default, Args)]
pub struct HomeArg {
    /// 配置目录（config.json、token、logs/、manifests/），默认 $APP_MCP_HOME 或 ~/.app-mcp。
    #[arg(long, value_name = "DIR")]
    pub home: Option<PathBuf>,
}

/// stdio / serve 共用的 Hub 参数。都是可选的：未指定时用配置文件或默认值。
#[derive(Debug, Clone, Default, Args)]
pub struct HubArgs {
    /// App 连接服务（WebSocket）监听地址，默认 127.0.0.1:7717。
    #[arg(long, value_name = "ADDR")]
    pub ws_addr: Option<String>,

    /// 静态清单文件，可重复。后加载的覆盖同 appId 的先加载的。
    #[arg(long = "manifest", value_name = "FILE")]
    pub manifests: Vec<PathBuf>,

    /// 静态清单目录（读取其中所有 *.json），可重复；默认 <home>/manifests（不存在时忽略）。
    #[arg(long = "manifest-dir", value_name = "DIR")]
    pub manifest_dirs: Vec<PathBuf>,

    /// 额外允许的 Origin，可重复；端口可用 * 通配，如 https://app.example.com:*。
    #[arg(long = "allow-origin", value_name = "PATTERN")]
    pub allow_origins: Vec<String>,

    /// 上游 MCP 服务器，可重复：<name>=<命令行>，如 files=npx -y @modelcontextprotocol/server-filesystem /tmp。
    #[arg(long = "upstream", value_name = "NAME=COMMAND")]
    pub upstreams: Vec<String>,

    /// 每次调用某 App 实例后发送的租约时长（毫秒，spec/lifecycle.md 4.2），默认 60000；0 关闭租约。
    #[arg(long, value_name = "MS")]
    pub lease_ms: Option<u64>,

    /// 唤醒休眠 / 未运行的 App 后等待其回连的上限（毫秒），默认 15000。
    #[arg(long, value_name = "MS")]
    pub wake_timeout_ms: Option<u64>,

    /// App 未运行且清单没有显式 wake 时，由清单 launch 推导唤醒方式（会打开 launch.web 地址等）。
    #[arg(long)]
    pub wake_from_launch: bool,

    /// 日志级别（trace / debug / info / warn / error），默认 info。设置 RUST_LOG 时以 RUST_LOG 为准。
    #[arg(long, value_name = "LEVEL")]
    pub log_level: Option<String>,

    /// 配置文件（格式见 README），默认 <home>/config.json。
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    #[command(flatten)]
    pub home: HomeArg,
}

impl HubArgs {
    pub fn overrides(&self) -> anyhow::Result<Overrides> {
        let mut upstreams = std::collections::BTreeMap::new();
        for spec in &self.upstreams {
            let (name, cfg) = parse_cli_spec(spec).map_err(anyhow::Error::msg)?;
            upstreams.insert(name, cfg);
        }
        Ok(Overrides {
            ws_addr: self.ws_addr.clone(),
            manifests: self.manifests.clone(),
            manifest_dirs: self.manifest_dirs.clone(),
            allow_origins: self.allow_origins.clone(),
            upstreams,
            lease_ms: self.lease_ms,
            wake_timeout_ms: self.wake_timeout_ms,
            wake_from_launch: self.wake_from_launch.then_some(true),
            log_level: self.log_level.clone(),
            ..Default::default()
        })
    }
}

#[derive(Debug, Clone, Default, Args)]
pub struct ServeArgs {
    #[command(flatten)]
    pub hub: HubArgs,

    /// MCP Streamable HTTP 监听地址（端点 http://<ADDR>/mcp），默认 127.0.0.1:7718。
    #[arg(long, value_name = "ADDR")]
    pub http: Option<String>,

    /// 允许绑定非回环地址并接受任意 Host 头（有安全风险）。
    #[arg(long)]
    pub http_allow_remote: bool,

    /// 本地访问令牌策略：browser（默认，带 Origin 的浏览器请求必须带令牌）/ all（所有请求）/ off。
    #[arg(long, value_enum, value_name = "MODE")]
    pub auth: Option<AuthMode>,

    /// 不写日志文件（只写 stderr）。
    #[arg(long)]
    pub no_log_file: bool,
}

impl ServeArgs {
    pub fn overrides(&self) -> anyhow::Result<Overrides> {
        let mut o = self.hub.overrides()?;
        o.http_addr = self.http.clone();
        o.http_allow_remote = self.http_allow_remote.then_some(true);
        o.auth = self.auth;
        o.log_file = self.no_log_file.then_some(false);
        Ok(o)
    }
}

#[derive(Debug, Clone, Default, Args)]
pub struct StdioArgs {
    #[command(flatten)]
    pub hub: HubArgs,
}

/// 旧用法：不带子命令。
#[derive(Debug, Clone, Default, Args)]
pub struct LegacyArgs {
    /// 以 stdio 作为 MCP 传输（不带子命令时的默认行为）。
    #[arg(long)]
    pub stdio: bool,

    /// 同时以 Streamable HTTP 提供 MCP（不带令牌校验；推荐改用 `serve`）。只给 --http 不给 --stdio 时只提供 HTTP。
    #[arg(long, value_name = "ADDR")]
    pub http: Option<String>,

    /// 允许 --http 绑定非回环地址并接受任意 Host 头（有安全风险）。
    #[arg(long)]
    pub http_allow_remote: bool,

    #[command(flatten)]
    pub hub: HubArgs,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_subcommands_and_legacy() {
        let cli = Cli::try_parse_from([
            "app-mcp-host",
            "serve",
            "--http",
            "127.0.0.1:1",
            "--auth",
            "all",
        ])
        .unwrap();
        let Some(Command::Serve(s)) = cli.command else {
            panic!()
        };
        assert_eq!(s.http.as_deref(), Some("127.0.0.1:1"));
        assert_eq!(s.auth, Some(AuthMode::All));

        let cli = Cli::try_parse_from(["app-mcp-host", "--stdio", "--manifest", "a.json"]).unwrap();
        assert!(cli.command.is_none());
        assert!(cli.legacy.stdio);
        assert_eq!(cli.legacy.hub.manifests, vec![PathBuf::from("a.json")]);

        let cli =
            Cli::try_parse_from(["app-mcp-host", "service", "install", "--manifest", "a.json"])
                .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Service {
                action: ServiceAction::Install(_)
            })
        ));
        let cli =
            Cli::try_parse_from(["app-mcp-host", "service", "status", "--home", "/x"]).unwrap();
        let Some(Command::Service {
            action: ServiceAction::Status(h),
        }) = cli.command
        else {
            panic!()
        };
        assert_eq!(h.home, Some(PathBuf::from("/x")));
    }
}
