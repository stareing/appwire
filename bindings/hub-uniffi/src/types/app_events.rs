//! App 事件（第 16 项 N3，spec/hub-api.md 3.17）：事件对象、`HubStatus.events` 的 uniffi 记录。

use app_mcp_hub as hub;

/// App 发出、经 Hub 去重与校验后的一个事件（[`super::HubEvent::AppEvent`]、事件回调）。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct AppEvent {
    /// Hub 分配：`ev-<n>`。
    pub id: String,
    pub app_id: String,
    pub instance_id: String,
    /// 事件名（App 内的局部名，如 `order.shipped`）。
    pub name: String,
    /// 载荷（JSON 对象文本）；App 未给出时为空。
    #[uniffi(default = None)]
    pub payload_json: Option<String>,
    /// Hub 收到时的 Unix 毫秒。
    pub at_ms: u64,
}

impl From<hub::AppEvent> for AppEvent {
    fn from(e: hub::AppEvent) -> Self {
        AppEvent {
            id: e.id,
            app_id: e.app_id,
            instance_id: e.instance_id,
            name: e.name,
            payload_json: e.payload.map(|v| v.to_string()),
            at_ms: e.at,
        }
    }
}

/// 事件订阅与丢弃统计（`HubStatus.events`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct EventsStatus {
    /// 全部订阅，按订阅方、订阅 ID 排序。
    pub subscriptions: Vec<EventSubscriptionStatus>,
    /// 因未声明、载荷不合法或超限而丢弃的事件数（启动以来）。
    pub dropped_invalid: u64,
}

impl From<hub::EventsStatus> for EventsStatus {
    fn from(s: hub::EventsStatus) -> Self {
        EventsStatus {
            subscriptions: s.subscriptions.into_iter().map(Into::into).collect(),
            dropped_invalid: s.dropped_invalid,
        }
    }
}

/// 一个订阅的状态。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct EventSubscriptionStatus {
    pub subscription_id: String,
    /// 订阅方：`agent:<名>`，或匿名调用方的调用方键（`mcp:<n>` / `principal:local` / `api` 等）。
    pub subscriber: String,
    pub app_id: String,
    /// 事件名；为空 = 该 App 的全部事件。
    #[uniffi(default = None)]
    pub event: Option<String>,
    /// 经本订阅入箱的事件数。
    pub delivered: u64,
    /// 本订阅因频率上限丢弃的事件数。
    pub dropped: u64,
    /// 订阅方信箱当前的事件数。
    pub pending: u64,
}

impl From<hub::EventSubscriptionStatus> for EventSubscriptionStatus {
    fn from(s: hub::EventSubscriptionStatus) -> Self {
        EventSubscriptionStatus {
            subscription_id: s.subscription_id,
            subscriber: s.subscriber,
            app_id: s.app_id,
            event: s.event,
            delivered: s.delivered,
            dropped: s.dropped,
            pending: u64::try_from(s.pending).unwrap_or(u64::MAX),
        }
    }
}
