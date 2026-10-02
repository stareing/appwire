//! 命令行定义。

use std::path::PathBuf;

use app_mcp_hub::{LeaseOverrides, LimitOverrides, McpProtocolMode, OutputValidation, ToolExposure, WakerConfig};
use app_mcp_hub::upstream::parse_cli_spec;
use clap::{Args, Parser, Subcommand};

use crate::config::{AuthMode, Overrides};
use crate::setup::agents::{AgentSelection, parse_selection};

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

/// stdio / serve 共用的 Hub 参数。都是可选的：未指定时用配置文件或默认值。
#[derive(Debug, Clone, Default, Args)]
pub struct HubArgs {
    /// HTTP 监听地址：同一端口承载 /app（App 的 WebSocket 连接）、/mcp（serve 模式）与 /healthz。
    /// 默认 127.0.0.1:7717（被占用时依次尝试 7737、7757）；显式指定时只绑定该地址。
    #[arg(long, value_name = "ADDR")]
    pub listen: Option<String>,

    /// 已弃用：--listen 的旧名（按 --listen 使用并记录提示）。
    #[arg(long, value_name = "ADDR", hide = true)]
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

    /// 导航等待上限（毫秒）：`app/navigate` 的回复与导航后等待目标工具注册合计，默认 5000。
    #[arg(long, value_name = "MS")]
    pub navigate_timeout_ms: Option<u64>,

    /// 唤醒令牌的有效期（毫秒，spec/lifecycle.md 4.4），默认 60000。
    #[arg(long, value_name = "MS")]
    pub wake_token_ttl_ms: Option<u64>,

    /// 每个 App 每分钟最多实际发出的唤醒次数（spec/lifecycle.md 第 12 节），默认 6；0 不限。
    /// 超出时调用返回 LAUNCH_FAILED（data.code = WAKE_RATE_LIMITED）。
    #[arg(long, value_name = "N")]
    pub wake_rate_limit: Option<u32>,

    /// 回退到 4e 之前的心跳：对所有 App 连接发 ping 并按无消息断开（忽略 SDK 的 heartbeatMs 声明，
    /// spec/lifecycle.md 第 11 节）。
    #[arg(long)]
    pub legacy_heartbeat: bool,

    /// 关闭自适应租约，每次调用后固定发 --lease-ms（4e 之前的行为，spec/lifecycle.md 第 13 节 B2）。
    #[arg(long)]
    pub fixed_lease: bool,

    /// 自适应租约：统计同一（会话, App）最近多少个调用间隔，默认 20。
    #[arg(long, value_name = "N")]
    pub lease_window: Option<u32>,

    /// 自适应租约：p90 之上的余量（毫秒），默认 5000。
    #[arg(long, value_name = "MS")]
    pub lease_margin_ms: Option<u64>,

    /// 自适应租约下限（毫秒），默认 5000。
    #[arg(long, value_name = "MS")]
    pub lease_min_ms: Option<u64>,

    /// 自适应租约上限（毫秒），默认 60000；超过它的调用间隔不计入统计。
    #[arg(long, value_name = "MS")]
    pub lease_max_ms: Option<u64>,

    /// MCP 会话无请求多久后收回其默认（无历史）租约（毫秒），默认 30000；0 不收回。
    #[arg(long, value_name = "MS")]
    pub lease_idle_revoke_ms: Option<u64>,

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

    /// 每个（App, 工具）每分钟最多调用次数（令牌桶，spec/hub-api.md 3.11），默认 120；0 不限。超出返回 RATE_LIMITED。
    #[arg(long, value_name = "N")]
    pub tool_rate_limit: Option<u32>,

    /// 每个（App, 工具）允许的突发调用数（令牌桶容量），默认 30。
    #[arg(long, value_name = "N")]
    pub tool_rate_burst: Option<u32>,

    /// 每个 App（所有工具合计）每分钟最多调用次数，默认 600；0 不限。
    #[arg(long, value_name = "N")]
    pub app_rate_limit: Option<u32>,

    /// 每个 App 允许的突发调用数，默认 60。
    #[arg(long, value_name = "N")]
    pub app_rate_burst: Option<u32>,

