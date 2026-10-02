//! Host 侧的生命周期配合（spec/lifecycle.md §9）：休眠、唤醒等待、租约、休眠记录过期与持久化。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use app_mcp_protocol::{ErrorKind, LeaseParams, ToolError, method};
use serde_json::{Value, json};
use tokio::sync::{Notify, oneshot};
use tokio::time::Instant;

use crate::connection::Connection;
use crate::dormant_store::{DormantStore, DormantStoreStatus};
use crate::hub::{HubShared, lock};
use crate::registry::{WakePlan, WakeTargetPresence};
use crate::task::{CallerKey, TaskLifetime};
use crate::types::{AwakeReason, HubEvent};
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
    /// 经按名拨号激活（spec/naming.md 第 3 节）：在 Hub 自己拨出的通道上就绪的实例直接认领，不需要令牌。
    dialed: bool,
}

impl PendingWake {
    /// 是否仍在等待回连。
    pub(crate) fn is_active(&self, now: Instant) -> bool {
        self.deadline > now
    }

    /// `(appId, 被唤醒的休眠实例)`。
    pub(crate) fn target(&self) -> (String, Option<String>) {
        (self.app_id.clone(), self.instance_id.clone())
    }
}

/// 会话发给某实例（连接）的租约。
///
/// @invariant 与 SDK 一致：SDK 对每次 `app/lease` 取当前与新值的较大截止时刻（spec/lifecycle.md 4.2），后发的较短租约
/// 不缩短先发的；因此按默认值与自适应分别记最晚截止，不能只记最后一次。
#[derive(Debug)]
pub(crate) struct LeaseEntry {
    pub conn: std::sync::Weak<Connection>,
    /// 以默认值发出的租约的最晚截止（请求流空闲可收回的部分）。
    pub default_until: Option<Instant>,
    /// 按调用间隔统计发出的租约的最晚截止（按时到期，不提前收回）。
    pub adaptive_until: Option<Instant>,
}

impl LeaseEntry {
    fn new(conn: &Arc<Connection>) -> Self {
        Self { conn: Arc::downgrade(conn), default_until: None, adaptive_until: None }
    }

    /// 记一次租约。
    fn add(&mut self, until: Instant, adaptive: bool) {
        let slot = if adaptive { &mut self.adaptive_until } else { &mut self.default_until };
        *slot = Some(slot.map_or(until, |u| u.max(until)));
    }

    /// 本会话租约在 SDK 侧的有效截止。
    pub fn expires(&self) -> Option<Instant> {
        self.default_until.max(self.adaptive_until)
    }

    /// 默认值部分是否仍决定着有效截止（未到期且晚于自适应部分）——只有这时收回才会让实例提前休眠。
    fn default_dominates(&self, now: Instant) -> bool {
        self.default_until.is_some_and(|d| d > now && self.adaptive_until.is_none_or(|a| d > a))
    }
}

/// 一次激活的方式（spec/naming.md 9.2 的路由顺序）。
enum Activation {
    /// 按名拨号（名字服务激活）。
    Dial(Arc<dyn crate::connector::Connector>, app_mcp_protocol::naming::Address),
    /// 唤醒描述（后备）。
    Waker(Arc<dyn crate::wake::Waker>, WakeDescriptor),
}

/// 收回会话租约的范围。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Revoke {
    /// 会话结束：全部。
    All,
    /// 请求流空闲：只收回默认值部分，自适应部分保留并补发剩余。
    DefaultOnly,
}

/// 会话请求的活动守卫（[`HubShared::session_request`]）。
pub(crate) struct SessionRequest {
    shared: Arc<HubShared>,
    key: String,
}

impl Drop for SessionRequest {
    fn drop(&mut self) {
        lock(&self.shared.leases).request_finished(&self.key, Instant::now());
        self.shared.lease_changed.notify_one();
    }
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

