use app_mcp_hub::{AwakeReason, HubStatus, InstancePower, LeaseStatus};

use crate::ports::PortOwner;

use super::checks_apps::*;
use super::checks_policy::*;
use super::*;

#[test]
fn proc_locks() {
    let text = "1: POSIX  ADVISORY  WRITE 900 00:19:555 0 EOF\n2: FLOCK  ADVISORY  WRITE 4242 08:02:123456 0 EOF\n2: -> FLOCK  ADVISORY  WRITE 4343 08:02:123456 0 EOF\n";
    assert_eq!(parse_proc_locks(text, 123456), Some(4242));
    assert_eq!(parse_proc_locks(text, 555), None, "POSIX 锁不算");
    assert_eq!(parse_proc_locks(text, 23456), None, "inode 需完整匹配");
}

#[test]
fn lease_summary() {
    let mut l = LeaseStatus {
        mode: "adaptive".into(),
        default_ms: 60_000,
        min_ms: 5_000,
        max_ms: 60_000,
        margin_ms: 5_000,
        window: 20,
        idle_revoke_ms: 30_000,
        adaptive_grants: 4,
        default_grants: 3,
        pairs: vec![app_mcp_hub::LeasePairStatus {
            session: "mcp:3".into(),
            app_id: "shop".into(),
            samples: 4,
            next_ttl_ms: 9_000,
            adaptive: true,
        }],
        ..Default::default()
    };
    let t = lease_text(&l);
    assert!(t.contains("p90 + 5000 ms") && t.contains("空闲 30000 ms") && t.contains("mcp:3→shop 9000 ms（统计"), "{t}");
    l.mode = "fixed".into();
    assert!(lease_text(&l).starts_with("固定 60000 ms"));
    l.mode = "off".into();
    assert!(lease_text(&l).contains("已关闭"));
    let c = lease_check(None);
    assert!(matches!(c.status, Level::Skip));
}

fn status_with_tools(rate_limited: u64) -> HubStatus {
    serde_json::from_value(json!({
        "service": "app-mcp", "version": "0", "pid": 1, "startedAtMs": 0, "mcpHttp": true,
        "auth": {"tokenConfigured": false, "tokenRequiredWithoutOrigin": false}, "mcpSessions": 0, "reports": [],
        "apps": [{
            "appId": "shop", "name": "商城", "kind": "app", "state": "connected", "instances": [],
            "rateLimited": rate_limited, "tooLarge": 1,
            "tools": [
                {"name": "cart.checkout", "risk": "payment", "effective": {"readOnlyHint": false, "destructiveHint": true}},
                {"name": "order.cancel", "risk": "write", "annotations": {"idempotentHint": true, "title": "取消"},
                 "effective": {"readOnlyHint": false, "idempotentHint": true, "title": "取消"}, "outputSchema": true}
            ]
        }],
        "limits": {"toolRatePerMinute": 120, "toolRateBurst": 30, "appRatePerMinute": 0, "appRateBurst": 60,
                   "maxArgumentsBytes": 1048576, "maxResultBytes": 0, "maxResourceBytes": 4194304},
        "outputValidation": "log"
    }))
    .unwrap()
}

#[test]
fn callers_text_counts_tasks() {
    let mut st = status_with_tools(0);
    assert_eq!(callers_text(&st), "MCP 会话 0 个", "旧 Host 不报告任务时省略");
    st.mcp_sessions = 1;
    st.tasks = Some(
        serde_json::from_value(json!([
            {"id": "task-1", "caller": "mcp:1", "kind": "mcpSession", "selections": [], "leases": [], "inflight": 0},
            {"id": "task-2", "caller": "principal:local", "kind": "principal",
             "selections": [{"appId": "shop", "instanceId": "a", "expiresInMs": 1000}],
             "leases": [{"connectionId": "c1", "expiresInMs": 500}], "inflight": 1, "idleMs": 3}
        ]))
        .unwrap(),
    );
    assert_eq!(callers_text(&st), "MCP 会话 1 个、Agent 任务 2 个");
    st.mcp_listen_streams = Some(3);
    assert_eq!(callers_text(&st), "MCP 会话 1 个、listen 流 3 个、Agent 任务 2 个");
    let c = apps_check(Some(&Ok(st)));
    assert!(c.summary.contains("listen 流 3 个、Agent 任务 2 个"), "{}", c.summary);
}

