//! 端到端单元测试（续）：Agent 登记（spec/hub-api.md 3.6「Agent 身份」）。

use std::io::{Read, Write};
use std::net::TcpStream;

use super::*;

const CLAUDE: &str = "claude-0123456789abcdef0123456789abcdef";
const CURSOR: &str = "cursor-0123456789abcdef0123456789abcdef";

fn cred(name: &str, token: &str) -> AgentCredential {
    AgentCredential { name: name.into(), token: token.into() }
}

/// 经 `/mcp` 以 `token` 发无会话（2026-07-28）`apps.task.begin` → 任务 ID。
fn begin_task(hub: &AppMcpHub, token: &str) -> String {
    let addr = hub.listen_addr().expect("监听地址");
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "apps.task.begin", "arguments": {}, "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"}
        }}
    })
    .to_string();
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nMcp-Protocol-Version: 2026-07-28\r\nMcp-Method: tools/call\r\n\
         Mcp-Name: apps.task.begin\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut s = TcpStream::connect(&addr).expect("连接");
    s.set_read_timeout(Some(Duration::from_secs(10))).expect("超时");
    s.write_all(req.as_bytes()).expect("发送");
    let mut text = String::new();
    s.read_to_string(&mut text).expect("读取");
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("data:").map(str::trim).or(Some(l)).filter(|d| d.starts_with('{')))
        .unwrap_or_else(|| panic!("{text}"));
    let v: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {text}"));
    v["result"]["structuredContent"]["taskId"].as_str().unwrap_or_else(|| panic!("{v}")).to_owned()
}

fn task_agent(hub: &AppMcpHub, id: &str) -> Option<String> {
    let st = hub.status().expect("status");
    st.tasks.into_iter().flatten().find(|t| t.id == id).and_then(|t| t.agent)
}

/// 配置 agents → `/mcp` 出示其令牌的请求归该 Agent；`set_agents` 替换，不合法时保留之前的登记；
/// 配置不合法时 `start` 报 `InvalidConfig`（信息不含令牌）；`Debug` 不输出令牌。
#[test]
fn agents_config_and_set_agents() {
    assert!(!format!("{:?}", cred("claude", CLAUDE)).contains(CLAUDE));
    let bad = AppMcpHub::start(HubConfig {
        enable_listen: false,
        enable_ipc: false,
        agents: Some(vec![cred("a b", CLAUDE)]),
        ..HubConfig::default()
    });
    match bad {
        Err(HubError::InvalidConfig { detail }) => assert!(detail.contains("agents") && !detail.contains(CLAUDE), "{detail}"),
        other => panic!("应为 InvalidConfig：{:?}", other.err()),
    }

    let hub = AppMcpHub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        mcp_http: true,
        enable_ipc: false,
        agents: Some(vec![cred("claude", CLAUDE)]),
        ..HubConfig::default()
    })
    .expect("start");
    assert_eq!(hub.status().expect("status").agents, Some(vec!["claude".to_owned()]));
    let task = begin_task(&hub, CLAUDE);
    assert_eq!(task_agent(&hub, &task).as_deref(), Some("claude"));

    let e = hub.set_agents(vec![cred("a", CLAUDE), cred("a", CURSOR)]);
    assert!(matches!(&e, Err(HubError::Tool { kind, .. }) if kind == "INVALID_INPUT"), "{e:?}");
    assert_eq!(hub.status().expect("status").agents, Some(vec!["claude".to_owned()]));

    hub.set_agents(vec![cred("cursor", CURSOR)]).expect("替换");
    assert_eq!(hub.status().expect("status").agents, Some(vec!["cursor".to_owned()]));
    let task = begin_task(&hub, CURSOR);
    assert_eq!(task_agent(&hub, &task).as_deref(), Some("cursor"));
    hub.set_agents(Vec::new()).expect("清空");
    assert_eq!(hub.status().expect("status").agents, Some(Vec::new()));
    hub.shutdown();
}