    /// 唤醒计划能否执行：可按名拨号，或配置了唤醒器且（休眠实例 / 有唤醒描述）。
    pub(crate) fn wake_reachable(&self, plan: &WakePlan) -> bool {
        self.named_route(&plan.app_id).is_some()
            || (self.wake_enabled() && (plan.instance_id.is_some() || self.resolve_wake_descriptor(plan).is_some()))
    }

    /// 某连接上全部会话租约中最晚的有效截止（按名拨入的通道据此决定关闭时刻，spec/naming.md 7.2）。
    pub(crate) fn lease_expiry(&self, conn_id: u64) -> Option<Instant> {
        self.agent_tasks().values().filter_map(|s| s.leases.get(&conn_id)).filter_map(LeaseEntry::expires).max()
    }

    /// 是否配置了唤醒器（`waker: none` 时为 `false`）。
    pub(crate) fn wake_enabled(&self) -> bool {
        lock(&self.waker).is_some()
    }

    /// 唤醒描述：休眠实例上报的优先，其次清单（spec/manifest.md 2.2）。
    pub(crate) fn resolve_wake_descriptor(&self, plan: &WakePlan) -> Option<WakeDescriptor> {
        if let Some(d) = &plan.descriptor {
            return Some(d.clone());
        }
        self.registry()
            .manifest(&plan.app_id)
            .and_then(|m| wake::manifest_descriptor(m.meta(), Platform::current(), self.config.wake_from_launch))
    }

