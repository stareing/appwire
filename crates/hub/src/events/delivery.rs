//! 连接上的 `events/sync` / `events/emit` 处理（spec/protocol.md 3.5「Host 行为」）、投递到信箱、按订阅方的提醒、
//! 信箱持久化与随会话 / 任务的回收。

use std::sync::Arc;

use app_mcp_protocol::{EventEmitParams, EventInfo, is_valid_name};

use crate::hub::{HubShared, lock, unix_millis};
use crate::policy::PolicyHook;
use crate::task::CallerKey;
use crate::types::HubEvent;

use super::catalog::{MAX_EVENT_ID_LEN, sanitize};
use super::inbox::{Inbox, Offer};
use super::store::payload_ok;
use super::types::{AppEvent, EventHandler, EventSubscriptionStatus, EventsStatus};
use super::{AGENT_OWNER_PREFIX, AppEvents, EventState, owner_agent, owner_of};

impl AppEvents {
    pub(super) fn state(&self) -> std::sync::MutexGuard<'_, EventState> {
        lock(&self.state)
    }

    /// 已登记 Agent 的信箱变化后同步写盘（`state_dir` 未设置或不是 Agent 时什么也不做）。写失败只记日志。
    ///
    /// @side-effect 文件 I/O（小文件，调用方持有状态锁，写入按变更顺序串行）。
    pub(super) fn persist(&self, owner: &str, inbox: &Inbox, now_ms: u64) {
        let (Some(store), Some(agent)) = (&self.store, owner_agent(owner)) else { return };
        if let Err(e) = store.save(agent, inbox, now_ms) {
            tracing::warn!(agent, "写入信箱文件失败：{e}");
        }
    }
}

impl EventState {
    /// 订阅方的信箱（不存在时新建）。
    pub(super) fn inbox_mut(&mut self, owner: &str) -> &mut Inbox {
        self.inboxes.entry(owner.to_owned()).or_default()
    }

    /// 信箱为空时移除。
    pub(super) fn prune(&mut self, owner: &str) {
        if self.inboxes.get(owner).is_some_and(Inbox::is_empty) {
            self.inboxes.remove(owner);
        }
    }
}

impl HubShared {
    /// 厂商回调（[`crate::Hub::set_event_handler`]）。
    pub(crate) fn set_event_handler(&self, h: Arc<dyn EventHandler>) {
        *lock(&self.app_events.handler) = Some(h);
    }

    /// `events/sync`：实例（连接 `conn_id`）的全量声明。名称不合法、重名与超出上限的条目丢弃并记日志。
    pub(crate) fn events_sync(&self, app_id: &str, conn_id: u64, cid: &str, events: Vec<EventInfo>) {
        let (events, dropped) = sanitize(events);
        if dropped > 0 {
            tracing::warn!(%cid, app_id, dropped, "events/sync 中有名称不合法、重名或超出上限的事件声明，已忽略");
        }
        self.app_events.state().catalog.sync(app_id, conn_id, events);
    }

    /// App 连接断开：移除该连接的事件声明（App 的最近一次声明保留）。
    pub(crate) fn events_disconnected(&self, app_id: &str, conn_id: u64) {
        self.app_events.state().catalog.disconnected(app_id, conn_id);
    }

    /// `events/emit`：校验（名称在该实例的运行时声明或清单中、载荷为对象且不超过 8 KiB）→ 按 `(连接, eventId)` 去重 →
    /// 厂商回调与 [`HubEvent::AppEvent`] → 投递。不合法的丢弃、记日志并计入 `dropped_invalid`；重复的静默丢弃。不回复（通知）。
    pub(crate) fn event_emit(self: &Arc<Self>, app_id: &str, instance_id: &str, conn_id: u64, cid: &str, p: EventEmitParams) {
        let in_manifest = self.registry().manifest(app_id).is_some_and(|m| m.meta().events.iter().any(|e| e.name == p.name));
        let event = {
            let mut st = self.app_events.state();
            let id_ok = !p.event_id.is_empty() && p.event_id.len() <= MAX_EVENT_ID_LEN;
            let declared = in_manifest || st.catalog.conn_declares(app_id, conn_id, &p.name);
            let problem = match () {
                _ if !is_valid_name(&p.name) => Some("事件名不合法"),
                _ if !id_ok => Some("eventId 为空或过长"),
                _ if !declared => Some("事件未声明（不在该实例的 events/sync 与清单 events 中）"),
                _ if !payload_ok(p.payload.as_ref()) => Some("载荷不是 JSON 对象或超过 8 KiB"),
                _ => None,
            };
            if let Some(problem) = problem {
                st.dropped_invalid += 1;
                tracing::warn!(%cid, app_id, instance_id, event = %p.name, "丢弃事件：{problem}");
                return;
            }
            if !st.catalog.first_seen(conn_id, &p.event_id) {
                tracing::debug!(%cid, app_id, instance_id, event_id = %p.event_id, "重复的事件，忽略");
                return;
            }
            let id = format!("ev-{}", st.next_event);
            st.next_event += 1;
            AppEvent {
                id,
                app_id: app_id.to_owned(),
                instance_id: instance_id.to_owned(),
                name: p.name,
                payload: p.payload,
                at: unix_millis(),
            }
        };
        let handler = lock(&self.app_events.handler).clone();
        if let Some(h) = handler {
            h.on_event(&event);
        }
        self.emit(HubEvent::AppEvent(event.clone()));
        self.deliver_event(&event);
    }

