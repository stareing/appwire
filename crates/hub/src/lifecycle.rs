//! Host 侧的生命周期配合（spec/lifecycle.md §9）：休眠、唤醒等待、租约、休眠记录过期。

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use app_mcp_protocol::{ErrorKind, LeaseParams, ToolError, method};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::connection::Connection;
use crate::hub::{HubShared, lock};
use crate::registry::WakePlan;
use crate::types::HubEvent;
use crate::wake::{self, Platform, WakeDescriptor, WakeRequest};

type WakeResult = Result<String, ToolError>;

/// 一次进行中的唤醒：同一目标的并发调用共用。
pub(crate) struct PendingWake {
    app_id: String,
    /// 被唤醒的休眠实例；`None` = 冷启动（该 App 任何实例就绪都算）。
    instance_id: Option<String>,
    token: String,
    token_expires: Instant,
    /// 等待回连的截止时刻。
    deadline: Instant,
    waiters: Vec<oneshot::Sender<WakeResult>>,
}

/// `app/sleep` 被拒绝时建议的重试间隔。
pub(crate) const SLEEP_RETRY_AFTER_MS: u64 = 1000;

impl HubShared {
    /// 是否有待派发给该实例的唤醒（冷启动唤醒对该 App 的所有实例都算）。
    pub(crate) fn has_pending_wake(&self, app_id: &str, instance_id: &str) -> bool {
        lock(&self.wakes).iter().any(|w| {
            w.app_id == app_id && w.instance_id.as_deref().is_none_or(|i| i == instance_id)
        })
    }

    /// 执行唤醒并等待回连，返回就绪实例的 ID。
    ///
    /// 同一目标已有唤醒进行中时加入等待，不重复激活。
    /// 唤醒描述：休眠实例上报的优先，其次清单（spec/manifest.md 2.2）。
    pub(crate) fn resolve_wake_descriptor(&self, plan: &WakePlan) -> Option<WakeDescriptor> {
        if let Some(d) = &plan.descriptor {
            return Some(d.clone());
        }
        self.registry()
            .manifest(&plan.app_id)
            .and_then(|m| wake::manifest_descriptor(m, Platform::current(), self.config.wake_from_launch))
    }

