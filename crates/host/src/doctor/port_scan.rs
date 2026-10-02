//! 监听端口探测：候选地址、端口占用者说明，以及 `service install` 之前的端口预检。

use app_mcp_protocol::LISTEN_CANDIDATE_PORTS;
use serde::Serialize;

use crate::config::Settings;
use crate::ports::{self, PortOwner};
use crate::probe::{self, Probe};

/// 一个端口上的情况。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PortState {
    Free,
    /// app-mcp Host（`own` = 本配置目录登记的实例）。
    AppMcp { pid: u32, user: Option<String>, own: bool },
    /// 其他程序。
    Other { description: String, owner: Option<PortOwner> },
}

/// 探测 `addr`（`host:port`）上的情况；`own_pid` 为本配置目录运行中实例的进程号。
pub async fn port_state(addr: &str, own_pid: Option<u32>) -> PortState {
    match probe::probe(addr).await {
        Probe::Free => PortState::Free,
        Probe::AppMcp(h) => PortState::AppMcp {
            pid: h.identity.pid,
            user: h.identity.user.clone(),
            own: own_pid == Some(h.identity.pid),
        },
        Probe::Other(description) => PortState::Other { description, owner: port_owner(addr) },
    }
}

/// 监听 `addr` 端口的进程（查询失败时为 `None`）。
pub fn port_owner(addr: &str) -> Option<PortOwner> {
    let port = addr.rsplit_once(':')?.1.parse().ok()?;
    ports::listening_owner(port).ok().flatten()
}

/// 端口占用者的一句话说明。
pub fn describe_port_state(addr: &str, st: &PortState) -> String {
    match st {
        PortState::Free => format!("{addr} 空闲"),
        PortState::AppMcp { pid, own: true, .. } => format!("{addr} 由本配置目录的 Host 监听（pid {pid}）"),
        PortState::AppMcp { pid, user, own: false } => format!(
            "{addr} 上是另一个 app-mcp Host（pid {pid}，用户 {}）：它使用不同的配置目录（--home / APP_MCP_HOME），或属于其他用户",
            user.as_deref().unwrap_or("未知")
        ),
        PortState::Other { description, owner } => match owner {
            Some(o) => format!("{addr} 被其他程序占用：{o}（{description}）"),
            None => format!("{addr} 被其他程序占用（{description}；无法查到进程）"),
        },
    }
}

fn host_of(addr: &str) -> &str {
    addr.rsplit_once(':').map(|(h, _)| h).unwrap_or("127.0.0.1")
}

/// 要检查的监听地址：显式配置时只有它；否则默认端口与备选端口。
pub fn candidate_addrs(s: &Settings) -> Vec<String> {
    if s.listen_explicit {
        // 端口 0（随机分配，测试用）没有可探测的固定地址。
        let addr = crate::probe_addr(&s.listen);
        return if addr.ends_with(":0") { Vec::new() } else { vec![addr] };
    }
    let host = host_of(&s.listen).to_owned();
    LISTEN_CANDIDATE_PORTS.iter().map(|p| format!("{host}:{p}")).collect()
}

/// `service install` 之前的端口检查结果。
#[derive(Debug, PartialEq, Eq)]
pub struct PortPreflight {
    /// Host 将会使用的地址（第一个空闲的候选）；全部被占用时为 `None`。
    pub chosen: Option<String>,
    /// 被占用的候选及说明。
    pub busy: Vec<(String, String)>,
}

/// 检查监听地址：依次探测候选地址，直到找到空闲的。
pub async fn port_preflight(s: &Settings) -> PortPreflight {
    let candidates = candidate_addrs(s);
    if candidates.is_empty() {
        return PortPreflight { chosen: Some(s.listen.clone()), busy: Vec::new() };
    }
    let mut busy = Vec::new();
    for addr in candidates {
        let st = port_state(&addr, None).await;
        if st == PortState::Free {
            return PortPreflight { chosen: Some(addr), busy };
        }
        let d = describe_port_state(&addr, &st);
        busy.push((addr, d));
    }
    PortPreflight { chosen: None, busy }
}
