//! 合并后的设置 → Hub 配置（含本平台的名字服务连接器）；`serve` 与 stdio 模式共用。

use std::time::Duration;

use app_mcp_hub::{HubConfig, load_manifests};

use crate::config::{AppHome, Settings};

pub(super) fn hub_config(s: &Settings, home: &AppHome) -> HubConfig {
    let mut manifests = Vec::new();
    for (dir, required) in &s.manifest_dirs {
        manifests.extend(load_manifests(&[], Some(dir), *required));
    }
    manifests.extend(load_manifests(&s.manifests, None, false));
    let defaults = HubConfig::default();
    HubConfig {
        listen: Some(s.listen.clone()),
        // 显式指定的地址只绑定它本身；缺省地址被占用时依次尝试备选端口（与网页 SDK 一致）。
        listen_alternates: if s.listen_explicit { Vec::new() } else { defaults.listen_alternates },
        ipc_endpoint: s.ipc_endpoint.clone(),
        run_dir: Some(home.run_dir()),
        state_dir: Some(home.state_dir()),
        manifests,
        allow_origins: s.allow_origins.clone(),
        upstreams: s.upstreams.clone(),
        lease_ttl: Duration::from_millis(s.lease_ms),
        wake_timeout: Duration::from_millis(s.wake_timeout_ms),
        navigate_timeout: Duration::from_millis(s.navigate_timeout_ms),
        wake_from_launch: s.wake_from_launch,
        wake_token_ttl: Duration::from_millis(s.wake_token_ttl_ms),
        wake_rate_limit: s.wake_rate_limit,
        legacy_heartbeat: s.legacy_heartbeat,
        lease: s.lease.clone(),
        waker: s.waker.clone(),
        tool_exposure: s.tool_exposure,
        tool_exposure_threshold: s.tool_exposure_threshold,
        limits: s.limits.clone(),
        result_cache: s.result_cache,
        output_validation: s.output_validation,
        progress_interval: Duration::from_millis(s.progress_interval_ms),
        connectors: name_service_connectors(s.name_service),
        channel_grace: Duration::from_millis(s.channel_grace_ms),
        task_idle_ttl: Duration::from_millis(s.task_idle_ttl_ms),
        stateless_tool_exposure: s.stateless_tool_exposure,
        principal_select_ttl: Duration::from_millis(s.principal_select_ttl_ms),
        stateless_list_ttl: Duration::from_millis(s.stateless_list_ttl_ms),
        mcp_protocol_mode: s.mcp_protocol_mode,
        max_listen_streams: s.max_listen_streams,
        max_listen_resources: s.max_listen_resources,
        max_task_handles: s.max_task_handles,
        max_locks: s.max_locks,
        ..defaults
    }
}