    pub(crate) async fn wake_and_wait(
        self: &Arc<Self>,
        plan: &WakePlan,
        cancel: std::pin::Pin<&mut (dyn std::future::Future<Output = ()> + Send + '_)>,
    ) -> WakeResult {
        let app_id = plan.app_id.clone();
        let descriptor = self.resolve_wake_descriptor(plan);
        let now = Instant::now();
        let (tx, rx) = oneshot::channel();
        let (trigger, deadline) = {
            let mut wakes = lock(&self.wakes);
            wakes.retain(|w| w.deadline > now);
            match wakes
                .iter_mut()
                .find(|w| w.app_id == app_id && w.instance_id == plan.instance_id)
            {
                Some(w) => {
                    w.waiters.push(tx);
                    (None, w.deadline)
                }
                None => {
                    let Some(descriptor) = descriptor else {
                        return Err(not_wakeable(&app_id, plan.instance_id.is_some()));
                    };
                    let token = wake::new_token();
                    let deadline = now + self.config.wake_timeout;
                    wakes.push(PendingWake {
                        app_id: app_id.clone(),
                        instance_id: plan.instance_id.clone(),
                        token: token.clone(),
                        token_expires: now + self.config.wake_token_ttl,
                        deadline,
                        waiters: vec![tx],
                    });
                    (Some((token, descriptor)), deadline)
                }
            }
        };
        if let Some((token, descriptor)) = trigger {
            tracing::info!(app_id, instance_id = ?plan.instance_id, kind = wake::kind_str(descriptor.kind), "唤醒 App");
            self.emit(HubEvent::AppWaking {
                app_id: app_id.clone(),
                instance_id: plan.instance_id.clone(),
            });
            let req = WakeRequest {
                app_id: app_id.clone(),
                instance_id: plan.instance_id.clone(),
                descriptor,
                activation_arg: format!("app-mcp-wake:{token}"),
                token: token.clone(),
            };
            let waker = lock(&self.waker).clone();
            let shared = self.clone();
            // 独立任务：调用方取消不影响已发出的激活。
            tokio::spawn(async move {
                if let Err(e) = waker.wake(req).await {
                    tracing::warn!(app_id = %app_id, error = %e, "唤醒失败");
                    let mut err = e.0;
                    err.message = format!("唤醒 App「{app_id}」失败：{}", err.message);
                    err.details = Some(json!({ "appId": app_id }));
                    shared.finish_wake(|w| w.token == token, Err(err));
                }
            });
        }
        tokio::select! {
            r = tokio::time::timeout_at(deadline, rx) => match r {
                Ok(Ok(res)) => res,
                Ok(Err(_)) => Err(not_responding(&plan.app_id, self.config.wake_timeout)),
                Err(_) => {
                    let now = Instant::now();
                    lock(&self.wakes).retain(|w| w.deadline > now);
                    Err(not_responding(&plan.app_id, self.config.wake_timeout))
                }
            },
            _ = cancel => Err(ToolError::new(ErrorKind::Cancelled, "调用已被取消。")),
        }
    }

    /// 结束匹配的唤醒并通知等待方。
    fn finish_wake(&self, pred: impl Fn(&PendingWake) -> bool, result: WakeResult) {
        let mut wakes = lock(&self.wakes);
        wakes.retain_mut(|w| {
            if !pred(w) {
                return true;
            }
            for tx in w.waiters.drain(..) {
                let _ = tx.send(result.clone());
            }
            false
        });
    }

    /// 实例就绪（`app/ready`）：若它是某次唤醒等待的回连（带有效令牌、同一实例 ID，
    /// 或冷启动唤醒中该 App 的任何实例），通知等待方。
    pub(crate) fn wake_arrived(&self, app_id: &str, instance_id: &str, launch_token: Option<&str>) {
        let now = Instant::now();
        self.finish_wake(
            |w| {
                w.app_id == app_id
                    && (launch_token.is_some_and(|t| t == w.token && now < w.token_expires)
                        || w.instance_id.as_deref().is_none_or(|i| i == instance_id))
            },
            Ok(instance_id.to_owned()),
        );
    }

    // ------------------------------------------------------------------
    // 租约
    // ------------------------------------------------------------------

    /// 调用某实例完成后发送 `app/lease { ttlMs }` 并记在会话上。
    pub(crate) fn grant_lease(&self, session_key: &str, conn: &Arc<Connection>) {
        let ttl = self.config.lease_ttl;
        if ttl.is_zero() {
            return;
        }
        send_lease(conn, ttl);
        self.session_state()
            .entry(session_key.to_owned())
            .or_default()
            .leases
            .insert(conn.id, (Arc::downgrade(conn), Instant::now() + ttl));
    }

    /// 会话结束：向其租约过的实例发送 `ttlMs: 0`；若其他会话仍持有未到期租约，随后补发剩余时长。
    pub(crate) fn release_leases(&self, session_key: &str) {
        let mut states = self.session_state();
        let Some(mine) = states.get_mut(session_key).map(|s| std::mem::take(&mut s.leases)) else {
            return;
        };
        let now = Instant::now();
        for (conn_id, (weak, _)) in mine {
            let Some(conn) = weak.upgrade() else { continue };
            let others = states
                .iter()
                .filter(|(k, _)| k.as_str() != session_key)
                .filter_map(|(_, s)| s.leases.get(&conn_id).map(|(_, exp)| *exp))
                .filter(|exp| *exp > now)
                .max();
            send_lease(&conn, Duration::ZERO);
            if let Some(exp) = others {
                send_lease(&conn, exp - now);
            }
        }
    }

    // ------------------------------------------------------------------
    // 休眠记录
    // ------------------------------------------------------------------

    /// 移除过期的休眠记录（每隔 `min(dormant_ttl, 60s)` 检查一次）。
    pub(crate) async fn dormant_sweep_loop(self: Arc<Self>) {
        let ttl = self.config.dormant_ttl;
        let period = ttl.min(Duration::from_secs(60)).max(Duration::from_millis(10));
        loop {
            tokio::time::sleep(period).await;
            let Some(before) = SystemTime::now().checked_sub(ttl) else {
                continue;
            };
            let expired = self.registry().expire_dormant(before);
            self.dormant_removed(expired);
        }
    }

    /// 休眠记录被移除（过期 / 被新实例替换）：发 `AppDisconnected` 并通知列表变化。
    pub(crate) fn dormant_removed(&self, removed: Vec<(String, String)>) {
        if removed.is_empty() {
            return;
        }
        for (app_id, instance_id) in removed {
            tracing::info!(app_id, instance_id, "移除休眠实例记录");
            self.emit(HubEvent::AppDisconnected { app_id, instance_id });
        }
        self.mark_tools_changed();
        self.mark_resources_changed();
    }
}

fn send_lease(conn: &Connection, ttl: Duration) {
    let p = LeaseParams {
        ttl_ms: ttl.as_millis() as u64,
    };
    conn.notify(
        method::LEASE,
        serde_json::to_value(p).unwrap_or(Value::Null),
    );
}

fn not_responding(app_id: &str, timeout: Duration) -> ToolError {
    ToolError::new(
        ErrorKind::AppNotResponding,
        format!(
            "已尝试唤醒 App「{app_id}」，但它在 {} 秒内没有回连。请让用户手动打开该 App 后重试。",
            timeout.as_secs_f32()
        ),
    )
    .with_details(json!({ "appId": app_id }))
}

fn not_wakeable(app_id: &str, dormant: bool) -> ToolError {
    let msg = if dormant {
        format!("App「{app_id}」已休眠，但没有提供唤醒方式。请让用户打开该 App 后重试。")
    } else {
        format!("App「{app_id}」当前未运行，且清单没有声明唤醒方式。请让用户先启动该 App，待其连接后重试。")
    };
    ToolError::new(ErrorKind::AppDisconnected, msg).with_details(json!({ "appId": app_id }))
}
