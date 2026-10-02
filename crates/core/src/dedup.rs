//! 调用去重表（spec/protocol.md 3.3，第 16 项 N7a）：`callId` → 首次结果，有效期内对同一 `callId` 重放；
//! 调用带 Agent 幂等键（`idempotencyKey`，第 4f 项 j）时另按（工具名, 幂等键）匹配。
//!
//! 纯状态：不读时钟，时间由调用方注入；条目按记录时刻先后排列（`now` 单调），过期与淘汰都从队首开始。
//! 用 `Vec` 而不是 `VecDeque`：条数上限很小（默认 64），且不为 WASM 多实例化一套容器代码。

use app_mcp_protocol::RpcError;

use crate::Millis;

/// 调用去重策略（[`crate::ClientConfig::call_dedup`]）。
///
/// @invariant 内存上限 = `max_entries` × 单个结果的 JSON 文本大小（结果受 Host 的结果大小上限约束，spec/hub-api.md 3.11）。
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

/// 一次调用的最终回复：`tools/invoke` 结果的 JSON 文本（JSON-RPC `result` 成员的值），或错误。
///
/// @why 存文本而不是 `Value`：对象较多的结果解析成 `Value` 约为 JSON 的 6–7 倍（1 MiB 对象数组 ≈ 6.8 MiB），
/// 去重表最多保留 64 条；文本也免去回复与重放时的深拷贝和再序列化。
pub(crate) type Outcome = Result<String, RpcError>;

/// 幂等键在去重表中的匹配键：工具名 + 换行 + 幂等键。
///
/// @invariant 工具名不含换行（spec/protocol.md 3.1 名称规则），因此组合键无歧义；同一幂等键用于不同工具时互不影响。
pub(crate) fn idempotency_match_key(tool_name: &str, idempotency_key: &str) -> String {
    let mut k = String::with_capacity(tool_name.len() + 1 + idempotency_key.len());
    k.push_str(tool_name);
    k.push('\n');
    k.push_str(idempotency_key);
    k
}

#[derive(Debug)]
struct Entry {
    call_id: String,
    /// [`idempotency_match_key`]；调用没有幂等键时为 `None`。
    idem: Option<String>,
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

    fn find(&self, call_id: &str, idem: Option<&str>) -> Option<&Entry> {
        self.entries.iter().find(|e| e.call_id == call_id || (idem.is_some() && e.idem.as_deref() == idem))
    }

    /// 有效期内该 `callId`（或同一幂等匹配键 `idem`，[`idempotency_match_key`]）的首次结果。
    pub fn lookup(&mut self, policy: &CallDedupPolicy, call_id: &str, idem: Option<&str>, now: Millis) -> Option<Outcome> {
        if !policy.enabled() {
            return None;
        }
        self.prune(now);
        self.find(call_id, idem).map(|e| e.outcome.clone())
    }

    /// 记下首次结果（同一 `callId` 或幂等匹配键已有记录时保留旧记录：首次结果为准）。
    pub fn record(&mut self, policy: &CallDedupPolicy, call_id: &str, idem: Option<&str>, outcome: Outcome, now: Millis) {
        if !policy.enabled() {
            return;
        }
        self.prune(now);
        if self.find(call_id, idem).is_some() {
            return;
        }
        let excess = (self.entries.len() + 1).saturating_sub(policy.max_entries);
        self.entries.drain(..excess.min(self.entries.len()));
        let expires_at = now.saturating_add(policy.ttl_ms);
        // @why 序列化缓冲按倍增扩容，容量最多约为长度的 2 倍；条目要保留到过期，收回多余容量。
        let outcome = outcome.map(|mut text| {
            text.shrink_to_fit();
            text
        });
        self.entries.push(Entry { call_id: call_id.to_owned(), idem: idem.map(str::to_owned), expires_at, outcome });
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(v: i64) -> String {
        v.to_string()
    }

    const P: CallDedupPolicy = CallDedupPolicy { ttl_ms: 100, max_entries: 2 };

    #[test]
    fn replays_until_expiry() {
        let mut t = DedupTable::default();
        t.record(&P, "a", None, Ok(json(1)), 0);
        assert_eq!(t.lookup(&P, "a", None, 99), Some(Ok(json(1))));
        assert_eq!(t.lookup(&P, "b", None, 99), None);
        assert_eq!(t.lookup(&P, "a", None, 100), None, "到期即失效");
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn first_outcome_wins() {
        let mut t = DedupTable::default();
        t.record(&P, "a", None, Ok(json(1)), 0);
        t.record(&P, "a", None, Err(RpcError::invalid_params("x")), 1);
        assert_eq!(t.lookup(&P, "a", None, 2), Some(Ok(json(1))));
    }

    #[test]
    fn bounded_evicts_oldest() {
        let mut t = DedupTable::default();
        t.record(&P, "a", None, Ok(json(1)), 0);
        t.record(&P, "b", None, Ok(json(2)), 1);
        t.record(&P, "c", None, Ok(json(3)), 2);
        assert_eq!(t.len(), 2);
        assert_eq!(t.lookup(&P, "a", None, 3), None);
        assert_eq!(t.lookup(&P, "c", None, 3), Some(Ok(json(3))));
    }

    /// 幂等键（第 4f 项 j）：不同 callId、同一（工具, 幂等键）命中首次结果；同一幂等键用于其他工具不命中。
    #[test]
    fn idempotency_key_matches_across_call_ids() {
        let mut t = DedupTable::default();
        let k = idempotency_match_key("cart.add", "order-7");
        t.record(&P, "a", Some(&k), Ok(json(1)), 0);
        assert_eq!(t.lookup(&P, "b", Some(&k), 1), Some(Ok(json(1))));
        assert_eq!(t.lookup(&P, "b", Some(&idempotency_match_key("cart.remove", "order-7")), 1), None);
        assert_eq!(t.lookup(&P, "b", None, 1), None, "没有幂等键时只按 callId");
        t.record(&P, "c", Some(&k), Ok(json(2)), 2);
        assert_eq!(t.lookup(&P, "c", None, 3), None, "同一幂等键已有记录：首次结果为准，不另记");
    }

    #[test]
    fn disabled_policy_records_nothing() {
        let mut t = DedupTable::default();
        for p in [CallDedupPolicy::OFF, CallDedupPolicy { ttl_ms: 10, max_entries: 0 }, CallDedupPolicy { ttl_ms: 0, max_entries: 5 }] {
            assert!(!p.enabled());
            t.record(&p, "a", None, Ok(json(1)), 0);
            assert_eq!(t.lookup(&p, "a", None, 0), None);
        }
        assert_eq!(t.len(), 0);
        assert!(CallDedupPolicy::default().enabled());
    }
}
