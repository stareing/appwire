//! `Hub.start(configJson)` 的配置：JSON（camelCase）→ [`HubConfig`]。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use app_mcp_hub::{
    AgentCredential, AgentsConfig, ApprovalPolicy, HubConfig, LeaseOverrides, LimitOverrides, McpProtocolMode, OutputValidation,
    PolicyConfig, ToolExposure, UpstreamConfig, WakerConfig, load_manifests,
};
use serde::{Deserialize, Deserializer};

use crate::event_limits::EventLimitOverrides;
use crate::result_cache::CacheLimitOverrides;
use crate::undo_limits::UndoLimitOverrides;

/// `Hub.start(configJson)` 的配置（camelCase）。时长均为毫秒。
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub(crate) struct ConfigJson {
    /// HTTP 监听地址（`/app`、`/healthz`，`mcpHttp` 时另有 `/mcp`）。缺省 `127.0.0.1:7717`（被占用时依次尝试
    /// 7737、7757）；显式给出时只绑定该地址；显式 `null` = 不开；端口 0 = 随机。
    #[serde(deserialize_with = "present")]
    listen: Option<Option<String>>,
    /// 是否在 `listen` 上提供 MCP Streamable HTTP（`/mcp`），默认 `false`。
    mcp_http: Option<bool>,
    /// 单实例锁与登记文件目录（`<runDir>/hub.lock`、`endpoints.json`）；缺省不参与。
    run_dir: Option<PathBuf>,
    /// 持久状态目录：休眠记录写到 `<stateDir>/dormant/<appId>.json`（原子写、仅当前用户可读），启动时读回；
    /// 缺省不读写任何文件（spec/hub-api.md 3.5「持久化」）。
    state_dir: Option<PathBuf>,
    /// 本地 IPC 端点（`unix:…` / `pipe:…`，spec/protocol.md 1.2）；缺省为平台默认端点；显式 `null` = 不开。
    #[serde(deserialize_with = "present")]
    ipc_endpoint: Option<Option<String>>,
    manifest_files: Vec<PathBuf>,
    manifest_dir: Option<PathBuf>,
    allow_origins: Vec<String>,
    ping_interval_ms: Option<u64>,
    idle_timeout_ms: Option<u64>,
    hidden_idle_timeout_ms: Option<u64>,
    invoke_timeout_ms: Option<u64>,
    response_timeout_ms: Option<u64>,
    list_changed_debounce_ms: Option<u64>,
    /// 调用进度的最小转发间隔（spec/hub-api.md 3.12），缺省 250。
    progress_interval_ms: Option<u64>,
    pairing_timeout_ms: Option<u64>,
    lease_ttl_ms: Option<u64>,
    wake_timeout_ms: Option<u64>,
    /// 导航等待上限（App 回复 + 目标工具注册，spec/hub-api.md 3.14 / 3.15），缺省 5000。
    navigate_timeout_ms: Option<u64>,
    wake_token_ttl_ms: Option<u64>,
    dormant_ttl_ms: Option<u64>,
    dormant_replaced_by_new_instance: Option<bool>,
    wake_from_launch: Option<bool>,
    wake_rate_limit: Option<u32>,
    legacy_heartbeat: Option<bool>,
    /// 自适应租约（spec/hub-api.md 3.5）。
    lease: Option<LeaseOverrides>,
    /// 资源保护：调用频率与大小上限（spec/hub-api.md 3.11），缺省字段取默认值。
    limits: Option<LimitOverrides>,
    /// 结果与 `outputSchema` 不符时的处理：`"off"` / `"log"`（默认）/ `"reject"`。
    output_validation: Option<OutputValidation>,
    /// 策略挂点（spec/hub-api.md 3.13）：`{"rules": [...]}`；规则不合法时 `Hub.start` 失败。
    policy: Option<PolicyConfig>,
    /// 标准意图的机主默认表（spec/intents.md 第 4 节）：`{动词: 工具全名}`；不合法时以空表启动、原因记入 `intents().lastError`。
    intent_defaults: Option<BTreeMap<String, String>>,
    /// `"system"` / `"none"` / `{"exec": [...]}`（spec/hub-api.md 3.5）。
    waker: Option<WakerConfig>,
    /// 渐进暴露（spec/hub-api.md 3.7）。
    tool_exposure: Option<ToolExposure>,
    tool_exposure_threshold: Option<usize>,
    /// 无会话 MCP 请求的 Agent 任务空闲回收时长（spec/hub-api.md 3.6），缺省 600000；0 不因空闲回收。
    task_idle_ttl_ms: Option<u64>,
    /// 无会话 MCP 请求的工具暴露方式（spec/hub-api.md 3.7「无会话请求的列表与总览」），缺省 `"all"`。
    stateless_tool_exposure: Option<ToolExposure>,
    /// 主体级 `apps.select` 的空闲有效期（spec/hub-api.md 3.6），缺省 60000；0 不单独过期。
    principal_select_ttl_ms: Option<u64>,
    /// 无会话请求列表结果的 `ttlMs`（spec/hub-api.md 3.7），缺省 5000。
    stateless_list_ttl_ms: Option<u64>,
    /// MCP 出口协商的协议版本范围（spec/hub-api.md 3.6「协议版本」），缺省 `"auto"`。
    mcp_protocol_mode: Option<McpProtocolMode>,
    /// 每个主体同时打开的 `subscriptions/listen` 流数上限（spec/hub-api.md 3.6「通知」），缺省 16；0 不提供 listen。
    max_listen_streams: Option<usize>,
    /// 一个 listen 流接受的资源 URI 数上限，缺省 256。
    max_listen_resources: Option<usize>,
    /// 每个主体同时存在的任务句柄数上限（spec/hub-api.md 3.6「任务句柄」），缺省 32；0 不提供任务句柄。
    max_task_handles: Option<usize>,
    /// 每个持有者同时持有的对象锁数上限（spec/hub-api.md 3.6「对象锁」），缺省 16；0 不提供对象锁。
    max_locks: Option<usize>,
    /// Agent 登记（spec/hub-api.md 3.6「Agent 身份」）：`[{"name","token"}]`；不合法时 `Hub.start` 失败。
    agents: Option<Vec<AgentCredential>>,
    /// 事件信箱上限（spec/hub-api.md 3.17）：`{maxSubscriptions?, maxInboxEvents?, inboxTtlMs?, perSubscriptionPerMinute?}`。
    event_limits: Option<EventLimitOverrides>,
    /// 只读结果缓存上限（spec/hub-api.md 3.20）：`{maxEntries?, maxBytes?, maxEntryBytes?}`；`maxEntries: 0` 关闭。
    result_cache: Option<CacheLimitOverrides>,
    /// 撤销上限（spec/hub-api.md 3.23）：`{ttlMs?, maxPerTask?}`；`maxPerTask: 0` 关闭撤销。
    undo: Option<UndoLimitOverrides>,
    upstreams: BTreeMap<String, UpstreamConfig>,
    approval: ApprovalPolicy,
}

