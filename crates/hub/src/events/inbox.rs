//! 订阅与信箱（纯状态，时刻由调用方传入，不读时钟、不做 I/O）：每个订阅方一个 [`Inbox`]，含其订阅、按到达顺序的事件与
//! 上次取件以来的丢弃数。

use std::collections::VecDeque;

use serde_json::{Map, Value};

use super::types::{AppEvent, EventLimits};

/// 频率上限的滑动窗口（毫秒）。
pub(crate) const RATE_WINDOW_MS: u64 = 60_000;

/// 一个订阅。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Subscription {
    pub id: String,
    pub app_id: String,
    /// `None` = 该 App 的全部事件。
    pub event: Option<String>,
    /// 载荷顶层字段的相等匹配；`None`（或空对象，创建时归一为 `None`）= 不过滤。
    pub filter: Option<Map<String, Value>>,
    pub delivered: u64,
    pub dropped: u64,
    /// 最近一分钟内经本订阅入箱的时刻（Unix 毫秒，旧的在前）；不持久化。
    recent: VecDeque<u64>,
}

impl Subscription {
    pub fn new(id: String, app_id: String, event: Option<String>, filter: Option<Map<String, Value>>) -> Self {
        Self { id, app_id, event, filter: filter.filter(|f| !f.is_empty()), delivered: 0, dropped: 0, recent: VecDeque::new() }
    }

    /// 是否为同一 `(appId, event, filter)`（重复订阅返回已有订阅）。
    pub fn same_as(&self, app_id: &str, event: Option<&str>, filter: Option<&Map<String, Value>>) -> bool {
        let filter = filter.filter(|f| !f.is_empty());
        self.app_id == app_id && self.event.as_deref() == event && self.filter.as_ref() == filter
    }

    /// 事件是否匹配：同一 App、事件名相同（或订阅全部）、`filter` 的每个字段都与载荷顶层字段相等。
    pub fn matches(&self, ev: &AppEvent) -> bool {
        if self.app_id != ev.app_id || self.event.as_deref().is_some_and(|n| n != ev.name) {
            return false;
        }
        let Some(filter) = &self.filter else { return true };
        let Some(payload) = ev.payload.as_ref().and_then(Value::as_object) else { return false };
        filter.iter().all(|(k, v)| payload.get(k) == Some(v))
    }

    /// 频率上限是否允许再入箱一条（先清掉窗口外的记录）。`limit = 0` = 不限。
    fn rate_allows(&mut self, now_ms: u64, limit: u32) -> bool {
        let since = now_ms.saturating_sub(RATE_WINDOW_MS);
        while self.recent.front().is_some_and(|t| *t <= since) {
            self.recent.pop_front();
        }
        limit == 0 || self.recent.len() < limit as usize
    }
}

/// 一个事件对一个信箱的投递结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Offer {
    /// 没有匹配的订阅。
    NoMatch,
    /// 有匹配的订阅，但被策略拒绝（`deny`）或 App 被隐藏。
    Denied,
    /// 入箱（`evicted` = 因信箱满丢掉的最旧事件数）。
    Delivered { evicted: bool },
    /// 所有匹配的订阅都超出频率上限，丢弃。
    RateDropped,
}

/// 一个订阅方的订阅与信箱。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Inbox {
    pub subscriptions: Vec<Subscription>,
    /// 按到达顺序。
    pub events: VecDeque<AppEvent>,
    /// 上次取件以来因信箱满 / 频率上限丢弃的条数（取件后清零）。
    pub dropped: u64,
}

impl Inbox {
    /// 没有订阅也没有事件（可以删除）。
    pub fn is_empty(&self) -> bool {
        self.subscriptions.is_empty() && self.events.is_empty()
    }

    /// 惰性清理：丢掉 `at` 早于 `now_ms - ttl` 的事件，返回丢掉的条数（过期不计入 `dropped`）。
    pub fn expire(&mut self, now_ms: u64, limits: &EventLimits) -> usize {
        let ttl = u64::try_from(limits.inbox_ttl.as_millis()).unwrap_or(u64::MAX);
        let before = now_ms.saturating_sub(ttl);
        let n = self.events.len();
        self.events.retain(|e| e.at >= before);
        n - self.events.len()
    }

