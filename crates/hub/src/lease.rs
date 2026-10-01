//! 自适应租约（spec/lifecycle.md 第 13 节 B2；spec/hub-api.md 3.5）：按（会话, App）统计相邻调用间隔，
//! 租约 = 最近 N 个间隔的 p90 + 余量，限制在 [下限, 上限]；无历史时用保守默认值（`HubConfig.lease_ttl`）。
//!
//! 本模块是纯状态（不做 I/O、不读时钟）：所有时刻由调用方传入，便于确定性测试。发送 `app/lease` 与
//! 收回的执行在 [`crate::lifecycle`]。
//!
//! - **间隔样本**：同一（会话, App）上一次调用完成（发出租约）到下一次调用开始的时长。超过 `max` 的间隔视为
//!   一轮对话结束后的停顿，不计入（租约本来就覆盖不到，计入只会把 p90 推到上限）。
//! - **请求流空闲**：会话没有进行中的请求，且距最近一次请求活动（开始或结束）已达 `idle_revoke`。此时收回
//!   该会话以**默认值**发出、仍未到期的租约；自适应租约本身就是对下一次调用的预测，按时到期，不提前收回。
//!
//! @invariant 统计表最多 [`MAX_LEASE_PAIRS`] 个（会话, App）与 [`MAX_LEASE_SESSIONS`] 个会话：超出时淘汰最久未活动
//! （会话：且无进行中请求）的一项；会话结束（[`LeaseBook::forget_session`]）时移除其全部统计。

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::Instant;

/// （会话, App）统计表的上限。
pub(crate) const MAX_LEASE_PAIRS: usize = 1024;

/// 会话活动表的上限。
pub(crate) const MAX_LEASE_SESSIONS: usize = 1024;

/// 至少有这么多间隔样本才按统计值发租约，否则用默认值。
///
/// @why 1–2 个样本的 p90 就是其最大值，偶然一次快速连续调用会让租约过短；3 个样本时 p90 仍取最大值，但已是
/// "连续三次都这么快"，误判代价只是一次回连（约 5.6 ms SDK 线程，TASKS.md 4e0）。
pub(crate) const MIN_LEASE_SAMPLES: usize = 3;

/// 自适应租约策略（`HubConfig.lease`）。JSON 配置形式见 [`LeaseOverrides`]。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeasePolicy {
    /// 是否按调用间隔自适应。`false` = 每次调用后固定发 `HubConfig.lease_ttl`，只在会话结束时收回（4e 之前的行为）。
    pub adaptive: bool,
    /// 统计最近多少个间隔（N ≥ 1）。
    pub window: u32,
    /// p90 之上的余量。
    pub margin: Duration,
    /// 自适应租约下限。
    pub min: Duration,
    /// 自适应租约上限（≥ `min`）；超过它的间隔不计入统计。
    pub max: Duration,
    /// 请求流空闲多久后收回该会话以默认值发出的租约；`0` = 不因空闲收回。
    pub idle_revoke: Duration,
}

impl Default for LeasePolicy {
    /// @why 上限取 60 s（= 默认 `lease_ttl`）：自适应租约永不比无历史时的保守默认值更长；余量 5 s 覆盖模型生成
    /// 参数的抖动；下限 5 s 避免一次调用完成到下一次开始之间（含模型往返）就已休眠；空闲 30 s 收回默认租约，
    /// 一组调用结束后无历史的 App 不必等满 60 s。
    fn default() -> Self {
        Self {
            adaptive: true,
            window: 20,
            margin: Duration::from_secs(5),
            min: Duration::from_secs(5),
            max: Duration::from_secs(60),
            idle_revoke: Duration::from_secs(30),
        }
    }
}

impl LeasePolicy {
    /// 校验配置（`Hub::start` 调用）；不合法时返回中文说明。
    pub fn validate(&self) -> Result<(), String> {
        if !self.adaptive {
            return Ok(());
        }
        if self.window == 0 {
            return Err("lease.window 必须 ≥ 1".to_owned());
        }
        if self.min > self.max {
            return Err(format!(
                "lease.min（{} ms）不能大于 lease.max（{} ms）",
                self.min.as_millis(),
                self.max.as_millis()
            ));
        }
        Ok(())
    }
}

