//! 内置工具 `apps.events.subscribe` / `apps.events.unsubscribe` / `apps.events`、资源 `app-mcp://apps/events` 与
//! `app-mcp://apps/self` 中的信箱摘要（spec/hub-api.md 3.17）。
//!
//! @security 订阅与信箱按订阅方（[`owner_of`]）隔离：退订他人的订阅与不存在的相同（`TOOL_NOT_FOUND`），读取只见自己的信箱。

use app_mcp_protocol::{ErrorKind, EventInfo, MAX_EVENT_PAYLOAD_BYTES, ToolError, is_valid_name};
use rmcp::ErrorData as McpError;
use rmcp::model::{CallToolResult, ReadResourceResult, ResourceContents};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::call::{json_result, unknown_app};
use crate::hub::{DEFAULT_MIME, HubShared, unix_millis};
use crate::names::{TOOL_APPS_EVENTS, TOOL_APPS_EVENTS_SUBSCRIBE, TOOL_APPS_EVENTS_UNSUBSCRIBE};
use crate::task::CallerKey;

use super::inbox::{Inbox, Subscription};
use super::types::{AppEvent, EventsSummary, MAX_EVENTS_PER_FETCH};
use super::{owner_agent, owner_of};

/// 订阅在工具结果与 `app-mcp://apps/events` 中的形式。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubscriptionView {
    pub subscription_id: String,
    pub app_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<Map<String, Value>>,
    pub delivered: u64,
    pub dropped: u64,
}

fn view(s: &Subscription) -> SubscriptionView {
    SubscriptionView {
        subscription_id: s.id.clone(),
        app_id: s.app_id.clone(),
        event: s.event.clone(),
        filter: s.filter.clone(),
        delivered: s.delivered,
        dropped: s.dropped,
    }
}

fn views(inbox: Option<&Inbox>) -> Vec<SubscriptionView> {
    inbox.map(|i| i.subscriptions.iter().map(view).collect()).unwrap_or_default()
}

fn invalid(message: String) -> ToolError {
    ToolError::new(ErrorKind::InvalidInput, message)
}

/// `app-mcp://apps/events` 的内容。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventsSelfView {
    pending: usize,
    subscriptions: Vec<SubscriptionView>,
    events: Vec<AppEvent>,
}

/// 新订阅的 ID：`sub-` + 64 位随机数（持久化的订阅跨重启保留，不用进程内序号以免重启后重复）。
fn new_subscription_id() -> String {
    format!("sub-{:016x}", rand::random::<u64>())
}

impl HubShared {
    /// App 的事件目录：清单 `events` ∪ 运行时声明（`apps.tools` 的 `events`、订阅时的校验）。
    pub(crate) fn declared_events(&self, app_id: &str) -> Vec<EventInfo> {
        let manifest: Vec<EventInfo> = self.registry().manifest(app_id).map(|m| m.meta().events.clone()).unwrap_or_default();
        self.app_events.state().catalog.declared(app_id, &manifest)
    }

    /// 订阅时 appId 是否可见：有清单、快照、实例或事件声明，且未被 `hide`。
    fn events_app_visible(&self, app_id: &str) -> bool {
        let known = self.registry().has_app(app_id) || self.app_events.state().catalog.knows(app_id);
        known && !self.app_hidden(app_id)
    }

