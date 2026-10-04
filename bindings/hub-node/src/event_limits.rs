//! 配置 `eventLimits`（spec/hub-api.md 3.17）：事件信箱上限的可选覆盖。

use std::time::Duration;

use app_mcp_hub::EventLimits;
use serde::Deserialize;

/// [`EventLimits`] 的可选覆盖（camelCase）；缺省字段沿用默认值（32 / 100 / 24 h / 60）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EventLimitOverrides {
    /// 每个订阅方最多的订阅数。
    max_subscriptions: Option<usize>,
    /// 每个信箱最多的事件数（Hub 至少按 1 处理）。
    max_inbox_events: Option<usize>,
    /// 信箱中事件的保留时长（毫秒）。
    inbox_ttl_ms: Option<u64>,
    /// 每个订阅每分钟最多入箱的事件数；`0` = 不限。
    per_subscription_per_minute: Option<u32>,
}

impl EventLimitOverrides {
    pub(crate) fn apply(&self, target: &mut EventLimits) {
        if let Some(v) = self.max_subscriptions {
            target.max_subscriptions = v;
        }
        if let Some(v) = self.max_inbox_events {
            target.max_inbox_events = v;
        }
        if let Some(ms) = self.inbox_ttl_ms {
            target.inbox_ttl = Duration::from_millis(ms);
        }
        if let Some(v) = self.per_subscription_per_minute {
            target.per_subscription_per_minute = v;
        }
    }
}