/// [`LeasePolicy`] 的 JSON 配置形式（各绑定与 `app-mcp-host` 配置文件共用，spec/hub-api.md 3.5）：
/// `{"adaptive": bool, "window": N, "marginMs", "minMs", "maxMs", "idleRevokeMs"}`，缺省字段取默认值。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct LeaseOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_revoke_ms: Option<u64>,
}

impl LeaseOverrides {
    /// 把给出的字段写入策略。
    pub fn apply(&self, p: &mut LeasePolicy) {
        let ms = |target: &mut Duration, v: Option<u64>| {
            if let Some(v) = v {
                *target = Duration::from_millis(v);
            }
        };
        if let Some(v) = self.adaptive {
            p.adaptive = v;
        }
        if let Some(v) = self.window {
            p.window = v;
        }
        ms(&mut p.margin, self.margin_ms);
        ms(&mut p.min, self.min_ms);
        ms(&mut p.max, self.max_ms);
        ms(&mut p.idle_revoke, self.idle_revoke_ms);
    }

    /// 合并：`other` 中给出的字段覆盖本对象（配置文件 ← 命令行）。
    pub fn merge(&mut self, other: &LeaseOverrides) {
        macro_rules! take {
            ($($f:ident),*) => { $( if other.$f.is_some() { self.$f = other.$f; } )* };
        }
        take!(adaptive, window, margin_ms, min_ms, max_ms, idle_revoke_ms);
    }
}

/// 一次租约的决定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LeaseGrant {
    pub ttl: Duration,
    /// `true` = 按统计值；`false` = 默认值（无足够历史或未开启自适应）。
    pub adaptive: bool,
}

/// 租约统计（`HubStatus.lease`，spec/hub-api.md 3.9）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LeaseStatus {
    /// `adaptive` / `fixed`（固定 `lease_ttl`）/ `off`（`lease_ttl = 0`）。
    pub mode: String,
    /// 默认（无历史 / 固定）租约毫秒数。
    pub default_ms: u64,
    pub min_ms: u64,
    pub max_ms: u64,
    pub margin_ms: u64,
    pub window: u32,
    /// 请求流空闲收回阈值；0 = 不因空闲收回。
    pub idle_revoke_ms: u64,
    /// 已发出的自适应租约次数。
    pub adaptive_grants: u64,
    /// 已发出的默认租约次数。
    pub default_grants: u64,
    /// 因会话结束而收回的次数（每个被收回租约的实例计一次）。
    pub revoked_session_end: u64,
    /// 因请求流空闲而收回的次数。
    pub revoked_idle: u64,
    /// 当前跟踪的（会话, App）。
    pub pairs: Vec<LeasePairStatus>,
}

/// 一个（会话, App）的统计。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LeasePairStatus {
    /// 会话键（MCP 为 `mcp:<n>`，Hub API 为 `api` / `api:<session>`）。
    pub session: String,
    pub app_id: String,
    /// 窗口内的间隔样本数。
    pub samples: u32,
    /// 下一次调用完成后将发出的租约毫秒数（按当前统计）。
    pub next_ttl_ms: u64,
    /// `next_ttl_ms` 是否来自统计（样本足够）。
    pub adaptive: bool,
}

#[derive(Debug)]
struct PairStats {
    gaps: VecDeque<Duration>,
    /// 最近一次发出租约（调用完成）的时刻。
    last_grant: Option<Instant>,
    touched: Instant,
}

#[derive(Debug)]
struct SessionActivity {
    inflight: u32,
    last: Instant,
    /// 该会话以默认值发出、可能尚未到期的租约中最晚的到期时刻（空闲收回的对象）。
    default_until: Option<Instant>,
}

#[derive(Debug, Default)]
pub(crate) struct LeaseBook {
    pairs: HashMap<(String, String), PairStats>,
    sessions: HashMap<String, SessionActivity>,
    adaptive_grants: u64,
    default_grants: u64,
    revoked_session_end: u64,
    revoked_idle: u64,
}

/// 第 90 百分位（最近秩法：排序后取第 ⌈0.9·n⌉ 个）。`samples` 非空。
fn p90(samples: &VecDeque<Duration>) -> Duration {
    let mut v: Vec<Duration> = samples.iter().copied().collect();
    v.sort_unstable();
    let rank = (v.len() * 9).div_ceil(10).max(1);
    v[rank - 1]
}

