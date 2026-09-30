//! 命令行定义。

use std::path::PathBuf;

use app_mcp_hub::{ToolExposure, WakerConfig};
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

    /// 本地 IPC 端点（原生 App 默认连接这里）：unix:<绝对路径>（Linux / macOS）或 pipe:\\.\pipe\<名称>
    /// （Windows）；none = 关闭。默认为平台默认端点（Linux $XDG_RUNTIME_DIR/app-mcp/hub.sock，
    /// 其次 ~/.app-mcp/run/hub.sock；macOS ~/.app-mcp/run/hub.sock；Windows \\.\pipe\app-mcp-<用户 SID>）。
    #[arg(long, value_name = "ENDPOINT|none")]
    pub ipc_endpoint: Option<String>,

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

    /// 唤醒器：system（默认，按平台执行系统激活）/ none（不唤醒，调用返回 APP_DISCONNECTED 与启动地址）/
    /// JSON 形式的 {"exec":["程序","参数",...]}（执行该程序，参数不经 shell，唤醒请求以一行 JSON 写入其 stdin）。
    #[arg(long, value_name = "system|none|JSON", value_parser = parse_waker)]
    pub waker: Option<WakerConfig>,

    /// 工具暴露方式：auto（默认，App 工具总数超过阈值时渐进）/ progressive（工具列表只含 apps.* 与本会话
    /// 展开过、调用过或选定了实例的 App，其余用 apps.tools 查看）/ all（全部列出）。
    #[arg(long, value_name = "auto|progressive|all", value_parser = parse_exposure)]
    pub tool_exposure: Option<ToolExposure>,

    /// auto 模式的阈值：App 与上游工具总数超过此值时渐进暴露，默认 40。
    #[arg(long, value_name = "N")]
    pub tool_exposure_threshold: Option<usize>,

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
            ipc_endpoint: self.ipc_endpoint.clone(),
            manifests: self.manifests.clone(),
            manifest_dirs: self.manifest_dirs.clone(),
            allow_origins: self.allow_origins.clone(),
            upstreams,
            lease_ms: self.lease_ms,
            wake_timeout_ms: self.wake_timeout_ms,
            wake_from_launch: self.wake_from_launch.then_some(true),
            waker: self.waker.clone(),
            tool_exposure: self.tool_exposure,
            tool_exposure_threshold: self.tool_exposure_threshold,
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


/// `--waker`：`system` / `none` 或 JSON（与配置文件 `lifecycle.waker` 相同的形式）。
fn parse_exposure(s: &str) -> Result<ToolExposure, String> {
    serde_json::from_value(serde_json::Value::String(s.trim().to_owned()))
        .map_err(|_| format!("应为 auto、progressive 或 all，而不是「{s}」"))
}

fn parse_waker(s: &str) -> Result<WakerConfig, String> {
    let s = s.trim();
    let json = if s.starts_with('{') || s.starts_with('"') {
        s.to_owned()
    } else {
        format!("\"{s}\"")
    };
    let w: WakerConfig = serde_json::from_str(&json)
        .map_err(|e| format!("应为 system、none 或 {{\"exec\":[\"程序\",...]}}：{e}"))?;
    if matches!(&w, WakerConfig::Exec(argv) if argv.first().is_none_or(String::is_empty)) {
        return Err("exec 至少需要一个元素（要执行的程序）".into());
    }
    Ok(w)
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
    fn parses_waker() {
        assert_eq!(parse_waker("system"), Ok(WakerConfig::System));
        assert_eq!(parse_waker("none"), Ok(WakerConfig::None));
        assert_eq!(
            parse_waker(r#"{"exec":["node","/t/wake.mjs","--x"]}"#),
            Ok(WakerConfig::Exec(vec!["node".into(), "/t/wake.mjs".into(), "--x".into()]))
        );
        assert!(parse_waker("shell").is_err());
        assert!(parse_waker(r#"{"exec":[]}"#).is_err());
        let cli = Cli::try_parse_from(["app-mcp-host", "serve", "--waker", "none"]).unwrap();
        let Some(Command::Serve(s)) = cli.command else {
            panic!()
        };
        assert_eq!(s.hub.overrides().unwrap().waker, Some(WakerConfig::None));
    }

    #[test]
    fn parses_tool_exposure() {
        assert_eq!(parse_exposure("progressive"), Ok(ToolExposure::Progressive));
        assert_eq!(parse_exposure("all"), Ok(ToolExposure::All));
        assert!(parse_exposure("some").is_err());
        let cli = Cli::try_parse_from([
            "app-mcp-host",
            "serve",
            "--tool-exposure",
            "auto",
            "--tool-exposure-threshold",
            "5",
        ])
        .unwrap();
        let Some(Command::Serve(s)) = cli.command else {
            panic!()
        };
        let o = s.hub.overrides().unwrap();
        assert_eq!(o.tool_exposure, Some(ToolExposure::Auto));
        assert_eq!(o.tool_exposure_threshold, Some(5));
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
