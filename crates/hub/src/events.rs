//! 事件、订阅与信箱（第 16 项 N3 + P4，spec/hub-api.md 3.17；协议 spec/protocol.md 3.5）。
//!
//! App 经 `events/sync` 声明、`events/emit` 发出事件；Hub 去重、校验后交给厂商回调与 [`crate::HubEvent::AppEvent`]，再投递到
//! 匹配订阅的**信箱**。Agent 用内置工具 `apps.events.subscribe` / `apps.events.unsubscribe` / `apps.events` 订阅与取件，
//! 资源 `app-mcp://apps/events` 的 `resources/updated` 只作提醒。
//!
//! - [`catalog`]：事件目录（清单 ∪ 运行时声明）与去重窗口。
//! - [`inbox`]：订阅、信箱、频率上限、TTL 惰性清理（纯状态）。
//! - [`store`]：已登记 Agent 信箱的持久化（P4）。
//! - [`delivery`]：连接上的 `events/*` 处理、投递、提醒与回收。
//! - [`builtin`]：内置工具、`app-mcp://apps/events` 与 `apps/self` 的摘要。
//!
//! 订阅方（[`owner_of`]）：已登记 Agent 按主体 `agent:<名>`（同名 Agent 的所有会话 / 任务共享，会话结束后保留并持久化）；
//! 其余调用方按调用方键，随会话 / 任务回收（[`crate::hub::HubShared::end_task`]）一并删除。
//!
//! @invariant 微内核与原则 4：不新增定时器、后台任务或连接；投递事件不唤醒 App 或 Agent；Hub 不代 Agent 发起调用。

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::task::CallerKey;

mod builtin;
mod catalog;
mod delivery;
mod inbox;
mod store;
#[cfg(test)]
mod tests;
mod types;

pub use store::{INBOX_DIR, INBOX_STORE_VERSION, MAX_INBOX_FILE_BYTES};
pub use types::{
    AppEvent, DEFAULT_EVENTS_PER_SUBSCRIPTION_PER_MINUTE, DEFAULT_INBOX_TTL, DEFAULT_MAX_EVENT_SUBSCRIPTIONS,
    DEFAULT_MAX_INBOX_EVENTS, EventHandler, EventLimits, EventSubscriptionStatus, EventsStatus, EventsSummary,
    MAX_EVENTS_PER_FETCH,
};

/// 已登记 Agent 订阅方的前缀（`agent:<名>`，与记账主体相同）。
const AGENT_OWNER_PREFIX: &str = "agent:";

/// 调用方的订阅方键：已登记 Agent → `agent:<名>`；其余 → 调用方键（`mcp:<n>`、`principal:local`、任务句柄、`api` 等）。
///
/// @invariant 调用方键不以 `agent:` 开头（见 [`crate::task`]），两类订阅方不会重名。
pub(crate) fn owner_of(caller: &CallerKey) -> String {
    match caller.agent() {
        Some(a) => format!("{AGENT_OWNER_PREFIX}{a}"),
        None => caller.as_str().to_owned(),
    }
}

/// 订阅方是已登记 Agent 时的名字（其信箱持久化、按其名匹配策略规则）。
pub(crate) fn owner_agent(owner: &str) -> Option<&str> {
    owner.strip_prefix(AGENT_OWNER_PREFIX)
}

/// 事件相关的可变状态（一把锁）。
#[derive(Default)]
pub(crate) struct EventState {
    catalog: catalog::Catalog,
    /// 订阅方 → 信箱（有序：状态按订阅方排序）。
    inboxes: BTreeMap<String, inbox::Inbox>,
    /// 订阅了 `app-mcp://apps/events` 的通知订阅方 ID（legacy 会话 / listen 流 / Hub API）→ 订阅方键。
    watchers: HashMap<u64, String>,
    /// 下一个事件序号（`ev-<n>`）；读回持久化的信箱后从其中最大的序号之后开始。
    next_event: u64,
    /// 因未声明、载荷不合法或超限而丢弃的事件数。
    dropped_invalid: u64,
}

/// Hub 中的事件部分（[`crate::hub::HubShared`] 持有一份）。
pub(crate) struct AppEvents {
    state: Mutex<EventState>,
    handler: Mutex<Option<Arc<dyn EventHandler>>>,
    /// `HubConfig::state_dir` 设置时的信箱持久化。
    store: Option<store::InboxStore>,
}

impl AppEvents {
    pub(crate) fn new(state_dir: Option<&Path>) -> Self {
        Self {
            state: Mutex::new(EventState { next_event: 1, ..EventState::default() }),
            handler: Mutex::new(None),
            store: state_dir.map(store::InboxStore::new),
        }
    }
}

/// `resources/list` 中的 `app-mcp://apps/events`。
#[cfg(feature = "mcp-server")]
pub(crate) fn events_self_resource() -> rmcp::model::Resource {
    rmcp::model::Resource::new(crate::names::RESOURCE_APPS_EVENTS_URI, "apps.events")
        .with_description(
            "你的事件信箱（只读，不移出）：订阅、未取出的事件。订阅本资源后信箱有新事件时收到 resources/updated 提醒；\
             取出事件用 apps.events。",
        )
        .with_mime_type(crate::hub::DEFAULT_MIME)
}
