//! 握手（`app/hello`）：校验、配对询问、快速恢复与实例登记。

use std::sync::Arc;

use app_mcp_manifest::is_reserved_app_id;
use app_mcp_protocol::{
    ConnectionErrorCode, ErrorKind, HelloParams, HelloResult, Message, PROTOCOL_VERSION, PairingResultParams,
    PairingStatus, Request, RpcError, ToolError, WakeKind, is_valid_app_id, method,
};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::connection::Connection;
use crate::hub::{HubShared, lock};
use crate::registry::{DormantInstance, NewInstance, client_kind_str};
use crate::types::{HubEvent, PairingRequest};

use super::{Flow, PendingPairing, Peer, Registered, parse_params};

/// `app/hello` 的结果，带 Host 身份（spec/protocol.md 1.6）与连接 ID（10.3）；`rejected` 时带原因与错误码（10.1）。
fn hello_result(
    shared: &HubShared,
    conn: &Connection,
    status: PairingStatus,
    token: Option<String>,
    rejection: Option<(ConnectionErrorCode, String)>,
    tools_current: bool,
) -> Value {
    let id = &shared.identity;
    let (code, reason) = match rejection {
        Some((code, reason)) => (Some(code.as_str().to_owned()), Some(reason)),
        None => (None, None),
    };
    let result = HelloResult {
        status,
        token,
        protocol_version: PROTOCOL_VERSION.to_owned(),
        host_version: id.version.clone(),
        reason,
        tools_current,
        service: Some(id.service.clone()),
        user: id.user.clone(),
        pid: Some(id.pid),
        connection_id: Some(conn.cid.clone()),
        code,
    };
    serde_json::to_value(result).unwrap_or(Value::Null)
}

/// 拒绝握手：回复 `rejected`（带错误码），记为该 App 的最近错误（appId 合法时），然后关闭连接。
fn reject(
    shared: &HubShared,
    conn: &Connection,
    req: &Request,
    app_id: &str,
    code: ConnectionErrorCode,
    reason: String,
) -> Flow {
    tracing::warn!(cid = %conn.cid, app_id, code = code.as_str(), "拒绝连接：{reason}");
    if is_valid_app_id(app_id) {
        shared.record_app_error(app_id, Some(code.as_str()), &reason);
    }
    conn.send(&Message::result(
        req.id.clone(),
        hello_result(shared, conn, PairingStatus::Rejected, None, Some((code, reason)), false),
    ));
    Flow::Close
}

pub(super) fn random_token() -> String {
    rand::random::<[u8; 16]>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(super) fn handle_handshake_request(
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
            shared,
            conn,
            &req,
            &hello.app_id,
            ConnectionErrorCode::ProtocolIncompatible,
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
            shared,
            conn,
            &req,
            &hello.app_id,
            ConnectionErrorCode::Rejected,
            format!("只接受来自本机回环地址的连接（来自 {}）", addr.ip()),
        );
    }
    let origin_ok = shared.origins.allows(origin);
    let pairing_handler = shared.pairing_handler();
    if !origin_ok && pairing_handler.is_none() {
        return reject(
            shared,
            conn,
            &req,
            &hello.app_id,
            ConnectionErrorCode::OriginNotAllowed,
            format!(
                "来源 {} 不在允许列表中；可用 --allow-origin 添加。",
                origin.unwrap_or_default()
            ),
        );
    }
    if !is_valid_app_id(&hello.app_id) {
        return reject(
            shared,
            conn,
            &req,
            &hello.app_id,
            ConnectionErrorCode::InvalidHello,
            format!(
                "appId「{}」格式不合法，应满足 [a-z][a-z0-9-]{{0,62}}",
                hello.app_id
            ),
        );
    }
    if is_reserved_app_id(&hello.app_id) {
        return reject(
            shared,
            conn,
            &req,
            &hello.app_id,
            ConnectionErrorCode::InvalidHello,
            format!("appId「{}」是保留名", hello.app_id),
        );
    }
    if shared.is_upstream(&hello.app_id) {
        return reject(
            shared,
            conn,
            &req,
            &hello.app_id,
            ConnectionErrorCode::InvalidHello,
            format!("appId「{}」已被上游 MCP 服务器占用", hello.app_id),
        );
    }
    if hello.instance_id.is_empty() {
        return reject(
            shared,
            conn,
            &req,
            &hello.app_id,
            ConnectionErrorCode::InvalidHello,
            "instanceId 不能为空".to_owned(),
        );
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
            conn.send(&Message::result(
                req.id,
                hello_result(shared, conn, PairingStatus::Pending, None, None, false),
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
            tracing::info!(cid = %conn.cid, app_id = %hello.app_id, ?origin, "等待用户确认配对");
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
    conn.send(&Message::result(
        req.id,
        hello_result(shared, conn, PairingStatus::Paired, Some(token), None, tools_current),
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
    shared.mark_dormant_dirty(&hello.app_id);
    let current = hello.resume_token.as_deref() == Some(d.resume_token.as_str())
        && hello.tools_hash.as_deref() == Some(d.snapshot_hash().as_str());
    tracing::info!(app_id = %hello.app_id, instance_id = %hello.instance_id, tools_current = current, "休眠实例回连");
    (Some(d), current)
}

pub(super) fn send_pairing_result(
    conn: &Connection,
    status: PairingStatus,
    token: Option<String>,
    rejection: Option<(ConnectionErrorCode, String)>,
) {
    let (code, reason) = match rejection {
        Some((code, reason)) => (Some(code.as_str().to_owned()), Some(reason)),
        None => (None, None),
    };
    let p = PairingResultParams {
        status,
        token,
        reason,
        code,
    };
    conn.notify(
        method::PAIRING_RESULT,
        serde_json::to_value(p).unwrap_or(Value::Null),
    );
}

/// 登记已配对的实例并通知变化。`snapshot` 为快速恢复时沿用的休眠快照。
pub(super) fn register_instance_with(
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
            navigate: hello.capabilities.as_ref().is_some_and(|c| c.navigate),
            conn: conn.clone(),
            wake: hello.wake.clone().filter(|w| w.kind != WakeKind::None),
        },
    );
    if let Some(old) = replaced {
        tracing::info!(cid = %conn.cid, old_cid = %old.cid, app_id = %hello.app_id, instance_id = %hello.instance_id, "同一实例重新连接，替换旧连接");
        old.close();
    }
    if let Some(snap) = &snapshot {
        shared.registry().restore_snapshot(&hello.app_id, conn.id, snap);
        shared.ensure_subscriptions(&hello.app_id);
    }
    tracing::info!(cid = %conn.cid, app_id = %hello.app_id, instance_id = %hello.instance_id, ?origin, %peer, "实例已配对");
    shared.emit(HubEvent::AppConnected {
        app_id: hello.app_id.clone(),
        instance_id: hello.instance_id.clone(),
    });
    shared.mark_tools_changed();
    shared.mark_resources_changed();
    lock(&shared.power).connected(
        &hello.app_id,
        &hello.instance_id,
        conn.id,
        hello.heartbeat_ms,
        hello.lifecycle_mode,
        Instant::now(),
    );
    Registered {
        app_id: hello.app_id,
        instance_id: hello.instance_id,
        launch_token: hello.launch_token.filter(|t| !t.is_empty()),
        heartbeat_ms: hello.heartbeat_ms,
        dialed: peer.dialed(),
    }
}
