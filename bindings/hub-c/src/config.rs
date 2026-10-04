//! `am_hub_start` 的 JSON 配置 → [`HubConfig`]。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use hub::{
    AgentCredential, AgentsConfig, ApprovalPolicy, EventLimits, HubConfig, LeaseOverrides, LimitOverrides, McpProtocolMode, OutputValidation, PolicyConfig, ToolExposure,
    UpstreamConfig, WakerConfig, load_manifests,
};
use serde::Deserialize;
use serde_json::Value;

use crate::ffi_util::{AmHubStatus, FfiError, FfiResult};

/// 默认 tokio 工作线程数。
const DEFAULT_WORKER_THREADS: usize = 2;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub(crate) struct ConfigJson {
    /// HTTP 监听地址（`/app`、`/healthz`，`mcpHttp` 时另有 `/mcp`）。缺省（字段不出现）为默认地址
    /// `127.0.0.1:7717`，被占用时依次尝试 7737、7757；显式给出地址时只绑定该地址；显式 `null` 表示不开。
    #[serde(deserialize_with = "explicit")]
    pub listen: Option<Option<String>>,
    /// 是否在 `listen` 上提供 MCP Streamable HTTP（`/mcp`），默认 `false`。
    pub mcp_http: Option<bool>,
    /// 单实例锁与登记文件目录（`<runDir>/hub.lock`、`endpoints.json`）；缺省不参与。
    pub run_dir: Option<PathBuf>,
    /// v12：持久状态目录（休眠记录写到 `<stateDir>/dormant/<appId>.json`，启动时读回）；缺省不读写任何文件。
    pub state_dir: Option<PathBuf>,
    /// 本地 IPC 端点（`unix:…` / `pipe:…`）；缺省为平台默认端点；显式 `null` 表示不开。
    pub ipc_endpoint: Option<String>,
    pub manifests: Vec<Value>,
    pub manifest_files: Vec<PathBuf>,
    pub manifest_dir: Option<PathBuf>,
    pub allow_origins: Vec<String>,
    pub ping_interval_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
    pub hidden_idle_timeout_ms: Option<u64>,
    pub invoke_timeout_ms: Option<u64>,
    pub response_timeout_ms: Option<u64>,
    pub list_changed_debounce_ms: Option<u64>,
    pub pairing_timeout_ms: Option<u64>,
    /// v11：进度转发的最小间隔（spec/hub-api.md 3.12），缺省 250。
    pub progress_interval_ms: Option<u64>,
    pub lease_ttl_ms: Option<u64>,
    pub wake_timeout_ms: Option<u64>,
    /// v13：自动 / 显式导航的回复与工具注册等待上限（spec/hub-api.md 3.14 / 3.15），缺省 5000。
    pub navigate_timeout_ms: Option<u64>,
    pub wake_token_ttl_ms: Option<u64>,
    pub dormant_ttl_ms: Option<u64>,
    pub dormant_replaced_by_new_instance: Option<bool>,
    pub wake_from_launch: Option<bool>,
    /// 每 App 每分钟最多唤醒次数（spec/lifecycle.md 第 12 节），缺省 6；0 不限。
    pub wake_rate_limit: Option<u32>,
    /// 回退到旧心跳（spec/lifecycle.md 第 11 节），缺省 `false`。
    pub legacy_heartbeat: Option<bool>,
    /// 自适应租约（spec/hub-api.md 3.5）：`{"adaptive","window","marginMs","minMs","maxMs","idleRevokeMs"}`。
    pub lease: Option<LeaseOverrides>,
    /// 资源保护（spec/hub-api.md 3.11）：`{"toolRatePerMinute","toolRateBurst","appRatePerMinute","appRateBurst",
    /// "maxArgumentsBytes","maxResultBytes","maxResourceBytes"}`。
    pub limits: Option<LimitOverrides>,
    /// 结果与 `outputSchema` 不符时的处理：`"off"` / `"log"`（默认）/ `"reject"`。
    pub output_validation: Option<OutputValidation>,
    /// 策略挂点（spec/hub-api.md 3.13）：`{"rules": [{"id","action","app","tool"?,"annotations"?,"hooks"?}]}`。
    pub policy: Option<PolicyConfig>,
    /// `"system"` / `"none"` / `{"exec": [...]}`（spec/hub-api.md 3.5）。
    pub waker: Option<WakerConfig>,
    /// 渐进暴露（spec/hub-api.md 3.7）。
    pub tool_exposure: Option<ToolExposure>,
    pub tool_exposure_threshold: Option<usize>,
    /// v15：无会话 MCP 请求的 Agent 任务空闲回收时长（spec/hub-api.md 3.6），缺省 600000；0 不因空闲回收。
    pub task_idle_ttl_ms: Option<u64>,
    /// v15：无会话 MCP 请求的工具暴露方式（spec/hub-api.md 3.7「无会话请求的列表与总览」），缺省 `"all"`。
    pub stateless_tool_exposure: Option<ToolExposure>,
    /// v15：主体级 `apps.select` 的空闲有效期（spec/hub-api.md 3.6），缺省 60000；0 不单独过期。
    pub principal_select_ttl_ms: Option<u64>,
    /// v15：无会话请求列表结果的 `ttlMs`（spec/hub-api.md 3.7），缺省 5000。
    pub stateless_list_ttl_ms: Option<u64>,
    /// v16：MCP 出口协商的协议版本范围（spec/hub-api.md 3.6「协议版本」），缺省 `"auto"`。
    pub mcp_protocol_mode: Option<McpProtocolMode>,
    /// v16：每个主体同时打开的 `subscriptions/listen` 流数上限（spec/hub-api.md 3.6「通知」），缺省 16；0 不提供 listen。
    pub max_listen_streams: Option<usize>,
    /// v16：一个 listen 流接受的资源 URI 数上限，缺省 256。
    pub max_listen_resources: Option<usize>,
    /// v17：每个主体同时存在的任务句柄数上限（spec/hub-api.md 3.6「任务句柄」），缺省 32；0 不提供任务句柄。
    pub max_task_handles: Option<usize>,
    /// v20：每个持有者同时持有的对象锁数上限（spec/hub-api.md 3.6「对象锁」），缺省 16；0 不提供对象锁。
    pub max_locks: Option<usize>,
    /// v19：Agent 登记（spec/hub-api.md 3.6「Agent 身份」）：`[{"name","token"}]`，缺省空（所有请求为本机主体）。
    pub agents: Option<Vec<AgentCredential>>,
    /// v22：事件信箱上限（spec/hub-api.md 3.17）：`{"maxSubscriptions","maxInboxEvents","inboxTtlMs","perSubscriptionPerMinute"}`。
    pub event_limits: Option<EventLimitOverrides>,
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    pub approval: ApprovalPolicy,
    pub worker_threads: Option<usize>,
}

