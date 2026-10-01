//! `am_hub_start` 的 JSON 配置 → [`HubConfig`]。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use hub::{ApprovalPolicy, HubConfig, LeaseOverrides, ToolExposure, UpstreamConfig, WakerConfig, load_manifests};
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
    pub lease_ttl_ms: Option<u64>,
    pub wake_timeout_ms: Option<u64>,
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
    /// `"system"` / `"none"` / `{"exec": [...]}`（spec/hub-api.md 3.5）。
    pub waker: Option<WakerConfig>,
    /// 渐进暴露（spec/hub-api.md 3.7）。
    pub tool_exposure: Option<ToolExposure>,
    pub tool_exposure_threshold: Option<usize>,
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
            lease_ttl_ms: None,
            wake_timeout_ms: None,
            wake_token_ttl_ms: None,
            dormant_ttl_ms: None,
            dormant_replaced_by_new_instance: None,
            wake_from_launch: None,
            wake_rate_limit: None,
            legacy_heartbeat: None,
            lease: None,
            waker: None,
            tool_exposure: None,
            tool_exposure_threshold: None,
            upstreams: BTreeMap::new(),
            approval: ApprovalPolicy::default(),
            worker_threads: None,
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
    // 生命周期（spec/hub-api.md 3.5）。
    set_ms(&mut hub.lease_ttl, c.lease_ttl_ms);
    set_ms(&mut hub.wake_timeout, c.wake_timeout_ms);
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
    if let Some(w) = c.waker {
        hub.waker = w;
    }
    if let Some(v) = c.tool_exposure {
        hub.tool_exposure = v;
    }
    if let Some(v) = c.tool_exposure_threshold {
        hub.tool_exposure_threshold = v;
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
        assert_eq!(p.worker_threads, DEFAULT_WORKER_THREADS);
        // 显式地址：只绑定该地址
        let p = parse(Some(r#"{"listen": "127.0.0.1:0", "mcpHttp": true, "runDir": "/tmp/r"}"#)).unwrap();
        assert_eq!(p.hub.listen.as_deref(), Some("127.0.0.1:0"));
        assert!(p.hub.listen_alternates.is_empty());
        assert!(p.hub.mcp_http);
        assert_eq!(p.hub.run_dir, Some(PathBuf::from("/tmp/r")));
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
