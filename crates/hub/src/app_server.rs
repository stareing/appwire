//! App 连接服务：`/app` 上的 WebSocket 连接（回环 TCP 与本地 IPC）、握手、消息分发、心跳。
//!
//! 监听与 HTTP 路由在 [`crate::http_server`]；升级完成的 WebSocket 交给 [`handle_websocket`]。
//! 两种传输上跑的是同一套 WebSocket 帧与 JSON-RPC 消息（spec/protocol.md 第 1 节），只有鉴权不同：
//! TCP 连接只接受回环地址；IPC 连接的对端用户已在 [`crate::ipc`] 中核对过。
//!
//! 每个连接一个任务：读循环在本任务中执行，写操作通过 [`Connection`] 的通道交给独立的写任务。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use app_mcp_protocol::HelloParams;
use futures::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

use crate::connection::{Connection, Outgoing};
use crate::hub::HubShared;

mod handshake;
mod messages;
mod mux;
mod session;

use mux::{mux_request, run_mux};
use session::run_session;

/// 连接的对端。
#[derive(Clone, Copy, Debug)]
pub(crate) enum Peer {
    /// 回环 TCP（网页，或显式配置 `ws://` 的原生 App）。
    Tcp(SocketAddr),
    /// 本地 IPC（Unix 域套接字 / 命名管道），对端已确认是同一用户；`pid` 为对端进程号。
    Ipc { pid: Option<u32> },
    /// Hub 按名拨出的通道（spec/naming.md 第 3 节），对端已由连接器确认是同一用户；关闭时机由 Hub 决定（7.2）。
    Dialed { pid: Option<u32> },
}

impl Peer {
    pub(crate) fn pid(self) -> Option<u32> {
        match self {
            Peer::Tcp(_) => None,
            Peer::Ipc { pid } | Peer::Dialed { pid } => pid,
        }
    }

    fn dialed(self) -> bool {
        matches!(self, Peer::Dialed { .. })
    }
}

impl std::fmt::Display for Peer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Peer::Tcp(addr) => write!(f, "tcp {addr}"),
            Peer::Ipc { pid: Some(pid) } => write!(f, "ipc pid {pid}"),
            Peer::Ipc { pid: None } => f.write_str("ipc"),
            Peer::Dialed { pid: Some(pid) } => write!(f, "dialed pid {pid}"),
            Peer::Dialed { pid: None } => f.write_str("dialed"),
        }
    }
}

/// 已握手的实例身份。
struct Registered {
    app_id: String,
    instance_id: String,
    /// `app/hello.launchToken`：唤醒令牌（用于匹配等待中的唤醒）。
    launch_token: Option<String>,
    /// `app/hello.heartbeatMs`：SDK 的心跳声明（spec/lifecycle.md 第 11 节）；`None` = 旧 SDK。
    heartbeat_ms: Option<u64>,
    /// 在 Hub 按名拨出的通道上（spec/naming.md 第 3 节）。
    dialed: bool,
}

/// 声明了 `heartbeatMs > 0` 的 SDK：无消息断开至少等待的心跳间隔数。
const SDK_HEARTBEAT_MISSES: u32 = 3;

/// 握手后的心跳方式（spec/lifecycle.md 第 11 节 A3）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeartbeatMode {
    /// 旧 SDK（未声明 `heartbeatMs`）或 `HubConfig::legacy_heartbeat`：Hub 发 `ping`，按无消息断开。
    Legacy,
    /// SDK 不发心跳（本地传输）：Hub 也不发，不做无消息断开，靠连接断开（EOF）感知。
    Off,
    /// SDK 单向心跳：Hub 不发 `ping`，无消息断开取 `max(配置值, 3 × 间隔)`。
    Sdk(Duration),
}

impl HeartbeatMode {
    fn of(declared: Option<u64>, legacy: bool) -> Self {
        match declared {
            _ if legacy => Self::Legacy,
            None => Self::Legacy,
            Some(0) => Self::Off,
            Some(ms) => Self::Sdk(Duration::from_millis(ms)),
        }
    }

    /// 握手后的无消息断开时长；`None` = 不做无消息断开。
    fn idle_timeout(self, configured: Duration) -> Option<Duration> {
        match self {
            Self::Legacy => Some(configured),
            Self::Off => None,
            Self::Sdk(interval) => Some(configured.max(interval.saturating_mul(SDK_HEARTBEAT_MISSES))),
        }
    }

    fn hub_pings(self) -> bool {
        self == Self::Legacy
    }
}

/// 等待 [`crate::PairingHandler`] 答复的握手。
struct PendingPairing {
    hello: HelloParams,
    token: String,
    rx: oneshot::Receiver<bool>,
}

enum Flow {
    Continue,
    Close,
    /// 握手成功，已登记实例。
    Paired(Registered),
    /// 已回复 `pending`，等待配对结果。
    Pending(Box<PendingPairing>),
}