    /// 投递一个事件：同一事件匹配多个订阅时只入箱一次（经第一个未超频率上限的订阅）。`denied` 只在有匹配的订阅时调用。
    pub fn offer(&mut self, ev: &AppEvent, now_ms: u64, limits: &EventLimits, denied: impl FnOnce() -> bool) -> Offer {
        let matching: Vec<usize> = (0..self.subscriptions.len()).filter(|i| self.subscriptions[*i].matches(ev)).collect();
        if matching.is_empty() {
            return Offer::NoMatch;
        }
        if denied() {
            return Offer::Denied;
        }
        self.expire(now_ms, limits);
        let limit = limits.per_subscription_per_minute;
        let Some(via) = matching.iter().copied().find(|i| self.subscriptions[*i].rate_allows(now_ms, limit)) else {
            for i in matching {
                self.subscriptions[i].dropped += 1;
            }
            self.dropped += 1;
            return Offer::RateDropped;
        };
        let sub = &mut self.subscriptions[via];
        sub.recent.push_back(now_ms);
        sub.delivered += 1;
        let evicted = self.events.len() >= limits.max_inbox_events.max(1);
        if evicted {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(ev.clone());
        Offer::Delivered { evicted }
    }

    /// 取出（移出）最多 `max` 条，返回 (事件, 上次取件以来的丢弃数)；丢弃数随之清零。
    pub fn take(&mut self, max: usize) -> (Vec<AppEvent>, u64) {
        let n = max.min(self.events.len());
        let events = self.events.drain(..n).collect();
        (events, std::mem::take(&mut self.dropped))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    fn ev(n: u64, name: &str, payload: Option<Value>, at: u64) -> AppEvent {
        AppEvent { id: format!("ev-{n}"), app_id: "shop".into(), instance_id: "i1".into(), name: name.into(), payload, at }
    }

    fn sub(id: &str, event: Option<&str>, filter: Option<Value>) -> Subscription {
        let filter = filter.and_then(|f| f.as_object().cloned());
        Subscription::new(id.into(), "shop".into(), event.map(str::to_owned), filter)
    }

    fn limits(max_inbox: usize, per_minute: u32) -> EventLimits {
        EventLimits { max_inbox_events: max_inbox, per_subscription_per_minute: per_minute, ..EventLimits::default() }
    }

    #[test]
    fn filter_is_top_level_equality() {
        let s = sub("s", Some("order.shipped"), Some(json!({"status": "done", "n": {"a": 1}})));
        assert!(s.matches(&ev(1, "order.shipped", Some(json!({"status": "done", "n": {"a": 1}, "x": 2})), 0)));
        assert!(!s.matches(&ev(1, "order.shipped", Some(json!({"status": "done", "n": {"a": 2}})), 0)), "值须整体相等");
        assert!(!s.matches(&ev(1, "order.shipped", Some(json!({"status": "done"})), 0)), "缺字段不匹配");
        assert!(!s.matches(&ev(1, "order.shipped", None, 0)), "无载荷不匹配非空 filter");
        assert!(!s.matches(&ev(1, "other", Some(json!({"status": "done", "n": {"a": 1}})), 0)), "事件名不同");
        let all = sub("a", None, Some(json!({})));
        assert!(all.filter.is_none(), "空 filter 归一为不过滤");
        assert!(all.matches(&ev(1, "anything", None, 0)));
        let mut other_app = ev(1, "anything", None, 0);
        other_app.app_id = "note".into();
        assert!(!all.matches(&other_app));
    }

    #[test]
    fn same_subscription_identity() {
        let s = sub("s", Some("e"), Some(json!({"k": 1})));
        assert!(s.same_as("shop", Some("e"), json!({"k": 1}).as_object()));
        assert!(!s.same_as("shop", Some("e"), None));
        assert!(!s.same_as("shop", None, json!({"k": 1}).as_object()));
        assert!(sub("t", None, None).same_as("shop", None, json!({}).as_object()), "空 filter 与不过滤相同");
    }

    #[test]
    fn one_entry_per_event_even_if_several_subscriptions_match() {
        let mut inbox = Inbox { subscriptions: vec![sub("a", None, None), sub("b", Some("e"), None)], ..Inbox::default() };
        let l = EventLimits::default();
        assert_eq!(inbox.offer(&ev(1, "e", None, 1000), 1000, &l, || false), Offer::Delivered { evicted: false });
        assert_eq!(inbox.events.len(), 1);
        assert_eq!((inbox.subscriptions[0].delivered, inbox.subscriptions[1].delivered), (1, 0));
        assert_eq!(inbox.offer(&ev(2, "x", None, 1000), 1000, &l, || false), Offer::Delivered { evicted: false });
        let mut none = Inbox { subscriptions: vec![sub("b", Some("e"), None)], ..Inbox::default() };
        assert_eq!(none.offer(&ev(3, "x", None, 1000), 1000, &l, || panic!("无匹配时不查策略")), Offer::NoMatch);
    }

    #[test]
    fn denied_skips_without_counting() {
        let mut inbox = Inbox { subscriptions: vec![sub("a", None, None)], ..Inbox::default() };
        assert_eq!(inbox.offer(&ev(1, "e", None, 0), 0, &EventLimits::default(), || true), Offer::Denied);
        assert!(inbox.events.is_empty());
        assert_eq!((inbox.dropped, inbox.subscriptions[0].delivered, inbox.subscriptions[0].dropped), (0, 0, 0));
    }

    #[test]
    fn full_inbox_drops_oldest_and_counts() {
        let mut inbox = Inbox { subscriptions: vec![sub("a", None, None)], ..Inbox::default() };
        let l = limits(2, 0);
        for i in 1..=3 {
            inbox.offer(&ev(i, "e", None, 10), 10, &l, || false);
        }
        assert_eq!(inbox.events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["ev-2", "ev-3"]);
        assert_eq!(inbox.dropped, 1);
        let (got, dropped) = inbox.take(10);
        assert_eq!((got.len(), dropped), (2, 1));
        assert_eq!(inbox.dropped, 0, "取件后清零");
        assert!(inbox.events.is_empty());
    }

    #[test]
    fn per_subscription_rate_limit_sliding_window() {
        let mut inbox = Inbox { subscriptions: vec![sub("a", None, None)], ..Inbox::default() };
        let l = limits(100, 2);
        assert!(matches!(inbox.offer(&ev(1, "e", None, 0), 1_000, &l, || false), Offer::Delivered { .. }));
        assert!(matches!(inbox.offer(&ev(2, "e", None, 0), 2_000, &l, || false), Offer::Delivered { .. }));
        assert_eq!(inbox.offer(&ev(3, "e", None, 0), 3_000, &l, || false), Offer::RateDropped);
        assert_eq!((inbox.dropped, inbox.subscriptions[0].dropped, inbox.subscriptions[0].delivered), (1, 1, 2));
        // 窗口滑过第一条后又可入箱一条。
        assert!(matches!(inbox.offer(&ev(4, "e", None, 0), 61_000, &l, || false), Offer::Delivered { .. }));
        assert_eq!(inbox.offer(&ev(5, "e", None, 0), 61_500, &l, || false), Offer::RateDropped);
        // 另一个匹配且未超限的订阅可以接手。
        inbox.subscriptions.push(sub("b", Some("e"), None));
        assert!(matches!(inbox.offer(&ev(6, "e", None, 0), 61_600, &l, || false), Offer::Delivered { .. }));
        assert_eq!(inbox.subscriptions[1].delivered, 1);
    }

    #[test]
    fn ttl_cleanup_is_lazy() {
        let l = EventLimits { inbox_ttl: Duration::from_secs(10), ..EventLimits::default() };
        let mut inbox = Inbox { subscriptions: vec![sub("a", None, None)], ..Inbox::default() };
        inbox.offer(&ev(1, "e", None, 1_000), 1_000, &l, || false);
        inbox.offer(&ev(2, "e", None, 5_000), 5_000, &l, || false);
        assert_eq!(inbox.events.len(), 2, "没有读写时不清理");
        assert_eq!(inbox.expire(12_000, &l), 1);
        assert_eq!(inbox.events[0].id, "ev-2");
        assert_eq!(inbox.dropped, 0, "过期不计入 dropped");
        // 投递时同样先清理。
        inbox.offer(&ev(3, "e", None, 20_000), 20_000, &l, || false);
        assert_eq!(inbox.events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["ev-3"]);
    }

    #[test]
    fn take_respects_max_and_order() {
        let mut inbox = Inbox::default();
        for i in 1..=5 {
            inbox.events.push_back(ev(i, "e", None, 0));
        }
        let (got, _) = inbox.take(2);
        assert_eq!(got.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["ev-1", "ev-2"]);
        assert_eq!(inbox.events.len(), 3);
    }
}