    /// 调用参数（JSON）的字节上限，默认 1048576；0 不限。超出返回 PAYLOAD_TOO_LARGE。
    #[arg(long, value_name = "BYTES")]
    pub max_arguments_bytes: Option<u64>,

    /// 调用结果的字节上限，默认 4194304；0 不限。
    #[arg(long, value_name = "BYTES")]
    pub max_result_bytes: Option<u64>,

    /// 资源内容的字节上限，默认 4194304；0 不限。
    #[arg(long, value_name = "BYTES")]
    pub max_resource_bytes: Option<u64>,

    /// App 结果与其声明的 outputSchema 不符时：log（默认，只记日志）/ reject（调用以 HANDLER_ERROR 结束）/ off（不校验）。
    #[arg(long, value_name = "off|log|reject", value_parser = parse_output_validation)]
    pub output_validation: Option<OutputValidation>,

    /// 按名寻址（spec/naming.md）：经系统名字服务发现 App（只读，不启动进程），调用时按名拨号、未运行由系统激活，
    /// 宽限后关闭通道。Linux 为 D-Bus 会话总线、Windows 为每 App 命名管道（App 都用 `app install` 登记）；
    /// 其他平台暂不支持（忽略并提示）。
    #[arg(long)]
    pub name_service: bool,

    /// 按名拨入的通道在最后一次调用后保持的时间（毫秒，spec/naming.md 7.2），默认 15000。
    #[arg(long, value_name = "MS")]
    pub channel_grace_ms: Option<u64>,

    /// 无会话 MCP 请求的工具暴露方式（spec/hub-api.md 3.7「无会话请求的列表与总览」）：all（默认，全部列出）/
    /// progressive / auto（列表只含 apps.* 与全局选定实例的 App，不随调用变化；其余按全名调用）。
    #[arg(long, value_name = "auto|progressive|all", value_parser = parse_exposure)]
    pub stateless_tool_exposure: Option<ToolExposure>,

    /// 无会话 MCP 请求的 Agent 任务在请求流空闲多久后回收（毫秒，spec/hub-api.md 3.6），默认 600000；0 不因空闲回收。
    #[arg(long, value_name = "MS")]
    pub task_idle_ttl_ms: Option<u64>,

    /// 无会话请求的主体级 apps.select 选择的空闲有效期（毫秒，spec/hub-api.md 3.6），默认 60000；0 不单独过期。
    #[arg(long, value_name = "MS")]
    pub principal_select_ttl_ms: Option<u64>,

    /// MCP 出口协商的协议版本范围（spec/hub-api.md 3.6「协议版本」）：auto（默认，可协商 2026-07-28）/
    /// legacy-only（回退开关，只声明到 2025-11-25，subscriptions/listen 不可用）。
    #[arg(long, value_name = "auto|legacy-only", value_parser = parse_protocol_mode)]
    pub mcp_protocol_mode: Option<McpProtocolMode>,

    /// 每个主体同时打开的 subscriptions/listen 流数上限（spec/hub-api.md 3.6「通知」），默认 16；0 不提供 listen。
    #[arg(long, value_name = "N")]
    pub max_listen_streams: Option<usize>,