/// `--name-service` / `lifecycle.nameService`：本平台的名字服务连接器（spec/naming.md 4.0）。
fn name_service_connectors(enabled: bool) -> Vec<std::sync::Arc<dyn app_mcp_hub::Connector>> {
    if !enabled {
        return Vec::new();
    }
    #[cfg(target_os = "linux")]
    {
        vec![std::sync::Arc::new(app_mcp_hub::connector::DbusConnector::new(None))]
    }
    #[cfg(windows)]
    {
        match app_mcp_hub::connector::PipeConnector::new(None) {
            Ok(c) => vec![std::sync::Arc::new(c)],
            Err(e) => {
                tracing::warn!("按名寻址不可用：{e}");
                Vec::new()
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        match app_mcp_hub::connector::LaunchdConnector::new(None) {
            Ok(c) => vec![std::sync::Arc::new(c)],
            Err(e) => {
                tracing::warn!("按名寻址不可用：{e}");
                Vec::new()
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        tracing::warn!("本平台尚未实现按名寻址（spec/naming.md 4.0），忽略 nameService");
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FileConfig, Overrides};

    #[test]
    fn hub_config_carries_stateless_settings() {
        let home = AppHome { dir: std::env::temp_dir().join(format!("app-mcp-hubcfg-{}", std::process::id())) };
        let file: FileConfig = serde_json::from_str(
            r#"{"lifecycle":{"taskIdleTtlMs":1234,"principalSelectTtlMs":0},
                "tools":{"statelessExposure":"progressive","statelessListTtlMs":750}}"#,
        )
        .unwrap();
        let s = Settings::resolve(&file, &Overrides::default(), &home).unwrap();
        let c = hub_config(&s, &home);
        assert_eq!(c.task_idle_ttl, Duration::from_millis(1234));
        assert_eq!(c.principal_select_ttl, Duration::ZERO);
        assert_eq!(c.stateless_tool_exposure, app_mcp_hub::ToolExposure::Progressive);
        assert_eq!(c.stateless_list_ttl, Duration::from_millis(750));
    }

    #[test]
    fn hub_config_carries_mcp_settings() {
        let home = AppHome { dir: std::env::temp_dir().join(format!("app-mcp-hubcfg-mcp-{}", std::process::id())) };
        let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home).unwrap();
        let c = hub_config(&s, &home);
        let d = HubConfig::default();
        assert_eq!(
            (c.mcp_protocol_mode, c.max_listen_streams, c.max_listen_resources),
            (d.mcp_protocol_mode, d.max_listen_streams, d.max_listen_resources)
        );
        let file: FileConfig = serde_json::from_str(
            r#"{"mcp":{"protocolMode":"legacyOnly","maxListenStreams":3,"maxListenResources":7}}"#,
        )
        .unwrap();
        let s = Settings::resolve(&file, &Overrides::default(), &home).unwrap();
        let c = hub_config(&s, &home);
        assert_eq!(c.mcp_protocol_mode, app_mcp_hub::McpProtocolMode::LegacyOnly);
        assert_eq!((c.max_listen_streams, c.max_listen_resources), (3, 7));
    }

    #[test]
    fn hub_config_carries_max_task_handles() {
        let home = AppHome { dir: std::env::temp_dir().join(format!("app-mcp-hubcfg-handles-{}", std::process::id())) };
        let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home).unwrap();
        assert_eq!(hub_config(&s, &home).max_task_handles, HubConfig::default().max_task_handles);
        let file: FileConfig = serde_json::from_str(r#"{"mcp":{"maxTaskHandles":0}}"#).unwrap();
        let s = Settings::resolve(&file, &Overrides::default(), &home).unwrap();
        assert_eq!(hub_config(&s, &home).max_task_handles, 0);
    }

    #[test]
    fn hub_config_carries_max_locks() {
        let home = AppHome { dir: std::env::temp_dir().join(format!("app-mcp-hubcfg-locks-{}", std::process::id())) };
        let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home).unwrap();
        assert_eq!(hub_config(&s, &home).max_locks, HubConfig::default().max_locks);
        let file: FileConfig = serde_json::from_str(r#"{"mcp":{"maxLocks":0}}"#).unwrap();
        let s = Settings::resolve(&file, &Overrides::default(), &home).unwrap();
        assert_eq!(hub_config(&s, &home).max_locks, 0);
    }

    #[test]
    fn hub_config_carries_result_cache_limits() {
        let home = AppHome { dir: std::env::temp_dir().join(format!("app-mcp-hubcfg-cache-{}", std::process::id())) };
        let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home).unwrap();
        assert_eq!(hub_config(&s, &home).result_cache, HubConfig::default().result_cache);
        let file: FileConfig =
            serde_json::from_str(r#"{"resultCache":{"maxEntries":3,"maxBytes":2048,"maxEntryBytes":256}}"#).unwrap();
        let s = Settings::resolve(&file, &Overrides::default(), &home).unwrap();
        let c = hub_config(&s, &home).result_cache;
        assert_eq!((c.max_entries, c.max_bytes, c.max_entry_bytes), (3, 2048, 256));
    }
}
