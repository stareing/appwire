//! 合并后的最终设置：命令行参数 > 配置文件 > 默认值。

use std::collections::BTreeMap;
use std::path::PathBuf;

use app_mcp_hub::{
    CacheLimits, LeasePolicy, LimitPolicy, McpProtocolMode, OutputValidation, ToolExposure, UpstreamConfig,
    WakerConfig,
};

use super::{AppHome, AuthMode, DEFAULT_LISTEN_ADDR, FileConfig, IPC_NONE, Overrides, absolute};

/// 合并后的最终设置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// HTTP 监听地址（`/app`、`/mcp`、`/healthz`）。
    pub listen: String,
    /// `listen` 是否为显式配置：显式时只绑定该地址；缺省时默认端口被占用会依次尝试备选端口 7737、7757。
    pub listen_explicit: bool,
    /// 兼容期的额外 MCP 监听地址（已弃用的 `http.addr` / `--http`）；`None` = 不开（默认）。
    pub compat_http_addr: Option<String>,
    /// 解析配置时产生的提示（弃用的配置项等），日志初始化后记录。
    pub notices: Vec<String>,
    /// 本地 IPC 端点；`None` = 关闭（配置为 `"none"`，或本平台没有默认端点）。
    pub ipc_endpoint: Option<String>,
    pub http_allow_remote: bool,
    pub auth: AuthMode,
    pub manifests: Vec<PathBuf>,
    /// `(目录, 是否必须存在)`。显式配置的目录必须存在；默认目录不存在时忽略。
    pub manifest_dirs: Vec<(PathBuf, bool)>,
    pub allow_origins: Vec<String>,
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    pub lease_ms: u64,
    pub wake_timeout_ms: u64,
    pub navigate_timeout_ms: u64,
    pub wake_from_launch: bool,
    pub wake_token_ttl_ms: u64,
    pub wake_rate_limit: u32,
    pub legacy_heartbeat: bool,
    pub lease: LeasePolicy,
    pub waker: WakerConfig,
    /// 按名寻址（Linux：D-Bus 会话总线连接器）。
    pub name_service: bool,
    pub channel_grace_ms: u64,
    /// 按需启动时的空闲退出时间（毫秒）；0 = 不退出。
    pub idle_exit_ms: u64,
    /// 无会话 Agent 任务的空闲回收时长（毫秒）；0 = 不回收。
    pub task_idle_ttl_ms: u64,
    /// 主体级 `apps.select` 的空闲有效期（毫秒）；0 = 不单独过期。
    pub principal_select_ttl_ms: u64,
    pub tool_exposure: ToolExposure,
    pub tool_exposure_threshold: usize,
    pub limits: LimitPolicy,
    /// 只读结果缓存的上限；`max_entries == 0` = 关闭。
    pub result_cache: CacheLimits,
    pub output_validation: OutputValidation,
    pub progress_interval_ms: u64,
    pub stateless_tool_exposure: ToolExposure,
    pub stateless_list_ttl_ms: u64,
    pub mcp_protocol_mode: McpProtocolMode,
    /// 每个主体的 `subscriptions/listen` 流数上限；0 = 不提供 listen。
    pub max_listen_streams: usize,
    pub max_listen_resources: usize,
    /// 每个主体的任务句柄数上限；0 = 不提供任务句柄。
    pub max_task_handles: usize,
    /// 每个任务的对象锁数上限；0 = 不提供对象锁。
    pub max_locks: usize,
    pub log_level: String,
    pub log_file: bool,
    pub log_max_bytes: u64,
    pub log_keep: usize,
}

