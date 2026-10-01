//! 调用进度的合并与过滤（spec/protocol.md 3.3、spec/hub-api.md 3.12）：App 的 `tools/progress` → MCP `notifications/progress`。
//!
//! 纯状态：不读时钟，时间（自调用开始的毫秒数）由调用方注入。规则：
//! - 进度值不递增的更新丢弃（MCP 要求每条通知的 `progress` 递增）；
//! - 两次转发至少间隔 `interval_ms`，间隔内只保留最新一条，到期再发（最后一条不会因合并丢失，除非调用先结束）；
//! - `message` 超过 [`MAX_PROGRESS_MESSAGE_CHARS`] 个字符时截断（进度说明只是提示，保护 Agent 上下文）。

use app_mcp_protocol::ToolsProgressParams;

/// 转发给 Agent 的进度说明的最大字符数。
pub const MAX_PROGRESS_MESSAGE_CHARS: usize = 200;

/// 一条要转发的进度。
#[derive(Clone, Debug, PartialEq)]
pub struct ProgressUpdate {
    pub progress: f64,
    pub total: Option<f64>,
    pub message: Option<String>,
}

impl From<ToolsProgressParams> for ProgressUpdate {
    fn from(p: ToolsProgressParams) -> Self {
        let message = p.message.map(|m| match m.char_indices().nth(MAX_PROGRESS_MESSAGE_CHARS) {
            Some((cut, _)) => format!("{}…", &m[..cut]),
            None => m,
        });
        Self { progress: p.progress, total: p.total.filter(|t| t.is_finite()), message }
    }
}

/// 一次调用的进度合并状态。
#[derive(Debug)]
pub(crate) struct ProgressThrottle {
    interval_ms: u64,
    last_sent_at: Option<u64>,
    /// 已转发或已暂存的最大进度值。
    high_water: Option<f64>,
    pending: Option<ProgressUpdate>,
}

impl ProgressThrottle {
    pub fn new(interval_ms: u64) -> Self {
        Self { interval_ms, last_sent_at: None, high_water: None, pending: None }
    }

    /// 收到一条进度：返回现在就要转发的更新；否则暂存（或丢弃不递增 / 非有限的值）。
    pub fn offer(&mut self, update: ProgressUpdate, now: u64) -> Option<ProgressUpdate> {
        if !update.progress.is_finite() || self.high_water.is_some_and(|h| update.progress <= h) {
            return None;
        }
        self.high_water = Some(update.progress);
        match self.due_at() {
            Some(due) if now < due => {
                self.pending = Some(update);
                None
            }
            _ => self.sent(update, now),
        }
    }

    /// 暂存的更新何时可以转发。
    pub fn due_at(&self) -> Option<u64> {
        let last = self.last_sent_at?;
        Some(last.saturating_add(self.interval_ms))
    }

    /// 暂存的更新何时需要转发（没有暂存时为 `None`）。
    pub fn flush_at(&self) -> Option<u64> {
        self.pending.as_ref().and(self.due_at())
    }

    /// 到期时取出暂存的更新。
    pub fn flush(&mut self, now: u64) -> Option<ProgressUpdate> {
        if self.flush_at().is_some_and(|due| now >= due) {
            let update = self.pending.take()?;
            return self.sent(update, now);
        }
        None
    }

    fn sent(&mut self, update: ProgressUpdate, now: u64) -> Option<ProgressUpdate> {
        self.pending = None;
        self.last_sent_at = Some(now);
        Some(update)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(progress: f64) -> ProgressUpdate {
        ProgressUpdate { progress, total: None, message: None }
    }

    #[test]
    fn first_update_is_sent_then_coalesced() {
        let mut t = ProgressThrottle::new(100);
        assert_eq!(t.offer(u(1.0), 0), Some(u(1.0)));
        assert_eq!(t.offer(u(2.0), 10), None);
        assert_eq!(t.offer(u(3.0), 20), None);
        assert_eq!(t.flush_at(), Some(100));
        assert_eq!(t.flush(99), None);
        assert_eq!(t.flush(100), Some(u(3.0)), "间隔内只保留最新一条");
        assert_eq!(t.flush_at(), None);
        assert_eq!(t.offer(u(4.0), 250), Some(u(4.0)), "间隔已过：立即转发");
    }

    #[test]
    fn non_increasing_and_non_finite_are_dropped() {
        let mut t = ProgressThrottle::new(0);
        assert_eq!(t.offer(u(5.0), 0), Some(u(5.0)));
        assert_eq!(t.offer(u(5.0), 1), None);
        assert_eq!(t.offer(u(4.0), 2), None);
        assert_eq!(t.offer(u(f64::NAN), 3), None);
        assert_eq!(t.offer(u(6.0), 3), Some(u(6.0)), "间隔为 0：不合并");
        // 暂存的值同样计入递增判断
        let mut t = ProgressThrottle::new(100);
        t.offer(u(1.0), 0);
        assert_eq!(t.offer(u(3.0), 1), None);
        assert_eq!(t.offer(u(2.0), 2), None);
        assert_eq!(t.flush(100), Some(u(3.0)));
    }

    #[test]
    fn conversion_truncates_message_and_drops_bad_total() {
        let long = "进".repeat(MAX_PROGRESS_MESSAGE_CHARS + 5);
        let p = ToolsProgressParams { call_id: "c".into(), progress: 1.0, total: Some(f64::INFINITY), message: Some(long) };
        let up = ProgressUpdate::from(p);
        assert_eq!(up.total, None);
        let m = up.message.unwrap();
        assert_eq!(m.chars().count(), MAX_PROGRESS_MESSAGE_CHARS + 1);
        assert!(m.ends_with('…'));
        let p = ToolsProgressParams { call_id: "c".into(), progress: 1.0, total: Some(2.0), message: Some("短".into()) };
        assert_eq!(ProgressUpdate::from(p), ProgressUpdate { progress: 1.0, total: Some(2.0), message: Some("短".into()) });
    }
}
