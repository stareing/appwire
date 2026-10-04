//! 探测：对监听地址发 `GET /healthz`，判断端口上是否是一个健康的 app-mcp Host（`service status`、
//! 端口被占用时说明占用者）。本配置目录的实例以登记文件为准，见 `lib.rs` 的 `running_instance`。
//!
//! [`fetch_status`]：读取运行中 Host 的 `GET /status`（`doctor`、`status`）——优先经本地 IPC（连接后核对监听方
//! 是同一用户，与原生 SDK 相同），IPC 关闭时经 TCP 携带令牌。[`post_policy`]：`POST /policy`（`policy reload`），[`post_agents`]：`POST /agents`（`agent add / remove`），同样的通道与授权。

use std::time::Duration;

use app_mcp_hub::http_server::{
    AGENTS_PATH, AgentsReply, HEALTH_PATH, INTENTS_PATH, IntentsReply, POLICY_PATH, PolicyReply, STATUS_PATH,
};
use app_mcp_hub::{Health, HubStatus};
use app_mcp_protocol::Endpoint;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
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
        Err(_) if bindable(addr) => return Probe::Free,
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

/// 连接超时时判断端口是否其实空闲：能绑定即没有监听者（随即释放）。
///
/// @why 部分环境（WSL 镜像网络、丢弃 SYN 的防火墙）连接无人监听的回环端口时不返回“拒绝”而是超时，
/// 仅凭超时会误判为“被其他程序占用”。只在超时这一含糊情形下做绑定检查。
fn bindable(addr: &str) -> bool {
    std::net::TcpListener::bind(addr).is_ok()
}

/// 发往运行中 Host 的一个请求（`GET /status`、`POST /policy`）。
#[derive(Clone, Copy, Debug)]
struct HostRequest<'a> {
    method: &'static str,
    path: &'static str,
    /// JSON 请求体（`POST`）。
    body: Option<&'a str>,
}

/// 在一条连接上发请求（`Connection: close`），读到连接关闭，返回 (状态码, 响应体)。
async fn http_request<S>(mut stream: S, host: &str, req: HostRequest<'_>, bearer: Option<&str>) -> Result<(u16, String), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let auth = bearer.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let body = req.body.unwrap_or_default();
    let content = match req.body {
        Some(b) => format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len()),
        None => String::new(),
    };
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\n{auth}{content}Connection: close\r\n\r\n{body}",
        req.method, req.path
    );
    stream.write_all(request.as_bytes()).await.map_err(|e| format!("发送请求失败：{e}"))?;
    let mut buf = Vec::new();
    tokio::time::timeout(READ_TIMEOUT, async {
        let mut chunk = [0u8; 8192];
        loop {
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > 8 * MAX_RESPONSE {
                        break;
                    }
                }
            }
        }
    })
    .await
    .map_err(|_| "读取响应超时".to_owned())?;
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n").ok_or("不是 HTTP 响应")?;
    let code = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or("不是 HTTP 响应")?;
    Ok((code, body.trim().to_owned()))
}

fn parse_status(code: u16, body: &str) -> Result<HubStatus, String> {
    if code != 200 {
        return Err(format!("HTTP {code}：{body}"));
    }
    serde_json::from_str(body).map_err(|e| format!("/status 响应无法解析：{e}"))
}

/// 经本地 IPC 端点读取 `/status`。连接后核对监听方是当前用户（防止他人抢占端点冒充 Host）。
pub async fn fetch_status_ipc(endpoint: &str) -> Result<HubStatus, String> {
    let (code, body) = request_ipc(endpoint, STATUS).await?;
    parse_status(code, &body)
}

const STATUS: HostRequest<'static> = HostRequest { method: "GET", path: STATUS_PATH, body: None };

/// 经本地 IPC 端点发请求。连接后核对监听方是当前用户。
async fn request_ipc(endpoint: &str, req: HostRequest<'_>) -> Result<(u16, String), String> {
    let ep = Endpoint::parse(endpoint)?;
    let fut = async {
        match ep {
            #[cfg(unix)]
            Endpoint::Unix(path) => {
                let stream = tokio::net::UnixStream::connect(&path)
                    .await
                    .map_err(|e| format!("连接 {} 失败：{e}", path.display()))?;
                let cred = stream.peer_cred().map_err(|e| format!("读取对端凭据失败：{e}"))?;
                let me = app_mcp_protocol::endpoint::current_uid();
                if cred.uid() != me {
                    return Err(format!("{} 的监听方属于其他用户（uid {}），拒绝连接", path.display(), cred.uid()));
                }
                http_request(stream, "localhost", req, None).await
            }
            #[cfg(windows)]
            Endpoint::Pipe(name) => {
                use app_mcp_protocol::endpoint::win;
                use std::os::windows::io::AsRawHandle;
                const ERROR_PIPE_BUSY: i32 = 231;
                let client = loop {
                    match tokio::net::windows::named_pipe::ClientOptions::new().open(&name) {
                        Ok(c) => break c,
                        Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                        Err(e) => return Err(format!("打开命名管道 {name} 失败：{e}")),
                    }
                };
                let owner = win::handle_owner_sid(client.as_raw_handle()).map_err(|e| format!("读取管道所有者失败：{e}"))?;
                let me = win::current_user_sid().map_err(|e| format!("读取当前用户失败：{e}"))?;
                if owner != me {
                    return Err(format!("命名管道 {name} 的所有者（{owner}）不是当前用户，拒绝连接"));
                }
                http_request(client, "localhost", req, None).await
            }
            other => Err(format!("不是本平台的本地 IPC 端点：{other}")),
        }
    };
    tokio::time::timeout(CONNECT_TIMEOUT + READ_TIMEOUT, fut)
        .await
        .map_err(|_| "超时".to_owned())?
}