impl Settings {
    /// 配置文件 + 命令行覆盖 → 最终设置。
    pub fn resolve(file: &FileConfig, o: &Overrides, home: &AppHome) -> anyhow::Result<Self> {
        let mut c = file.clone();
        c.apply(o)?;
        let manifest_dirs = match c.manifest_dirs {
            Some(dirs) => dirs
                .iter()
                .map(|d| absolute(d).map(|d| (d, true)))
                .collect::<anyhow::Result<_>>()?,
            None => vec![(home.manifest_dir(), false)],
        };
        let manifests = c
            .manifests
            .iter()
            .map(|m| absolute(m))
            .collect::<anyhow::Result<_>>()?;
        let ipc_endpoint = match c.ipc_endpoint.as_deref() {
            Some(IPC_NONE) => None,
            Some(text) => {
                let e = app_mcp_protocol::Endpoint::parse(text).map_err(anyhow::Error::msg)?;
                anyhow::ensure!(
                    e.is_ipc(),
                    "ipcEndpoint 必须是 unix:<绝对路径> 或 pipe:\\\\.\\pipe\\<名称>（或 none）：{text}"
                );
                Some(e.to_string())
            }
            None => app_mcp_protocol::endpoint::default_ipc_endpoint().map(|e| e.to_string()),
        };
        let mut notices = Vec::new();
        let listen = match (c.listen, c.ws_addr) {
            (Some(l), Some(w)) if l != w => anyhow::bail!(
                "配置同时设置了 listen（{l}）与已弃用的 wsAddr / --ws-addr（{w}）：请只保留 listen"
            ),
            (Some(l), _) => Some(l),
            (None, Some(w)) => {
                notices.push(format!(
                    "wsAddr / --ws-addr 已弃用，按 listen = {w} 使用：App 连接、MCP 与 /healthz 已合并到同一端口（listen）"
                ));
                Some(w)
            }
            (None, None) => None,
        };
        let mut lease = LeasePolicy::default();
        if let Some(o) = &c.lifecycle.lease {
            o.apply(&mut lease);
        }
        lease.validate().map_err(|e| anyhow::anyhow!("lifecycle.lease 无效：{e}"))?;
        let mut limits = LimitPolicy::default();
        c.limits.apply(&mut limits);
        limits.validate().map_err(|e| anyhow::anyhow!("配置无效：{e}"))?;
        let result_cache = c.result_cache.resolve().map_err(|e| anyhow::anyhow!("配置无效：{e}"))?;
        let listen_explicit = listen.is_some();
        let listen = listen.unwrap_or_else(|| DEFAULT_LISTEN_ADDR.to_owned());
        let compat_http_addr = c.http.addr.filter(|a| *a != listen);
        if let Some(a) = &compat_http_addr {
            notices.push(format!(
                "http.addr / --http 已弃用：MCP 已合并到 http://{listen}/mcp；兼容期内另在 {a} 提供同样的服务，请改用 listen 并更新 MCP 客户端配置"
            ));
        }
        Ok(Self {
            listen,
            listen_explicit,
            compat_http_addr,
            notices,
            ipc_endpoint,
            http_allow_remote: c.http.allow_remote.unwrap_or(false),
            auth: c.http.auth.unwrap_or_default(),
            manifests,
            manifest_dirs,
            allow_origins: c.allow_origins,
            upstreams: c.upstreams,
            lease_ms: c.lifecycle.lease_ms.unwrap_or(60_000),
            wake_timeout_ms: c.lifecycle.wake_timeout_ms.unwrap_or(15_000),
            navigate_timeout_ms: c
                .lifecycle
                .navigate_timeout_ms
                .unwrap_or(app_mcp_hub::DEFAULT_NAVIGATE_TIMEOUT.as_millis() as u64),
            wake_from_launch: c.lifecycle.wake_from_launch.unwrap_or(false),
            wake_token_ttl_ms: c.lifecycle.wake_token_ttl_ms.unwrap_or(60_000),
            wake_rate_limit: c.lifecycle.wake_rate_limit.unwrap_or(app_mcp_hub::DEFAULT_WAKE_RATE_LIMIT),
            legacy_heartbeat: c.lifecycle.legacy_heartbeat.unwrap_or(false),
            lease,
            waker: c.lifecycle.waker.unwrap_or_default(),
            name_service: c.lifecycle.name_service.unwrap_or(false),
            channel_grace_ms: c
                .lifecycle
                .channel_grace_ms
                .unwrap_or(app_mcp_hub::DEFAULT_CHANNEL_GRACE.as_millis() as u64),
            idle_exit_ms: c.lifecycle.idle_exit_ms.unwrap_or(crate::activation::DEFAULT_IDLE_EXIT_MS),
            task_idle_ttl_ms: c
                .lifecycle
                .task_idle_ttl_ms
                .unwrap_or(app_mcp_hub::DEFAULT_TASK_IDLE_TTL.as_millis() as u64),
            principal_select_ttl_ms: c
                .lifecycle
                .principal_select_ttl_ms
                .unwrap_or(app_mcp_hub::DEFAULT_PRINCIPAL_SELECT_TTL.as_millis() as u64),
            tool_exposure: c.tools.exposure.unwrap_or_default(),
            tool_exposure_threshold: c
                .tools
                .threshold
                .unwrap_or(app_mcp_hub::DEFAULT_TOOL_EXPOSURE_THRESHOLD),
            limits,
            result_cache,
            output_validation: c.tools.output_validation.unwrap_or_default(),
            progress_interval_ms: c
                .tools
                .progress_interval_ms
                .unwrap_or(app_mcp_hub::DEFAULT_PROGRESS_INTERVAL.as_millis() as u64),
            stateless_tool_exposure: c
                .tools
                .stateless_exposure
                .unwrap_or_else(|| app_mcp_hub::HubConfig::default().stateless_tool_exposure),
            stateless_list_ttl_ms: c
                .tools
                .stateless_list_ttl_ms
                .unwrap_or(app_mcp_hub::DEFAULT_STATELESS_LIST_TTL.as_millis() as u64),
            mcp_protocol_mode: c.mcp.protocol_mode.unwrap_or_default(),
            max_listen_streams: c.mcp.max_listen_streams.unwrap_or(app_mcp_hub::DEFAULT_MAX_LISTEN_STREAMS),
            max_listen_resources: c.mcp.max_listen_resources.unwrap_or(app_mcp_hub::DEFAULT_MAX_LISTEN_RESOURCES),
            max_task_handles: c.mcp.max_task_handles.unwrap_or(app_mcp_hub::DEFAULT_MAX_TASK_HANDLES),
            max_locks: c.mcp.max_locks.unwrap_or(app_mcp_hub::DEFAULT_MAX_LOCKS),
            log_level: c.log.level.unwrap_or_else(|| "info".to_owned()),
            log_file: c.log.file.unwrap_or(true),
            log_max_bytes: c.log.max_bytes.unwrap_or(5 * 1024 * 1024),
            log_keep: c.log.keep.unwrap_or(3),
        })
    }
}