#[test]
fn tools_check_lists_declarations() {
    let st = status_with_tools(0);
    let c = tools_check(Some(&Ok(st)));
    assert!(matches!(c.status, Level::Info));
    assert!(c.summary.contains("2 个工具"), "{}", c.summary);
    assert!(
        c.summary.contains("shop.cart.checkout：risk payment，readOnlyHint=false destructiveHint=true idempotentHint=- openWorldHint=-（注解按 risk 推导）"),
        "{}",
        c.summary
    );
    assert!(c.summary.contains("shop.order.cancel：risk write") && c.summary.contains("title=「取消」（已声明注解，有 outputSchema）"), "{}", c.summary);
    assert_eq!(c.details["shop"][1]["annotations"]["idempotentHint"], true);
    assert!(matches!(tools_check(None).status, Level::Skip));
    let mut empty = status_with_tools(0);
    empty.apps[0].tools.clear();
    assert!(tools_check(Some(&Ok(empty))).summary.contains("没有已知工具"));
}

#[test]
fn policy_check_levels() {
    use app_mcp_hub::PolicyConfig;
    let rules = PolicyConfig::from_json(r#"{"rules": [{"id": "h", "action": "hide", "app": "notes"}]}"#).unwrap();
    let file = |r: Result<PolicyConfig, String>| ("/x/policy.json".to_owned(), r);
    let mut st = status_with_tools(0);
    st.policy = Some(serde_json::from_value(json!({
        "rules": [{"id": "h", "action": "hide", "app": "notes", "hits": 2}], "loadedAtMs": 1
    })).unwrap());
    let c = policy_check(file(Ok(rules.clone())), Some(&Ok(st.clone())));
    assert!(matches!(c.status, Level::Info) && c.summary.contains("h：hide app=notes，命中 2 次"), "{}", c.summary);
    let c = policy_check(file(Ok(PolicyConfig::default())), Some(&Ok(st.clone())));
    assert!(matches!(c.status, Level::Warn) && c.summary.contains("尚未重载"), "{}", c.summary);
    let c = policy_check(file(Err("坏了".into())), Some(&Ok(st.clone())));
    assert!(matches!(c.status, Level::Error) && c.summary.contains("继续使用之前的规则"), "{}", c.summary);
    assert!(policy_check(file(Err("坏了".into())), None).summary.contains("启动时会因此失败"));
    let mut failed = st.clone();
    failed.policy.as_mut().unwrap().last_error = Some(app_mcp_hub::PolicyLoadError { message: "id 重复".into(), at_ms: 2 });
    let c = policy_check(file(Ok(rules)), Some(&Ok(failed)));
    assert!(matches!(c.status, Level::Error) && c.summary.contains("id 重复"), "{}", c.summary);
    let mut empty = st;
    empty.policy = Some(Default::default());
    let c = policy_check(file(Ok(PolicyConfig::default())), Some(&Ok(empty)));
    assert!(matches!(c.status, Level::Ok) && c.summary.contains("默认放行"), "{}", c.summary);
    assert!(matches!(policy_check(file(Ok(PolicyConfig::default())), None).status, Level::Skip));
}

#[test]
fn agents_check_levels() {
    use app_mcp_hub::{AgentCredential, AgentsConfig};
    let two = AgentsConfig {
        agents: ["cursor", "claude"].map(|n| AgentCredential { name: n.into(), token: format!("{n}-{}", "0".repeat(40)) }).to_vec(),
    };
    let file = |config: Result<AgentsConfig, String>, mode: Option<u32>| AgentsFile { path: "/x/agents.json".into(), config, mode };
    let mut st = status_with_tools(0);
    st.agents = Some(vec!["claude".into(), "cursor".into()]);
    st.tasks = Some(vec![serde_json::from_value(json!({
        "id": "task-1", "caller": "principal:agent:claude", "kind": "principal", "agent": "claude",
        "selections": [], "leases": [], "inflight": 0
    })).unwrap()]);

    let c = agents_check(&file(Ok(two.clone()), Some(0o600)), Some(&Ok(st.clone())));
    assert!(matches!(c.status, Level::Info) && c.summary == "claude（1 个任务）、cursor（0 个任务）", "{}", c.summary);
    assert!(!c.details.to_string().contains("0000000000"), "不含令牌");
    let c = agents_check(&file(Ok(two.clone()), Some(0o644)), Some(&Ok(st.clone())));
    assert!(matches!(c.status, Level::Warn) && c.summary.contains("644"), "{}", c.summary);
    let c = agents_check(&file(Ok(AgentsConfig::default()), None), Some(&Ok(st.clone())));
    assert!(matches!(c.status, Level::Warn) && c.summary.contains("不一致"), "{}", c.summary);
    let c = agents_check(&file(Err("重复".into()), Some(0o600)), Some(&Ok(st.clone())));
    assert!(matches!(c.status, Level::Error) && c.summary.contains("继续使用之前的登记"), "{}", c.summary);
    assert!(agents_check(&file(Err("重复".into()), None), None).summary.contains("启动时会因此失败"));
    assert!(matches!(agents_check(&file(Ok(two.clone()), None), None).status, Level::Skip));
    assert!(matches!(agents_check(&file(Ok(AgentsConfig::default()), None), None).status, Level::Ok));
    let mut old = st.clone();
    old.agents = None;
    assert!(agents_check(&file(Ok(two), None), Some(&Ok(old))).summary.contains("不支持"));
    let mut empty = st;
    empty.agents = Some(Vec::new());
    assert!(matches!(agents_check(&file(Ok(AgentsConfig::default()), None), Some(&Ok(empty))).status, Level::Ok));
}

#[test]
fn limits_check_reports_policy_and_rejections() {
    let c = limits_check(Some(&Ok(status_with_tools(0))));
    assert!(matches!(c.status, Level::Warn), "超大 1 次也算");
    assert!(c.summary.contains("每工具 每分钟 120 次、突发 30 次；每 App 不限") && c.summary.contains("结果 不限 字节"), "{}", c.summary);
    assert!(c.summary.contains("shop：限流 0 次、超大 1 次") && c.summary.contains("不符时 log"), "{}", c.summary);
    let mut ok = status_with_tools(0);
    ok.apps[0].too_large = 0;
    let c = limits_check(Some(&Ok(ok)));
    assert!(matches!(c.status, Level::Ok) && c.summary.contains("没有被拒绝的调用"), "{}", c.summary);
    let mut old = status_with_tools(3);
    old.limits = None;
    assert!(matches!(limits_check(Some(&Ok(old))).status, Level::Skip));
}

fn status_with_error(code: Option<&str>) -> HubStatus {
    let mut st = status_with_tools(0);
    st.apps[0].last_error = code.map(|c| app_mcp_hub::LastError {
        code: Some(c.into()),
        message: format!("唤醒 App「shop」失败：{c}"),
        at_ms: 1,
    });
    st
}

#[test]
fn wake_check_hints_priority_on_wake_failures() {
    for code in ["LAUNCH_FAILED", "APP_NOT_RESPONDING"] {
        let c = wake_check(Some(&Ok(status_with_error(Some(code)))));
        assert!(matches!(c.status, Level::Warn), "{code}");
        assert!(c.summary.contains(&format!("shop：[{code}]")), "{}", c.summary);
        let hint = c.hint.as_deref().unwrap_or_default();
        assert!(hint.contains("优先级") && hint.contains("效率模式") && hint.contains("满载"), "{hint}");
    }
    for code in [None, Some("WAKE_RATE_LIMITED"), Some("PAIRING_REJECTED")] {
        let c = wake_check(Some(&Ok(status_with_error(code))));
        assert!(matches!(c.status, Level::Ok) && c.hint.is_none(), "{code:?}");
    }
    assert!(matches!(wake_check(None).status, Level::Skip));
    assert!(matches!(wake_check(Some(&Err("x".into()))).status, Level::Skip));
}

#[test]
fn report_render_and_errors() {
    let r = Report {
        version: "0",
        home: "/h".into(),
        checks: vec![
            Check::new("a", "甲", Level::Ok, "好"),
            Check::new("b", "乙", Level::Error, "坏").code(ConnectionErrorCode::PortBusy),
        ],
    };
    assert!(r.has_errors());
    let text = r.render();
    assert!(text.contains("[错误] 乙") && text.contains("PORT_BUSY") && text.contains("建议："), "{text}");
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["checks"][1]["status"], "error");
    assert_eq!(v["checks"][1]["code"], "PORT_BUSY");
}

