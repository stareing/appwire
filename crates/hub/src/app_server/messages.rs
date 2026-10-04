//! 握手后的消息处理：SDK 发来的请求（`ping`、`app/sleep` 等）与通知（工具 / 资源同步与变化、可见性、诊断、进度、事件）。

use std::sync::Arc;

use app_mcp_protocol::{
    DiagnosticParams, Message, Notification, Request, ResourceUpdatedParams, ResourcesChangedParams,
    ResourcesSyncParams, RpcError, SleepParams, SleepResult, ToolsChangedParams, ToolsSyncParams, VisibilityParams,
    method,
};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::connection::Connection;
use crate::hub::{HubShared, lock};
use crate::lifecycle::SLEEP_RETRY_AFTER_MS;
use crate::registry::{sanitize_resources, sanitize_tools};
use crate::types::{DiagnosticReport, HubEvent};

use super::{Registered, parse_params};
use super::handshake::random_token;

pub(super) fn handle_request(shared: &Arc<HubShared>, conn: &Arc<Connection>, reg: &Registered, req: Request) {
    match req.method.as_str() {
        method::PING => {
            lock(&shared.power).heartbeat(&reg.app_id, &reg.instance_id, Instant::now());
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
        tracing::debug!(cid = %conn.cid, app_id = %reg.app_id, instance_id = %reg.instance_id, "有待派发的调用，拒绝休眠");
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
    tracing::info!(cid = %conn.cid, app_id = %reg.app_id, instance_id = %reg.instance_id, reason = ?p.reason, "实例进入休眠");
    shared.mark_dormant_dirty(&reg.app_id);
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

pub(super) fn handle_notification(
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
                    tracing::warn!(cid = %conn.cid, method = %n.method, "通知参数无效：{e}");
                    return;
                }
            }
        };
    }
    match n.method.as_str() {
        method::TOOLS_SYNC => {
            let p = params!(ToolsSyncParams);
            warn_prefixed_names(conn, app_id, &p.tools);
            let tools = sanitize_tools(app_id, p.tools);
            // @why 握手时的全量同步只在声明变化时清缓存：冷启动后的数据新旧由条目 TTL 兜底（spec/hub-api.md 3.20）。
            if shared.registry().tools_declaration_differs(app_id, conn.id, &tools) {
                shared.invalidate_app_cache(app_id);
            }
            if shared.registry().sync_sanitized_tools(app_id, conn.id, tools) {
                shared.mark_tools_changed();
            }
        }
        method::TOOLS_CHANGED => {
            let p = params!(ToolsChangedParams);
            warn_prefixed_names(conn, app_id, &p.upserted);
            shared.invalidate_app_cache(app_id);
            if shared
                .registry()
                .change_tools(app_id, conn.id, p.upserted, p.removed)
            {
                shared.mark_tools_changed();
            }
        }
        method::RESOURCES_SYNC => {
            let p = params!(ResourcesSyncParams);
            let resources = sanitize_resources(app_id, p.resources);
            // @why 同 `tools/sync`：声明未变时保留缓存，数据新旧由 TTL 兜底。
            if shared.registry().resources_declaration_differs(app_id, conn.id, &resources) {
                shared.invalidate_app_cache(app_id);
            }
            if shared
                .registry()
                .sync_sanitized_resources(app_id, conn.id, resources)
            {
                shared.mark_resources_changed();
            }
            shared.ensure_subscriptions(app_id);
        }
        method::RESOURCES_CHANGED => {
            let p = params!(ResourcesChangedParams);
            shared.invalidate_app_cache(app_id);
            if shared
                .registry()
                .change_resources(app_id, conn.id, p.upserted, p.removed)
            {
                shared.mark_resources_changed();
            }
            shared.ensure_subscriptions(app_id);
        }
        method::TOOLS_PROGRESS => {
            let p = params!(app_mcp_protocol::ToolsProgressParams);
            shared.route_progress(conn.id, p);
        }
        method::EVENTS_SYNC => {
            let p = params!(app_mcp_protocol::EventsSyncParams);
            shared.events_sync(app_id, conn.id, &conn.cid, p.events);
        }
        method::EVENTS_EMIT => {
            let p = params!(app_mcp_protocol::EventEmitParams);
            shared.event_emit(app_id, &reg.instance_id, conn.id, &conn.cid, p);
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
        method::DIAGNOSTIC => {
            let p = params!(DiagnosticParams);
            tracing::warn!(
                cid = %conn.cid,
                app_id,
                instance_id = %reg.instance_id,
                code = %p.code,
                count = p.count,
                "SDK 上报连接问题：{}",
                p.message
            );
            shared.record_report(DiagnosticReport {
                app_id: app_id.to_owned(),
                instance_id: reg.instance_id.clone(),
                connection_id: conn.cid.clone(),
                code: p.code,
                message: p.message,
                count: p.count.max(1),
                received_at_ms: crate::hub::unix_millis(),
            });
        }
        method::READY => {
            if shared.registry().set_ready(app_id, conn.id) {
                shared.mark_dormant_dirty(app_id);
            }
            // 回连后重新订阅仍被订阅的资源（spec/lifecycle.md 第 13 节 B3）；同步时已订阅的不重复发送。
            shared.ensure_subscriptions(app_id);
            shared.wake_arrived(app_id, &reg.instance_id, reg.launch_token.as_deref(), reg.dialed);
        }
        other => tracing::warn!(cid = %conn.cid, method = other, "未知通知，忽略"),
    }
}

/// 局部名以 `<appId>.` 开头时记录警告（spec/protocol.md 3.1）：名称照常登记，不改写。
fn warn_prefixed_names(conn: &Connection, app_id: &str, tools: &[app_mcp_protocol::ToolInfo]) {
    for t in tools.iter().filter(|t| app_mcp_protocol::has_app_id_prefix(&t.name, app_id)) {
        tracing::warn!(cid = %conn.cid, "{}", app_mcp_protocol::app_id_prefix_warning(&t.name, app_id));
    }
}
