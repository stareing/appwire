use app_mcp_hub::{
    LeaseOverrides, LeasePolicy, LimitOverrides, LimitPolicy, McpProtocolMode, OutputValidation, ToolExposure, WakerConfig,
};

use super::*;

fn abs(p: &str) -> PathBuf {
    absolute(Path::new(p)).unwrap()
}

fn home() -> AppHome {
    AppHome {
        dir: PathBuf::from("/h/.app-mcp"),
    }
}

#[test]
fn name_service_overrides() {
    let o = Overrides { name_service: Some(true), channel_grace_ms: Some(500), ..Default::default() };
    let s = Settings::resolve(&FileConfig::default(), &o, &home()).unwrap();
    assert!(s.name_service);
    assert_eq!(s.channel_grace_ms, 500);
    let file: FileConfig =
        serde_json::from_str(r#"{"lifecycle":{"nameService":true,"channelGraceMs":2000}}"#).unwrap();
    let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
    assert!(s.name_service);
    assert_eq!(s.channel_grace_ms, 2000);
}

#[test]
fn stateless_settings_from_file_and_cli() {
    let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
    let hub = app_mcp_hub::HubConfig::default();
    assert_eq!(
        (s.task_idle_ttl_ms, s.principal_select_ttl_ms, s.stateless_tool_exposure, s.stateless_list_ttl_ms),
        (
            hub.task_idle_ttl.as_millis() as u64,
            hub.principal_select_ttl.as_millis() as u64,
            hub.stateless_tool_exposure,
            hub.stateless_list_ttl.as_millis() as u64
        ),
        "默认值与 HubConfig 一致"
    );
    let file: FileConfig = serde_json::from_str(
        r#"{"lifecycle":{"taskIdleTtlMs":0,"principalSelectTtlMs":1500},
                "tools":{"statelessExposure":"progressive","statelessListTtlMs":0}}"#,
    )
    .unwrap();
    let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
    assert_eq!(
        (s.task_idle_ttl_ms, s.principal_select_ttl_ms, s.stateless_tool_exposure, s.stateless_list_ttl_ms),
        (0, 1500, ToolExposure::Progressive, 0)
    );
    // 命令行覆盖；service install 持久化为配置文件的键
    let o = Overrides {
        task_idle_ttl_ms: Some(2000),
        principal_select_ttl_ms: Some(0),
        stateless_tool_exposure: Some(ToolExposure::Auto),
        ..Default::default()
    };
    let s = Settings::resolve(&file, &o, &home()).unwrap();
    assert_eq!(
        (s.task_idle_ttl_ms, s.principal_select_ttl_ms, s.stateless_tool_exposure, s.stateless_list_ttl_ms),
        (2000, 0, ToolExposure::Auto, 0)
    );
    let mut f = FileConfig::default();
    f.apply(&o).unwrap();
    let v = serde_json::to_value(&f).unwrap();
    assert_eq!(v["lifecycle"], serde_json::json!({"taskIdleTtlMs": 2000, "principalSelectTtlMs": 0}));
    assert_eq!(v["tools"], serde_json::json!({"statelessExposure": "auto"}));
    // 类型不对：明确报错
    assert!(serde_json::from_str::<FileConfig>(r#"{"tools":{"statelessExposure":"some"}}"#).is_err());
    assert!(serde_json::from_str::<FileConfig>(r#"{"lifecycle":{"taskIdleTtlMs":-1}}"#).is_err());
}

#[test]
fn max_task_handles_from_file_and_cli() {
    let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
    assert_eq!(s.max_task_handles, app_mcp_hub::HubConfig::default().max_task_handles, "默认值与 HubConfig 一致");
    let file: FileConfig = serde_json::from_str(r#"{"mcp":{"maxTaskHandles":0}}"#).unwrap();
    let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
    assert_eq!(s.max_task_handles, 0);
    // 命令行覆盖；service install 持久化为配置文件的键
    let o = Overrides { max_task_handles: Some(5), ..Default::default() };
    let s = Settings::resolve(&file, &o, &home()).unwrap();
    assert_eq!(s.max_task_handles, 5);
    let mut f = FileConfig::default();
    f.apply(&o).unwrap();
    let v = serde_json::to_value(&f).unwrap();
    assert_eq!(v["mcp"], serde_json::json!({"maxTaskHandles": 5}));
    // 类型不对：明确报错
    assert!(serde_json::from_str::<FileConfig>(r#"{"mcp":{"maxTaskHandles":-1}}"#).is_err());
    assert!(serde_json::from_str::<FileConfig>(r#"{"mcp":{"maxTaskHandles":1.5}}"#).is_err());
    assert!(serde_json::from_str::<FileConfig>(r#"{"mcp":{"maxTaskHandles":"8"}}"#).is_err());
}

#[test]
fn mcp_settings_from_file_and_cli() {
    let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
    let hub = app_mcp_hub::HubConfig::default();
    assert_eq!(
        (s.mcp_protocol_mode, s.max_listen_streams, s.max_listen_resources),
        (hub.mcp_protocol_mode, hub.max_listen_streams, hub.max_listen_resources),
        "默认值与 HubConfig 一致"
    );
    let file: FileConfig = serde_json::from_str(
        r#"{"mcp":{"protocolMode":"legacyOnly","maxListenStreams":0,"maxListenResources":8}}"#,
    )
    .unwrap();
    let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
    assert_eq!(
        (s.mcp_protocol_mode, s.max_listen_streams, s.max_listen_resources),
        (McpProtocolMode::LegacyOnly, 0, 8)
    );
    // 命令行覆盖；service install 持久化为配置文件的键
    let o = Overrides {
        mcp_protocol_mode: Some(McpProtocolMode::Auto),
        max_listen_streams: Some(4),
        ..Default::default()
    };
    let s = Settings::resolve(&file, &o, &home()).unwrap();
    assert_eq!(
        (s.mcp_protocol_mode, s.max_listen_streams, s.max_listen_resources),
        (McpProtocolMode::Auto, 4, 8)
    );
    let mut f = FileConfig::default();
    f.apply(&o).unwrap();
    let v = serde_json::to_value(&f).unwrap();
    assert_eq!(v["mcp"], serde_json::json!({"protocolMode": "auto", "maxListenStreams": 4}));
    assert!(serde_json::to_value(FileConfig::default()).unwrap().get("mcp").is_none(), "未设置时不写出");
    // 类型不对：明确报错
    assert!(serde_json::from_str::<FileConfig>(r#"{"mcp":{"protocolMode":"modern"}}"#).is_err());
    assert!(serde_json::from_str::<FileConfig>(r#"{"mcp":{"maxListenStreams":-1}}"#).is_err());
    assert!(serde_json::from_str::<FileConfig>(r#"{"mcp":{"maxListenResources":1.5}}"#).is_err());
}

#[test]
fn idle_exit_from_file_and_override() {
    let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
    assert_eq!(s.idle_exit_ms, crate::activation::DEFAULT_IDLE_EXIT_MS);
    let mut file: FileConfig = serde_json::from_str(r#"{"lifecycle":{"idleExitMs":0}}"#).unwrap();
    assert_eq!(Settings::resolve(&file, &Overrides::default(), &home()).unwrap().idle_exit_ms, 0);
    let o = Overrides { idle_exit_ms: Some(1500), ..Default::default() };
    assert_eq!(Settings::resolve(&file, &o, &home()).unwrap().idle_exit_ms, 1500);
    // service install 持久化覆盖项
    file.apply(&o).unwrap();
    assert!(serde_json::to_string(&file).unwrap().contains(r#""idleExitMs":1500"#));
}

#[test]
fn defaults() {
    let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
    assert_eq!(s.listen, "127.0.0.1:7717");
    assert!(!s.listen_explicit);
    assert_eq!(s.compat_http_addr, None, "旧 MCP 端口默认不开");
    assert!(s.notices.is_empty());
    assert_eq!(s.auth, AuthMode::Browser);
    assert_eq!(
        s.manifest_dirs,
        vec![(PathBuf::from("/h/.app-mcp/manifests"), false)]
    );
    assert!(s.log_file);
    assert!(!s.name_service, "按名寻址默认关闭");
    assert_eq!(s.channel_grace_ms, 15_000);
    assert_eq!(s.lease_ms, 60_000);
    assert_eq!(
        (s.wake_token_ttl_ms, s.wake_rate_limit, s.legacy_heartbeat),
        (60_000, app_mcp_hub::DEFAULT_WAKE_RATE_LIMIT, false)
    );
    assert_eq!(s.waker, WakerConfig::System);
    assert_eq!(s.tool_exposure, ToolExposure::Auto);
    assert_eq!(s.tool_exposure_threshold, 40);
    assert_eq!(
        s.ipc_endpoint,
        app_mcp_protocol::endpoint::default_ipc_endpoint().map(|e| e.to_string())
    );
}

#[test]
fn ipc_endpoint_config() {
    let resolve = |file: &str, cli: Option<&str>| {
        let f: FileConfig = serde_json::from_str(file).unwrap();
        let o = Overrides {
            ipc_endpoint: cli.map(str::to_owned),
            ..Default::default()
        };
        Settings::resolve(&f, &o, &home()).map(|s| s.ipc_endpoint)
    };
    assert_eq!(
        resolve(r#"{"ipcEndpoint": "unix:/run/x/hub.sock"}"#, None).unwrap().as_deref(),
        Some("unix:/run/x/hub.sock")
    );
    assert_eq!(
        resolve(r#"{"ipcEndpoint": "unix:/run/x/hub.sock"}"#, Some(r"pipe:\\.\pipe\p")).unwrap().as_deref(),
        Some(r"pipe:\\.\pipe\p")
    );
    assert_eq!(resolve(r#"{"ipcEndpoint": "none"}"#, None).unwrap(), None);
    assert_eq!(resolve("{}", Some("none")).unwrap(), None);
    assert!(resolve(r#"{"ipcEndpoint": "ws://127.0.0.1:1"}"#, None).is_err());
    assert!(resolve("{}", Some("hub.sock")).is_err());
}

#[test]
fn parse_full_and_legacy() {
    let full: FileConfig = serde_json::from_str(
        r#"{
              "listen": "127.0.0.1:9000",
              "http": { "addr": "127.0.0.1:9001", "auth": "all" },
              "manifests": ["/m/a.json"],
              "manifestDirs": ["/m"],
              "allowOrigins": ["https://app.example.com"],
              "upstreams": { "files": { "command": "npx", "args": ["x"] } },
              "lifecycle": { "leaseMs": 500, "wakeTimeoutMs": 2000, "navigateTimeoutMs": 3000, "wakeFromLaunch": true,
                             "waker": { "exec": ["node", "wake.mjs"] } },
              "tools": { "exposure": "progressive", "threshold": 10 },
              "log": { "level": "debug", "file": false, "maxBytes": 1024, "keep": 1 }
            }"#,
    )
    .unwrap();
    let s = Settings::resolve(&full, &Overrides::default(), &home()).unwrap();
    assert_eq!(s.listen, "127.0.0.1:9000");
    assert!(s.listen_explicit);
    // 旧的独立 MCP 端口：兼容期内显式配置才另开，并记录弃用提示
    assert_eq!(s.compat_http_addr.as_deref(), Some("127.0.0.1:9001"));
    assert_eq!(s.notices.len(), 1);
    assert_eq!(s.auth, AuthMode::All);
    // Windows 上 "/m" 不是绝对路径，会接到当前目录（盘符）上
    assert_eq!(s.manifest_dirs, vec![(abs("/m"), true)]);
    assert_eq!(s.upstreams["files"].command, "npx");
    assert_eq!(
        (s.lease_ms, s.wake_timeout_ms, s.navigate_timeout_ms, s.wake_from_launch),
        (500, 2000, 3000, true)
    );
    assert_eq!(
        s.waker,
        WakerConfig::Exec(vec!["node".into(), "wake.mjs".into()])
    );
    assert_eq!(
        (s.tool_exposure, s.tool_exposure_threshold),
        (ToolExposure::Progressive, 10)
    );
    assert_eq!(
        (
            s.log_level.as_str(),
            s.log_file,
            s.log_max_bytes,
            s.log_keep
        ),
        ("debug", false, 1024, 1)
    );

    // 旧的 --config 文件（只有 upstreams）仍可解析
    let legacy: FileConfig =
        serde_json::from_str(r#"{"upstreams":{"e":{"command":"echo"}}}"#).unwrap();
    assert_eq!(legacy.upstreams.len(), 1);
}

#[test]
fn cli_overrides_file() {
    let file: FileConfig = serde_json::from_str(
        r#"{"listen":"127.0.0.1:9000","manifests":["/m/a.json"],"lifecycle":{"leaseMs":500}}"#,
    )
    .unwrap();
    let o = Overrides {
        listen: Some("127.0.0.1:1".into()),
        manifests: vec![PathBuf::from("/m/b.json")],
        auth: Some(AuthMode::Off),
        ..Default::default()
    };
    let s = Settings::resolve(&file, &o, &home()).unwrap();
    assert_eq!(s.listen, "127.0.0.1:1");
    assert_eq!(s.manifests, vec![abs("/m/a.json"), abs("/m/b.json")]);
    assert_eq!(s.lease_ms, 500);
    assert_eq!(s.auth, AuthMode::Off);
}

#[test]
fn power_settings_from_file_and_cli() {
    let file: FileConfig = serde_json::from_str(
        r#"{"lifecycle":{"wakeTokenTtlMs":30000,"wakeRateLimit":2,"legacyHeartbeat":true}}"#,
    )
    .unwrap();
    let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
    assert_eq!((s.wake_token_ttl_ms, s.wake_rate_limit, s.legacy_heartbeat), (30_000, 2, true));
    let o = Overrides { wake_token_ttl_ms: Some(5_000), wake_rate_limit: Some(0), ..Default::default() };
    let s = Settings::resolve(&file, &o, &home()).unwrap();
    assert_eq!((s.wake_token_ttl_ms, s.wake_rate_limit, s.legacy_heartbeat), (5_000, 0, true));
    // service install 写入配置：命令行覆盖项持久化为 lifecycle 下的键
    let mut f = FileConfig::default();
    f.apply(&Overrides {
        wake_token_ttl_ms: Some(1_000),
        wake_rate_limit: Some(3),
        legacy_heartbeat: Some(true),
        ..Default::default()
    })
    .unwrap();
    let v = serde_json::to_value(&f).unwrap();
    assert_eq!(
        v["lifecycle"],
        serde_json::json!({"wakeTokenTtlMs": 1000, "wakeRateLimit": 3, "legacyHeartbeat": true})
    );
}

#[test]
fn lease_settings_from_file_and_cli() {
    let file: FileConfig =
        serde_json::from_str(r#"{"lifecycle":{"lease":{"window":8,"maxMs":30000}}}"#).unwrap();
    let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
    assert_eq!(s.lease, LeasePolicy::default());
    let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
    assert_eq!((s.lease.window, s.lease.max), (8, std::time::Duration::from_secs(30)));
    // 命令行按字段覆盖
    let o = Overrides {
        lease: LeaseOverrides { adaptive: Some(false), window: Some(3), ..Default::default() },
        ..Default::default()
    };
    let s = Settings::resolve(&file, &o, &home()).unwrap();
    assert_eq!(
        (s.lease.adaptive, s.lease.window, s.lease.max),
        (false, 3, std::time::Duration::from_secs(30))
    );
    let mut f = file.clone();
    f.apply(&o).unwrap();
    assert_eq!(
        serde_json::to_value(&f).unwrap()["lifecycle"]["lease"],
        serde_json::json!({"adaptive": false, "window": 3, "maxMs": 30000})
    );
    // 不合法：明确报错
    let bad: FileConfig = serde_json::from_str(r#"{"lifecycle":{"lease":{"minMs":9000,"maxMs":1000}}}"#).unwrap();
    let e = Settings::resolve(&bad, &Overrides::default(), &home()).unwrap_err().to_string();
    assert!(e.contains("lifecycle.lease"), "{e}");
    assert!(serde_json::from_str::<FileConfig>(r#"{"lifecycle":{"lease":{"bogus":1}}}"#).is_err());
}

#[test]
fn limit_settings_from_file_and_cli() {
    let s = Settings::resolve(&FileConfig::default(), &Overrides::default(), &home()).unwrap();
    assert_eq!((s.limits.clone(), s.output_validation), (LimitPolicy::default(), OutputValidation::Log));
    let file: FileConfig = serde_json::from_str(
        r#"{"limits":{"toolRatePerMinute":10,"toolRateBurst":2,"maxResultBytes":0},"tools":{"outputValidation":"reject"}}"#,
    )
    .unwrap();
    let s = Settings::resolve(&file, &Overrides::default(), &home()).unwrap();
    assert_eq!((s.limits.tool_rate.per_minute, s.limits.tool_rate.burst, s.limits.max_result_bytes), (10, 2, 0));
    assert_eq!(s.output_validation, OutputValidation::Reject);
    assert_eq!(s.progress_interval_ms, 250, "默认进度间隔");
    let no_merge: FileConfig = serde_json::from_str(r#"{"tools":{"progressIntervalMs":0}}"#).unwrap();
    assert_eq!(Settings::resolve(&no_merge, &Overrides::default(), &home()).unwrap().progress_interval_ms, 0);
    // 命令行按字段覆盖，写回配置时合并
    let o = Overrides {
        limits: LimitOverrides { tool_rate_burst: Some(5), app_rate_per_minute: Some(0), ..Default::default() },
        output_validation: Some(OutputValidation::Off),
        ..Default::default()
    };
    let s = Settings::resolve(&file, &o, &home()).unwrap();
    assert_eq!((s.limits.tool_rate.per_minute, s.limits.tool_rate.burst), (10, 5));
    assert!(s.limits.app_rate.is_unlimited());
    assert_eq!(s.output_validation, OutputValidation::Off);
    let mut f = file.clone();
    f.apply(&o).unwrap();
    let v = serde_json::to_value(&f).unwrap();
    assert_eq!(
        v["limits"],
        serde_json::json!({"toolRatePerMinute": 10, "toolRateBurst": 5, "appRatePerMinute": 0, "maxResultBytes": 0})
    );
    assert_eq!(v["tools"]["outputValidation"], "off");
    // 不合法：明确报错
    let bad: FileConfig = serde_json::from_str(r#"{"limits":{"appRateBurst":0}}"#).unwrap();
    let e = Settings::resolve(&bad, &Overrides::default(), &home()).unwrap_err().to_string();
    assert!(e.contains("appRateBurst"), "{e}");
    // 第 16 项 P3：每 Agent 配额经配置文件给出（默认不限）
    assert!(LimitPolicy::default().agent_rate.is_unlimited());
    let quota: FileConfig = serde_json::from_str(r#"{"limits":{"agentRatePerMinute":30,"agentRateBurst":5}}"#).unwrap();
    let s = Settings::resolve(&quota, &Overrides::default(), &home()).unwrap();
    assert_eq!((s.limits.agent_rate.per_minute, s.limits.agent_rate.burst), (30, 5));
    let bad: FileConfig = serde_json::from_str(r#"{"limits":{"agentRatePerMinute":30,"agentRateBurst":0}}"#).unwrap();
    assert!(Settings::resolve(&bad, &Overrides::default(), &home()).unwrap_err().to_string().contains("agentRateBurst"));
    assert!(serde_json::from_str::<FileConfig>(r#"{"limits":{"bogus":1}}"#).is_err());
}

#[test]
fn deprecated_ws_addr_and_http_addr() {
    let resolve = |file: &str, o: Overrides| {
        let f: FileConfig = serde_json::from_str(file).unwrap();
        Settings::resolve(&f, &o, &home())
    };
    // 只有旧名 wsAddr：按 listen 使用，带提示
    let s = resolve(r#"{"wsAddr":"127.0.0.1:9000"}"#, Overrides::default()).unwrap();
    assert_eq!((s.listen.as_str(), s.listen_explicit), ("127.0.0.1:9000", true));
    assert_eq!(s.notices.len(), 1);
    // 新旧同时设置且不同：报错；相同：接受
    assert!(resolve(r#"{"wsAddr":"127.0.0.1:9000","listen":"127.0.0.1:9001"}"#, Overrides::default()).is_err());
    assert!(resolve(r#"{"wsAddr":"127.0.0.1:9000","listen":"127.0.0.1:9000"}"#, Overrides::default()).is_ok());
    // 命令行 --listen 取代文件中的旧名（写回时迁移）
    let o = Overrides { listen: Some("127.0.0.1:1".into()), ..Default::default() };
    let mut f: FileConfig = serde_json::from_str(r#"{"wsAddr":"127.0.0.1:9000"}"#).unwrap();
    f.apply(&o).unwrap();
    assert_eq!((f.listen.as_deref(), f.ws_addr.as_deref()), (Some("127.0.0.1:1"), None));
    // http.addr 与 listen 相同：不另开监听
    let s = resolve(r#"{"listen":"127.0.0.1:9000","http":{"addr":"127.0.0.1:9000"}}"#, Overrides::default()).unwrap();
    assert_eq!(s.compat_http_addr, None);
}

#[test]
fn save_roundtrip_skips_empty() {
    let dir = std::env::temp_dir().join(format!("app-mcp-cfg-{}", std::process::id()));
    let path = dir.join("config.json");
    let mut c = FileConfig::default();
    c.apply(&Overrides {
        listen: Some("127.0.0.1:7717".into()),
        ..Default::default()
    })
    .unwrap();
    c.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, "{\n  \"listen\": \"127.0.0.1:7717\"\n}\n");
    assert_eq!(FileConfig::load(&path, true).unwrap(), c);
    assert_eq!(
        FileConfig::load(&dir.join("missing.json"), false).unwrap(),
        FileConfig::default()
    );
    assert!(FileConfig::load(&dir.join("missing.json"), true).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}
