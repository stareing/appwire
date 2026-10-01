//! 探测：对监听地址发 `GET /healthz`，判断端口上是否是一个健康的 app-mcp Host（`service status`、
//! 端口被占用时说明占用者）。本配置目录的实例以登记文件为准，见 `lib.rs` 的 `running_instance`。

use std::time::Duration;

use app_mcp_hub::Health;
use app_mcp_hub::http_server::HEALTH_PATH;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// 探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// 没有程序在监听。
    Free,
    /// 一个健康的 app-mcp 实例。
    AppMcp(Health),
    /// 端口被其他程序占用（附简短说明）。
    Other(String),
}

// Windows 上连接未监听的回环端口约 2 秒后才返回“拒绝”。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_RESPONSE: usize = 64 * 1024;

/// 探测 `addr`（`host:port`）。
pub async fn probe(addr: &str) -> Probe {
    let mut stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => return Probe::Free,
        Ok(Err(e)) => return Probe::Other(format!("连接失败：{e}")),
        Err(_) => return Probe::Other("连接超时".into()),
    };
    let request = format!(
        "GET {HEALTH_PATH} HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    if let Err(e) = stream.write_all(request.as_bytes()).await {
        return Probe::Other(format!("发送探测请求失败：{e}"));
    }
    let mut buf = Vec::new();
    let read = tokio::time::timeout(READ_TIMEOUT, async {
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > MAX_RESPONSE {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
    .await;
    if read.is_err() && buf.is_empty() {
        return Probe::Other("没有 HTTP 响应".into());
    }
    parse_response(&buf)
}

/// 解析 `/healthz` 的 HTTP 响应（只支持 Content-Length / 读到 EOF 的响应体）。
pub fn parse_response(raw: &[u8]) -> Probe {
    let text = String::from_utf8_lossy(raw);
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return Probe::Other("不是 HTTP 响应".into());
    };
    let status = head.lines().next().unwrap_or_default();
    if !status.starts_with("HTTP/1.") {
        return Probe::Other("不是 HTTP 响应".into());
    }
    if status.split_whitespace().nth(1) != Some("200") {
        return Probe::Other(format!("HTTP 服务（{status}）"));
    }
    match serde_json::from_str::<Health>(body.trim()) {
        Ok(h) if h.is_app_mcp() => Probe::AppMcp(h),
        _ => Probe::Other("HTTP 服务，但 /healthz 不是 app-mcp".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_health() {
        let body = r#"{"service":"app-mcp","version":"0.1.0","user":"1000","pid":42,"listen":"127.0.0.1:7717","appPath":"/app","mcpPath":"/mcp","tokenRequiredForBrowsers":true}"#;
        let raw = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        let Probe::AppMcp(h) = parse_response(raw.as_bytes()) else {
            panic!("应识别为 app-mcp");
        };
        assert_eq!(h.identity.pid, 42);
        assert_eq!(h.identity.user.as_deref(), Some("1000"));
        assert_eq!(h.listen.as_deref(), Some("127.0.0.1:7717"));
    }

    #[test]
    fn rejects_others() {
        assert!(matches!(
            parse_response(b"HTTP/1.1 404 Not Found\r\n\r\nnope"),
            Probe::Other(_)
        ));
        assert!(matches!(
            parse_response(b"HTTP/1.1 200 OK\r\n\r\n{\"service\":\"x\"}"),
            Probe::Other(_)
        ));
        assert!(matches!(
            parse_response(b"SSH-2.0-OpenSSH"),
            Probe::Other(_)
        ));
    }
}