fn key(session: &str, app_id: &str) -> (String, String) {
    (session.to_owned(), app_id.to_owned())
}

impl LeaseBook {
    fn pair(&mut self, session: &str, app_id: &str, now: Instant) -> &mut PairStats {
        let k = key(session, app_id);
        if !self.pairs.contains_key(&k) && self.pairs.len() >= MAX_LEASE_PAIRS {
            let oldest = self.pairs.iter().min_by_key(|(_, p)| p.touched).map(|(k, _)| k.clone());
            if let Some(o) = oldest {
                self.pairs.remove(&o);
            }
        }
        let p = self.pairs.entry(k).or_insert_with(|| PairStats {
            gaps: VecDeque::new(),
            last_grant: None,
            touched: now,
        });
        p.touched = now;
        p
    }

    fn session(&mut self, session: &str, now: Instant) -> &mut SessionActivity {
        if !self.sessions.contains_key(session) && self.sessions.len() >= MAX_LEASE_SESSIONS {
            let oldest = self
                .sessions
                .iter()
                .filter(|(_, s)| s.inflight == 0)
                .min_by_key(|(_, s)| s.last)
                .map(|(k, _)| k.clone());
            if let Some(o) = oldest {
                self.sessions.remove(&o);
            }
        }
        self.sessions.entry(session.to_owned()).or_insert(SessionActivity {
            inflight: 0,
            last: now,
            default_until: None,
        })
    }

    /// 会话对某 App 的一次调用开始：记录距上一次租约的间隔。
    pub fn call_started(&mut self, session: &str, app_id: &str, policy: &LeasePolicy, now: Instant) {
        if !policy.adaptive {
            return;
        }
        let window = policy.window.max(1) as usize;
        let max = policy.max;
        let p = self.pair(session, app_id, now);
        let Some(last) = p.last_grant.take() else { return };
        let gap = now.saturating_duration_since(last);
        if gap > max {
            return;
        }
        if p.gaps.len() >= window {
            p.gaps.pop_front();
        }
        p.gaps.push_back(gap);
    }

    /// 按当前统计应发出的租约（不改变状态）。
    fn decide(&self, session: &str, app_id: &str, policy: &LeasePolicy, default_ttl: Duration) -> LeaseGrant {
        let fallback = LeaseGrant { ttl: default_ttl, adaptive: false };
        if !policy.adaptive {
            return fallback;
        }
        match self.pairs.get(&key(session, app_id)) {
            Some(p) if p.gaps.len() >= MIN_LEASE_SAMPLES => LeaseGrant {
                ttl: (p90(&p.gaps) + policy.margin).clamp(policy.min, policy.max),
                adaptive: true,
            },
            _ => fallback,
        }
    }

    /// 调用完成：决定本次租约并记下发出时刻（下一次调用的间隔从这里算起）。
    pub fn grant(
        &mut self,
        session: &str,
        app_id: &str,
        policy: &LeasePolicy,
        default_ttl: Duration,
        now: Instant,
    ) -> LeaseGrant {
        let g = self.decide(session, app_id, policy, default_ttl);
        if policy.adaptive {
            self.pair(session, app_id, now).last_grant = Some(now);
        }
        if g.adaptive {
            self.adaptive_grants += 1;
        } else {
            self.default_grants += 1;
            if policy.adaptive {
                let until = now + g.ttl;
                let s = self.session(session, now);
                s.default_until = Some(s.default_until.map_or(until, |u| u.max(until)));
            }
        }
        g
    }

    /// 会话的一个请求开始（任意 MCP 请求 / API 调用）。
    pub fn request_started(&mut self, session: &str, now: Instant) {
        let s = self.session(session, now);
        s.inflight += 1;
        s.last = now;
    }

    /// 会话的一个请求结束。会话已结束（[`LeaseBook::forget_session`]）或被淘汰时忽略，不重新登记。
    pub fn request_finished(&mut self, session: &str, now: Instant) {
        if let Some(s) = self.sessions.get_mut(session) {
            s.inflight = s.inflight.saturating_sub(1);
            s.last = now;
        }
    }