impl Default for ConfigJson {
    fn default() -> Self {
        Self {
            listen: None,
            mcp_http: None,
            run_dir: None,
            state_dir: None,
            ipc_endpoint: HubConfig::default().ipc_endpoint,
            manifests: Vec::new(),
            manifest_files: Vec::new(),
            manifest_dir: None,
            allow_origins: Vec::new(),
            ping_interval_ms: None,
            idle_timeout_ms: None,
            hidden_idle_timeout_ms: None,
            invoke_timeout_ms: None,
            response_timeout_ms: None,
            list_changed_debounce_ms: None,
            pairing_timeout_ms: None,
            progress_interval_ms: None,
            lease_ttl_ms: None,
            wake_timeout_ms: None,
            navigate_timeout_ms: None,
            wake_token_ttl_ms: None,
            dormant_ttl_ms: None,
            dormant_replaced_by_new_instance: None,
            wake_from_launch: None,
            wake_rate_limit: None,
            legacy_heartbeat: None,
            lease: None,
            limits: None,
            output_validation: None,
            policy: None,
            waker: None,
            tool_exposure: None,
            tool_exposure_threshold: None,
            task_idle_ttl_ms: None,
            stateless_tool_exposure: None,
            principal_select_ttl_ms: None,
            stateless_list_ttl_ms: None,
            mcp_protocol_mode: None,
            max_listen_streams: None,
            max_listen_resources: None,
            max_task_handles: None,
            max_locks: None,
            agents: None,
            event_limits: None,
            upstreams: BTreeMap::new(),
            approval: ApprovalPolicy::default(),
            worker_threads: None,
        }
    }
}