#[cfg(unix)]
#[test]
fn ipc_check_reports_too_long_path() {
    let long = format!("unix:/{}", "p".repeat(app_mcp_protocol::endpoint::MAX_UNIX_SOCKET_PATH_BYTES));
    let c = ipc_check(Some(&long), false, None);
    assert!(matches!(c.status, Level::Error), "{c:?}");
    assert_eq!(c.code, Some("IPC_PATH_TOO_LONG"));
    assert!(c.hint.as_deref().is_some_and(|h| h.contains("--ipc-endpoint")), "{:?}", c.hint);
}

#[test]
fn ipc_check_reports_too_long_pipe_name() {
    let long = format!(r"pipe:\\.\pipe\{}", "p".repeat(app_mcp_protocol::endpoint::MAX_PIPE_NAME_CHARS));
    let c = ipc_check(Some(&long), false, None);
    assert!(matches!(c.status, Level::Error), "{c:?}");
    assert_eq!(c.code, Some("IPC_PATH_TOO_LONG"));
    assert!(c.hint.as_deref().is_some_and(|h| h.contains("--ipc-endpoint")), "{:?}", c.hint);
}

#[test]
fn power_line() {
    let p = InstancePower {
        reconnects: 2,
        wakes: 1,
        online_secs: 30,
        heartbeats: 0,
        heartbeat_ms: Some(0),
        lifecycle_mode: Some(app_mcp_protocol::LifecycleMode::Idle),
        awake_reasons: vec![AwakeReason::Lease, AwakeReason::Subscription],
    };
    let t = power_text(&p);
    assert!(t.contains("回连 2 次") && t.contains("无（靠连接断开）") && t.contains("模式 idle"), "{t}");
    assert!(t.contains("未休眠原因：租约、资源订阅"), "{t}");
    assert!(power_text(&InstancePower::default()).contains("双向（旧 SDK）"));
}