    /// 把事件放进每个有匹配订阅的信箱（同一事件对同一订阅方只入箱一次）；App 被隐藏或对订阅方 `deny`（按订阅方的
    /// Agent 名匹配，执行点 `call`，事件名按工具名模式匹配）时跳过。之后提醒订阅了 `app-mcp://apps/events` 的订阅方。
    pub(crate) fn deliver_event(self: &Arc<Self>, event: &AppEvent) {
        let policy = self.policy();
        let hidden = policy.app_hidden(&event.app_id).is_some();
        let limits = self.config.event_limits;
        let now = unix_millis();
        let mut hits = Vec::new();
        let mut notify = Vec::new();
        {
            let mut st = self.app_events.state();
            for (owner, inbox) in st.inboxes.iter_mut() {
                let agent = owner_agent(owner);
                let denied = || {
                    if hidden || policy.is_empty() {
                        return hidden;
                    }
                    let hit = policy.denied(PolicyHook::Call, &event.app_id, Some((&event.name, None)), agent);
                    hits.extend(hit);
                    hit.is_some()
                };
                match inbox.offer(event, now, &limits, denied) {
                    Offer::Delivered { .. } => {
                        self.app_events.persist(owner, inbox, now);
                        notify.push(owner.clone());
                    }
                    // @why 频率上限丢弃不写盘：事件风暴时不放大写入（丢弃计数随下一次变更写入）。
                    Offer::RateDropped | Offer::NoMatch | Offer::Denied => {}
                }
            }
        }
        for i in hits {
            self.policy_hit(&policy, i);
        }
        self.notify_event_watchers(&notify);
    }

    /// 订阅了 `app-mcp://apps/events` 的通知订阅方中，订阅方键在 `owners` 内的发 `resources/updated`（不通知他人）。
    pub(super) fn notify_event_watchers(self: &Arc<Self>, owners: &[String]) {
        if owners.is_empty() {
            return;
        }
        let ids: Vec<u64> =
            self.app_events.state().watchers.iter().filter(|(_, o)| owners.contains(o)).map(|(id, _)| *id).collect();
        if !ids.is_empty() {
            self.send_resource_updated(&ids, crate::names::RESOURCE_APPS_EVENTS_URI);
        }
    }

    /// 通知订阅方 `id` 订阅了 `app-mcp://apps/events`，以 `caller` 的信箱为准。
    pub(crate) fn watch_events_self(&self, id: u64, caller: &CallerKey) {
        self.app_events.state().watchers.insert(id, owner_of(caller));
    }

    pub(crate) fn unwatch_events_self(&self, id: u64) {
        self.app_events.state().watchers.remove(&id);
    }

    /// 会话 / 任务结束（[`HubShared::end_task`]）：按调用方键归属的订阅方（非已登记 Agent）的订阅与信箱一并删除。
    /// 已登记 Agent 的保留（P4：下次接触时取件）。
    pub(crate) fn events_end_owner(&self, key: &CallerKey) {
        if key.agent().is_some() {
            return;
        }
        if self.app_events.state().inboxes.remove(key.as_str()).is_some() {
            tracing::debug!(caller = %key, "回收调用方的事件订阅与信箱");
        }
    }

    /// `HubStatus.events`。
    pub(crate) fn events_status(&self) -> EventsStatus {
        let st = self.app_events.state();
        let subscriptions = st
            .inboxes
            .iter()
            .flat_map(|(owner, inbox)| {
                inbox.subscriptions.iter().map(move |s| EventSubscriptionStatus {
                    subscription_id: s.id.clone(),
                    subscriber: owner.clone(),
                    app_id: s.app_id.clone(),
                    event: s.event.clone(),
                    delivered: s.delivered,
                    dropped: s.dropped,
                    pending: inbox.events.len(),
                })
            })
            .collect();
        EventsStatus { subscriptions, dropped_invalid: st.dropped_invalid }
    }

    /// 启动时读回已登记 Agent 的信箱（`state_dir` 设置时）；事件序号从读回的最大序号之后继续。
    pub(crate) fn load_inboxes(&self) {
        let Some(store) = &self.app_events.store else { return };
        let loaded = store.load(unix_millis(), &self.config.event_limits);
        let mut st = self.app_events.state();
        for (agent, inbox) in loaded {
            let max_seq = inbox.events.iter().filter_map(|e| e.id.strip_prefix("ev-")?.parse::<u64>().ok()).max();
            if let Some(n) = max_seq {
                st.next_event = st.next_event.max(n.saturating_add(1));
            }
            tracing::info!(%agent, subscriptions = inbox.subscriptions.len(), pending = inbox.events.len(), "读回 Agent 的事件信箱");
            st.inboxes.insert(format!("{AGENT_OWNER_PREFIX}{agent}"), inbox);
        }
    }
}
