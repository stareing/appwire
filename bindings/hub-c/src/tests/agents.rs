//! 单元测试（续）：Agent 登记（v19，spec/hub-api.md 3.6「Agent 身份」）。

use std::io::{Read, Write};
use std::net::TcpStream;

use super::*;

const CLAUDE: &str = "claude-0123456789abcdef0123456789abcdef";
const CURSOR: &str = "cursor-0123456789abcdef0123456789abcdef";

fn set_agents(hub: *mut AmHub, agents: &str) -> AmHubStatus {
    let a = c(agents);
    // SAFETY: 有效参数。
    unsafe { am_hub_set_agents(hub, a.as_ptr()) }
}

fn status(hub: *mut AmHub) -> Value {
    // SAFETY: 有效参数。
    query_json(|o| unsafe { am_hub_status_json(hub, o) })
}

/// 经 `/mcp` 以 `token` 发无会话（2026-07-28）`apps.task.begin` → 任务 ID。
fn begin_task(hub: *mut AmHub, token: &str) -> String {
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
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
    s.set_read_timeout(Some(WAIT)).expect("超时");
    s.write_all(req.as_bytes()).expect("发送");
    let mut text = String::new();
    s.read_to_string(&mut text).expect("读取");
    let json_line = text
        .lines()
        .find_map(|l| l.strip_prefix("data:").map(str::trim).or(Some(l)).filter(|d| d.starts_with('{')))
        .unwrap_or_else(|| panic!("{text}"));
    let v: Value = serde_json::from_str(json_line).unwrap_or_else(|e| panic!("{e}: {text}"));
    v["result"]["structuredContent"]["taskId"].as_str().unwrap_or_else(|| panic!("{v}")).to_owned()
}

/// 配置 agents → `/mcp` 出示其令牌的请求归该 Agent；status 只列名字不含令牌；am_hub_set_agents 替换，
/// 不合法时保留之前的登记；配置不合法时 am_hub_start 报 InvalidConfig。
#[test]
fn agents_config_and_set_agents() {
    let bad = c(r#"{"listen":null,"ipcEndpoint":null,"agents":[{"name":"a b","token":"0123456789abcdef0123456789abcdef"}]}"#);
    let mut out = ptr::null_mut();
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_start(bad.as_ptr(), &mut out) }, AmHubStatus::InvalidConfig);
    assert!(out.is_null());
    assert!(last_error().contains("agents"), "{}", last_error());

    let hub = start_hub(&format!(r#"{{"listen":"127.0.0.1:0","mcpHttp":true,"agents":[{{"name":"claude","token":"{CLAUDE}"}}]}}"#));
    let st = status(hub);
    assert_eq!(st["agents"], json!(["claude"]), "{st}");
    assert!(!st.to_string().contains(CLAUDE), "status 不含令牌");

    let task = begin_task(hub, CLAUDE);
    let st = status(hub);
    let t = st["tasks"].as_array().into_iter().flatten().find(|t| t["id"] == task.as_str()).cloned();
    let t = t.unwrap_or_else(|| panic!("{st}"));
    assert_eq!(t["agent"], "claude", "{t}");

    // 不合法 / 结构不符：报错，之前的登记继续生效
    assert_eq!(set_agents(hub, "{"), AmHubStatus::InvalidJson);
    assert_eq!(set_agents(hub, r#"{"agents":[]}"#), AmHubStatus::InvalidJson);
    let dup = format!(r#"[{{"name":"a","token":"{CLAUDE}"}},{{"name":"a","token":"{CURSOR}"}}]"#);
    assert_eq!(set_agents(hub, &dup), AmHubStatus::InvalidConfig);
    assert!(last_error().contains("重复") && !last_error().contains(CLAUDE), "{}", last_error());
    assert_eq!(status(hub)["agents"], json!(["claude"]));

    // 替换：新 Agent 生效
    let next = format!(r#"[{{"name":"cursor","token":"{CURSOR}"}}]"#);
    assert_eq!(set_agents(hub, &next), AmHubStatus::Ok, "{}", last_error());
    assert_eq!(status(hub)["agents"], json!(["cursor"]));
    let task = begin_task(hub, CURSOR);
    let st = status(hub);
    let t = st["tasks"].as_array().into_iter().flatten().find(|t| t["id"] == task.as_str()).cloned();
    assert_eq!(t.map(|t| t["agent"].clone()), Some(json!("cursor")), "{st}");

    assert_eq!(set_agents(hub, "[]"), AmHubStatus::Ok);
    assert_eq!(status(hub)["agents"], json!([]));
    // SAFETY: NULL 参数。
    assert_eq!(unsafe { am_hub_set_agents(hub, ptr::null()) }, AmHubStatus::InvalidArgument);
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}