/// 区分“字段缺省”（外层 `None`）与“显式 null”（`Some(None)`）。
fn present<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

impl ConfigJson {
    pub(crate) fn into_config(self) -> HubConfig {
        let mut c = HubConfig::default();
        if let Some(addr) = self.listen {
            // 显式给出（或 null）：只绑定该地址，不尝试备选端口。
            c.listen = addr;
            c.listen_alternates = Vec::new();
        }
        if let Some(v) = self.mcp_http {
            c.mcp_http = v;
        }
        c.run_dir = self.run_dir;
        c.state_dir = self.state_dir;
        if let Some(endpoint) = self.ipc_endpoint {
            c.ipc_endpoint = endpoint;
        }
        if !self.manifest_files.is_empty() || self.manifest_dir.is_some() {
            c.manifests = load_manifests(&self.manifest_files, self.manifest_dir.as_deref(), false);
        }
        c.allow_origins = self.allow_origins;
        let ms = Duration::from_millis;
        if let Some(v) = self.ping_interval_ms {
            c.ping_interval = ms(v);
        }
        if let Some(v) = self.idle_timeout_ms {
            c.idle_timeout = ms(v);
        }
        if let Some(v) = self.hidden_idle_timeout_ms {
            c.hidden_idle_timeout = ms(v);
        }
        if let Some(v) = self.invoke_timeout_ms {
            c.invoke_timeout = ms(v);
        }
        if let Some(v) = self.response_timeout_ms {
            c.response_timeout = ms(v);
        }
        if let Some(v) = self.list_changed_debounce_ms {
            c.list_changed_debounce = ms(v);
        }
        if let Some(v) = self.progress_interval_ms {
            c.progress_interval = ms(v);
        }
        if let Some(v) = self.pairing_timeout_ms {
            c.pairing_timeout = ms(v);
        }
        if let Some(v) = self.lease_ttl_ms {
            c.lease_ttl = ms(v);
        }
        if let Some(v) = self.wake_timeout_ms {
            c.wake_timeout = ms(v);
        }
        if let Some(v) = self.navigate_timeout_ms {
            c.navigate_timeout = ms(v);
        }
        if let Some(v) = self.wake_token_ttl_ms {
            c.wake_token_ttl = ms(v);
        }
        if let Some(v) = self.dormant_ttl_ms {
            c.dormant_ttl = ms(v);
        }
        if let Some(v) = self.dormant_replaced_by_new_instance {
            c.dormant_replaced_by_new_instance = v;
        }
        if let Some(v) = self.wake_from_launch {
            c.wake_from_launch = v;
        }
        if let Some(v) = self.wake_rate_limit {
            c.wake_rate_limit = v;
        }
        if let Some(v) = self.legacy_heartbeat {
            c.legacy_heartbeat = v;
        }
        if let Some(o) = &self.lease {
            o.apply(&mut c.lease);
        }
        if let Some(o) = &self.limits {
            o.apply(&mut c.limits);
        }
        if let Some(v) = self.output_validation {
            c.output_validation = v;
        }
        if let Some(p) = self.policy {
            c.policy = p;
        }
        if let Some(d) = self.intent_defaults {
            c.intent_defaults = d;
        }
        if let Some(w) = self.waker {
            c.waker = w;
        }
        if let Some(v) = self.tool_exposure {
            c.tool_exposure = v;
        }
        if let Some(v) = self.tool_exposure_threshold {
            c.tool_exposure_threshold = v;
        }
        if let Some(v) = self.task_idle_ttl_ms {
            c.task_idle_ttl = ms(v);
        }
        if let Some(v) = self.stateless_tool_exposure {
            c.stateless_tool_exposure = v;
        }
        if let Some(v) = self.principal_select_ttl_ms {
            c.principal_select_ttl = ms(v);
        }
        if let Some(v) = self.stateless_list_ttl_ms {
            c.stateless_list_ttl = ms(v);
        }
        if let Some(v) = self.mcp_protocol_mode {
            c.mcp_protocol_mode = v;
        }
        if let Some(v) = self.max_listen_streams {
            c.max_listen_streams = v;
        }
        if let Some(v) = self.max_listen_resources {
            c.max_listen_resources = v;
        }
        if let Some(v) = self.max_task_handles {
            c.max_task_handles = v;
        }
        if let Some(v) = self.max_locks {
            c.max_locks = v;
        }
        if let Some(agents) = self.agents {
            c.agents = AgentsConfig { agents };
        }
        if let Some(o) = &self.event_limits {
            o.apply(&mut c.event_limits);
        }
        if let Some(o) = &self.result_cache {
            o.apply(&mut c.result_cache);
        }
        if let Some(o) = &self.undo {
            o.apply(&mut c.undo);
        }
        c.upstreams = self.upstreams;
        c.approval = self.approval;
        c
    }
}