    /// `apps.events.subscribe {appId, event?, filter?}`。
    ///
    /// @error appId 未知或被隐藏 → `TOOL_NOT_FOUND`；事件名不合法、不在已有的声明中、filter 不是对象或过大 → `INVALID_INPUT`；
    /// 订阅数达 `EventLimits::max_subscriptions` → `RATE_LIMITED`（`scope: "events"`）。
    pub(crate) fn builtin_events_subscribe(&self, caller: &CallerKey, args: &Value) -> Result<CallToolResult, ToolError> {
        let app_id = args.get("appId").and_then(Value::as_str).unwrap_or_default();
        if !self.events_app_visible(app_id) {
            return Err(unknown_app(app_id));
        }
        let event = match args.get("event") {
            None | Some(Value::Null) => None,
            Some(Value::String(e)) if is_valid_name(e) => Some(e.clone()),
            Some(other) => return Err(invalid(format!("事件名 {other} 不合法，应满足 [a-zA-Z0-9_.-]{{1,64}}。"))),
        };
        let declared = self.declared_events(app_id);
        if let Some(e) = &event
            && !declared.is_empty()
            && !declared.iter().any(|d| &d.name == e)
        {
            let names: Vec<&str> = declared.iter().map(|d| d.name.as_str()).collect();
            return Err(invalid(format!("App「{app_id}」没有事件「{e}」。已声明的事件：{}。", names.join("、"))));
        }
        let filter = match args.get("filter") {
            None | Some(Value::Null) => None,
            Some(Value::Object(m)) => Some(m.clone()),
            Some(_) => return Err(invalid("filter 必须是对象（载荷顶层字段 → 期望值）。".to_owned())),
        };
        if filter.as_ref().is_some_and(|f| serde_json::to_vec(f).map_or(true, |b| b.len() > MAX_EVENT_PAYLOAD_BYTES)) {
            return Err(invalid(format!("filter 过大（上限 {MAX_EVENT_PAYLOAD_BYTES} 字节）。")));
        }
        let owner = owner_of(caller);
        let persistent = owner_agent(&owner).is_some();
        if !persistent {
            // 匿名订阅随任务回收：确保任务存在（空闲回收与会话结束都经 end_task）。
            self.agent_tasks().entry(caller);
        }
        let limit = self.config.event_limits.max_subscriptions;
        let (sub, existing) = {
            let mut st = self.app_events.state();
            let inbox = st.inbox_mut(&owner);
            match inbox.subscriptions.iter().find(|s| s.same_as(app_id, event.as_deref(), filter.as_ref())) {
                Some(s) => (view(s), true),
                None if inbox.subscriptions.len() >= limit => {
                    st.prune(&owner);
                    return Err(ToolError::new(
                        ErrorKind::RateLimited,
                        format!("你的事件订阅已达上限（{limit} 个）。请先用 {TOOL_APPS_EVENTS_UNSUBSCRIBE} 退订不再需要的订阅。"),
                    )
                    .with_details(json!({ "scope": "events", "limit": limit })));
                }
                None => {
                    let s = Subscription::new(new_subscription_id(), app_id.to_owned(), event.clone(), filter);
                    let v = view(&s);
                    inbox.subscriptions.push(s);
                    self.app_events.persist(&owner, inbox, unix_millis());
                    (v, false)
                }
            }
        };
        let what = event.as_deref().map_or_else(|| "全部事件".to_owned(), |e| format!("事件 {e}"));
        let scope = if persistent {
            "订阅与信箱属于你的 Agent 身份，会话结束后保留。"
        } else {
            "订阅随本会话 / 任务结束而删除。"
        };
        let later = if declared.is_empty() { "该 App 尚未声明任何事件，声明并发出后才会收到。" } else { "" };
        let message = format!(
            "{}App「{app_id}」的{what}。事件到达后放进你的信箱：调用 {TOOL_APPS_EVENTS} 取出；也可订阅资源 {} 在有新事件时收到提醒。{scope}{later}",
            if existing { "已有相同订阅：" } else { "已订阅 " },
            crate::names::RESOURCE_APPS_EVENTS_URI,
        );
        let mut body = serde_json::to_value(&sub).unwrap_or_else(|_| json!({}));
        body["existing"] = json!(existing);
        body["message"] = json!(message);
        Ok(json_result(body))
    }

    /// `apps.events.unsubscribe {subscriptionId}`：只能退订自己的；他人的与不存在的相同 → `TOOL_NOT_FOUND`。
    pub(crate) fn builtin_events_unsubscribe(&self, caller: &CallerKey, args: &Value) -> Result<CallToolResult, ToolError> {
        let id = args.get("subscriptionId").and_then(Value::as_str).unwrap_or_default();
        let owner = owner_of(caller);
        let removed = {
            let mut st = self.app_events.state();
            let removed = st.inboxes.get_mut(&owner).and_then(|inbox| {
                let pos = inbox.subscriptions.iter().position(|s| s.id == id)?;
                inbox.subscriptions.remove(pos);
                self.app_events.persist(&owner, inbox, unix_millis());
                Some(())
            });
            st.prune(&owner);
            removed.is_some()
        };
        if !removed {
            return Err(ToolError::new(
                ErrorKind::ToolNotFound,
                format!("没有属于你的事件订阅「{id}」。可调用 {TOOL_APPS_EVENTS} 查看你的订阅。"),
            )
            .with_details(json!({ "subscriptionId": id })));
        }
        Ok(json_result(json!({
            "subscriptionId": id,
            "unsubscribed": true,
            "message": format!("已退订 {id}。信箱中已有的事件仍可用 {TOOL_APPS_EVENTS} 取出。"),
        })))
    }