    /// 最早的空闲收回时刻；没有需要收回的会话时为 `None`。
    pub fn next_idle_deadline(&self, policy: &LeasePolicy) -> Option<Instant> {
        if !policy.adaptive || policy.idle_revoke.is_zero() {
            return None;
        }
        self.sessions
            .values()
            .filter(|s| s.inflight == 0)
            .filter_map(|s| {
                let at = s.last + policy.idle_revoke;
                // 默认租约在空闲判定之前就到期的，不需要收回。
                s.default_until.filter(|u| *u > at).map(|_| at)
            })
            .min()
    }

    /// 取出已空闲的会话（并清除其待收回标记）。
    pub fn take_idle_sessions(&mut self, policy: &LeasePolicy, now: Instant) -> Vec<String> {
        if !policy.adaptive || policy.idle_revoke.is_zero() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (k, s) in &mut self.sessions {
            let at = s.last + policy.idle_revoke;
            if s.inflight > 0 || at > now {
                continue;
            }
            if s.default_until.take().is_some_and(|u| u > now) {
                out.push(k.clone());
            }
        }
        out.sort();
        out
    }

    /// 记录收回次数。
    pub fn count_revoked(&mut self, idle: bool, n: u64) {
        if idle {
            self.revoked_idle += n;
        } else {
            self.revoked_session_end += n;
        }
    }

    /// 会话结束：移除其全部统计。
    pub fn forget_session(&mut self, session: &str) {
        self.sessions.remove(session);
        self.pairs.retain(|(s, _), _| s != session);
    }

    pub fn status(&self, policy: &LeasePolicy, default_ttl: Duration) -> LeaseStatus {
        let mode = if default_ttl.is_zero() {
            "off"
        } else if policy.adaptive {
            "adaptive"
        } else {
            "fixed"
        };
        let mut pairs: Vec<LeasePairStatus> = self
            .pairs
            .iter()
            .map(|((s, a), p)| {
                let g = self.decide(s, a, policy, default_ttl);
                LeasePairStatus {
                    session: s.clone(),
                    app_id: a.clone(),
                    samples: p.gaps.len() as u32,
                    next_ttl_ms: g.ttl.as_millis() as u64,
                    adaptive: g.adaptive,
                }
            })
            .collect();
        pairs.sort_by(|x, y| (&x.session, &x.app_id).cmp(&(&y.session, &y.app_id)));
        LeaseStatus {
            mode: mode.to_owned(),
            default_ms: default_ttl.as_millis() as u64,
            min_ms: policy.min.as_millis() as u64,
            max_ms: policy.max.as_millis() as u64,
            margin_ms: policy.margin.as_millis() as u64,
            window: policy.window,
            idle_revoke_ms: policy.idle_revoke.as_millis() as u64,
            adaptive_grants: self.adaptive_grants,
            default_grants: self.default_grants,
            revoked_session_end: self.revoked_session_end,
            revoked_idle: self.revoked_idle,
            pairs,
        }
    }

