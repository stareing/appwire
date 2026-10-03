//! 事件的公开类型（spec/hub-api.md 3.17）：事件对象、上限配置、厂商回调与状态。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// [`EventLimits::max_subscriptions`] 的默认值。
pub const DEFAULT_MAX_EVENT_SUBSCRIPTIONS: usize = 32;
/// [`EventLimits::max_inbox_events`] 的默认值。
pub const DEFAULT_MAX_INBOX_EVENTS: usize = 100;
/// [`EventLimits::inbox_ttl`] 的默认值。
pub const DEFAULT_INBOX_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// [`EventLimits::per_subscription_per_minute`] 的默认值。
pub const DEFAULT_EVENTS_PER_SUBSCRIPTION_PER_MINUTE: u32 = 60;
/// `apps.events {max}` 一次最多取出的条数（也是 `max` 的默认值）。
pub const MAX_EVENTS_PER_FETCH: usize = 100;

/// App 发出、经 Hub 校验后的一个事件（[`crate::HubEvent::AppEvent`]、`apps.events`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEvent {
    /// Hub 分配：`ev-<n>`。
    pub id: String,
    pub app_id: String,
    pub instance_id: String,
    /// 事件名（App 内的局部名，如 `order.shipped`）。
    pub name: String,
    /// 载荷（JSON 对象）；App 未给出时省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    /// Hub 收到时的 Unix 毫秒。
    pub at: u64,
}

/// 事件信箱的资源上限（`HubConfig::event_limits`，B-07）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventLimits {
    /// 每个订阅方最多的订阅数；超出时 `apps.events.subscribe` 返回 `RATE_LIMITED`（`scope: "events"`）。
    pub max_subscriptions: usize,
    /// 每个信箱最多的事件数（至少按 1 处理）；满时丢最旧并计入 `dropped`。
    pub max_inbox_events: usize,
    /// 信箱中事件的保留时长；过期的在下次读写该信箱时清理（不加定时器）。
    pub inbox_ttl: Duration,
    /// 每个订阅每分钟（滑动窗口）最多入箱的事件数；超出丢弃并计数。`0` = 不限。
    pub per_subscription_per_minute: u32,
}

impl Default for EventLimits {
    fn default() -> Self {
        Self {
            max_subscriptions: DEFAULT_MAX_EVENT_SUBSCRIPTIONS,
            max_inbox_events: DEFAULT_MAX_INBOX_EVENTS,
            inbox_ttl: DEFAULT_INBOX_TTL,
            per_subscription_per_minute: DEFAULT_EVENTS_PER_SUBSCRIPTION_PER_MINUTE,
        }
    }
}

/// 厂商 / 机主的事件回调（[`crate::Hub::set_event_handler`]）。
pub trait EventHandler: Send + Sync {
    /// 每个通过校验的事件（不论有无订阅）同步回调一次。
    ///
    /// @invariant 在 Hub 的 App 连接任务上执行：应很快返回，耗时工作请转交其他线程。
    fn on_event(&self, event: &AppEvent);
}

/// 事件订阅与丢弃统计（`HubStatus.events`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsStatus {
    /// 全部订阅，按订阅方、订阅 ID 排序。
    pub subscriptions: Vec<EventSubscriptionStatus>,
    /// 因未声明、载荷不合法或超限而丢弃的事件数（启动以来）。
    pub dropped_invalid: u64,
}

/// 一个订阅的状态。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventSubscriptionStatus {
    pub subscription_id: String,
    /// 订阅方：`agent:<名>`，或匿名调用方的调用方键（`mcp:<n>` / `principal:local` / `api` 等）。
    pub subscriber: String,
    pub app_id: String,
    /// 事件名；`None` = 该 App 的全部事件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// 经本订阅入箱的事件数。
    pub delivered: u64,
    /// 本订阅因频率上限丢弃的事件数。
    pub dropped: u64,
    /// 订阅方信箱当前的事件数。
    pub pending: usize,
}

/// 读取方信箱的摘要（`app-mcp://apps/self` 的 `events`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsSummary {
    /// 信箱中未取出的事件数。
    pub pending: usize,
    /// 订阅数。
    pub subscriptions: usize,
}
