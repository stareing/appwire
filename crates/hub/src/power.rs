//! 功耗观测与唤醒速率上限（spec/lifecycle.md 第 12 节；spec/hub-api.md 3.9）。
//!
//! - 每实例计数（按 `(appId, instanceId)`，跨重连 / 休眠保留）：连接次数、以该实例为目标的唤醒激活次数、
//!   累计在线时长、心跳次数（Hub 发出的 `ping` + 收到 SDK 的 `ping`），以及 SDK 在 `app/hello` 中声明的
//!   `heartbeatMs` / `lifecycleMode`。
//! - 每 App：实际发出的唤醒激活次数，以及 60 秒滑动窗口内的激活预约（[`PowerBook::reserve_wake`]）。
//!
//! @invariant 计数表最多 [`MAX_TRACKED_INSTANCES`] 项：超出时淘汰最久未活动、当前不在线的实例；
//! 休眠记录被移除（过期 / 被新实例替换）时同步移除其计数。

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use app_mcp_protocol::LifecycleMode;
use tokio::time::Instant;

use crate::types::InstancePower;

/// 计数表的实例上限。
pub(crate) const MAX_TRACKED_INSTANCES: usize = 1024;

/// 唤醒速率窗口。
pub(crate) const WAKE_RATE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Live {
    conn_id: u64,
    since: Instant,
}

#[derive(Debug)]
struct InstanceCounters {
    connections: u64,
    wakes: u64,
    /// 已结束连接的在线时长之和。
    online_closed: Duration,
    heartbeats: u64,
    heartbeat_ms: Option<u64>,
    lifecycle_mode: Option<LifecycleMode>,
    live: Option<Live>,
    touched: Instant,
}

impl InstanceCounters {
    fn new(now: Instant) -> Self {
        Self {
            connections: 0,
            wakes: 0,
            online_closed: Duration::ZERO,
            heartbeats: 0,
            heartbeat_ms: None,
            lifecycle_mode: None,
            live: None,
            touched: now,
        }
    }

    fn close_live(&mut self, now: Instant) {
        if let Some(l) = self.live.take() {
            self.online_closed += now.saturating_duration_since(l.since);
        }
    }
}

#[derive(Debug, Default)]
struct AppWakes {
    total: u64,
    /// 窗口内的激活预约时刻（升序）。
    window: VecDeque<Instant>,
}

/// 唤醒速率超限：距窗口内最早一次激活滑出窗口还需等待的时长。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RateLimited {
    pub retry_after: Duration,
}

#[derive(Debug, Default)]
pub(crate) struct PowerBook {
    instances: HashMap<(String, String), InstanceCounters>,
    apps: HashMap<String, AppWakes>,
}

impl PowerBook {
    fn entry(&mut self, app_id: &str, instance_id: &str, now: Instant) -> &mut InstanceCounters {
        let key = (app_id.to_owned(), instance_id.to_owned());
        if !self.instances.contains_key(&key) {
            self.evict_if_full();
        }
        let c = self.instances.entry(key).or_insert_with(|| InstanceCounters::new(now));
        c.touched = now;
        c
    }

    fn evict_if_full(&mut self) {
        if self.instances.len() < MAX_TRACKED_INSTANCES {
            return;
        }
        let oldest = self
            .instances
            .iter()
            .filter(|(_, c)| c.live.is_none())
            .min_by_key(|(_, c)| c.touched)
            .map(|(k, _)| k.clone());
        if let Some(k) = oldest {
            self.instances.remove(&k);
        }
    }

    /// 实例完成握手（新连接）。同一实例的旧连接若仍记为在线，先结束其在线区间（被新连接替换）。
    pub fn connected(
        &mut self,
        app_id: &str,
        instance_id: &str,
        conn_id: u64,
        heartbeat_ms: Option<u64>,
        lifecycle_mode: Option<LifecycleMode>,
        now: Instant,
    ) {
        let c = self.entry(app_id, instance_id, now);
        c.close_live(now);
        c.connections += 1;
        c.heartbeat_ms = heartbeat_ms;
        c.lifecycle_mode = lifecycle_mode;
        c.live = Some(Live { conn_id, since: now });
    }

    /// 连接结束（断开或休眠后关闭）。只结束同一连接的在线区间。
    pub fn disconnected(&mut self, app_id: &str, instance_id: &str, conn_id: u64, now: Instant) {
        if let Some(c) = self.instances.get_mut(&(app_id.to_owned(), instance_id.to_owned()))
            && c.live.as_ref().is_some_and(|l| l.conn_id == conn_id)
        {
            c.close_live(now);
            c.touched = now;
        }
    }

    /// 一次心跳（Hub 发出 `ping` 或收到 SDK 的 `ping`）。
    pub fn heartbeat(&mut self, app_id: &str, instance_id: &str, now: Instant) {
        self.entry(app_id, instance_id, now).heartbeats += 1;
    }

    /// 移除实例计数（休眠记录过期 / 被替换）；仍在线的保留。
    pub fn forget(&mut self, app_id: &str, instance_id: &str) {
        let key = (app_id.to_owned(), instance_id.to_owned());
        if self.instances.get(&key).is_some_and(|c| c.live.is_none()) {
            self.instances.remove(&key);
        }
    }