    #[cfg(test)]
    fn sizes(&self) -> (usize, usize) {
        (self.pairs.len(), self.sessions.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: Duration = Duration::from_secs(60);

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// 在 `t` 开始调用、`t + 1s` 完成，返回租约。
    fn call(b: &mut LeaseBook, p: &LeasePolicy, t: Instant) -> LeaseGrant {
        b.call_started("s", "a", p, t);
        b.grant("s", "a", p, DEFAULT, t + secs(1))
    }

    #[test]
    fn default_until_enough_samples_then_p90_plus_margin() {
        let p = LeasePolicy::default();
        let mut b = LeaseBook::default();
        let t0 = Instant::now();
        // 第 1 次调用无历史；第 2–3 次各带来 1 个样本（间隔 = 上次完成 → 本次开始 = 4 s）
        for i in 0..3 {
            let g = call(&mut b, &p, t0 + secs(5 * i));
            assert_eq!(g, LeaseGrant { ttl: DEFAULT, adaptive: false }, "第 {i} 次");
        }
        // 第 4 次：3 个 4 s 样本 → p90 4 s + 余量 5 s = 9 s
        let g = call(&mut b, &p, t0 + secs(15));
        assert_eq!(g, LeaseGrant { ttl: secs(9), adaptive: true });
        let st = b.status(&p, DEFAULT);
        assert_eq!((st.mode.as_str(), st.adaptive_grants, st.default_grants), ("adaptive", 1, 3));
        assert_eq!((st.pairs[0].samples, st.pairs[0].next_ttl_ms), (3, 9_000));
    }

    #[test]
    fn p90_nearest_rank() {
        let d = |v: &[u64]| p90(&v.iter().map(|s| secs(*s)).collect());
        assert_eq!(d(&[3]), secs(3));
        assert_eq!(d(&[1, 2, 3]), secs(3));
        // 10 个样本：第 9 个
        assert_eq!(d(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]), secs(9));
        // 20 个：第 18 个（1 个离群值不影响）
        let mut v: Vec<u64> = vec![2; 19];
        v.push(50);
        assert_eq!(d(&v), secs(2));
    }

    #[test]
    fn clamped_to_bounds() {
        let p = LeasePolicy { min: secs(10), max: secs(30), margin: secs(0), ..Default::default() };
        let mut b = LeaseBook::default();
        let t0 = Instant::now();
        let mut t = t0;
        for _ in 0..4 {
            call(&mut b, &p, t);
            t += secs(2); // 间隔 1 s
        }
        assert_eq!(call(&mut b, &p, t).ttl, secs(10), "下限");
        // 间隔 25 s → p90 25 s + 余量 10 s = 35 s → 上限 30 s
        let p = LeasePolicy { margin: secs(10), ..p };
        let mut b = LeaseBook::default();
        let mut t = t0;
        for _ in 0..4 {
            call(&mut b, &p, t);
            t += secs(26);
        }
        assert_eq!(call(&mut b, &p, t).ttl, secs(30), "上限");
    }

    #[test]
    fn gaps_beyond_max_are_turn_breaks() {
        let p = LeasePolicy::default();
        let mut b = LeaseBook::default();
        let t0 = Instant::now();
        let mut t = t0;
        for _ in 0..4 {
            call(&mut b, &p, t);
            t += secs(3); // 间隔 2 s
        }
        // 一轮结束后停顿 10 分钟再调用：不计入，租约仍按 2 s 间隔
        t += secs(600);
        assert_eq!(call(&mut b, &p, t).ttl, secs(7));
        assert_eq!(b.status(&p, DEFAULT).pairs[0].samples, 3);
    }

    #[test]
    fn window_keeps_last_n() {
        let p = LeasePolicy { window: 3, margin: secs(0), min: secs(1), ..Default::default() };
        let mut b = LeaseBook::default();
        let t0 = Instant::now();
        let mut t = t0;
        // 3 个 20 s 间隔，再 3 个 2 s 间隔：窗口只留后 3 个
        for gap in [20, 20, 20, 2, 2, 2] {
            call(&mut b, &p, t);
            t += secs(1 + gap);
        }
        assert_eq!(call(&mut b, &p, t).ttl, secs(2));
    }

    #[test]
    fn fixed_mode_is_legacy() {
        let p = LeasePolicy { adaptive: false, ..Default::default() };
        let mut b = LeaseBook::default();
        let t0 = Instant::now();
        for i in 0..10 {
            b.request_started("s", t0 + secs(i));
            assert_eq!(call(&mut b, &p, t0 + secs(i)), LeaseGrant { ttl: DEFAULT, adaptive: false });
            b.request_finished("s", t0 + secs(i) + secs(1));
        }
        assert_eq!(b.next_idle_deadline(&p), None, "固定模式不因空闲收回");
        assert!(b.take_idle_sessions(&p, t0 + secs(3600)).is_empty());
        assert_eq!(b.status(&p, DEFAULT).mode, "fixed");
        assert_eq!(b.status(&p, Duration::ZERO).mode, "off");
        assert_eq!(b.sizes().0, 0, "固定模式不统计间隔");
    }

    #[test]
    fn idle_revokes_only_default_leases() {
        let p = LeasePolicy::default();
        let mut b = LeaseBook::default();
        let t0 = Instant::now();
        b.request_started("s", t0);
        assert!(!call(&mut b, &p, t0).adaptive);
        b.request_finished("s", t0 + secs(1));
        // 空闲 30 s（自最后活动 t0+1）后到期，早于默认租约到期（t0+61）
        assert_eq!(b.next_idle_deadline(&p), Some(t0 + secs(31)));
        assert!(b.take_idle_sessions(&p, t0 + secs(30)).is_empty(), "未到时刻");
        // 其他请求活动推迟空闲判定
        b.request_started("s", t0 + secs(20));
        assert_eq!(b.next_idle_deadline(&p), None, "有进行中请求");
        b.request_finished("s", t0 + secs(25));
        assert_eq!(b.next_idle_deadline(&p), Some(t0 + secs(55)));
        assert_eq!(b.take_idle_sessions(&p, t0 + secs(55)), vec!["s".to_string()]);
        assert_eq!(b.next_idle_deadline(&p), None, "收回后不再重复");
        assert!(b.take_idle_sessions(&p, t0 + secs(56)).is_empty());
    }

    #[test]
    fn idle_skipped_when_default_lease_expires_first() {
        let p = LeasePolicy { idle_revoke: secs(90), ..Default::default() };
        let mut b = LeaseBook::default();
        let t0 = Instant::now();
        b.request_started("s", t0);
        call(&mut b, &p, t0);
        b.request_finished("s", t0 + secs(1));
        assert_eq!(b.next_idle_deadline(&p), None);
        let p0 = LeasePolicy { idle_revoke: Duration::ZERO, ..Default::default() };
        assert_eq!(b.next_idle_deadline(&p0), None, "0 = 不因空闲收回");
    }

    #[test]
    fn forget_session_and_bounded_tables() {
        let p = LeasePolicy::default();
        let mut b = LeaseBook::default();
        let t0 = Instant::now();
        for i in 0..MAX_LEASE_PAIRS + 10 {
            let s = format!("s{i}");
            let t = t0 + Duration::from_millis(i as u64);
            b.request_started(&s, t);
            b.call_started(&s, "a", &p, t);
            b.grant(&s, "a", &p, DEFAULT, t);
            b.request_finished(&s, t);
        }
        assert_eq!(b.sizes(), (MAX_LEASE_PAIRS, MAX_LEASE_SESSIONS));
        b.forget_session(&format!("s{}", MAX_LEASE_PAIRS + 9));
        assert_eq!(b.sizes(), (MAX_LEASE_PAIRS - 1, MAX_LEASE_SESSIONS - 1));
        // 有进行中请求的会话不被淘汰
        let mut b = LeaseBook::default();
        b.request_started("busy", t0);
        for i in 0..MAX_LEASE_SESSIONS + 5 {
            b.request_started(&format!("x{i}"), t0 + Duration::from_millis(i as u64 + 1));
            b.request_finished(&format!("x{i}"), t0 + Duration::from_millis(i as u64 + 1));
        }
        assert!(b.sessions.contains_key("busy"));
    }

    #[test]
    fn overrides_json() {
        let o: LeaseOverrides = serde_json::from_str(r#"{"adaptive": false, "window": 5, "minMs": 1000, "idleRevokeMs": 0}"#).unwrap();
        let mut p = LeasePolicy::default();
        o.apply(&mut p);
        assert_eq!(
            p,
            LeasePolicy { adaptive: false, window: 5, min: secs(1), idle_revoke: Duration::ZERO, ..Default::default() }
        );
        assert!(serde_json::from_str::<LeaseOverrides>(r#"{"windowSize": 5}"#).is_err(), "未知字段报错");
        let mut a = LeaseOverrides { window: Some(3), max_ms: Some(9), ..Default::default() };
        a.merge(&LeaseOverrides { window: Some(4), ..Default::default() });
        assert_eq!((a.window, a.max_ms), (Some(4), Some(9)));
    }

    #[test]
    fn validate() {
        assert!(LeasePolicy::default().validate().is_ok());
        assert!(LeasePolicy { window: 0, ..Default::default() }.validate().is_err());
        assert!(LeasePolicy { min: secs(9), max: secs(8), ..Default::default() }.validate().is_err());
        assert!(LeasePolicy { adaptive: false, window: 0, ..Default::default() }.validate().is_ok());
    }
}
