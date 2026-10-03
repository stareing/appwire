use app_mcp_hub::{LimitOverrides, McpProtocolMode, OutputValidation, ToolExposure, WakerConfig};
use clap::CommandFactory;

use super::hub_args::{parse_exposure, parse_output_validation, parse_protocol_mode, parse_waker};
use super::*;
use crate::config::AuthMode;

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
fn parses_max_locks() {
    let cli = Cli::try_parse_from(["app-mcp-host", "serve", "--max-locks", "0"]).unwrap();
    let Some(Command::Serve(s)) = cli.command else {
        panic!()
    };
    assert_eq!(s.hub.overrides().unwrap().max_locks, Some(0));
    assert!(Cli::try_parse_from(["app-mcp-host", "serve", "--max-locks", "-1"]).is_err());
}

#[test]
fn parses_limits() {
    assert_eq!(parse_output_validation("reject"), Ok(OutputValidation::Reject));
    assert!(parse_output_validation("strict").is_err());
    let cli = Cli::try_parse_from([
        "app-mcp-host", "serve", "--tool-rate-limit", "10", "--app-rate-burst", "3", "--max-arguments-bytes", "0",
        "--max-resource-bytes", "99", "--output-validation", "off", "--agent-rate-limit", "30", "--agent-rate-burst", "5",
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
            agent_rate_per_minute: Some(30),
            agent_rate_burst: Some(5),
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
    let Some(Command::Policy { action: PolicyCommand::Deny { rule, wake: true, agent: None } }) = cli.command else { panic!() };
    assert_eq!((rule.app.as_str(), rule.tool.as_deref()), ("shop", Some("pay*")));
    let cli = Cli::try_parse_from(["app-mcp-host", "policy", "deny", "shop", "--agent", "cursor"]).unwrap();
    let Some(Command::Policy { action: PolicyCommand::Deny { agent: Some(agent), .. } }) = cli.command else { panic!() };
    assert_eq!(agent, "cursor");
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