/// 经 TCP 读取 `/status`（需要令牌）。
pub async fn fetch_status_tcp(addr: &str, token: Option<&str>) -> Result<HubStatus, String> {
    let (code, body) = request_tcp(addr, token, STATUS).await?;
    parse_status(code, &body)
}

async fn request_tcp(addr: &str, token: Option<&str>, req: HostRequest<'_>) -> Result<(u16, String), String> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| "连接超时".to_owned())?
        .map_err(|e| format!("连接 {addr} 失败：{e}"))?;
    http_request(stream, addr, req, token).await
}

/// 发往运行中 Host：有 IPC 端点时经 IPC，否则经 TCP 携带令牌。
async fn request_host(
    ipc_endpoint: Option<&str>,
    listen: Option<&str>,
    token: Option<&str>,
    req: HostRequest<'_>,
) -> Result<(u16, String), String> {
    match (ipc_endpoint, listen) {
        (Some(ep), _) => request_ipc(ep, req).await,
        (None, Some(addr)) => request_tcp(addr, token, req).await,
        (None, None) => Err("Host 既没有本地 IPC 端点也没有 TCP 监听".to_owned()),
    }
}

/// 读取运行中 Host 的 `/status`：有 IPC 端点时经 IPC，否则经 TCP 携带令牌。
pub async fn fetch_status(ipc_endpoint: Option<&str>, listen: Option<&str>, token: Option<&str>) -> Result<HubStatus, String> {
    let (code, body) = request_host(ipc_endpoint, listen, token, STATUS).await?;
    parse_status(code, &body)
}

/// 把策略规则（`policy.json` 的原文）交给运行中的 Host 替换（`POST /policy`）。Host 校验不合法时返回
/// `PolicyReply { ok: false, error }` 并保留之前的规则。
///
/// @error 连接失败、旧版 Host 不支持（404）、响应无法解析。
pub async fn post_policy(
    ipc_endpoint: Option<&str>,
    listen: Option<&str>,
    token: Option<&str>,
    rules_json: &str,
) -> Result<PolicyReply, String> {
    let req = HostRequest { method: "POST", path: POLICY_PATH, body: Some(rules_json) };
    let (code, body) = request_host(ipc_endpoint, listen, token, req).await?;
    if code == 404 {
        return Err("运行中的 Host 版本不支持重载策略规则（POST /policy），请升级后重启 Host".to_owned());
    }
    serde_json::from_str(&body).map_err(|_| format!("HTTP {code}：{body}"))
}

/// 把 Agent 登记（`agents.json` 的原文）交给运行中的 Host 替换（`POST /agents`，第 16 项 N5）。Host 校验不合法时返回
/// `AgentsReply { ok: false, error }` 并保留之前的登记。
///
/// @error 连接失败、旧版 Host 不支持（404）、响应无法解析。
pub async fn post_agents(
    ipc_endpoint: Option<&str>,
    listen: Option<&str>,
    token: Option<&str>,
    agents_json: &str,
) -> Result<AgentsReply, String> {
    let req = HostRequest { method: "POST", path: AGENTS_PATH, body: Some(agents_json) };
    let (code, body) = request_host(ipc_endpoint, listen, token, req).await?;
    if code == 404 {
        return Err("运行中的 Host 版本不支持 Agent 登记（POST /agents），请升级后重启 Host".to_owned());
    }
    serde_json::from_str(&body).map_err(|_| format!("HTTP {code}：{body}"))
}

/// 把意图默认表（`intents.json` 的原文）交给运行中的 Host 替换（`POST /intents`，spec/intents.md 第 4 节）。Host 校验不合法时
/// 返回 `IntentsReply { ok: false, error }` 并保留之前的默认表。
///
/// @error 连接失败、旧版 Host 不支持（404）、响应无法解析。
pub async fn post_intents(
    ipc_endpoint: Option<&str>,
    listen: Option<&str>,
    token: Option<&str>,
    intents_json: &str,
) -> Result<IntentsReply, String> {
    let req = HostRequest { method: "POST", path: INTENTS_PATH, body: Some(intents_json) };
    let (code, body) = request_host(ipc_endpoint, listen, token, req).await?;
    if code == 404 {
        return Err("运行中的 Host 版本不支持意图默认表（POST /intents），请升级后重启 Host".to_owned());
    }
    serde_json::from_str(&body).map_err(|_| format!("HTTP {code}：{body}"))
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

    /// 超时时的绑定检查：有监听者时不可绑定（不会误判为空闲）。
    #[test]
    fn bindable_detects_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        assert!(!bindable(&addr));
        assert!(bindable("127.0.0.1:0"));
    }

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