    /// 执行唤醒并等待回连，返回就绪实例的 ID。
    ///
    /// 去重（spec/lifecycle.md §9）：同一目标已有唤醒进行中时加入等待，不重复激活；目标实例已连接并就绪时
    /// 直接返回；已连接但握手未完成时只等它的 `app/ready`，不激活。
    pub(crate) async fn wake_and_wait(
        self: &Arc<Self>,
        plan: &WakePlan,
        cancel: std::pin::Pin<&mut (dyn std::future::Future<Output = ()> + Send + '_)>,
    ) -> WakeResult {
        let app_id = plan.app_id.clone();
        // 路由顺序（spec/naming.md 9.2）：按名拨号优先于唤醒描述。
        let named = self.named_route(&app_id);
        let waker = lock(&self.waker).clone();
        // 不唤醒（`waker: none`）且不能按名拨号：按未连接处理，带清单的启动提示（launchUrl）。
        if named.is_none() && waker.is_none() {
            return Err(self.registry().disconnected_error(&app_id));
        }
        let descriptor = if named.is_some() { None } else { self.resolve_wake_descriptor(plan) };
        let now = Instant::now();
        let (tx, rx) = oneshot::channel();
        let (trigger, deadline) = {
            let mut wakes = lock(&self.wakes);
            wakes.retain(|w| w.deadline > now);
            if let Some(w) = wakes
                .iter_mut()
                .find(|w| w.app_id == app_id && w.instance_id == plan.instance_id)
            {
                w.waiters.push(tx);
                (None, w.deadline)
            } else {
                // @why 在 wakes 锁内复查：路由计划之后实例可能已因可见回连 / App 主动 wake 连上；
                // `app/ready` 先置 ready 再取 wakes 锁，锁内看到未就绪则一定能被随后的 wake_arrived 匹配。
                let presence = self
                    .registry()
                    .wake_target_presence(&app_id, plan.instance_id.as_deref());
                let activation = match presence {
                    WakeTargetPresence::Ready(id) => {
                        tracing::debug!(app_id, instance_id = %id, "唤醒目标已连接，不再唤醒");
                        return Ok(id);
                    }
                    WakeTargetPresence::Handshaking => None,
                    WakeTargetPresence::Absent => {
                        let how = match (named.clone(), descriptor) {
                            (Some((connector, address)), _) => Activation::Dial(connector, address),
                            (None, Some(d)) => match waker.clone() {
                                Some(w) => Activation::Waker(w, d),
                                None => return Err(self.registry().disconnected_error(&app_id)),
                            },
                            (None, None) => return Err(not_wakeable(&app_id, plan.instance_id.is_some())),
                        };
                        // 唤醒速率上限（spec/lifecycle.md 第 12 节；按名拨号同样计数，spec/naming.md R-14）：只对真正要发出的激活计数。
                        let limit = self.config.wake_rate_limit;
                        match lock(&self.power).reserve_wake(&app_id, limit, now) {
                            Ok(at) => Some((how, at)),
                            Err(limited) => {
                                drop(wakes);
                                let err = rate_limited(&app_id, limit, limited.retry_after);
                                tracing::warn!(app_id, limit, "唤醒次数达到上限，不再唤醒");
                                self.record_app_error(&app_id, Some(WAKE_RATE_LIMITED), &err.message);
                                return Err(err);
                            }
                        }
                    }
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
                    dialed: matches!(activation, Some((Activation::Dial(..), _))),
                });
                (activation.map(|a| (token, a)), deadline)
            }
        };
        if let Some((token, (how, reserved))) = trigger {
            let shared = self.clone();
            let instance_id = plan.instance_id.clone();
            match how {
                Activation::Dial(connector, address) => {
                    tracing::info!(app_id, ?instance_id, %address, connector = connector.kind(), "按名拨号 App");
                    self.emit(HubEvent::AppWaking { app_id: app_id.clone(), instance_id: instance_id.clone() });
                    // 独立任务：调用方取消不影响已发出的拨号（通道建立后照常握手、宽限后关闭）。
                    tokio::spawn(async move {
                        if !shared.wake_token_pending(&token) {
                            tracing::debug!(app_id = %app_id, "唤醒已被回连认领，跳过拨号");
                            lock(&shared.power).cancel_wake(&app_id, reserved);
                            return;
                        }
                        lock(&shared.power).wake_activated(&app_id, instance_id.as_deref(), Instant::now());
                        if let Err(err) = shared.dial_and_serve(connector, address).await {
                            tracing::warn!(app_id = %app_id, error = %err.message, "按名拨号失败");
                            let code = err.details.as_ref().and_then(|d| d["code"].as_str()).map(str::to_owned);
                            shared.record_app_error(&app_id, code.as_deref(), &err.message);
                            shared.finish_wake(|w| w.token == token, Err(err));
                        }
                    });
                }
                Activation::Waker(waker, descriptor) => {
                    tracing::info!(app_id, ?instance_id, kind = wake::kind_str(descriptor.kind), "唤醒 App");
                    self.emit(HubEvent::AppWaking { app_id: app_id.clone(), instance_id: instance_id.clone() });
                    let req = WakeRequest {
                        app_id: app_id.clone(),
                        instance_id,
                        descriptor,
                        activation_arg: format!("app-mcp-wake:{token}"),
                        token: token.clone(),
                    };
                    // 独立任务：调用方取消不影响已发出的激活。
                    tokio::spawn(async move {
                        // 激活前实例已被其他原因的回连认领（令牌已作废）：不再激活。
                        if !shared.wake_token_pending(&token) {
                            tracing::debug!(app_id = %app_id, "唤醒已被回连认领，跳过激活");
                            lock(&shared.power).cancel_wake(&app_id, reserved);
                            return;
                        }
                        lock(&shared.power).wake_activated(&app_id, req.instance_id.as_deref(), Instant::now());
                        if let Err(e) = waker.wake(req).await {
                            tracing::warn!(app_id = %app_id, error = %e, "唤醒失败");
                            let mut err = e.0;
                            err.message = format!("唤醒 App「{app_id}」失败：{}", err.message);
                            err.details = Some(json!({ "appId": app_id }));
                            shared.record_app_error(&app_id, Some(err.kind.as_str()), &err.message);
                            shared.finish_wake(|w| w.token == token, Err(err));
                        }
                    });
                }
            }
        }
        tokio::select! {
            r = tokio::time::timeout_at(deadline, rx) => match r {
                Ok(Ok(res)) => res,
                Ok(Err(_)) | Err(_) => {
                    let now = Instant::now();
                    lock(&self.wakes).retain(|w| w.deadline > now);
                    let err = not_responding(&plan.app_id, self.config.wake_timeout);
                    self.record_app_error(&plan.app_id, Some(err.kind.as_str()), &err.message);
                    Err(err)
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

    /// 该令牌对应的唤醒是否仍在等待回连（未被认领、未超时）。
    pub(crate) fn wake_token_pending(&self, token: &str) -> bool {
        let now = Instant::now();
        lock(&self.wakes).iter().any(|w| w.token == token && w.is_active(now))
    }

    /// 实例就绪（`app/ready`）：认领匹配的唤醒并通知等待方。匹配任一即可：
    /// - 带有效（未过期）令牌；
    /// - 同一实例 ID（无论是否带令牌：可见回连、App 主动 `wake()` 先于激活到达）；
    /// - 冷启动唤醒中该 App 的任何实例；
    /// - 被唤醒的休眠实例已没有记录（被新实例 ID 替换 / 过期）时，该 App 的任何实例。
    ///
    /// @invariant 认领即从等待表移除，令牌随之作废：之后携带它的握手按普通连接处理（不再匹配任何唤醒）。
    ///
    /// `dialed`：该实例在 Hub 按名拨出的通道上就绪——认领该 App 经拨号激活的全部唤醒（spec/naming.md 第 3 节"认领"）。
    pub(crate) fn wake_arrived(&self, app_id: &str, instance_id: &str, launch_token: Option<&str>, dialed: bool) {
        let now = Instant::now();
        let targets: Vec<String> = lock(&self.wakes)
            .iter()
            .filter(|w| w.app_id == app_id)
            .filter_map(|w| w.instance_id.clone())
            .collect();
        let superseded: Vec<String> = {
            let reg = self.registry();
            targets.into_iter().filter(|t| !reg.instance_known(app_id, t)).collect()
        };
        self.finish_wake(
            |w| {
                w.app_id == app_id
                    && ((dialed && w.dialed)
                        || launch_token.is_some_and(|t| t == w.token && now < w.token_expires)
                        || w.instance_id
                            .as_deref()
                            .is_none_or(|i| i == instance_id || superseded.iter().any(|s| s == i)))
            },
            Ok(instance_id.to_owned()),
        );
    }

    // ------------------------------------------------------------------
    // 功耗观测（spec/lifecycle.md 第 12 节）
    // ------------------------------------------------------------------

    /// 持有未到期租约（任一会话）的连接 ID。
    pub(crate) fn leased_connections(&self, now: Instant) -> std::collections::HashSet<u64> {
        self.agent_tasks()
            .values()
            .flat_map(|s| s.leases.iter())
            .filter(|(_, l)| l.expires().is_some_and(|e| e > now))
            .map(|(id, _)| *id)
            .collect()
    }

    /// 已连接实例当前不能休眠的原因（Hub 可见部分）。
    pub(crate) fn awake_reasons(
        &self,
        app_id: &str,
        instance_id: &str,
        mode: Option<app_mcp_protocol::LifecycleMode>,
        leased: &std::collections::HashSet<u64>,
    ) -> Vec<AwakeReason> {
        let (call, lease, subscription) = {
            let reg = self.registry();
            let Some(inst) = reg.instance(app_id, instance_id) else {
                return Vec::new();
            };
            (inst.conn.inflight() > 0, leased.contains(&inst.conn.id), inst.has_realtime_subscription())
        };
        [
            (mode == Some(app_mcp_protocol::LifecycleMode::Persistent), AwakeReason::Persistent),
            (call, AwakeReason::Call),
            (lease, AwakeReason::Lease),
            (subscription, AwakeReason::Subscription),
            (self.has_pending_wake(app_id, instance_id), AwakeReason::WakePending),
        ]
        .into_iter()
        .filter_map(|(on, r)| on.then_some(r))
        .collect()
    }

    // ------------------------------------------------------------------
    // 租约（spec/lifecycle.md 4.2；第 13 节 B2 自适应租约）
    // ------------------------------------------------------------------

    /// 会话对某 App 的一次工具调用开始：记录距该（会话, App）上一次租约的间隔。
    pub(crate) fn lease_call_started(&self, caller: &CallerKey, app_id: &str) {
        lock(&self.leases).call_started(caller.as_str(), app_id, &self.config.lease, Instant::now());
    }

    /// 会话的一个请求（MCP 请求 / API 调用、资源读取）开始；返回的守卫在请求结束时记录活动（空闲收回据此计时）。
    pub(crate) fn session_request(self: &Arc<Self>, caller: &CallerKey) -> SessionRequest {
        lock(&self.leases).request_started(caller.as_str(), Instant::now());
        self.lease_changed.notify_one();
        SessionRequest { shared: self.clone(), key: caller.as_str().to_owned() }
    }

    /// 调用某实例完成后按（会话, App）的调用间隔决定租约，发送 `app/lease { ttlMs }` 并记在会话上。
    pub(crate) fn grant_lease(&self, caller: &CallerKey, app_id: &str, conn: &Arc<Connection>) {
        if self.config.lease_ttl.is_zero() {
            return;
        }
        let now = Instant::now();
        let g = lock(&self.leases).grant(caller.as_str(), app_id, &self.config.lease, self.config.lease_ttl, now);
        tracing::debug!(cid = %conn.cid, app_id, session = %caller, ttl_ms = g.ttl.as_millis() as u64, adaptive = g.adaptive, "发出租约");
        send_lease(conn, g.ttl, g.adaptive);
        self.agent_tasks()
            .entry(caller)
            .leases
            .entry(conn.id)
            .or_insert_with(|| LeaseEntry::new(conn))
            .add(now + g.ttl, g.adaptive);
        if !g.adaptive {
            self.lease_changed.notify_one();
        }
    }

    /// 会话 / 任务结束：收回其全部租约并移除其调用间隔统计。
    pub(crate) fn release_leases(&self, caller: &CallerKey) {
        let n = self.revoke_leases(caller, Revoke::All);
        let mut book = lock(&self.leases);
        book.count_revoked(false, n);
        book.forget_session(caller.as_str());
    }

    /// 收回会话的租约：向实例发送 `ttlMs: 0`（SDK 只能整体取消），随后补发仍应保留的剩余时长——其他会话对同一实例的
    /// 未到期租约，以及 [`Revoke::DefaultOnly`] 时本会话的自适应部分。返回实际发出收回（实例仍在连接）的个数。
    /// `DefaultOnly` 时默认值部分不决定有效截止的连接只清除记录、不发消息。
    fn revoke_leases(&self, caller: &CallerKey, scope: Revoke) -> u64 {
        self.revoke_leases_where(caller, scope, |_| true)
    }

    /// `apps.release`（spec/hub-api.md 3.5「显式释放」）：收回会话在 `conn_ids` 这些连接上的全部租约，其他会话的未到期租约
    /// 随后补发。返回实际发出收回的个数。
    pub(crate) fn release_leases_on(&self, caller: &CallerKey, conn_ids: &std::collections::HashSet<u64>) -> u64 {
        self.revoke_leases_where(caller, Revoke::All, |id| conn_ids.contains(&id))
    }

    /// [`HubShared::revoke_leases`]，只作用于 `include` 选中的连接。
    fn revoke_leases_where(&self, caller: &CallerKey, scope: Revoke, include: impl Fn(u64) -> bool) -> u64 {
        let mut states = self.agent_tasks();
        let now = Instant::now();
        let Some(mine) = states.get_mut(caller).map(|s| {
            let ids: Vec<u64> = s.leases.keys().copied().filter(|id| include(*id)).collect();
            let mut out = Vec::new();
            for id in ids {
                let Some(mut entry) = s.leases.remove(&id) else { continue };
                if scope == Revoke::All {
                    out.push((id, entry.conn.clone(), None));
                    continue;
                }
                let send = entry.default_dominates(now);
                entry.default_until = None;
                let keep = entry.adaptive_until.filter(|a| *a > now);
                if send {
                    out.push((id, entry.conn.clone(), keep));
                }
                if keep.is_some() {
                    s.leases.insert(id, entry);
                }
            }
            out
        }) else {
            return 0;
        };
        let mut n = 0;
        for (conn_id, conn, keep) in mine {
            let Some(conn) = conn.upgrade() else { continue };
            // @why 按种类分别补发：SDK 分别记两种租约，后台连接只认自适应租约（spec/lifecycle.md 第 13 节 B4）。
            let others: Vec<&LeaseEntry> = states
                .iter()
                .filter(|(k, _)| *k != caller)
                .filter_map(|(_, s)| s.leases.get(&conn_id))
                .collect();
            let default = others.iter().filter_map(|l| l.default_until).max().filter(|e| *e > now);
            let adaptive = others.iter().filter_map(|l| l.adaptive_until).max().max(keep).filter(|e| *e > now);
            send_lease(&conn, Duration::ZERO, false);
            if let Some(exp) = default.filter(|d| adaptive.is_none_or(|a| d > &a)) {
                send_lease(&conn, exp - now, false);
            }
            if let Some(exp) = adaptive {
                send_lease(&conn, exp - now, true);
            }
            n += 1;
        }
        n
    }

    /// 请求流空闲收回（spec/lifecycle.md 第 13 节 B2）：会话没有进行中的请求、距最近活动达到 `lease.idle_revoke` 时，
    /// 收回其以默认值发出、仍未到期的租约。同一循环回收空闲的 Agent 任务（[`HubShared::expire_idle_tasks`]）。
    /// 没有待收回的会话、没有按空闲回收的任务时不设定时器。
    pub(crate) async fn lease_idle_loop(self: Arc<Self>) {
        loop {
            let lease_next = lock(&self.leases).next_idle_deadline(&self.config.lease);
            let next = lease_next.into_iter().chain(self.next_task_expiry()).min();
            match next {
                Some(at) => {
                    tokio::select! {
                        _ = tokio::time::sleep_until(at) => {}
                        _ = self.lease_changed.notified() => continue,
                    }
                }
                None => {
                    self.lease_changed.notified().await;
                    continue;
                }
            }
            let now = Instant::now();
            let idle = lock(&self.leases).take_idle_sessions(&self.config.lease, now);
            for key in idle {
                let Some(caller) = self.agent_tasks().caller_key(&key) else {
                    // 没有任务 = 没有记下的租约，无需收回。
                    continue;
                };
                let n = self.revoke_leases(&caller, Revoke::DefaultOnly);
                if n > 0 {
                    tracing::debug!(session = %key, revoked = n, "会话请求流空闲，收回默认租约");
                }
                lock(&self.leases).count_revoked(true, n);
            }
            self.expire_idle_tasks(now);
        }
    }

    // ------------------------------------------------------------------
    // Agent 任务的空闲回收（docs/plans/16-agent-os.md P1 / U9）
    // ------------------------------------------------------------------

    /// 按空闲回收的任务（[`TaskLifetime::UntilIdle`]）各自的到期时刻：请求流空闲（复用租约的请求活动记录）达
    /// `task_idle_ttl`。有进行中请求的任务没有到期时刻（请求结束时会唤醒循环重新计算）；活动记录已被淘汰的任务立即到期。
    fn task_expiries(&self, now: Instant) -> Vec<(CallerKey, Instant)> {
        let ttl = self.config.task_idle_ttl;
        if ttl.is_zero() {
            return Vec::new();
        }
        let keys = self.agent_tasks().idle_lifetime_keys();
        let book = lock(&self.leases);
        keys.into_iter()
            .filter_map(|k| {
                let at = match book.activity(k.as_str()) {
                    Some((0, last)) => last + ttl,
                    Some(_) => return None,
                    None => now,
                };
                Some((k, at))
            })
            .collect()
    }

    /// 最早的任务到期时刻；没有按空闲回收的任务时为 `None`（不设定时器）。
    fn next_task_expiry(&self) -> Option<Instant> {
        self.task_expiries(Instant::now()).into_iter().map(|(_, at)| at).min()
    }

    /// 回收已空闲到期的任务：收回其仍未到期的租约（已到期的记录直接丢弃，不再发 `ttlMs: 0`），清除其状态与租约统计。
    ///
    /// @invariant 只回收 [`TaskLifetime::UntilIdle`] 的任务；legacy MCP 会话与 Hub API 会话的任务只随其结束信号回收。
    pub(crate) fn expire_idle_tasks(&self, now: Instant) {
        for (key, at) in self.task_expiries(now) {
            if at > now {
                continue;
            }
            debug_assert_eq!(key.lifetime(), TaskLifetime::UntilIdle);
            if let Some(task) = self.agent_tasks().get_mut(&key) {
                task.prune_expired_leases(now);
            }
            tracing::debug!(caller = %key, "Agent 任务请求流空闲，回收");
            self.end_task(&key);
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
            lock(&self.power).forget(&app_id, &instance_id);
            self.mark_dormant_dirty(&app_id);
            self.emit(HubEvent::AppDisconnected { app_id, instance_id });
        }
        self.mark_tools_changed();
        self.mark_resources_changed();
    }

    // ------------------------------------------------------------------
    // 休眠记录持久化（spec/hub-api.md 3.5「持久化」；格式与文件 I/O 见 crate::dormant_store）
    // ------------------------------------------------------------------

    /// 某 App 的休眠记录变化（休眠、回连取走、移除）：未配置持久化时不做任何事；否则由 [`HubShared::persist_loop`] 重写其文件。
    pub(crate) fn mark_dormant_dirty(&self, app_id: &str) {
        if let Some(p) = &self.persist {
            lock(&p.dirty).insert(app_id.to_owned());
            p.notify.notify_one();
        }
    }

    /// 启动时读回休眠记录并登记到注册表（[`Hub::start`](crate::Hub::start) 在监听之前调用）。
    pub(crate) fn load_persisted(&self) {
        let Some(p) = &self.persist else { return };
        let report = p.store.load(SystemTime::now(), self.config.dormant_ttl);
        let (mut loaded, mut expired) = (0u64, 0u64);
        for app in report.apps {
            expired += app.expired as u64;
            let n = self.registry().restore_dormant(&app.app_id, app.instances, &app.page_tools);
            loaded += n as u64;
            if app.expired > 0 {
                // 文件里还有过期实例：按内存中的记录重写。
                self.mark_dormant_dirty(&app.app_id);
            }
            tracing::info!(app_id = %app.app_id, instances = n, "读回休眠记录");
        }
        let mut st = lock(&p.status);
        st.loaded_instances = loaded;
        st.expired_instances = expired;
        st.issues = report.issues;
    }

    /// 按变化重写休眠记录文件。没有变化时只等待通知，不设定时器；未配置持久化时立即结束。
    pub(crate) async fn persist_loop(self: Arc<Self>) {
        let Some(p) = &self.persist else { return };
        loop {
            p.notify.notified().await;
            let shared = self.clone();
            // @why 文件写入是阻塞 I/O，放到阻塞线程池；同一时刻只有这一个写入方，同一 App 的写入不会乱序。
            if let Err(e) = tokio::task::spawn_blocking(move || shared.persist_flush()).await {
                tracing::error!(error = %e, "休眠记录写入任务异常结束");
            }
        }
    }

    /// 立即写出所有待写的 App（[`Hub::shutdown`](crate::Hub::shutdown) 与 [`HubShared::persist_loop`]）。阻塞。
    pub(crate) fn persist_flush(&self) {
        let Some(p) = &self.persist else { return };
        // @why 串行化：shutdown 与仍在运行的持久化任务同时写同一 App 时，后取快照的必须后写。
        let _writing = lock(&p.writing);
        let apps = std::mem::take(&mut *lock(&p.dirty));
        for app_id in apps {
            let (dormant, page_tools) = self.registry().dormant_view(&app_id);
            let content = crate::dormant_store::snapshot(&app_id, &dormant, &page_tools, SystemTime::now());
            let result = p.store.save(&app_id, content.as_ref());
            let mut st = lock(&p.status);
            match result {
                Ok(()) => st.writes += 1,
                Err(e) => {
                    tracing::warn!(app_id, error = %e, dir = %p.store.dir().display(), "写休眠记录失败");
                    st.last_error = Some(format!("{app_id}：{e}"));
                }
            }
        }
    }
}

/// 休眠记录持久化的运行状态（[`HubShared::persist`]，配置了 `HubConfig::state_dir` 时存在）。
///
/// @invariant 只有 [`HubShared::persist_flush`] 写文件（持久化任务与 `shutdown` 都经它，持 `writing` 串行执行；`dirty` 取出即清空）。
pub(crate) struct Persist {
    store: DormantStore,
    writing: std::sync::Mutex<()>,
    /// 待重写的 appId。
    dirty: std::sync::Mutex<BTreeSet<String>>,
    notify: Notify,
    status: std::sync::Mutex<DormantStoreStatus>,
}

impl Persist {
    pub(crate) fn new(state_dir: &Path) -> Self {
        let store = DormantStore::new(state_dir);
        let status = DormantStoreStatus { dir: store.dir().display().to_string(), ..Default::default() };
        Self {
            store,
            writing: std::sync::Mutex::new(()),
            dirty: Default::default(),
            notify: Notify::new(),
            status: std::sync::Mutex::new(status),
        }
    }

    pub(crate) fn status(&self) -> DormantStoreStatus {
        lock(&self.status).clone()
    }
}

/// 发送 `app/lease`；`adaptive` 标明租约来自调用间隔统计（spec/lifecycle.md 第 13 节 B4：后台连接只认这种）。
fn send_lease(conn: &Connection, ttl: Duration, adaptive: bool) {
    let p = LeaseParams {
        ttl_ms: ttl.as_millis() as u64,
        adaptive,
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
            "已尝试唤醒 App「{app_id}」，但它在 {} 秒内没有回连。请让用户手动打开该 App 后重试；\
             App 已在运行时，也可能是系统负载过高或 App 进程优先级过低，未能及时处理唤醒（可运行 app-mcp-host doctor 查看）。",
            timeout.as_secs_f32()
        ),
    )
    .with_details(json!({ "appId": app_id }))
}

/// 唤醒速率超限的错误码（spec/protocol.md 10.1）。
const WAKE_RATE_LIMITED: &str = "WAKE_RATE_LIMITED";

fn rate_limited(app_id: &str, limit: u32, retry_after: Duration) -> ToolError {
    let secs = retry_after.as_secs_f32().ceil() as u64;
    ToolError::new(
        ErrorKind::LaunchFailed,
        format!(
            "App「{app_id}」最近一分钟内已被唤醒 {limit} 次，达到上限，本次不再唤醒。请约 {secs} 秒后重试，或让用户打开该 App 后重试。"
        ),
    )
    .with_details(json!({
        "appId": app_id,
        "code": WAKE_RATE_LIMITED,
        "retryAfterMs": retry_after.as_millis() as u64,
    }))
}

fn not_wakeable(app_id: &str, dormant: bool) -> ToolError {
    let msg = if dormant {
        format!("App「{app_id}」已休眠，但没有提供唤醒方式。请让用户打开该 App 后重试。")
    } else {
        format!("App「{app_id}」当前未运行，且清单没有声明唤醒方式。请让用户先启动该 App，待其连接后重试。")
    };
    ToolError::new(ErrorKind::AppDisconnected, msg).with_details(json!({ "appId": app_id }))
}
