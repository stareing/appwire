//! 多路复用连接（spec/protocol.md 第 9 节）：拆帧 / 装帧、通道开关与连接级空闲超时，每个通道跑一个实例会话。

use std::collections::HashMap;
use std::sync::Arc;

use app_mcp_protocol::{
    MUX_MAX_CHANNELS, MUX_VERSION, Message, MuxFrame, MuxParams, MuxResult, Request, RpcError, method,
};
use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};
use tokio_tungstenite::tungstenite::protocol::Message as WsMessage;

use crate::connection::{Connection, Outgoing};
use crate::hub::HubShared;

use super::{Peer, parse_params};
use super::session::run_session;

/// 连接上的第一条消息是否为 `app/mux` 请求。
pub(super) fn mux_request(text: &str) -> Option<Request> {
    match Message::parse(text) {
        Ok(Message::Request(req)) if req.method == method::MUX => Some(req),
        _ => None,
    }
}

/// 多路复用连接（spec/protocol.md 第 9 节）：每个通道各跑一个 [`run_session`]，与独立连接完全相同；
/// 本函数只负责拆帧 / 装帧、通道开关与连接级空闲超时。
pub(super) async fn run_mux<Sk>(
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
                        let (conn, out_rx) = shared.new_connection();
                        tracing::debug!(%peer, ch, cid = %conn.cid, ?origin, "新通道");
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