/// 事件信箱上限（[`EventLimits`]）的可选覆盖；缺省字段沿用默认值（32 / 100 / 24 h / 60）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EventLimitOverrides {
    /// 每个订阅方最多的订阅数。
    pub max_subscriptions: Option<usize>,
    /// 每个信箱最多的事件数（Hub 至少按 1 处理）。
    pub max_inbox_events: Option<usize>,
    /// 信箱中事件的保留时长（毫秒）。
    pub inbox_ttl_ms: Option<u64>,
    /// 每个订阅每分钟最多入箱的事件数；`0` = 不限。
    pub per_subscription_per_minute: Option<u32>,
}

impl EventLimitOverrides {
    fn apply(&self, target: &mut EventLimits) {
        if let Some(v) = self.max_subscriptions {
            target.max_subscriptions = v;
        }
        if let Some(v) = self.max_inbox_events {
            target.max_inbox_events = v;
        }
        set_ms(&mut target.inbox_ttl, self.inbox_ttl_ms);
        if let Some(v) = self.per_subscription_per_minute {
            target.per_subscription_per_minute = v;
        }
    }
}

/// 字段出现时（含 `null`）包一层 `Some`，用于区分“未给出”与“显式 null”。
fn explicit<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

pub(crate) struct ParsedConfig {
    pub hub: HubConfig,
    pub worker_threads: usize,
}

/// 校验 Agent 登记（`am_hub_start` 的 `agents` 与 `am_hub_set_agents` 共用）。
///
/// @error 不合法 → `InvalidConfig`（信息不含令牌）。
pub(crate) fn parse_agents(agents: Vec<AgentCredential>) -> FfiResult<AgentsConfig> {
    let config = AgentsConfig { agents };
    config
        .validate()
        .map_err(|e| FfiError::new(AmHubStatus::InvalidConfig, format!("agents 无效：{e}")))?;
    Ok(config)
}

fn set_ms(target: &mut Duration, v: Option<u64>) {
    if let Some(ms) = v {
        *target = Duration::from_millis(ms);
    }
}

