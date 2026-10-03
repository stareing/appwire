//! 集成测试共用：经裸 HTTP/1.1 向 Hub 的 `/mcp` 发无会话（2026-07-28）请求（可带 Agent 令牌）。
//! 用法：`#[path = "support/mcp_http.rs"] mod mcp_http;`。

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const T: Duration = Duration::from_secs(10);

pub struct Reply {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Reply {
    /// JSON-RPC 响应（JSON 或 SSE 的第一条 `data:`）。
    pub fn json(&self) -> Value {
        let text = self
            .body
            .lines()
            .find_map(|l| l.strip_prefix("data:").map(str::trim).filter(|d| d.starts_with('{')))
            .unwrap_or(self.body.trim());
        serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }
}

pub async fn http(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(T, s.read_to_end(&mut buf)).await.unwrap().unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())))
        .collect();
    Reply { status, headers, body: body.to_owned() }
}

/// 无会话（2026-07-28）请求；`token` 为 `None` 时不带 `Authorization`。`name`：`Mcp-Name` 头（工具名 / 资源 URI）。
pub async fn modern_request(addr: SocketAddr, token: Option<&str>, method: &str, name: &str, mut params: Value) -> Reply {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "spoofed-claude", "version": "1"}
    });
    let req = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let auth = token.map(|t| format!("Bearer {t}"));
    let mut headers = vec![
        ("content-type", "application/json"),
        ("accept", "application/json, text/event-stream"),
        ("mcp-protocol-version", "2026-07-28"),
        ("mcp-method", method),
    ];
    if !name.is_empty() {
        headers.push(("mcp-name", name));
    }
    if let Some(a) = &auth {
        headers.push(("authorization", a));
    }
    http(addr, "POST", "/mcp", &headers, &req).await
}

/// 无会话 `tools/call`。
pub async fn modern_call(addr: SocketAddr, token: Option<&str>, tool: &str, args: Value) -> Reply {
    modern_request(addr, token, "tools/call", tool, json!({"name": tool, "arguments": args})).await
}