    /// 预约一次唤醒激活：`limit` 为每 [`WAKE_RATE_WINDOW`] 的上限（0 = 不限）。成功返回预约时刻
    /// （激活未发出时用 [`PowerBook::cancel_wake`] 撤销）。
    pub fn reserve_wake(&mut self, app_id: &str, limit: u32, now: Instant) -> Result<Instant, RateLimited> {
        let w = self.apps.entry(app_id.to_owned()).or_default();
        while w.window.front().is_some_and(|t| now.saturating_duration_since(*t) >= WAKE_RATE_WINDOW) {
            w.window.pop_front();
        }
        if limit > 0 && w.window.len() >= limit as usize {
            let oldest = w.window.front().copied().unwrap_or(now);
            return Err(RateLimited { retry_after: (oldest + WAKE_RATE_WINDOW).saturating_duration_since(now) });
        }
        w.window.push_back(now);
        Ok(now)
    }

    /// 撤销预约（激活任务开始前唤醒已被认领，没有发出激活）。
    pub fn cancel_wake(&mut self, app_id: &str, at: Instant) {
        if let Some(w) = self.apps.get_mut(app_id)
            && let Some(i) = w.window.iter().position(|t| *t == at)
        {
            w.window.remove(i);
        }
    }

    /// 激活已发出：计入 App 总数，目标为休眠实例时也计入该实例。
    pub fn wake_activated(&mut self, app_id: &str, instance_id: Option<&str>, now: Instant) {
        self.apps.entry(app_id.to_owned()).or_default().total += 1;
        if let Some(i) = instance_id {
            self.entry(app_id, i, now).wakes += 1;
        }
    }

    /// 该 App 实际发出的唤醒激活次数。
    pub fn app_wakes(&self, app_id: &str) -> u64 {
        self.apps.get(app_id).map_or(0, |w| w.total)
    }

    /// 实例的功耗观测（不含"未能休眠的原因"，由调用方填写）。
    pub fn instance(&self, app_id: &str, instance_id: &str, now: Instant) -> Option<InstancePower> {
        let c = self.instances.get(&(app_id.to_owned(), instance_id.to_owned()))?;
        let live = c.live.as_ref().map_or(Duration::ZERO, |l| now.saturating_duration_since(l.since));
        Some(InstancePower {
            reconnects: c.connections.saturating_sub(1),
            wakes: c.wakes,
            online_secs: (c.online_closed + live).as_secs(),
            heartbeats: c.heartbeats,
            heartbeat_ms: c.heartbeat_ms,
            lifecycle_mode: c.lifecycle_mode,
            awake_reasons: Vec::new(),
        })
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.instances.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_survive_reconnects() {
        let mut b = PowerBook::default();
        let t0 = Instant::now();
        b.connected("a", "i", 1, Some(0), Some(LifecycleMode::Idle), t0);
        b.heartbeat("a", "i", t0);
        b.disconnected("a", "i", 1, t0 + Duration::from_secs(10));
        b.connected("a", "i", 2, Some(15_000), None, t0 + Duration::from_secs(20));
        // 旧连接晚到的断开不影响新连接
        b.disconnected("a", "i", 1, t0 + Duration::from_secs(21));
        let p = b.instance("a", "i", t0 + Duration::from_secs(25)).unwrap();
        assert_eq!((p.reconnects, p.online_secs, p.heartbeats), (1, 15, 1));
        assert_eq!((p.heartbeat_ms, p.lifecycle_mode), (Some(15_000), None));
        // 同一实例的新连接替换仍在线的旧连接：结束旧区间
        b.connected("a", "i", 3, None, None, t0 + Duration::from_secs(30));
        let p = b.instance("a", "i", t0 + Duration::from_secs(30)).unwrap();
        assert_eq!((p.reconnects, p.online_secs), (2, 20));
        b.forget("a", "i");
        assert!(b.instance("a", "i", t0).is_some(), "在线实例不移除");
    }

    #[test]
    fn wake_rate_window() {
        let mut b = PowerBook::default();
        let t0 = Instant::now();
        let r1 = b.reserve_wake("a", 2, t0).unwrap();
        b.reserve_wake("a", 2, t0 + Duration::from_secs(10)).unwrap();
        let e = b.reserve_wake("a", 2, t0 + Duration::from_secs(20)).unwrap_err();
        assert_eq!(e.retry_after, Duration::from_secs(40));
        assert!(b.reserve_wake("b", 2, t0).is_ok(), "按 App 分别计数");
        // 撤销未发出的激活后可以再预约
        b.cancel_wake("a", r1);
        assert!(b.reserve_wake("a", 2, t0 + Duration::from_secs(20)).is_ok());
        assert!(b.reserve_wake("a", 2, t0 + Duration::from_secs(21)).is_err());
        // 窗口滑过后恢复
        assert!(b.reserve_wake("a", 2, t0 + Duration::from_secs(71)).is_ok());
        // 0 = 不限
        for s in 0..100 {
            assert!(b.reserve_wake("c", 0, t0 + Duration::from_millis(s)).is_ok());
        }
        b.wake_activated("a", Some("i"), t0);
        b.wake_activated("a", None, t0);
        assert_eq!(b.app_wakes("a"), 2);
        assert_eq!(b.instance("a", "i", t0).unwrap().wakes, 1);
    }

    #[test]
    fn bounded_table_evicts_offline() {
        let mut b = PowerBook::default();
        let t0 = Instant::now();
        b.connected("a", "live", 1, None, None, t0);
        for i in 0..MAX_TRACKED_INSTANCES + 10 {
            b.heartbeat("a", &format!("i{i}"), t0 + Duration::from_millis(i as u64 + 1));
        }
        assert_eq!(b.len(), MAX_TRACKED_INSTANCES);
        assert!(b.instance("a", "live", t0).is_some(), "在线实例不被淘汰");
    }
}