    /// 每个主体同时存在的任务句柄数上限（spec/hub-api.md 3.6「任务句柄」），默认 32；0 不提供任务句柄（apps.task.* 不列出）。
    #[arg(long, value_name = "N")]
    pub max_task_handles: Option<usize>,

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
            listen: self.listen.clone(),
            ws_addr: self.ws_addr.clone(),
            ipc_endpoint: self.ipc_endpoint.clone(),
            manifests: self.manifests.clone(),
            manifest_dirs: self.manifest_dirs.clone(),
            allow_origins: self.allow_origins.clone(),
            upstreams,
            lease_ms: self.lease_ms,
            wake_timeout_ms: self.wake_timeout_ms,
            navigate_timeout_ms: self.navigate_timeout_ms,
            wake_token_ttl_ms: self.wake_token_ttl_ms,
            wake_rate_limit: self.wake_rate_limit,
            legacy_heartbeat: self.legacy_heartbeat.then_some(true),
            lease: LeaseOverrides {
                adaptive: self.fixed_lease.then_some(false),
                window: self.lease_window,
                margin_ms: self.lease_margin_ms,
                min_ms: self.lease_min_ms,
                max_ms: self.lease_max_ms,
                idle_revoke_ms: self.lease_idle_revoke_ms,
            },
            wake_from_launch: self.wake_from_launch.then_some(true),
            waker: self.waker.clone(),
            tool_exposure: self.tool_exposure,
            tool_exposure_threshold: self.tool_exposure_threshold,
            limits: LimitOverrides {
                tool_rate_per_minute: self.tool_rate_limit,
                tool_rate_burst: self.tool_rate_burst,
                app_rate_per_minute: self.app_rate_limit,
                app_rate_burst: self.app_rate_burst,
                max_arguments_bytes: self.max_arguments_bytes,
                max_result_bytes: self.max_result_bytes,
                max_resource_bytes: self.max_resource_bytes,
            },
            output_validation: self.output_validation,
            log_level: self.log_level.clone(),
            name_service: self.name_service.then_some(true),
            channel_grace_ms: self.channel_grace_ms,
            stateless_tool_exposure: self.stateless_tool_exposure,
            task_idle_ttl_ms: self.task_idle_ttl_ms,
            principal_select_ttl_ms: self.principal_select_ttl_ms,
            mcp_protocol_mode: self.mcp_protocol_mode,
            max_listen_streams: self.max_listen_streams,
            max_task_handles: self.max_task_handles,
            ..Default::default()
        })
    }
}

#[derive(Debug, Clone, Default, Args)]
pub struct ServeArgs {
    #[command(flatten)]
    pub hub: HubArgs,

    /// 已弃用（兼容期）：另开一个监听器提供 MCP（http://<ADDR>/mcp），如旧的 127.0.0.1:7718。
    /// MCP 已合并到 --listen 的 /mcp；只在仍有客户端使用旧端口时设置。
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

    /// 按需启动（监听套接字由 systemd / launchd 交来，spec/protocol.md 1.9）时，没有连接、调用、在线 App 与 MCP 会话
    /// 持续多久后退出（毫秒），默认 600000；0 = 不退出。普通 `serve` 不受影响。
    #[arg(long, value_name = "MS")]
    pub idle_exit_ms: Option<u64>,
}

impl ServeArgs {
    pub fn overrides(&self) -> anyhow::Result<Overrides> {
        let mut o = self.hub.overrides()?;
        o.http_addr = self.http.clone();
        o.http_allow_remote = self.http_allow_remote.then_some(true);
        o.auth = self.auth;
        o.log_file = self.no_log_file.then_some(false);
        o.idle_exit_ms = self.idle_exit_ms;
        Ok(o)
    }
}

/// `service install` 的参数。
#[derive(Debug, Clone, Default, Args)]
pub struct ServiceInstallArgs {
    #[command(flatten)]
    pub serve: ServeArgs,

    /// 按需启动（spec/protocol.md 1.9）：Linux systemd 套接字激活、macOS launchd Sockets 代为监听，首个连接时启动 Host，
    /// 空闲 `--idle-exit-ms`（默认 10 分钟）后退出。缺省为登录自启（常驻）。Windows 不支持。
    #[arg(long)]
    pub on_demand: bool,
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
fn parse_output_validation(s: &str) -> Result<OutputValidation, String> {
    serde_json::from_value(serde_json::Value::String(s.trim().to_owned()))
        .map_err(|_| format!("应为 off、log 或 reject，而不是「{s}」"))
}

fn parse_exposure(s: &str) -> Result<ToolExposure, String> {
    serde_json::from_value(serde_json::Value::String(s.trim().to_owned()))
        .map_err(|_| format!("应为 auto、progressive 或 all，而不是「{s}」"))
}

/// @input 配置文件的取值 `auto` / `legacyOnly`，或命令行习惯的 `legacy-only`。
fn parse_protocol_mode(s: &str) -> Result<McpProtocolMode, String> {
    let s = s.trim();
    let name = if s == "legacy-only" { "legacyOnly" } else { s };
    serde_json::from_value(serde_json::Value::String(name.to_owned()))
        .map_err(|_| format!("应为 auto 或 legacy-only，而不是「{s}」"))
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
        assert_eq!(o.stateless_tool_exposure, None);
    }

