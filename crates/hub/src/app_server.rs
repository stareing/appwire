//! App 连接服务：WebSocket 监听（回环 TCP 与本地 IPC）、握手、消息分发、心跳。
//!
//! 两种传输上跑的是同一套 WebSocket 帧与 JSON-RPC 消息（spec/protocol.md 第 1 节），只有鉴权不同：
//! TCP 连接只接受回环地址；IPC 连接的对端用户已在 [`crate::ipc`] 中核对过。
//!
//! 每个连接一个任务：读循环在本任务中执行，写操作通过 [`Connection`] 的通道交给独立的写任务。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use app_mcp_manifest::is_reserved_app_id;
use app_mcp_protocol::{
    ErrorKind, HelloParams, HelloResult, MUX_MAX_CHANNELS, MUX_VERSION, Message, MuxFrame, MuxParams,
    MuxResult, Notification, PROTOCOL_VERSION,
    PairingResultParams, PairingStatus, Request, ResourceUpdatedParams, ResourcesChangedParams,
    ResourcesSyncParams, RpcError, SleepParams, SleepResult, ToolError, ToolsChangedParams,
    ToolsSyncParams, Visibility, VisibilityParams, is_valid_app_id, method,
};
use futures::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, interval_at, sleep_until};
use tokio_tungstenite::tungstenite::handshake::server::{
    Callback, ErrorResponse, Request as HttpRequest, Response as HttpResponse,
};
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

use crate::connection::{Connection, Outgoing};
use crate::hub::HubShared;
use crate::lifecycle::SLEEP_RETRY_AFTER_MS;
use crate::registry::{DormantInstance, NewInstance, client_kind_str};
use crate::types::{HubEvent, PairingRequest};

const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 连接的对端。
#[derive(Clone, Copy, Debug)]
pub(crate) enum Peer {
    /// 回环 TCP（网页，或显式配置 `ws://` 的原生 App）。
    Tcp(SocketAddr),
    /// 本地 IPC（Unix 域套接字 / 命名管道），对端已确认是同一用户；`pid` 为对端进程号。
    Ipc { pid: Option<u32> },
}

impl Peer {
    fn pid(self) -> Option<u32> {
        match self {
            Peer::Tcp(_) => None,
            Peer::Ipc { pid } => pid,
        }
    }
}

impl std::fmt::Display for Peer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Peer::Tcp(addr) => write!(f, "tcp {addr}"),
            Peer::Ipc { pid: Some(pid) } => write!(f, "ipc pid {pid}"),
            Peer::Ipc { pid: None } => f.write_str("ipc"),
        }
    }
}

