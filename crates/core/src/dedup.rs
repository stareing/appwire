//! 调用去重表（spec/protocol.md 3.3，第 16 项 N7a）：`callId` → 首次结果，有效期内对同一 `callId` 重放。
//!
//! 纯状态：不读时钟，时间由调用方注入；条目按记录时刻先后排列（`now` 单调），过期与淘汰都从队首开始。
//! 用 `Vec` 而不是 `VecDeque`：条数上限很小（默认 64），且不为 WASM 多实例化一套容器代码。

use app_mcp_protocol::RpcError;
use serde_json::Value;

use crate::Millis;

/// 调用去重策略（[`crate::ClientConfig::call_dedup`]）。
///
/// @invariant 内存上限 = `max_entries` × 单个结果大小（结果受 Host 的结果大小上限约束，spec/hub-api.md 3.11）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CallDedupPolicy {
    /// 首次结果的保留时长。0 表示关闭去重（重复的进行中 `callId` 按旧行为返回 -32602）。
    pub ttl_ms: Millis,
    /// 最多保留的结果数，超出时淘汰最早的。0 表示关闭去重。
    pub max_entries: usize,
}

impl CallDedupPolicy {
    /// 默认保留 5 分钟。
    pub const DEFAULT_TTL_MS: Millis = 5 * 60 * 1000;
    /// 默认最多 64 条。
    pub const DEFAULT_MAX_ENTRIES: usize = 64;
    /// 关闭去重。
    pub const OFF: Self = Self { ttl_ms: 0, max_entries: 0 };

    pub fn enabled(&self) -> bool {
        self.ttl_ms > 0 && self.max_entries > 0
    }
}

impl Default for CallDedupPolicy {
    fn default() -> Self {
        Self { ttl_ms: Self::DEFAULT_TTL_MS, max_entries: Self::DEFAULT_MAX_ENTRIES }
    }
}

/// 一次调用的最终回复：`tools/invoke` 的结果或错误。
pub(crate) type Outcome = Result<Value, RpcError>;

#[derive(Debug)]
struct Entry {
    call_id: String,
    expires_at: Millis,
    outcome: Outcome,
}

#[derive(Debug, Default)]
pub(crate) struct DedupTable {
    entries: Vec<Entry>,
}

impl DedupTable {
    fn prune(&mut self, now: Millis) {
        let expired = self.entries.iter().take_while(|e| e.expires_at <= now).count();
        self.entries.drain(..expired);
    }

    /// 有效期内该 `callId` 的首次结果。
    pub fn lookup(&mut self, policy: &CallDedupPolicy, call_id: &str, now: Millis) -> Option<Outcome> {
        if !policy.enabled() {
            return None;
        }
        self.prune(now);
        self.entries.iter().find(|e| e.call_id == call_id).map(|e| e.outcome.clone())
    }

    /// 记下首次结果（同一 `callId` 已有记录时保留旧记录：首次结果为准）。
    pub fn record(&mut self, policy: &CallDedupPolicy, call_id: &str, outcome: Outcome, now: Millis) {
        if !policy.enabled() {
            return;
        }
        self.prune(now);
        if self.entries.iter().any(|e| e.call_id == call_id) {
            return;
        }
        let excess = (self.entries.len() + 1).saturating_sub(policy.max_entries);
        self.entries.drain(..excess.min(self.entries.len()));
        let expires_at = now.saturating_add(policy.ttl_ms);
        self.entries.push(Entry { call_id: call_id.to_owned(), expires_at, outcome });
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const P: CallDedupPolicy = CallDedupPolicy { ttl_ms: 100, max_entries: 2 };

    #[test]
    fn replays_until_expiry() {
        let mut t = DedupTable::default();
        t.record(&P, "a", Ok(json!(1)), 0);
        assert_eq!(t.lookup(&P, "a", 99), Some(Ok(json!(1))));
        assert_eq!(t.lookup(&P, "b", 99), None);
        assert_eq!(t.lookup(&P, "a", 100), None, "到期即失效");
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn first_outcome_wins() {
        let mut t = DedupTable::default();
        t.record(&P, "a", Ok(json!(1)), 0);
        t.record(&P, "a", Err(RpcError::invalid_params("x")), 1);
        assert_eq!(t.lookup(&P, "a", 2), Some(Ok(json!(1))));
    }

    #[test]
    fn bounded_evicts_oldest() {
        let mut t = DedupTable::default();
        t.record(&P, "a", Ok(json!(1)), 0);
        t.record(&P, "b", Ok(json!(2)), 1);
        t.record(&P, "c", Ok(json!(3)), 2);
        assert_eq!(t.len(), 2);
        assert_eq!(t.lookup(&P, "a", 3), None);
        assert_eq!(t.lookup(&P, "c", 3), Some(Ok(json!(3))));
    }

    #[test]
    fn disabled_policy_records_nothing() {
        let mut t = DedupTable::default();
        for p in [CallDedupPolicy::OFF, CallDedupPolicy { ttl_ms: 10, max_entries: 0 }, CallDedupPolicy { ttl_ms: 0, max_entries: 5 }] {
            assert!(!p.enabled());
            t.record(&p, "a", Ok(json!(1)), 0);
            assert_eq!(t.lookup(&p, "a", 0), None);
        }
        assert_eq!(t.len(), 0);
        assert!(CallDedupPolicy::default().enabled());
    }
}