    #[test]
    fn parses_stateless_settings() {
        let cli = Cli::try_parse_from([
            "app-mcp-host",
            "serve",
            "--stateless-tool-exposure",
            "progressive",
            "--task-idle-ttl-ms",
            "0",
            "--principal-select-ttl-ms",
            "1500",
        ])
        .unwrap();
        let Some(Command::Serve(s)) = cli.command else {
            panic!()
        };
        let o = s.hub.overrides().unwrap();
        assert_eq!(o.stateless_tool_exposure, Some(ToolExposure::Progressive));
        assert_eq!((o.task_idle_ttl_ms, o.principal_select_ttl_ms), (Some(0), Some(1500)));
        assert!(Cli::try_parse_from(["app-mcp-host", "serve", "--stateless-tool-exposure", "some"]).is_err());
    }

    #[test]
    fn parses_mcp_settings() {
        assert_eq!(parse_protocol_mode("legacy-only"), Ok(McpProtocolMode::LegacyOnly));
        assert_eq!(parse_protocol_mode("legacyOnly"), Ok(McpProtocolMode::LegacyOnly));
        assert_eq!(parse_protocol_mode("auto"), Ok(McpProtocolMode::Auto));
        assert!(parse_protocol_mode("modern").is_err());
        let cli =
            Cli::try_parse_from(["app-mcp-host", "serve", "--mcp-protocol-mode", "legacy-only", "--max-listen-streams", "0"])
                .unwrap();
        let Some(Command::Serve(s)) = cli.command else {
            panic!()
        };
        let o = s.hub.overrides().unwrap();
        assert_eq!((o.mcp_protocol_mode, o.max_listen_streams), (Some(McpProtocolMode::LegacyOnly), Some(0)));
        let cli = Cli::try_parse_from(["app-mcp-host", "serve"]).unwrap();
        let Some(Command::Serve(s)) = cli.command else {
            panic!()
        };
        let o = s.hub.overrides().unwrap();
        assert_eq!((o.mcp_protocol_mode, o.max_listen_streams), (None, None));
        assert!(Cli::try_parse_from(["app-mcp-host", "serve", "--max-listen-streams", "-1"]).is_err());
        assert!(Cli::try_parse_from(["app-mcp-host", "serve", "--mcp-protocol-mode", "modern"]).is_err());
    }

    #[test]
    fn parses_max_task_handles() {
        let cli = Cli::try_parse_from(["app-mcp-host", "serve", "--max-task-handles", "0"]).unwrap();
        let Some(Command::Serve(s)) = cli.command else {
            panic!()
        };
        assert_eq!(s.hub.overrides().unwrap().max_task_handles, Some(0));
        let cli = Cli::try_parse_from(["app-mcp-host", "serve"]).unwrap();
        let Some(Command::Serve(s)) = cli.command else {
            panic!()
        };
        assert_eq!(s.hub.overrides().unwrap().max_task_handles, None);
        assert!(Cli::try_parse_from(["app-mcp-host", "serve", "--max-task-handles", "-1"]).is_err());
        assert!(Cli::try_parse_from(["app-mcp-host", "serve", "--max-task-handles", "many"]).is_err());
    }