#[test]
fn port_state_text() {
    let other = PortState::Other {
        description: "HTTP 服务（HTTP/1.1 404）".into(),
        owner: Some(PortOwner { pid: Some(7), name: Some("nginx".into()), uid: None }),
    };
    assert!(describe_port_state("127.0.0.1:7717", &other).contains("nginx（pid 7）"));
    assert_eq!(
        serde_json::to_value(PortState::AppMcp { pid: 1, user: None, own: true }).unwrap()["kind"],
        "appMcp"
    );
}

/// 显式地址被占用：预检报告占用者（本进程），没有可用地址。
#[tokio::test]
async fn preflight_reports_owner() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    // 有连接进来就回一个非 app-mcp 的 HTTP 响应
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        while let Ok((mut s, _)) = l.accept().await {
            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
        }
    });
    let home = AppHome { dir: std::env::temp_dir().join("app-mcp-doctor-unused") };
    let file = crate::config::FileConfig { listen: Some(addr.clone()), ..Default::default() };
    let s = Settings::resolve(&file, &crate::config::Overrides::default(), &home).unwrap();
    let p = port_preflight(&s).await;
    assert_eq!(p.chosen, None);
    assert_eq!(p.busy.len(), 1);
    assert!(p.busy[0].1.contains("其他程序"), "{:?}", p.busy);
    #[cfg(any(target_os = "linux", windows))]
    assert!(p.busy[0].1.contains(&format!("pid {}", std::process::id())), "{:?}", p.busy);
}

#[test]
fn dormant_store_check_reports_skipped_files() {
    let n: u64 = rand::random();
    let state = std::env::temp_dir().join(format!("app-mcp-doctor-state-{}-{n:x}", std::process::id()));
    assert!(matches!(dormant_store_check(&state, None).status, Level::Info), "目录不存在");
    let dir = state.join("dormant");
    std::fs::create_dir_all(&dir).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let good = json!({"version": 1, "appId": "calc", "savedAtMs": now, "tools": [], "instances": [{
        "instanceId": "i1", "appName": "计算器", "clientKind": "native", "tools": [], "resumeToken": "r",
        "toolsHash": "h", "sleptAtMs": now, "connectedAtMs": now}]});
    std::fs::write(dir.join("calc.json"), good.to_string()).unwrap();
    let c = dormant_store_check(&state, None);
    assert!(matches!(c.status, Level::Ok), "{}", c.summary);
    assert!(c.summary.contains("1 个 App、1 个休眠实例"), "{}", c.summary);
    std::fs::write(dir.join("broken.json"), "{").unwrap();
    let c = dormant_store_check(&state, None);
    assert!(matches!(c.status, Level::Warn));
    assert!(c.summary.contains("broken.json"), "{}", c.summary);
    assert!(c.hint.is_some());
    std::fs::remove_dir_all(&state).unwrap();
}