    /// `apps.events {max?}`：按到达顺序取出并移出最多 `max`（默认与上限 [`MAX_EVENTS_PER_FETCH`]）条。
    pub(crate) fn builtin_events_fetch(&self, caller: &CallerKey, args: &Value) -> Result<CallToolResult, ToolError> {
        let max = args.get("max").and_then(Value::as_u64).map_or(MAX_EVENTS_PER_FETCH, |m| {
            usize::try_from(m).unwrap_or(MAX_EVENTS_PER_FETCH).clamp(1, MAX_EVENTS_PER_FETCH)
        });
        let owner = owner_of(caller);
        let limits = self.config.event_limits;
        let now = unix_millis();
        let (events, dropped, subscriptions, pending) = {
            let mut st = self.app_events.state();
            let out = match st.inboxes.get_mut(&owner) {
                None => (Vec::new(), 0, Vec::new(), 0),
                Some(inbox) => {
                    let expired = inbox.expire(now, &limits);
                    let (events, dropped) = inbox.take(max);
                    if expired > 0 || !events.is_empty() || dropped > 0 {
                        self.app_events.persist(&owner, inbox, now);
                    }
                    (events, dropped, views(Some(inbox)), inbox.events.len())
                }
            };
            st.prune(&owner);
            out
        };
        let mut message = if events.is_empty() && subscriptions.is_empty() {
            format!("你还没有订阅任何事件。可用 {TOOL_APPS_EVENTS_SUBSCRIBE} 订阅某个 App 的事件（事件目录见 apps.tools 的 events）。")
        } else if events.is_empty() {
            "信箱中没有新事件。".to_owned()
        } else {
            format!("取出 {} 个事件（已从信箱移出）。", events.len())
        };
        if pending > 0 {
            message.push_str(&format!("信箱中还有 {pending} 个，可再次调用 {TOOL_APPS_EVENTS}。"));
        }
        if dropped > 0 {
            message.push_str(&format!("上次取件以来有 {dropped} 个事件因信箱已满或超出频率上限被丢弃。"));
        }
        Ok(json_result(json!({
            "events": events,
            "subscriptions": subscriptions,
            "dropped": dropped,
            "pending": pending,
            "message": message,
        })))
    }

    /// `app-mcp://apps/self` 的 `events`：读取方信箱的未取事件数与订阅数（先做 TTL 惰性清理，不移出）。
    pub(crate) fn events_summary(&self, caller: &CallerKey) -> EventsSummary {
        let owner = owner_of(caller);
        let now = unix_millis();
        let mut st = self.app_events.state();
        let Some(inbox) = st.inboxes.get_mut(&owner) else { return EventsSummary::default() };
        if inbox.expire(now, &self.config.event_limits) > 0 {
            self.app_events.persist(&owner, inbox, now);
        }
        EventsSummary { pending: inbox.events.len(), subscriptions: inbox.subscriptions.len() }
    }

    /// 读取 `app-mcp://apps/events`：读取方信箱的 `{pending, subscriptions, events}`（不移出；先做 TTL 惰性清理）。
    pub(crate) fn read_events_self(&self, uri: &str, caller: &CallerKey) -> Result<ReadResourceResult, McpError> {
        let owner = owner_of(caller);
        let limits = self.config.event_limits;
        let now = unix_millis();
        let body = {
            let mut st = self.app_events.state();
            let body = match st.inboxes.get_mut(&owner) {
                None => EventsSelfView { pending: 0, subscriptions: Vec::new(), events: Vec::new() },
                Some(inbox) => {
                    if inbox.expire(now, &limits) > 0 {
                        self.app_events.persist(&owner, inbox, now);
                    }
                    EventsSelfView {
                        pending: inbox.events.len(),
                        subscriptions: views(Some(inbox)),
                        events: inbox.events.iter().cloned().collect(),
                    }
                }
            };
            st.prune(&owner);
            body
        };
        let text = serde_json::to_string(&body).map_err(|e| McpError::internal_error(format!("序列化信箱失败：{e}"), None))?;
        Ok(ReadResourceResult::new(vec![ResourceContents::text(text, uri).with_mime_type(DEFAULT_MIME)]))
    }
}
