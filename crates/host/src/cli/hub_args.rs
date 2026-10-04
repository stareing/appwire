//! stdio / serve 共用的 Hub 参数、各运行模式的参数，以及它们到配置覆盖项（[`Overrides`]）的转换。

use std::path::PathBuf;

use app_mcp_hub::upstream::parse_cli_spec;
use app_mcp_hub::{LeaseOverrides, LimitOverrides, McpProtocolMode, OutputValidation, ToolExposure, WakerConfig};
use clap::Args;

use super::HomeArg;
use crate::config::{AuthMode, Overrides, ResultCacheSection};

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

    /// 每个已登记 Agent（agent add，所有 App 合计）每分钟最多调用次数，默认 0（不限）；本机主体不受此限。
    #[arg(long, value_name = "N")]
    pub agent_rate_limit: Option<u32>,

    /// 每个已登记 Agent 允许的突发调用数（设置了 --agent-rate-limit 时必须 ≥ 1）。
    #[arg(long, value_name = "N")]
    pub agent_rate_burst: Option<u32>,

    /// 调用参数（JSON）的字节上限，默认 1048576；0 不限。超出返回 PAYLOAD_TOO_LARGE。
    #[arg(long, value_name = "BYTES")]
    pub max_arguments_bytes: Option<u64>,

    /// 调用结果的字节上限，默认 4194304；0 不限。
    #[arg(long, value_name = "BYTES")]
    pub max_result_bytes: Option<u64>,

    /// 资源内容的字节上限，默认 4194304；0 不限。
    #[arg(long, value_name = "BYTES")]
    pub max_resource_bytes: Option<u64>,

    /// 只读结果缓存的条目数上限（spec/hub-api.md 3.20），默认 1024；0 关闭缓存。
    #[arg(long, value_name = "N")]
    pub cache_max_entries: Option<usize>,

    /// 只读结果缓存的总字节上限（键 + 序列化结果），默认 8388608。
    #[arg(long, value_name = "BYTES")]
    pub cache_max_bytes: Option<usize>,

    /// 只读结果缓存的单条字节上限，默认 65536；超出的结果不缓存。
    #[arg(long, value_name = "BYTES")]
    pub cache_max_entry_bytes: Option<usize>,

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

    /// 每个任务同时持有的对象锁数上限（spec/hub-api.md 3.6「对象锁」），默认 16；0 不提供对象锁（apps.lock / apps.unlock 不列出）。
    #[arg(long, value_name = "N")]
    pub max_locks: Option<usize>,

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
                agent_rate_per_minute: self.agent_rate_limit,
                agent_rate_burst: self.agent_rate_burst,
                max_arguments_bytes: self.max_arguments_bytes,
                max_result_bytes: self.max_result_bytes,
                max_resource_bytes: self.max_resource_bytes,
            },
            result_cache: ResultCacheSection {
                max_entries: self.cache_max_entries,
                max_bytes: self.cache_max_bytes,
                max_entry_bytes: self.cache_max_entry_bytes,
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
            max_locks: self.max_locks,
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
pub(super) fn parse_output_validation(s: &str) -> Result<OutputValidation, String> {
    serde_json::from_value(serde_json::Value::String(s.trim().to_owned()))
        .map_err(|_| format!("应为 off、log 或 reject，而不是「{s}」"))
}

pub(super) fn parse_exposure(s: &str) -> Result<ToolExposure, String> {
    serde_json::from_value(serde_json::Value::String(s.trim().to_owned()))
        .map_err(|_| format!("应为 auto、progressive 或 all，而不是「{s}」"))
}

/// @input 配置文件的取值 `auto` / `legacyOnly`，或命令行习惯的 `legacy-only`。
pub(super) fn parse_protocol_mode(s: &str) -> Result<McpProtocolMode, String> {
    let s = s.trim();
    let name = if s == "legacy-only" { "legacyOnly" } else { s };
    serde_json::from_value(serde_json::Value::String(name.to_owned()))
        .map_err(|_| format!("应为 auto 或 legacy-only，而不是「{s}」"))
}

pub(super) fn parse_waker(s: &str) -> Result<WakerConfig, String> {
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