/// 一条已完成 WebSocket 握手的 App 连接（[`crate::http_server::Router`] 在 `/app` 升级后调用）。
/// `origin` 为升级请求的 `Origin` 头。
pub(crate) async fn handle_websocket<S>(
    shared: Arc<HubShared>,
    ws: tokio_tungstenite::WebSocketStream<S>,
    origin: Option<String>,
    peer: Peer,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sink, stream) = ws.split();
    let mut frames = text_frames(stream);

    // 第一条消息决定连接模式：`app/mux` → 多路复用（spec/protocol.md 第 9 节），否则为单实例连接。
    let first = match tokio::time::timeout(shared.config.idle_timeout, frames.next()).await {
        Ok(Some(text)) => text,
        Ok(None) => return,
        Err(_) => {
            tracing::debug!(%peer, "连接建立后没有收到任何消息，断开");
            return;
        }
    };
    if let Some(req) = mux_request(&first) {
        run_mux(shared, sink, frames, origin, peer, req).await;
        return;
    }

    let (conn, mut rx) = shared.new_connection();
    tracing::debug!(%peer, cid = %conn.cid, ?origin, "新连接");
    let writer = tokio::spawn(async move {
        let mut sink = sink;
        while let Some(out) = rx.recv().await {
            match out {
                Outgoing::Text(text) => {
                    if sink.send(WsMessage::text(text)).await.is_err() {
                        break;
                    }
                }
                Outgoing::Close => break,
            }
        }
        let _ = sink.close().await;
    });
    let inbound = futures::stream::once(std::future::ready(first)).chain(frames);
    run_session(shared, conn, inbound, origin, peer).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), writer).await;
}

/// Hub 按名拨出的通道（[`crate::connector::Connector::dial`]）：SDK 在其上发 WebSocket 升级请求（`/app`，
/// spec/naming.md 第 3 节"帧不变"），完成升级后与其他 App 连接走同一个会话。
pub(crate) async fn handle_dialed_channel(shared: Arc<HubShared>, channel: crate::connector::DialedChannel) {
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request as WsRequest, Response as WsResponse};
    let peer = Peer::Dialed { pid: channel.pid };
    #[allow(clippy::result_large_err)] // @why 签名由 tungstenite 的握手 Callback 规定（Err 为 http::Response）
    let check_path = |req: &WsRequest, resp: WsResponse| -> Result<WsResponse, ErrorResponse> {
        let path = req.uri().path();
        if path == app_mcp_protocol::APP_PATH || path == "/" {
            return Ok(resp);
        }
        let mut err = ErrorResponse::new(Some(format!("通道上只接受 {} 的升级请求", app_mcp_protocol::APP_PATH)));
        *err.status_mut() = http::StatusCode::NOT_FOUND;
        Err(err)
    };
    let accept = tokio_tungstenite::accept_hdr_async_with_config(
        channel.stream,
        check_path,
        Some(crate::http_server::app_ws_config()),
    );
    match tokio::time::timeout(shared.config.idle_timeout, accept).await {
        Ok(Ok(ws)) => handle_websocket(shared, ws, None, peer).await,
        Ok(Err(e)) => tracing::info!(%peer, "按名拨出的通道上 WebSocket 升级失败：{e}"),
        Err(_) => tracing::info!(%peer, "按名拨出的通道上没有及时收到升级请求，关闭"),
    }
}

/// 按名拨入的通道何时检查关闭（spec/naming.md 7.2）：有进行中调用或待派唤醒时隔一个宽限再查；否则为
/// `max(最后一条消息 + 宽限, 租约到期)`。
fn channel_close_at(shared: &HubShared, conn: &Connection, reg: &Registered, last_rx: Instant) -> Instant {
    let grace = shared.config.channel_grace;
    if channel_busy(shared, conn, reg) {
        return Instant::now() + grace;
    }
    let lease = shared.lease_expiry(conn.id);
    (last_rx + grace).max(lease.unwrap_or(last_rx))
}

fn channel_busy(shared: &HubShared, conn: &Connection, reg: &Registered) -> bool {
    conn.inflight() > 0 || shared.has_pending_wake(&reg.app_id, &reg.instance_id)
}

/// WebSocket 消息流 → 文本消息流：Close 帧或读取出错时结束，二进制帧记录警告后忽略。
fn text_frames<St>(stream: St) -> futures::stream::BoxStream<'static, String>
where
    St: futures::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Send + 'static,
{
    stream
        .take_while(|m| {
            let go = match m {
                Ok(m) => !m.is_close(),
                Err(e) => {
                    tracing::debug!("读取失败：{e}");
                    false
                }
            };
            std::future::ready(go)
        })
        .filter_map(|m| {
            std::future::ready(match m {
                Ok(WsMessage::Text(text)) => Some(text.as_str().to_owned()),
                Ok(WsMessage::Binary(_)) => {
                    tracing::warn!("忽略二进制消息");
                    None
                }
                _ => None,
            })
        })
        .boxed()
}

fn parse_params<T: DeserializeOwned>(params: &Value) -> Result<T, String> {
    let v = if params.is_null() {
        json!({})
    } else {
        params.clone()
    };
    serde_json::from_value(v).map_err(|e| e.to_string())
}
