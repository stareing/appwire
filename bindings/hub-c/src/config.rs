//! `am_hub_start` 的 JSON 配置 → [`HubConfig`]。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use hub::{ApprovalPolicy, HubConfig, UpstreamConfig, load_manifests};
use serde::Deserialize;
use serde_json::Value;

use crate::ffi_util::{AmHubStatus, FfiError, FfiResult};

/// 默认 tokio 工作线程数。
const DEFAULT_WORKER_THREADS: usize = 2;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub(crate) struct ConfigJson {
    /// 缺省为默认地址；显式 `null` 表示不开 WebSocket 服务。
    pub ws_addr: Option<String>,
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
    pub upstreams: BTreeMap<String, UpstreamConfig>,
    pub approval: ApprovalPolicy,
    pub worker_threads: Option<usize>,
}

impl Default for ConfigJson {
    fn default() -> Self {
        Self {
            ws_addr: HubConfig::default().ws_addr,
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
            upstreams: BTreeMap::new(),
            approval: ApprovalPolicy::default(),
            worker_threads: None,
        }
    }
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
    let mut hub = HubConfig {
        ws_addr: c.ws_addr,
        allow_origins: c.allow_origins,
        upstreams: c.upstreams,
        approval: c.approval,
        ..HubConfig::default()
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
    fn defaults_and_null_ws_addr() {
        let p = parse(None).map_err(|e| e.message).expect("默认配置");
        assert_eq!(p.hub.ws_addr, HubConfig::default().ws_addr);
        assert_eq!(p.worker_threads, DEFAULT_WORKER_THREADS);
        let p = parse(Some(r#"{"wsAddr": null, "responseTimeoutMs": 1500,
            "approval": {"requireAtOrAbove": "payment", "timeout": 200}}"#))
        .map_err(|e| e.message)
        .expect("解析");
        assert_eq!(p.hub.ws_addr, None);
        assert_eq!(p.hub.response_timeout, Duration::from_millis(1500));
        assert_eq!(p.hub.approval.timeout, Some(Duration::from_millis(200)));
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
    fn rejects_unknown_fields_and_bad_manifests() {
        let e = parse(Some(r#"{"wsadr": "x"}"#)).err().map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidJson));
        let e = parse(Some(r#"{"manifests": [{"appId": "Bad Id"}]}"#))
            .err()
            .map(|e| e.status);
        assert_eq!(e, Some(AmHubStatus::InvalidConfig));
    }
}