pub(crate) fn parse(text: Option<&str>) -> FfiResult<ParsedConfig> {
    let c: ConfigJson = match text.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => serde_json::from_str(t).map_err(|e| FfiError::json("config_json", e))?,
        None => ConfigJson::default(),
    };
    let defaults = HubConfig::default();
    let (listen, listen_alternates) = match c.listen {
        // 字段不出现：默认地址与备选地址。
        None => (defaults.listen.clone(), defaults.listen_alternates.clone()),
        // 显式给出（或 null）：只绑定该地址。
        Some(addr) => (addr, Vec::new()),
    };
    let mut hub = HubConfig {
        listen,
        listen_alternates,
        mcp_http: c.mcp_http.unwrap_or(defaults.mcp_http),
        run_dir: c.run_dir,
        state_dir: c.state_dir,
        ipc_endpoint: c.ipc_endpoint,
        allow_origins: c.allow_origins,
        upstreams: c.upstreams,
        approval: c.approval,
        ..defaults
    };
    set_ms(&mut hub.ping_interval, c.ping_interval_ms);
    set_ms(&mut hub.idle_timeout, c.idle_timeout_ms);
    set_ms(&mut hub.hidden_idle_timeout, c.hidden_idle_timeout_ms);
    set_ms(&mut hub.invoke_timeout, c.invoke_timeout_ms);
    set_ms(&mut hub.response_timeout, c.response_timeout_ms);
    set_ms(&mut hub.list_changed_debounce, c.list_changed_debounce_ms);
    set_ms(&mut hub.pairing_timeout, c.pairing_timeout_ms);
    set_ms(&mut hub.progress_interval, c.progress_interval_ms);
    // 生命周期（spec/hub-api.md 3.5）。
    set_ms(&mut hub.lease_ttl, c.lease_ttl_ms);
    set_ms(&mut hub.wake_timeout, c.wake_timeout_ms);
    set_ms(&mut hub.navigate_timeout, c.navigate_timeout_ms);
    set_ms(&mut hub.wake_token_ttl, c.wake_token_ttl_ms);
    set_ms(&mut hub.dormant_ttl, c.dormant_ttl_ms);
    if let Some(v) = c.dormant_replaced_by_new_instance {
        hub.dormant_replaced_by_new_instance = v;
    }
    if let Some(v) = c.wake_from_launch {
        hub.wake_from_launch = v;
    }
    if let Some(v) = c.wake_rate_limit {
        hub.wake_rate_limit = v;
    }
    if let Some(v) = c.legacy_heartbeat {
        hub.legacy_heartbeat = v;
    }
    if let Some(o) = &c.lease {
        o.apply(&mut hub.lease);
        hub.lease
            .validate()
            .map_err(|e| FfiError::new(AmHubStatus::InvalidConfig, format!("lease 无效：{e}")))?;
    }
    if let Some(o) = &c.limits {
        o.apply(&mut hub.limits);
        hub.limits
            .validate()
            .map_err(|e| FfiError::new(AmHubStatus::InvalidConfig, format!("limits 无效：{e}")))?;
    }
    if let Some(v) = c.output_validation {
        hub.output_validation = v;
    }
    if let Some(p) = c.policy {
        p.validate()
            .map_err(|e| FfiError::new(AmHubStatus::InvalidConfig, format!("policy 无效：{e}")))?;
        hub.policy = p;
    }
    if let Some(w) = c.waker {
        hub.waker = w;
    }
    if let Some(v) = c.tool_exposure {
        hub.tool_exposure = v;
    }
    if let Some(v) = c.tool_exposure_threshold {
        hub.tool_exposure_threshold = v;
    }
    // 无会话 MCP 请求（spec/hub-api.md 3.6 / 3.7）。
    set_ms(&mut hub.task_idle_ttl, c.task_idle_ttl_ms);
    set_ms(&mut hub.principal_select_ttl, c.principal_select_ttl_ms);
    set_ms(&mut hub.stateless_list_ttl, c.stateless_list_ttl_ms);
    if let Some(v) = c.stateless_tool_exposure {
        hub.stateless_tool_exposure = v;
    }
    // MCP 出口协议版本与 listen 上限（spec/hub-api.md 3.6）。
    if let Some(v) = c.mcp_protocol_mode {
        hub.mcp_protocol_mode = v;
    }
    if let Some(v) = c.max_listen_streams {
        hub.max_listen_streams = v;
    }
    if let Some(v) = c.max_listen_resources {
        hub.max_listen_resources = v;
    }
    if let Some(v) = c.max_task_handles {
        hub.max_task_handles = v;
    }
    if let Some(v) = c.max_locks {
        hub.max_locks = v;
    }
    if let Some(agents) = c.agents {
        hub.agents = parse_agents(agents)?;
    }
    if let Some(o) = &c.event_limits {
        o.apply(&mut hub.event_limits);
    }

    // 目录与文件：失败的清单由 Hub 记录日志后跳过（与 app-mcp-host 一致）。
    hub.manifests = load_manifests(&c.manifest_files, c.manifest_dir.as_deref(), false);
    // 内联清单：无效时直接报错，便于调用方发现问题。
    for (i, m) in c.manifests.iter().enumerate() {
        let loaded = app_mcp_manifest::load_str(&m.to_string()).map_err(|e| {
            FfiError::new(AmHubStatus::InvalidConfig, format!("manifests[{i}] 无效：{e}"))
        })?;
        hub.manifests.push(loaded.manifest);
    }

    let worker_threads = c.worker_threads.unwrap_or(DEFAULT_WORKER_THREADS).max(1);
    Ok(ParsedConfig { hub, worker_threads })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_null_listen() {
        let d = HubConfig::default();
        let p = parse(None).map_err(|e| e.message).expect("默认配置");
        assert_eq!(p.hub.listen, d.listen);
        assert_eq!(p.hub.listen_alternates, d.listen_alternates);
        assert!(!p.hub.mcp_http);
        assert_eq!(p.hub.run_dir, None);
        assert_eq!(p.hub.state_dir, None);
        assert_eq!(p.worker_threads, DEFAULT_WORKER_THREADS);
        // 显式地址：只绑定该地址
        let p = parse(Some(r#"{"listen": "127.0.0.1:0", "mcpHttp": true, "runDir": "/tmp/r", "stateDir": "/tmp/s"}"#)).unwrap();
        assert_eq!(p.hub.listen.as_deref(), Some("127.0.0.1:0"));
        assert!(p.hub.listen_alternates.is_empty());
        assert!(p.hub.mcp_http);
        assert_eq!(p.hub.run_dir, Some(PathBuf::from("/tmp/r")));
        assert_eq!(p.hub.state_dir, Some(PathBuf::from("/tmp/s")));
        let p = parse(Some(r#"{"listen": null, "responseTimeoutMs": 1500,
            "approval": {"requireAtOrAbove": "payment", "timeout": 200}}"#))
        .map_err(|e| e.message)
        .expect("解析");
        assert_eq!(p.hub.listen, None);
        // 旧名不再接受
        assert!(parse(Some(r#"{"wsAddr": "127.0.0.1:0"}"#)).is_err());
        assert_eq!(p.hub.response_timeout, Duration::from_millis(1500));
        assert_eq!(p.hub.approval.timeout, Some(Duration::from_millis(200)));
        assert_eq!(p.hub.ipc_endpoint, HubConfig::default().ipc_endpoint);
        let p = parse(Some(r#"{"ipcEndpoint": null}"#)).unwrap();
        assert_eq!(p.hub.ipc_endpoint, None);
        let p = parse(Some(r#"{"ipcEndpoint": "unix:/run/x/hub.sock"}"#)).unwrap();
        assert_eq!(p.hub.ipc_endpoint.as_deref(), Some("unix:/run/x/hub.sock"));
    }

    #[test]
    fn lifecycle_fields() {
        let d = HubConfig::default();
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert_eq!(p.hub.lease_ttl, d.lease_ttl);
        assert!(p.hub.dormant_replaced_by_new_instance);
        assert!(!p.hub.wake_from_launch);
        let p = parse(Some(
            r#"{"leaseTtlMs": 0, "wakeTimeoutMs": 2000, "wakeTokenTtlMs": 3000, "dormantTtlMs": 4000,
                "dormantReplacedByNewInstance": false, "wakeFromLaunch": true}"#,
        ))
        .map_err(|e| e.message)
        .expect("解析");
        assert_eq!(p.hub.lease_ttl, Duration::ZERO);
        assert_eq!(p.hub.wake_timeout, Duration::from_millis(2000));
        assert_eq!(p.hub.wake_token_ttl, Duration::from_millis(3000));
        assert_eq!(p.hub.dormant_ttl, Duration::from_millis(4000));
        assert!(!p.hub.dormant_replaced_by_new_instance);
        assert!(p.hub.wake_from_launch);
    }

    #[test]
    fn navigate_timeout_field() {
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert_eq!(p.hub.navigate_timeout, hub::DEFAULT_NAVIGATE_TIMEOUT);
        let p = parse(Some(r#"{"navigateTimeoutMs": 1234}"#)).map_err(|e| e.message).expect("解析");
        assert_eq!(p.hub.navigate_timeout, Duration::from_millis(1234));
        let e = parse(Some(r#"{"navigateTimeoutMs": "5s"}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidJson), "非整数报错");
    }

    #[test]
    fn stateless_fields() {
        let d = HubConfig::default();
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert_eq!(
            (p.hub.task_idle_ttl, p.hub.stateless_tool_exposure, p.hub.principal_select_ttl, p.hub.stateless_list_ttl),
            (d.task_idle_ttl, d.stateless_tool_exposure, d.principal_select_ttl, d.stateless_list_ttl)
        );
        let p = parse(Some(
            r#"{"taskIdleTtlMs": 0, "statelessToolExposure": "progressive", "principalSelectTtlMs": 1500,
                "statelessListTtlMs": 750}"#,
        ))
        .map_err(|e| e.message)
        .expect("解析");
        assert_eq!(p.hub.task_idle_ttl, Duration::ZERO);
        assert_eq!(p.hub.stateless_tool_exposure, ToolExposure::Progressive);
        assert_eq!(p.hub.principal_select_ttl, Duration::from_millis(1500));
        assert_eq!(p.hub.stateless_list_ttl, Duration::from_millis(750));
        let e = parse(Some(r#"{"statelessToolExposure": "some"}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidJson), "非法取值报错");
        let e = parse(Some(r#"{"taskIdleTtlMs": -1}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidJson), "负数报错");
    }

    #[test]
    fn mcp_listen_fields() {
        let d = HubConfig::default();
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert_eq!(
            (p.hub.mcp_protocol_mode, p.hub.max_listen_streams, p.hub.max_listen_resources),
            (d.mcp_protocol_mode, d.max_listen_streams, d.max_listen_resources)
        );
        let p = parse(Some(r#"{"mcpProtocolMode": "legacyOnly", "maxListenStreams": 0, "maxListenResources": 8}"#))
            .map_err(|e| e.message)
            .expect("解析");
        assert_eq!(
            (p.hub.mcp_protocol_mode, p.hub.max_listen_streams, p.hub.max_listen_resources),
            (McpProtocolMode::LegacyOnly, 0, 8)
        );
        for bad in [r#"{"mcpProtocolMode": "modern"}"#, r#"{"maxListenStreams": -1}"#, r#"{"maxListenResources": 1.5}"#] {
            let e = parse(Some(bad)).err().map(|e| e.status);
            assert_eq!(e, Some(AmHubStatus::InvalidJson), "{bad}");
        }
    }

    #[test]
    fn max_task_handles_field() {
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert_eq!(p.hub.max_task_handles, HubConfig::default().max_task_handles);
        let p = parse(Some(r#"{"maxTaskHandles": 0}"#)).map_err(|e| e.message).expect("解析");
        assert_eq!(p.hub.max_task_handles, 0);
        let p = parse(Some(r#"{"maxTaskHandles": 5}"#)).map_err(|e| e.message).expect("解析");
        assert_eq!(p.hub.max_task_handles, 5);
        for bad in [r#"{"maxTaskHandles": -1}"#, r#"{"maxTaskHandles": 1.5}"#, r#"{"maxTaskHandles": "8"}"#] {
            let e = parse(Some(bad)).err().map(|e| e.status);
            assert_eq!(e, Some(AmHubStatus::InvalidJson), "{bad}");
        }
    }

    #[test]
    fn max_locks_field() {
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert_eq!(p.hub.max_locks, hub::DEFAULT_MAX_LOCKS);
        let p = parse(Some(r#"{"maxLocks": 0}"#)).map_err(|e| e.message).expect("解析");
        assert_eq!(p.hub.max_locks, 0);
        let p = parse(Some(r#"{"maxLocks": 3}"#)).map_err(|e| e.message).expect("解析");
        assert_eq!(p.hub.max_locks, 3);
        for bad in [r#"{"maxLocks": -1}"#, r#"{"maxLocks": 1.5}"#, r#"{"maxLocks": "8"}"#] {
            let e = parse(Some(bad)).err().map(|e| e.status);
            assert_eq!(e, Some(AmHubStatus::InvalidJson), "{bad}");
        }
    }

    #[test]
    fn power_fields() {
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert_eq!((p.hub.wake_rate_limit, p.hub.legacy_heartbeat), (hub::DEFAULT_WAKE_RATE_LIMIT, false));
        let p = parse(Some(r#"{"wakeRateLimit": 0, "legacyHeartbeat": true}"#)).map_err(|e| e.message).expect("解析");
        assert_eq!((p.hub.wake_rate_limit, p.hub.legacy_heartbeat), (0, true));
        // 自适应租约（v8）
        assert_eq!(p.hub.lease, hub::LeasePolicy::default());
        let p = parse(Some(r#"{"lease": {"adaptive": false, "window": 4, "maxMs": 30000}}"#)).map_err(|e| e.message).expect("解析");
        assert_eq!((p.hub.lease.adaptive, p.hub.lease.window, p.hub.lease.max), (false, 4, Duration::from_secs(30)));
        assert!(parse(Some(r#"{"lease": {"bogus": 1}}"#)).is_err(), "lease 内未知字段报错");
        let e = parse(Some(r#"{"lease": {"window": 0}}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidConfig));
    }

    #[test]
    fn limit_fields() {
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert_eq!(p.hub.limits, hub::LimitPolicy::default());
        assert_eq!(p.hub.output_validation, OutputValidation::Log);
        let p = parse(Some(
            r#"{"limits": {"toolRatePerMinute": 10, "toolRateBurst": 2, "maxResultBytes": 0}, "outputValidation": "reject"}"#,
        ))
        .map_err(|e| e.message)
        .expect("解析");
        let d = hub::LimitPolicy::default();
        assert_eq!((p.hub.limits.tool_rate.per_minute, p.hub.limits.tool_rate.burst), (10, 2));
        assert_eq!(p.hub.limits.max_result_bytes, 0);
        assert_eq!((p.hub.limits.app_rate, p.hub.limits.max_arguments_bytes), (d.app_rate, d.max_arguments_bytes));
        assert_eq!(p.hub.output_validation, OutputValidation::Reject);
        // 不限（perMinute = 0）时 burst 可为 0
        assert!(parse(Some(r#"{"limits": {"appRatePerMinute": 0, "appRateBurst": 0}}"#)).is_ok());
        let e = parse(Some(r#"{"limits": {"toolRateBurst": 0}}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidConfig), "限流时 burst = 0 被拒");
        let e = parse(Some(r#"{"limits": {"bogus": 1}}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidJson), "limits 内未知字段报错");
        let e = parse(Some(r#"{"outputValidation": "strict"}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidJson));
    }

    #[test]
    fn policy_field() {
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert!(p.hub.policy.is_empty());
        let p = parse(Some(
            r#"{"policy": {"rules": [{"id": "no-clear", "action": "deny", "app": "shop*", "tool": "cart.clear", "hooks": ["call", "wake"]}]}}"#,
        ))
        .map_err(|e| e.message)
        .expect("解析");
        assert_eq!(p.hub.policy.rules.len(), 1);
        assert_eq!(p.hub.policy.rules[0].action, hub::PolicyAction::Deny);
        let e = parse(Some(r#"{"policy": {"rules": [{"id": "a b", "action": "hide", "app": "shop"}]}}"#)).err();
        let e = e.expect("不合法的 id");
        assert_eq!(e.status, AmHubStatus::InvalidConfig);
        assert!(e.message.contains("policy"), "{}", e.message);
        let e = parse(Some(r#"{"policy": {"rules": [], "bogus": 1}}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidJson), "policy 内未知字段报错");
    }

    #[test]
    fn event_limits_field() {
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert_eq!(p.hub.event_limits, EventLimits::default());
        let p = parse(Some(r#"{"eventLimits": {"maxInboxEvents": 5, "inboxTtlMs": 1500}}"#)).map_err(|e| e.message).expect("解析");
        let d = EventLimits::default();
        assert_eq!(
            p.hub.event_limits,
            EventLimits { max_inbox_events: 5, inbox_ttl: Duration::from_millis(1500), ..d },
            "只覆盖给出的字段"
        );
        let p = parse(Some(
            r#"{"eventLimits": {"maxSubscriptions": 2, "maxInboxEvents": 3, "inboxTtlMs": 0, "perSubscriptionPerMinute": 0}}"#,
        ))
        .map_err(|e| e.message)
        .expect("解析");
        assert_eq!(
            p.hub.event_limits,
            EventLimits { max_subscriptions: 2, max_inbox_events: 3, inbox_ttl: Duration::ZERO, per_subscription_per_minute: 0 }
        );
        for bad in [r#"{"eventLimits": {"bogus": 1}}"#, r#"{"eventLimits": {"maxSubscriptions": -1}}"#, r#"{"eventLimits": {"inboxTtlMs": "1h"}}"#] {
            let e = parse(Some(bad)).err().map(|e| e.status);
            assert_eq!(e, Some(AmHubStatus::InvalidJson), "{bad}");
        }
    }

    #[test]
    fn agents_field() {
        const T: &str = "0123456789abcdef0123456789abcdef";
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert!(p.hub.agents.agents.is_empty());
        let p = parse(Some(&format!(r#"{{"agents": [{{"name": "claude", "token": "{T}"}}]}}"#)))
            .map_err(|e| e.message)
            .expect("解析");
        assert_eq!(p.hub.agents.agents[0].name, "claude");
        let e = parse(Some(r#"{"agents": [{"name": "claude", "token": "short"}]}"#)).err().expect("令牌过短");
        assert_eq!(e.status, AmHubStatus::InvalidConfig);
        assert!(e.message.contains("agents") && !e.message.contains("short"), "{}", e.message);
        let e = parse(Some(r#"{"agents": {"agents": []}}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidJson), "agents 是数组");
    }

    #[test]
    fn exposure_and_waker_fields() {
        let d = HubConfig::default();
        let p = parse(None).map_err(|e| e.message).expect("默认");
        assert_eq!(p.hub.tool_exposure, d.tool_exposure);
        assert_eq!(p.hub.tool_exposure_threshold, d.tool_exposure_threshold);
        assert_eq!(p.hub.waker, WakerConfig::System);
        let p = parse(Some(
            r#"{"toolExposure": "progressive", "toolExposureThreshold": 7, "waker": {"exec": ["node", "w.mjs"]}}"#,
        ))
        .map_err(|e| e.message)
        .expect("解析");
        assert_eq!(p.hub.tool_exposure, ToolExposure::Progressive);
        assert_eq!(p.hub.tool_exposure_threshold, 7);
        assert_eq!(p.hub.waker, WakerConfig::Exec(vec!["node".into(), "w.mjs".into()]));
        let p = parse(Some(r#"{"waker": "none"}"#)).map_err(|e| e.message).expect("解析");
        assert_eq!(p.hub.waker, WakerConfig::None);
        let e = parse(Some(r#"{"toolExposure": "some"}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidJson));
    }

    #[test]
    fn rejects_unknown_fields_and_bad_manifests() {
        let e = parse(Some(r#"{"wsadr": "x"}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidJson));
        let e = parse(Some(r#"{"manifests": [{"appId": "Bad Id"}]}"#))
            .err()
            .map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidConfig));
    }
}
