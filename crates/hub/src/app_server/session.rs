//! 实例会话：握手、消息分发、心跳、空闲超时与断开清理；文本消息按类型分派。

use std::sync::Arc;

use app_mcp_protocol::{
    ConnectionErrorCode, ErrorKind, HelloParams, Message, PairingStatus, RpcError, ToolError, Visibility, method,
};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::time::{Instant, interval_at, sleep_until};

use crate::connection::Connection;
use crate::hub::{HubShared, lock};
use crate::types::HubEvent;

use super::{Flow, HeartbeatMode, Peer, Registered, channel_busy, channel_close_at};
use super::handshake::{handle_handshake_request, random_token, register_instance_with, send_pairing_result};
use super::messages::{handle_notification, handle_request};

/// 一个实例会话（独立连接或多路复用的一个通道）：握手、消息分发、心跳、空闲超时与断开清理。
/// `inbound` 结束即视为连接断开；会话结束时调用 [`Connection::close`]，由写端负责关闭底层连接 / 通道。
pub(super) async fn run_session<I>(shared: Arc<HubShared>, conn: Arc<Connection>, mut inbound: I, origin: Option<String>, peer: Peer)
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
        let configured = if background {
            cfg.hidden_idle_timeout
        } else {
            cfg.idle_timeout
        };
        // 握手前一律按配置值；握手后按 SDK 的心跳声明（spec/lifecycle.md 第 11 节）。
        let mode = registered.as_ref().map(|r| HeartbeatMode::of(r.heartbeat_ms, cfg.legacy_heartbeat));
        let idle = match mode {
            None => Some(configured),
            Some(m) => m.idle_timeout(configured),
        };
        let hub_pings = mode.is_some_and(HeartbeatMode::hub_pings);
        let close_at = registered.as_ref().filter(|r| r.dialed).map(|r| channel_close_at(&shared, &conn, r, last_rx));
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
                    let reason = "用户拒绝了配对请求";
                    let code = ConnectionErrorCode::PairingRejected;
                    tracing::info!(cid = %conn.cid, app_id = %hello.app_id, code = code.as_str(), "配对被拒绝");
                    shared.record_app_error(&hello.app_id, Some(code.as_str()), reason);
                    send_pairing_result(&conn, PairingStatus::Rejected, None, Some((code, reason.to_owned())));
                    break;
                }
                shared.remember_pairing(&hello.app_id, origin.as_deref(), &token, true);
                send_pairing_result(&conn, PairingStatus::Paired, Some(token), None);
                registered = Some(register_instance_with(&shared, &conn, hello, origin.as_deref(), peer, None));
                last_rx = Instant::now();
            }
            // 等待配对确认期间 SDK 不发心跳，暂停空闲超时（配对本身有 pairing_timeout）。
            _ = sleep_until(last_rx + idle.unwrap_or_default()), if pairing_rx.is_none() && idle.is_some() => {
                let secs = idle.unwrap_or_default().as_secs();
                tracing::info!(cid = %conn.cid, background, "{secs} 秒内没有收到消息，断开连接");
                break;
            }
            _ = ping.tick(), if hub_pings => {
                if let Some(r) = &registered {
                    lock(&shared.power).heartbeat(&r.app_id, &r.instance_id, Instant::now());
                }
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
                tracing::debug!(cid = %conn.cid, "连接被关闭（被新连接替换）");
                break;
            }
            _ = sleep_until(close_at.unwrap_or_else(Instant::now)), if close_at.is_some() => {
                // 到期时复查：期间可能有调用开始（不经本循环）。
                let Some(reg) = registered.as_ref() else { continue };
                if channel_busy(&shared, &conn, reg) || channel_close_at(&shared, &conn, reg, last_rx) > Instant::now() {
                    continue;
                }
                tracing::info!(cid = %conn.cid, app_id = %reg.app_id, instance_id = %reg.instance_id, "宽限到期，关闭按名拨入的通道");
                break;
            }
        }
    }

    // 清理
    if let Some(reg) = &registered {
        lock(&shared.power).disconnected(&reg.app_id, &reg.instance_id, conn.id, Instant::now());
    }
    // 按名拨入的通道（Hub 关闭或对端死亡）：实例转为休眠快照，名字保留，下次调用再拨号（spec/naming.md 第 3、7.5 节）。
    if let Some(reg) = registered.as_ref().filter(|r| r.dialed)
        && shared.registry().make_dormant_by_hub(&reg.app_id, conn.id, random_token()).is_some()
    {
        tracing::info!(cid = %conn.cid, app_id = %reg.app_id, instance_id = %reg.instance_id, "通道关闭，实例转为休眠");
        shared.mark_dormant_dirty(&reg.app_id);
        shared.emit(HubEvent::AppDormant { app_id: reg.app_id.clone(), instance_id: reg.instance_id.clone() });
    }
    if let Some(reg) = registered {
        shared.events_disconnected(&reg.app_id, conn.id);
        let removed = shared.registry().remove_instance(&reg.app_id, conn.id);
        if let Some(inst) = &removed {
            if inst.wake.is_some() {
                shared.mark_dormant_dirty(&reg.app_id);
            }
            tracing::info!(cid = %conn.cid, app_id = %reg.app_id, instance_id = %reg.instance_id, "实例断开");
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
            tracing::warn!(cid = %conn.cid, "无法解析的消息：{e}");
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
                None => tracing::warn!(cid = %conn.cid, method = %n.method, "握手前的通知，忽略"),
                Some(reg) => handle_notification(shared, conn, reg, n),
            }
            Flow::Continue
        }
        Message::Response(resp) => {
            if !conn.resolve(resp) {
                tracing::warn!(cid = %conn.cid, "未知 ID 的响应，忽略");
            }
            Flow::Continue
        }
    }
}