/// 接受 TCP 连接，直到任务被中止。
pub(crate) async fn accept_loop(shared: Arc<HubShared>, listener: TcpListener) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                tokio::spawn(handle_connection(shared.clone(), stream, Peer::Tcp(addr)));
            }
            Err(e) => {
                tracing::warn!("接受连接失败：{e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// 接受本地 IPC 连接，直到任务被中止（中止时监听器被丢弃，Unix 上删除套接字文件）。
pub(crate) async fn accept_ipc_loop(shared: Arc<HubShared>, mut listener: crate::ipc::IpcListener) {
    loop {
        match listener.accept().await {
            Ok(a) => {
                tokio::spawn(handle_connection(shared.clone(), a.stream, Peer::Ipc { pid: a.pid }));
            }
            Err(e) => {
                tracing::warn!("接受 IPC 连接失败：{e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// 已握手的实例身份。
struct Registered {
    app_id: String,
    instance_id: String,
    /// `app/hello.launchToken`：唤醒令牌（用于匹配等待中的唤醒）。
    launch_token: Option<String>,
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

/// 握手回调：只记录请求的 `Origin` 头，从不拒绝握手（来源校验在 `app/hello` 阶段进行）。
///
/// 回调签名由 tungstenite 的 [`Callback`] trait 固定（错误类型 `ErrorResponse` 是约 136 字节的
/// `http::Response`）；以具名类型实现该 trait，签名归 trait 所有，本处从不构造错误值。
struct CaptureOrigin<'a>(&'a mut Option<String>);

impl Callback for CaptureOrigin<'_> {
    fn on_request(self, req: &HttpRequest, resp: HttpResponse) -> Result<HttpResponse, ErrorResponse> {
        *self.0 = req.headers().get("origin").and_then(|v| v.to_str().ok()).map(str::to_owned);
        Ok(resp)
    }
}

async fn handle_connection<S>(shared: Arc<HubShared>, stream: S, peer: Peer)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut origin: Option<String> = None;
    let ws = match tokio_tungstenite::accept_hdr_async(stream, CaptureOrigin(&mut origin)).await {
        Ok(ws) => ws,
        Err(e) => {
            tracing::debug!(%peer, "WebSocket 握手失败：{e}");
            return;
        }
    };
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

    let (conn, mut rx) = Connection::new(shared.next_id());
    tracing::debug!(%peer, conn = conn.id, ?origin, "新连接");
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

/// 连接上的第一条消息是否为 `app/mux` 请求。
fn mux_request(text: &str) -> Option<Request> {
    match Message::parse(text) {
        Ok(Message::Request(req)) if req.method == method::MUX => Some(req),
        _ => None,
    }
}

/// 多路复用连接（spec/protocol.md 第 9 节）：每个通道各跑一个 [`run_session`]，与独立连接完全相同；
/// 本函数只负责拆帧 / 装帧、通道开关与连接级空闲超时。
async fn run_mux<Sk>(
    shared: Arc<HubShared>,
    mut sink: Sk,
    mut frames: futures::stream::BoxStream<'static, String>,
    origin: Option<String>,
    peer: Peer,
    req: Request,
) where
    Sk: futures::Sink<WsMessage> + Unpin + Send + 'static,
{
    let params: MuxParams = match parse_params(&req.params) {
        Ok(p) => p,
        Err(e) => {
            let err = RpcError::invalid_params(format!("app/mux 参数无效：{e}"));
            let _ = sink.send(WsMessage::text(Message::error(req.id, err).to_json())).await;
            let _ = sink.close().await;
            return;
        }
    };
    let max_channels = MUX_MAX_CHANNELS;
    let result = MuxResult { version: params.version.clamp(1, MUX_VERSION), max_channels };
    let reply = Message::result(req.id, serde_json::to_value(result).unwrap_or(Value::Null));
    if sink.send(WsMessage::text(reply.to_json())).await.is_err() {
        return;
    }
    tracing::debug!(%peer, ?origin, "多路复用连接");

    // 所有通道的出站帧汇入同一个写任务。
    let (sock_tx, mut sock_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(text) = sock_rx.recv().await {
            if sink.send(WsMessage::text(text)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // 通道号 → (代次, 入站发送端)。代次用于区分同号通道的先后两次打开。
    let mut channels: HashMap<u32, (u64, mpsc::UnboundedSender<String>)> = HashMap::new();
    let (ended_tx, mut ended_rx) = mpsc::unbounded_channel::<(u32, u64)>();
    let idle = shared.config.hidden_idle_timeout.max(shared.config.idle_timeout);
    let mut last_rx = Instant::now();

    loop {
        tokio::select! {
            text = frames.next() => {
                let Some(text) = text else { break };
                last_rx = Instant::now();
                let frame = match MuxFrame::parse(&text) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(%peer, "无法解析的多路复用帧：{e}");
                        continue;
                    }
                };
                match frame {
                    MuxFrame::Open { ch } => {
                        if ch == 0 || channels.contains_key(&ch) {
                            tracing::warn!(%peer, ch, "通道号无效或已打开，忽略 open");
                            continue;
                        }
                        if channels.len() >= max_channels as usize {
                            let _ = sock_tx.send(MuxFrame::close_text(ch, Some("通道数已达上限")));
                            continue;
                        }
                        let (in_tx, in_rx) = mpsc::unbounded_channel::<String>();
                        let (conn, out_rx) = Connection::new(shared.next_id());
                        tracing::debug!(%peer, ch, conn = conn.id, ?origin, "新通道");
                        channels.insert(ch, (conn.id, in_tx));
                        let ctx = ChannelCtx {
                            shared: shared.clone(),
                            sock_tx: sock_tx.clone(),
                            ended_tx: ended_tx.clone(),
                            origin: origin.clone(),
                            peer,
                        };
                        tokio::spawn(run_channel(ctx, ch, conn, out_rx, in_rx));
                    }
                    MuxFrame::Msg { ch, msg } => match channels.get(&ch) {
                        Some((_, tx)) => {
                            let _ = tx.send(msg.get().to_owned());
                        }
                        None => tracing::debug!(%peer, ch, "发往未打开通道的消息，丢弃"),
                    },
                    MuxFrame::Close { ch, .. } => {
                        // 丢弃发送端：该通道的会话读到流结束，按断开处理。
                        channels.remove(&ch);
                    }
                }
            }
            Some((ch, generation)) = ended_rx.recv() => {
                if channels.get(&ch).is_some_and(|(g, _)| *g == generation) {
                    channels.remove(&ch);
                }
            }
            // 没有通道时才计连接级空闲；有通道时由各通道自己的心跳与超时负责。
            _ = sleep_until(last_rx + idle), if channels.is_empty() => {
                tracing::debug!(%peer, "多路复用连接空闲，断开");
                break;
            }
        }
    }
    // 连接断开：丢弃全部入站发送端，各通道会话随之结束并各自清理。
    channels.clear();
    drop(sock_tx);
    drop(ended_tx);
    while ended_rx.recv().await.is_some() {}
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), writer).await;
}

/// 多路复用连接上所有通道共用的上下文。
struct ChannelCtx {
    shared: Arc<HubShared>,
    /// 连接写任务的输入（已装帧的文本）。
    sock_tx: mpsc::UnboundedSender<String>,
    /// 通道结束时报告 `(通道号, 代次)`。
    ended_tx: mpsc::UnboundedSender<(u32, u64)>,
    origin: Option<String>,
    peer: Peer,
}

/// 一个多路复用通道：出站消息装帧后交给连接的写任务，会话结束时发送 `close` 帧。
async fn run_channel(
    ctx: ChannelCtx,
    ch: u32,
    conn: Arc<Connection>,
    mut out_rx: mpsc::UnboundedReceiver<Outgoing>,
    in_rx: mpsc::UnboundedReceiver<String>,
) {
    let ChannelCtx { shared, sock_tx, ended_tx, origin, peer } = ctx;
    let generation = conn.id;
    let forward = {
        let sock_tx = sock_tx.clone();
        tokio::spawn(async move {
            while let Some(out) = out_rx.recv().await {
                match out {
                    Outgoing::Text(text) => match MuxFrame::wrap(ch, &text) {
                        Ok(frame) => {
                            if sock_tx.send(frame).is_err() {
                                break;
                            }
                        }
                        Err(e) => tracing::warn!(ch, "无法装帧的出站消息：{e}"),
                    },
                    Outgoing::Close => break,
                }
            }
            let _ = sock_tx.send(MuxFrame::close_text(ch, None));
        })
    };
    let inbound = futures::stream::unfold(in_rx, |mut rx| async move { rx.recv().await.map(|t| (t, rx)) }).boxed();
    run_session(shared, conn, inbound, origin, peer).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), forward).await;
    let _ = ended_tx.send((ch, generation));
}

/// 一个实例会话（独立连接或多路复用的一个通道）：握手、消息分发、心跳、空闲超时与断开清理。
/// `inbound` 结束即视为连接断开；会话结束时调用 [`Connection::close`]，由写端负责关闭底层连接 / 通道。
async fn run_session<I>(shared: Arc<HubShared>, conn: Arc<Connection>, mut inbound: I, origin: Option<String>, peer: Peer)
where
    I: futures::Stream<Item = String> + Unpin,
{
    let cfg = &shared.config;
    let mut ping = interval_at(Instant::now() + cfg.ping_interval, cfg.ping_interval);
    let mut last_rx = Instant::now();
    let mut registered: Option<Registered> = None;
    // 等待配对确认中的握手：(hello, token) 与结果通道。
    let mut pairing: Option<(HelloParams, String)> = None;
    let mut pairing_rx: Option<oneshot::Receiver<bool>> = None;

    loop {
        // 可见性只会随收到的消息变化，因此每轮按最近上报的可见性重新计算截止时间。
        let background = registered.as_ref().is_some_and(|r| {
            matches!(
                shared.registry().visibility_of(&r.app_id, conn.id),
                Some(Visibility::Hidden | Visibility::Frozen)
            )
        });
        let idle = if background {
            cfg.hidden_idle_timeout
        } else {
            cfg.idle_timeout
        };
        tokio::select! {
            text = inbound.next() => {
                let Some(text) = text else { break };
                last_rx = Instant::now();
                let pending = pairing_rx.is_some();
                match handle_text(&shared, &conn, registered.as_ref(), pending, origin.as_deref(), peer, &text) {
                    Flow::Continue => {}
                    Flow::Close => break,
                    Flow::Paired(r) => registered = Some(r),
                    Flow::Pending(p) => {
                        pairing = Some((p.hello, p.token));
                        pairing_rx = Some(p.rx);
                    }
                }
            }
            approved = async {
                match pairing_rx.as_mut() {
                    Some(rx) => rx.await.unwrap_or(false),
                    None => std::future::pending().await,
                }
            }, if pairing_rx.is_some() => {
                pairing_rx = None;
                let Some((hello, token)) = pairing.take() else { break };
                if !approved {
                    tracing::info!(conn = conn.id, app_id = %hello.app_id, "配对被拒绝");
                    send_pairing_result(&conn, PairingStatus::Rejected, None, Some("用户拒绝了配对请求".to_owned()));
                    break;
                }
                shared.remember_pairing(&hello.app_id, origin.as_deref(), &token, true);
                send_pairing_result(&conn, PairingStatus::Paired, Some(token), None);
                registered = Some(register_instance_with(&shared, &conn, hello, origin.as_deref(), peer, None));
                last_rx = Instant::now();
            }
            // 等待配对确认期间 SDK 不发心跳，暂停空闲超时（配对本身有 pairing_timeout）。
            _ = sleep_until(last_rx + idle), if pairing_rx.is_none() => {
                tracing::info!(conn = conn.id, background, "{} 秒内没有收到消息，断开连接", idle.as_secs());
                break;
            }
            _ = ping.tick(), if registered.is_some() => {
                // 心跳：响应由 Connection 丢弃；存活判断只看是否收到消息。
                if let Ok((id, rx)) = conn.start_request(method::PING, Value::Null) {
                    drop(rx);
                    let conn = conn.clone();
                    let timeout = cfg.hidden_idle_timeout.max(cfg.idle_timeout);
                    tokio::spawn(async move {
                        tokio::time::sleep(timeout).await;
                        conn.forget(&id);
                    });
                }
            }
            _ = conn.shutdown_requested() => {
                tracing::debug!(conn = conn.id, "连接被关闭（被新连接替换）");
                break;
            }
        }
    }

    // 清理
    if let Some(reg) = registered {
        let removed = shared.registry().remove_instance(&reg.app_id, conn.id);
        if removed.is_some() {
            tracing::info!(app_id = %reg.app_id, instance_id = %reg.instance_id, "实例断开");
            shared.emit(HubEvent::AppDisconnected {
                app_id: reg.app_id.clone(),
                instance_id: reg.instance_id.clone(),
            });
            shared.mark_tools_changed();
            shared.mark_resources_changed();
        }
    }
    conn.fail_all();
    conn.close();
}

fn parse_params<T: DeserializeOwned>(params: &Value) -> Result<T, String> {
    let v = if params.is_null() {
        json!({})
    } else {
        params.clone()
    };
    serde_json::from_value(v).map_err(|e| e.to_string())
}

fn handle_text(
    shared: &Arc<HubShared>,
    conn: &Arc<Connection>,
    registered: Option<&Registered>,
    pending: bool,
    origin: Option<&str>,
    peer: Peer,
    text: &str,
) -> Flow {
    let msg = match Message::parse(text) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(conn = conn.id, "无法解析的消息：{e}");
            return Flow::Continue;
        }
    };
    match msg {
        Message::Request(req) => match registered {
            None if pending => {
                if req.method == method::PING {
                    conn.send(&Message::result(req.id, json!({})));
                } else {
                    let err: RpcError =
                        ToolError::new(ErrorKind::Unauthorized, "正在等待用户确认配对。").into();
                    conn.send(&Message::error(req.id, err));
                }
                Flow::Continue
            }
            None => handle_handshake_request(shared, conn, origin, peer, req),
            Some(reg) => {
                handle_request(shared, conn, reg, req);
                Flow::Continue
            }
        },
        Message::Notification(n) => {
            match registered {
                None => tracing::warn!(conn = conn.id, method = %n.method, "握手前的通知，忽略"),
                Some(reg) => handle_notification(shared, conn, reg, n),
            }
            Flow::Continue
        }
        Message::Response(resp) => {
            if !conn.resolve(resp) {
                tracing::warn!(conn = conn.id, "未知 ID 的响应，忽略");
            }
            Flow::Continue
        }
    }
}

fn reject(conn: &Connection, req: &Request, reason: String) -> Flow {
    tracing::warn!(conn = conn.id, "拒绝连接：{reason}");
    let result = HelloResult {
        status: PairingStatus::Rejected,
        token: None,
        protocol_version: PROTOCOL_VERSION.to_owned(),
        host_version: HOST_VERSION.to_owned(),
        reason: Some(reason),
        ..Default::default()
    };
    conn.send(&Message::result(
        req.id.clone(),
        serde_json::to_value(result).unwrap_or(Value::Null),
    ));
    Flow::Close
}

fn random_token() -> String {
    rand::random::<[u8; 16]>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn handle_handshake_request(
    shared: &Arc<HubShared>,
    conn: &Arc<Connection>,
    origin: Option<&str>,
    peer: Peer,
    req: Request,
) -> Flow {
    match req.method.as_str() {
        method::HELLO => {}
        method::PING => {
            conn.send(&Message::result(req.id, json!({})));
            return Flow::Continue;
        }
        _ => {
            let err: RpcError = ToolError::new(
                ErrorKind::Unauthorized,
                "尚未完成握手，请先发送 app/hello。",
            )
            .into();
            conn.send(&Message::error(req.id, err));
            return Flow::Continue;
        }
    }
    let hello: HelloParams = match parse_params(&req.params) {
        Ok(h) => h,
        Err(e) => {
            conn.send(&Message::error(
                req.id,
                RpcError::invalid_params(format!("app/hello 参数无效：{e}")),
            ));
            return Flow::Continue;
        }
    };
    if hello.protocol_version != PROTOCOL_VERSION {
        return reject(
            conn,
            &req,
            format!(
                "协议版本不兼容：SDK 使用 {}，Host 只支持 {PROTOCOL_VERSION}。请升级 SDK 或 Host。",
                hello.protocol_version
            ),
        );
    }
    if let Peer::Tcp(addr) = peer
        && !addr.ip().is_loopback()
    {
        return reject(
            conn,
            &req,
            format!("只接受来自本机回环地址的连接（来自 {}）", addr.ip()),
        );
    }
    let origin_ok = shared.origins.allows(origin);
    let pairing_handler = shared.pairing_handler();
    if !origin_ok && pairing_handler.is_none() {
        return reject(
            conn,
            &req,
            format!(
                "来源 {} 不在允许列表中；可用 --allow-origin 添加。",
                origin.unwrap_or_default()
            ),
        );
    }
    if !is_valid_app_id(&hello.app_id) {
        return reject(
            conn,
            &req,
            format!(
                "appId「{}」格式不合法，应满足 [a-z][a-z0-9-]{{0,62}}",
                hello.app_id
            ),
        );
    }
    if is_reserved_app_id(&hello.app_id) {
        return reject(conn, &req, format!("appId「{}」是保留名", hello.app_id));
    }
    if shared.is_upstream(&hello.app_id) {
        return reject(
            conn,
            &req,
            format!("appId「{}」已被上游 MCP 服务器占用", hello.app_id),
        );
    }
    if hello.instance_id.is_empty() {
        return reject(conn, &req, "instanceId 不能为空".to_owned());
    }

    let token = hello
        .token
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(random_token);

    // 设置了 PairingHandler：未知 App（无静态清单，或 Origin 不在白名单）且之前没配对过时先询问。
    if let Some(handler) = pairing_handler {
        let unknown = !origin_ok || !shared.registry().has_manifest(&hello.app_id);
        if unknown && !shared.is_paired(&hello.app_id, origin, hello.token.as_deref()) {
            // 需要重新确认配对：不做快速恢复（pairingResult 不带 toolsCurrent）。
            take_resume(shared, &hello);
            let result = HelloResult {
                status: PairingStatus::Pending,
                token: None,
                protocol_version: PROTOCOL_VERSION.to_owned(),
                host_version: HOST_VERSION.to_owned(),
                reason: None,
                ..Default::default()
            };
            conn.send(&Message::result(
                req.id,
                serde_json::to_value(result).unwrap_or(Value::Null),
            ));
            let pair_req = PairingRequest {
                app_id: hello.app_id.clone(),
                app_name: hello.app_name.clone(),
                origin: origin.map(str::to_owned),
                client_kind: client_kind_str(hello.client_kind).to_owned(),
                instance_id: hello.instance_id.clone(),
            };
            let (tx, rx) = oneshot::channel();
            let timeout = shared.config.pairing_timeout;
            tracing::info!(conn = conn.id, app_id = %hello.app_id, ?origin, "等待用户确认配对");
            tokio::spawn(async move {
                let ok = tokio::time::timeout(timeout, handler.pair(pair_req))
                    .await
                    .unwrap_or(false);
                let _ = tx.send(ok);
            });
            return Flow::Pending(Box::new(PendingPairing { hello, token, rx }));
        }
        shared.remember_pairing(&hello.app_id, origin, &token, false);
    }

    // 快速恢复（spec/protocol.md 8.3）：恢复令牌有效且摘要与休眠快照一致 → toolsCurrent。
    let (snapshot, tools_current) = take_resume(shared, &hello);
    let result = HelloResult {
        status: PairingStatus::Paired,
        token: Some(token),
        protocol_version: PROTOCOL_VERSION.to_owned(),
        host_version: HOST_VERSION.to_owned(),
        reason: None,
        tools_current,
    };
    conn.send(&Message::result(
        req.id,
        serde_json::to_value(result).unwrap_or(Value::Null),
    ));
    let snapshot = snapshot.filter(|_| tools_current);
    Flow::Paired(register_instance_with(shared, conn, hello, origin, peer, snapshot))
}

/// 实例（重新）连接：取出其休眠记录，并判断能否快速恢复。
/// 同一 appId 以新的实例 ID 连接时，按配置移除该 App 的全部休眠记录。
fn take_resume(shared: &Arc<HubShared>, hello: &HelloParams) -> (Option<DormantInstance>, bool) {
    let mut reg = shared.registry();
    let Some(d) = reg.take_dormant(&hello.app_id, &hello.instance_id) else {
        let removed = if shared.config.dormant_replaced_by_new_instance {
            reg.clear_dormant(&hello.app_id)
        } else {
            Vec::new()
        };
        drop(reg);
        shared.dormant_removed(
            removed
                .into_iter()
                .map(|i| (hello.app_id.clone(), i))
                .collect(),
        );
        return (None, false);
    };
    drop(reg);
    let current = hello.resume_token.as_deref() == Some(d.resume_token.as_str())
        && hello.tools_hash.as_deref() == Some(d.snapshot_hash().as_str());
    tracing::info!(app_id = %hello.app_id, instance_id = %hello.instance_id, tools_current = current, "休眠实例回连");
    (Some(d), current)
}

fn send_pairing_result(
    conn: &Connection,
    status: PairingStatus,
    token: Option<String>,
    reason: Option<String>,
) {
    let p = PairingResultParams {
        status,
        token,
        reason,
    };
    conn.notify(
        method::PAIRING_RESULT,
        serde_json::to_value(p).unwrap_or(Value::Null),
    );
}

/// 登记已配对的实例并通知变化。`snapshot` 为快速恢复时沿用的休眠快照。
fn register_instance_with(
    shared: &Arc<HubShared>,
    conn: &Arc<Connection>,
    hello: HelloParams,
    origin: Option<&str>,
    peer: Peer,
    snapshot: Option<DormantInstance>,
) -> Registered {
    let replaced = shared.registry().add_instance(
        &hello.app_id,
        NewInstance {
            instance_id: hello.instance_id.clone(),
            app_name: hello.app_name.clone(),
            client_kind: hello.client_kind,
            app_version: hello.app_version.clone(),
            title: hello.instance_title.clone(),
            url: hello.instance_url.clone(),
            overview: hello.overview.clone(),
            pid: peer.pid(),
            conn: conn.clone(),
        },
    );
    if let Some(old) = replaced {
        tracing::info!(app_id = %hello.app_id, instance_id = %hello.instance_id, "同一实例重新连接，替换旧连接");
        old.close();
    }
    if let Some(snap) = &snapshot {
        shared.registry().restore_snapshot(&hello.app_id, conn.id, snap);
        shared.ensure_subscriptions(&hello.app_id);
    }
    tracing::info!(app_id = %hello.app_id, instance_id = %hello.instance_id, ?origin, %peer, "实例已配对");
    shared.emit(HubEvent::AppConnected {
        app_id: hello.app_id.clone(),
        instance_id: hello.instance_id.clone(),
    });
    shared.mark_tools_changed();
    shared.mark_resources_changed();
    Registered {
        app_id: hello.app_id,
        instance_id: hello.instance_id,
        launch_token: hello.launch_token.filter(|t| !t.is_empty()),
    }
}

fn handle_request(shared: &Arc<HubShared>, conn: &Arc<Connection>, reg: &Registered, req: Request) {
    match req.method.as_str() {
        method::PING => {
            conn.send(&Message::result(req.id, json!({})));
        }
        method::SLEEP => {
            let p: SleepParams = match parse_params(&req.params) {
                Ok(p) => p,
                Err(e) => {
                    conn.send(&Message::error(
                        req.id,
                        RpcError::invalid_params(format!("app/sleep 参数无效：{e}")),
                    ));
                    return;
                }
            };
            let result = handle_sleep(shared, conn, reg, p);
            conn.send(&Message::result(
                req.id,
                serde_json::to_value(result).unwrap_or(Value::Null),
            ));
        }
        method::HELLO => {
            conn.send(&Message::error(
                req.id,
                RpcError::new(RpcError::INVALID_REQUEST, "已完成握手"),
            ));
        }
        other => {
            conn.send(&Message::error(req.id, RpcError::method_not_found(other)));
        }
    }
}

/// `app/sleep`（spec/protocol.md 8.1）：有待派发给本实例的调用时拒绝；否则转为休眠记录。
///
/// 实例立即从已连接列表移到休眠列表（之后的调用走唤醒路径），连接由 SDK 随后关闭。
fn handle_sleep(
    shared: &Arc<HubShared>,
    conn: &Arc<Connection>,
    reg: &Registered,
    p: SleepParams,
) -> SleepResult {
    let busy = conn.inflight() > 0 || shared.has_pending_wake(&reg.app_id, &reg.instance_id);
    if busy {
        tracing::debug!(app_id = %reg.app_id, instance_id = %reg.instance_id, "有待派发的调用，拒绝休眠");
        return SleepResult {
            accepted: false,
            resume_token: None,
            retry_after_ms: Some(SLEEP_RETRY_AFTER_MS),
        };
    }
    let resume_token = random_token();
    let made = shared.registry().make_dormant(
        &reg.app_id,
        conn.id,
        resume_token.clone(),
        p.tools_hash,
        p.wake,
    );
    if made.is_none() {
        return SleepResult {
            accepted: false,
            resume_token: None,
            retry_after_ms: None,
        };
    }
    tracing::info!(app_id = %reg.app_id, instance_id = %reg.instance_id, reason = ?p.reason, "实例进入休眠");
    shared.emit(HubEvent::AppDormant {
        app_id: reg.app_id.clone(),
        instance_id: reg.instance_id.clone(),
    });
    SleepResult {
        accepted: true,
        resume_token: Some(resume_token),
        retry_after_ms: None,
    }
}

fn handle_notification(
    shared: &Arc<HubShared>,
    conn: &Connection,
    reg: &Registered,
    n: Notification,
) {
    let app_id = reg.app_id.as_str();
    macro_rules! params {
        ($t:ty) => {
            match parse_params::<$t>(&n.params) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(conn = conn.id, method = %n.method, "通知参数无效：{e}");
                    return;
                }
            }
        };
    }
    match n.method.as_str() {
        method::TOOLS_SYNC => {
            let p = params!(ToolsSyncParams);
            warn_prefixed_names(conn, app_id, &p.tools);
            if shared.registry().sync_tools(app_id, conn.id, p.tools) {
                shared.mark_tools_changed();
            }
        }
        method::TOOLS_CHANGED => {
            let p = params!(ToolsChangedParams);
            warn_prefixed_names(conn, app_id, &p.upserted);
            if shared
                .registry()
                .change_tools(app_id, conn.id, p.upserted, p.removed)
            {
                shared.mark_tools_changed();
            }
        }
        method::RESOURCES_SYNC => {
            let p = params!(ResourcesSyncParams);
            if shared
                .registry()
                .sync_resources(app_id, conn.id, p.resources)
            {
                shared.mark_resources_changed();
            }
            shared.ensure_subscriptions(app_id);
        }
        method::RESOURCES_CHANGED => {
            let p = params!(ResourcesChangedParams);
            if shared
                .registry()
                .change_resources(app_id, conn.id, p.upserted, p.removed)
            {
                shared.mark_resources_changed();
            }
            shared.ensure_subscriptions(app_id);
        }
        method::RESOURCES_UPDATED => {
            let p = params!(ResourceUpdatedParams);
            shared.resource_updated(app_id, &p.name);
        }
        method::VISIBILITY => {
            let p = params!(VisibilityParams);
            shared
                .registry()
                .set_visibility(app_id, conn.id, p.visibility, p.focused);
            shared.emit(HubEvent::VisibilityChanged {
                app_id: app_id.to_owned(),
                instance_id: reg.instance_id.clone(),
                visibility: p.visibility,
            });
        }
        method::READY => {
            shared.registry().set_ready(app_id, conn.id);
            shared.wake_arrived(app_id, &reg.instance_id, reg.launch_token.as_deref());
        }
        other => tracing::warn!(conn = conn.id, method = other, "未知通知，忽略"),
    }
}

/// 局部名以 `<appId>.` 开头时记录警告（spec/protocol.md 3.1）：名称照常登记，不改写。
fn warn_prefixed_names(conn: &Connection, app_id: &str, tools: &[app_mcp_protocol::ToolInfo]) {
    for t in tools.iter().filter(|t| app_mcp_protocol::has_app_id_prefix(&t.name, app_id)) {
        tracing::warn!(conn = conn.id, "{}", app_mcp_protocol::app_id_prefix_warning(&t.name, app_id));
    }
}
