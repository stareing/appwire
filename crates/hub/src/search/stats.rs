//! 按（记账主体, 工具全名）的使用统计（spec/hub-api.md 3.18「使用统计」）：最近调用时刻、调用数、成功数。
//!
//! 纯状态：不做 I/O、不读时钟（时刻由调用方传入），只在内存，Hub 重启清零。
//!
//! @invariant 每个主体至多 [`MAX_TOOLS_PER_SUBJECT`] 个工具，满时淘汰最久未用的；主体至多 [`MAX_STATS_SUBJECTS`] 个，
//! 满时淘汰最久未用的主体（B-07）。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::usage::MAX_USAGE_SUBJECTS;

/// 每个主体统计的工具数上限。
pub(crate) const MAX_TOOLS_PER_SUBJECT: usize = 512;
/// 主体数上限（与记账主体同一上限）。
pub(crate) const MAX_STATS_SUBJECTS: usize = MAX_USAGE_SUBJECTS;
/// "最近用过"的时间窗。
pub(crate) const RECENT_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// 一个工具的统计。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ToolStat {
    pub last_used: Instant,
    pub calls: u64,
    pub successes: u64,
}

impl ToolStat {
    /// `now` 时是否在 [`RECENT_WINDOW`] 内用过。
    pub fn used_within_window(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_used) < RECENT_WINDOW
    }

    /// 成功率；没有调用时为 `None`。
    pub fn success_rate(&self) -> Option<f64> {
        // @why u64 → f64 只用于比较阈值，调用数远小于 2^53，无精度问题。
        #[allow(clippy::cast_precision_loss)]
        (self.calls > 0).then(|| self.successes as f64 / self.calls as f64)
    }
}

#[derive(Debug, Default)]
struct SubjectStats {
    last_used: Option<Instant>,
    tools: HashMap<String, ToolStat>,
}

/// 使用统计表（`HubShared` 持有，一把锁）。
#[derive(Debug, Default)]
pub(crate) struct SearchStats {
    subjects: HashMap<String, SubjectStats>,
}

impl SearchStats {
    /// 记一次调用结束：`success` 为 `false` 表示错误或 `isError` 结果。
    pub fn record(&mut self, subject: &str, tool: &str, success: bool, now: Instant) {
        if !self.subjects.contains_key(subject) && self.subjects.len() >= MAX_STATS_SUBJECTS {
            evict_oldest(&mut self.subjects, |s| s.last_used);
        }
        let book = self.subjects.entry(subject.to_owned()).or_default();
        book.last_used = Some(now);
        if !book.tools.contains_key(tool) && book.tools.len() >= MAX_TOOLS_PER_SUBJECT {
            evict_oldest(&mut book.tools, |t| Some(t.last_used));
        }
        let stat = book.tools.entry(tool.to_owned()).or_insert(ToolStat { last_used: now, calls: 0, successes: 0 });
        stat.last_used = now;
        stat.calls = stat.calls.saturating_add(1);
        stat.successes = stat.successes.saturating_add(u64::from(success));
    }

    /// 主体对某个工具的统计。
    pub fn get(&self, subject: &str, tool: &str) -> Option<ToolStat> {
        self.subjects.get(subject)?.tools.get(tool).copied()
    }

    /// 主体统计的工具数。
    #[cfg(test)]
    pub fn tool_count(&self, subject: &str) -> usize {
        self.subjects.get(subject).map_or(0, |s| s.tools.len())
    }
}

/// 移除 `last_used` 最早的一项（`None` 视为最早；同时刻按键取最小，确定性）。
fn evict_oldest<V>(map: &mut HashMap<String, V>, last_used: impl Fn(&V) -> Option<Instant>) {
    let oldest = map
        .iter()
        .min_by(|(ka, a), (kb, b)| last_used(a).cmp(&last_used(b)).then_with(|| ka.cmp(kb)))
        .map(|(k, _)| k.clone());
    if let Some(k) = oldest {
        map.remove(&k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_calls_and_success_rate() {
        let t0 = Instant::now();
        let mut s = SearchStats::default();
        assert!(s.get("local", "shop.a").is_none());
        s.record("local", "shop.a", true, t0);
        s.record("local", "shop.a", false, t0 + Duration::from_secs(1));
        s.record("local", "shop.a", true, t0 + Duration::from_secs(2));
        let st = s.get("local", "shop.a").unwrap();
        assert_eq!((st.calls, st.successes, st.last_used), (3, 2, t0 + Duration::from_secs(2)));
        assert_eq!(st.success_rate(), Some(2.0 / 3.0));
        assert!(s.get("api", "shop.a").is_none(), "按主体隔离");
        assert!(s.get("local", "shop.b").is_none());
    }

    #[test]
    fn recent_window_is_24_hours() {
        let t0 = Instant::now();
        let mut s = SearchStats::default();
        s.record("local", "shop.a", true, t0);
        let st = s.get("local", "shop.a").unwrap();
        assert!(st.used_within_window(t0));
        assert!(st.used_within_window(t0 + RECENT_WINDOW - Duration::from_secs(1)));
        assert!(!st.used_within_window(t0 + RECENT_WINDOW));
        assert!(st.used_within_window(t0.checked_sub(Duration::from_secs(1)).unwrap_or(t0)), "时钟倒退按刚用过");
    }

    #[test]
    fn evicts_least_recently_used_tool_at_capacity() {
        let t0 = Instant::now();
        let mut s = SearchStats::default();
        for i in 0..MAX_TOOLS_PER_SUBJECT {
            s.record("local", &format!("app.t{i}"), true, t0 + Duration::from_secs(i as u64));
        }
        // 再用一次 t0，使 t1 成为最久未用
        s.record("local", "app.t0", true, t0 + Duration::from_secs(10_000));
        assert_eq!(s.tool_count("local"), MAX_TOOLS_PER_SUBJECT);
        s.record("local", "app.new", false, t0 + Duration::from_secs(10_001));
        assert_eq!(s.tool_count("local"), MAX_TOOLS_PER_SUBJECT, "满时淘汰后再加入");
        assert!(s.get("local", "app.t1").is_none(), "淘汰最久未用的");
        assert!(s.get("local", "app.t0").is_some(), "最近用过的保留");
        assert_eq!(s.get("local", "app.new").map(|t| (t.calls, t.successes)), Some((1, 0)));
        // 已有工具再记不淘汰
        s.record("local", "app.t2", true, t0 + Duration::from_secs(10_002));
        assert_eq!(s.tool_count("local"), MAX_TOOLS_PER_SUBJECT);
        assert!(s.get("local", "app.t3").is_some());
    }

    #[test]
    fn evicts_least_recently_used_subject_at_capacity() {
        let t0 = Instant::now();
        let mut s = SearchStats::default();
        for i in 0..MAX_STATS_SUBJECTS {
            s.record(&format!("agent:a{i}"), "app.t", true, t0 + Duration::from_secs(i as u64));
        }
        s.record("agent:new", "app.t", true, t0 + Duration::from_secs(100_000));
        assert!(s.get("agent:a0", "app.t").is_none());
        assert!(s.get("agent:a1", "app.t").is_some());
        assert!(s.get("agent:new", "app.t").is_some());
    }
}
