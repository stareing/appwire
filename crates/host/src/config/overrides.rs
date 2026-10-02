//! 命令行覆盖项，以及把覆盖项写入配置文件结构（`service install` 持久化、`serve` 合并）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use app_mcp_hub::{
    LeaseOverrides, LimitOverrides, McpProtocolMode, OutputValidation, ToolExposure, UpstreamConfig,
    WakerConfig,
};

use super::{AuthMode, FileConfig, absolute};

/// 命令行给出的覆盖项（`None` / 空 = 未指定，沿用配置文件）。
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub listen: Option<String>,
    /// 已弃用的 `--ws-addr`（同 `wsAddr`）。
    pub ws_addr: Option<String>,
    pub ipc_endpoint: Option<String>,
    pub http_addr: Option<String>,
    pub http_allow_remote: Option<bool>,
    pub auth: Option<AuthMode>,
    pub manifests: Vec<PathBuf>,
    pub manifest_dirs: Vec<PathBuf>,
    pub allow_origins: Vec<String>,
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    pub lease_ms: Option<u64>,
    pub wake_timeout_ms: Option<u64>,
    pub navigate_timeout_ms: Option<u64>,
    pub wake_from_launch: Option<bool>,
    pub wake_token_ttl_ms: Option<u64>,
    pub wake_rate_limit: Option<u32>,
    pub legacy_heartbeat: Option<bool>,
    /// 自适应租约的命令行覆盖项（字段级合并到配置文件的 `lifecycle.lease`）。
    pub lease: LeaseOverrides,
    pub waker: Option<WakerConfig>,
    pub tool_exposure: Option<ToolExposure>,
    pub tool_exposure_threshold: Option<usize>,
    /// 资源保护的命令行覆盖项（字段级合并到配置文件的 `limits`）。
    pub limits: LimitOverrides,
    pub output_validation: Option<OutputValidation>,
    pub log_level: Option<String>,
    pub log_file: Option<bool>,
    pub name_service: Option<bool>,
    pub channel_grace_ms: Option<u64>,
    pub idle_exit_ms: Option<u64>,
    pub task_idle_ttl_ms: Option<u64>,
    pub principal_select_ttl_ms: Option<u64>,
    pub stateless_tool_exposure: Option<ToolExposure>,
    pub mcp_protocol_mode: Option<McpProtocolMode>,
    pub max_listen_streams: Option<usize>,
    pub max_task_handles: Option<usize>,
}

impl FileConfig {
    /// 把命令行覆盖项写入配置（`service install` 用于持久化；`serve` 用于合并）。
    /// 标量覆盖；列表 / 映射追加（同名上游被覆盖）。路径转为绝对路径。
    pub fn apply(&mut self, o: &Overrides) -> anyhow::Result<()> {
        fn set<T: Clone>(dst: &mut Option<T>, src: &Option<T>) {
            if let Some(v) = src {
                *dst = Some(v.clone());
            }
        }
        if o.listen.is_some() {
            // 新名称取代旧名称：写回配置时迁移。
            self.ws_addr = None;
        }
        set(&mut self.listen, &o.listen);
        set(&mut self.ws_addr, &o.ws_addr);
        set(&mut self.ipc_endpoint, &o.ipc_endpoint);
        set(&mut self.http.addr, &o.http_addr);
        set(&mut self.http.allow_remote, &o.http_allow_remote);
        set(&mut self.http.auth, &o.auth);
        set(&mut self.lifecycle.lease_ms, &o.lease_ms);
        set(&mut self.lifecycle.wake_timeout_ms, &o.wake_timeout_ms);
        set(&mut self.lifecycle.navigate_timeout_ms, &o.navigate_timeout_ms);
        set(&mut self.lifecycle.wake_from_launch, &o.wake_from_launch);
        set(&mut self.lifecycle.wake_token_ttl_ms, &o.wake_token_ttl_ms);
        set(&mut self.lifecycle.wake_rate_limit, &o.wake_rate_limit);
        set(&mut self.lifecycle.legacy_heartbeat, &o.legacy_heartbeat);
        if o.lease != LeaseOverrides::default() {
            self.lifecycle.lease.get_or_insert_with(LeaseOverrides::default).merge(&o.lease);
        }
        set(&mut self.lifecycle.waker, &o.waker);
        set(&mut self.lifecycle.name_service, &o.name_service);
        set(&mut self.lifecycle.channel_grace_ms, &o.channel_grace_ms);
        set(&mut self.lifecycle.idle_exit_ms, &o.idle_exit_ms);
        set(&mut self.lifecycle.task_idle_ttl_ms, &o.task_idle_ttl_ms);
        set(&mut self.lifecycle.principal_select_ttl_ms, &o.principal_select_ttl_ms);
        set(&mut self.tools.stateless_exposure, &o.stateless_tool_exposure);
        set(&mut self.mcp.protocol_mode, &o.mcp_protocol_mode);
        set(&mut self.mcp.max_listen_streams, &o.max_listen_streams);
        set(&mut self.mcp.max_task_handles, &o.max_task_handles);
        set(&mut self.tools.exposure, &o.tool_exposure);
        set(&mut self.tools.threshold, &o.tool_exposure_threshold);
        set(&mut self.tools.output_validation, &o.output_validation);
        self.limits.merge(&o.limits);
        set(&mut self.log.level, &o.log_level);
        set(&mut self.log.file, &o.log_file);
        for m in &o.manifests {
            let m = absolute(m)?;
            if !self.manifests.contains(&m) {
                self.manifests.push(m);
            }
        }
        if !o.manifest_dirs.is_empty() {
            let dirs = o
                .manifest_dirs
                .iter()
                .map(|d| absolute(d))
                .collect::<anyhow::Result<Vec<_>>>()?;
            self.manifest_dirs = Some(dirs);
        }
        for origin in &o.allow_origins {
            if !self.allow_origins.contains(origin) {
                self.allow_origins.push(origin.clone());
            }
        }
        for (name, cfg) in &o.upstreams {
            self.upstreams.insert(name.clone(), cfg.clone());
        }
        Ok(())
    }
}