    #[test]
    fn parses_limits() {
        assert_eq!(parse_output_validation("reject"), Ok(OutputValidation::Reject));
        assert!(parse_output_validation("strict").is_err());
        let cli = Cli::try_parse_from([
            "app-mcp-host", "serve", "--tool-rate-limit", "10", "--app-rate-burst", "3", "--max-arguments-bytes", "0",
            "--max-resource-bytes", "99", "--output-validation", "off",
        ])
        .unwrap();
        let Some(Command::Serve(s)) = cli.command else {
            panic!()
        };
        let o = s.hub.overrides().unwrap();
        assert_eq!(
            o.limits,
            LimitOverrides {
                tool_rate_per_minute: Some(10),
                app_rate_burst: Some(3),
                max_arguments_bytes: Some(0),
                max_resource_bytes: Some(99),
                ..Default::default()
            }
        );
        assert_eq!(o.output_validation, Some(OutputValidation::Off));
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

        let cli = Cli::try_parse_from(["app-mcp-host", "serve", "--listen", "127.0.0.1:0"]).unwrap();
        let Some(Command::Serve(s)) = cli.command else {
            panic!()
        };
        assert_eq!(s.overrides().unwrap().listen.as_deref(), Some("127.0.0.1:0"));
        // 旧名仍可解析（隐藏），由配置解析记录弃用提示
        let cli = Cli::try_parse_from(["app-mcp-host", "serve", "--ws-addr", "127.0.0.1:1"]).unwrap();
        let Some(Command::Serve(s)) = cli.command else {
            panic!()
        };
        assert_eq!(s.overrides().unwrap().ws_addr.as_deref(), Some("127.0.0.1:1"));

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
        let cli = Cli::try_parse_from(["app-mcp-host", "service", "install", "--on-demand", "--idle-exit-ms", "60000"]).unwrap();
        let Some(Command::Service { action: ServiceAction::Install(a) }) = cli.command else { panic!() };
        assert!(a.on_demand);
        assert_eq!(a.serve.overrides().unwrap().idle_exit_ms, Some(60_000));
        assert!(Cli::try_parse_from(["app-mcp-host", "serve", "--on-demand"]).is_err(), "--on-demand 只属于 service install");
        let cli =
            Cli::try_parse_from(["app-mcp-host", "service", "status", "--home", "/x"]).unwrap();
        let Some(Command::Service {
            action: ServiceAction::Status(h),
        }) = cli.command
        else {
            panic!()
        };
        assert_eq!(h.home, Some(PathBuf::from("/x")));

        let cli = Cli::try_parse_from(["app-mcp-host", "doctor", "--json", "--home", "/x"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Doctor { json: true, .. })));
        let cli = Cli::try_parse_from(["app-mcp-host", "status"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Status(_))));
        let cli = Cli::try_parse_from(["app-mcp-host", "policy", "deny", "shop", "--tool", "pay*", "--wake"]).unwrap();
        let Some(Command::Policy { action: PolicyCommand::Deny { rule, wake: true } }) = cli.command else { panic!() };
        assert_eq!((rule.app.as_str(), rule.tool.as_deref()), ("shop", Some("pay*")));
        let cli = Cli::try_parse_from(["app-mcp-host", "policy", "reload", "--home", "/tmp/x"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Policy { action: PolicyCommand::Reload(_) })));

        let cli = Cli::try_parse_from(["app-mcp-host", "agent", "add", "claude", "--rotate"]).unwrap();
        let Some(Command::Agent { action: AgentCommand::Add { name, rotate: true, .. } }) = cli.command else { panic!() };
        assert_eq!(name, "claude");
        let cli = Cli::try_parse_from(["app-mcp-host", "agent", "list", "--json"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Agent { action: AgentCommand::List { json: true, .. } })));
        assert!(Cli::try_parse_from(["app-mcp-host", "agent", "token"]).is_err(), "token 需要名字");

        let cli = Cli::try_parse_from(["app-mcp-host", "setup"]).unwrap();
        let Some(Command::Setup(a)) = cli.command else { panic!() };
        assert_eq!(a.agents, AgentSelection::All);
        assert!(!a.force && !a.dry_run && !a.json);
        let cli = Cli::try_parse_from([
            "app-mcp-host", "setup", "--agents", "claude-code,cursor", "--force", "--dry-run", "--json", "--home", "/x",
        ])
        .unwrap();
        let Some(Command::Setup(a)) = cli.command else { panic!() };
        assert!(matches!(a.agents, AgentSelection::List(ref l) if l.len() == 2));
        assert!(a.force && a.dry_run && a.json);
        assert_eq!(a.home.home, Some(PathBuf::from("/x")));
        assert!(Cli::try_parse_from(["app-mcp-host", "setup", "--agents", "nope"]).is_err());
        let cli = Cli::try_parse_from(["app-mcp-host", "uninstall", "--purge"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Uninstall(UninstallArgs { purge: true, .. }))));
    }
}
