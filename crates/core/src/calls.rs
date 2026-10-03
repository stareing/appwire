//! 调用队列：进行中的调用与按到达顺序排队的调用。

use std::collections::VecDeque;

use app_mcp_protocol::{CallPriority, RequestId};
use serde_json::Value;

use crate::{Millis, ToolId};

#[derive(Clone, Debug)]
pub(crate) struct Call {
    pub call_id: String,
    /// `tools/invoke` 请求的 ID，用于回复。
    pub request_id: RequestId,
    pub tool: ToolId,
    pub name: String,
    /// 排队期间的参数；开始执行时移交给 [`crate::Event::InvokeTool`]，之后为 `Null`。
    pub arguments: Value,
    /// Agent 给出的幂等键（`ToolsInvokeParams.idempotencyKey`，spec/protocol.md 3.3），原样交给 handler。
    pub idempotency_key: Option<String>,
    pub timeout_ms: Option<Millis>,
    /// 从收到请求起算的截止时刻（包含排队时间）。
    pub deadline: Option<Millis>,
    /// 执行期间到达的同一 `callId` 的重复请求（spec/protocol.md 3.3），完成时一并回复。
    pub waiters: Vec<RequestId>,
    /// Agent 给出的优先级（`ToolsInvokeParams.priority`）：队列先按它、再按到达顺序排列。
    pub priority: CallPriority,
    /// 开始执行时工具声明的互斥组（[`crate::ToolDef::exclusive`]）；排队中为 `None`。
    pub exclusive: Option<String>,
}

impl Call {
    /// 去重表中的幂等匹配键（[`crate::dedup::idempotency_match_key`]）。
    pub fn idem(&self) -> Option<String> {
        self.idempotency_key.as_deref().map(|k| crate::dedup::idempotency_match_key(&self.name, k))
    }
}

#[derive(Debug, Default)]
pub(crate) struct Calls {
    running: Vec<Call>,
    queued: VecDeque<Call>,
}

impl Calls {
    pub fn contains(&self, call_id: &str) -> bool {
        self.running.iter().chain(self.queued.iter()).any(|c| c.call_id == call_id)
    }

    /// 把重复请求挂到同一（工具名, 幂等键）的进行中 / 排队调用上；没有该调用时返回 `false`。
    pub fn attach_idempotent(&mut self, name: &str, key: &str, request_id: RequestId) -> bool {
        let same = |c: &&mut Call| c.name == name && c.idempotency_key.as_deref() == Some(key);
        match self.running.iter_mut().chain(self.queued.iter_mut()).find(same) {
            Some(c) => {
                c.waiters.push(request_id);
                true
            }
            None => false,
        }
    }

    pub fn is_running(&self, call_id: &str) -> bool {
        self.running.iter().any(|c| c.call_id == call_id)
    }

    /// 把重复请求挂到同一 `callId` 的进行中 / 排队调用上；没有该调用时返回 `false`。
    pub fn attach(&mut self, call_id: &str, request_id: RequestId) -> bool {
        match self.running.iter_mut().chain(self.queued.iter_mut()).find(|c| c.call_id == call_id) {
            Some(c) => {
                c.waiters.push(request_id);
                true
            }
            None => false,
        }
    }

    pub fn running_len(&self) -> usize {
        self.running.len()
    }

    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }

    /// 按优先级插入：排在同级及更高优先级的调用之后、更低优先级的调用之前（同级按到达顺序）。
    pub fn enqueue(&mut self, call: Call) {
        let at = self.queued.iter().position(|c| c.priority < call.priority).unwrap_or(self.queued.len());
        self.queued.insert(at, call);
    }

    /// 排队中优先级低于 `than` 的调用里最后到达的一个（队列超限时为更高优先级的调用让路）。
    pub fn lowest_below(&self, than: CallPriority) -> Option<&Call> {
        self.queued.iter().rev().find(|c| c.priority < than)
    }

    pub fn queued_priority(&self, call_id: &str) -> Option<CallPriority> {
        self.queued.iter().find(|c| c.call_id == call_id).map(|c| c.priority)
    }

    /// 排队中第 `index` 个调用（按到达顺序）。
    pub fn queued_at(&self, index: usize) -> Option<&Call> {
        self.queued.get(index)
    }

    pub fn remove_queued(&mut self, index: usize) -> Option<Call> {
        self.queued.remove(index)
    }

    /// 按工具声明，`tool` 的一个调用此刻能否开始：本工具执行中的调用数小于 `concurrency`（0 = 不限），
    /// 且互斥组 `exclusive` 中没有执行中的调用。全局并发上限由调用方检查。
    pub fn can_start(&self, tool: ToolId, concurrency: u32, exclusive: Option<&str>) -> bool {
        let same_tool = self.running.iter().filter(|c| c.tool == tool).count();
        let tool_free = concurrency == 0 || same_tool < concurrency as usize;
        let group_free = exclusive.is_none_or(|g| !self.running.iter().any(|c| c.exclusive.as_deref() == Some(g)));
        tool_free && group_free
    }

    pub fn start(&mut self, call: Call) {
        self.running.push(call);
    }

    pub fn take_running(&mut self, call_id: &str) -> Option<Call> {
        let idx = self.running.iter().position(|c| c.call_id == call_id)?;
        Some(self.running.remove(idx))
    }

    pub fn take_queued(&mut self, call_id: &str) -> Option<Call> {
        let idx = self.queued.iter().position(|c| c.call_id == call_id)?;
        self.queued.remove(idx)
    }

    /// 取出所有已到期的调用：`(进行中, 排队中)`。
    pub fn take_expired(&mut self, now: Millis) -> (Vec<Call>, Vec<Call>) {
        let expired = |c: &Call| c.deadline.is_some_and(|d| d <= now);
        let (running_expired, running): (Vec<Call>, Vec<Call>) = self.running.drain(..).partition(expired);
        self.running = running;
        let (queued_expired, queued): (Vec<Call>, Vec<Call>) = self.queued.drain(..).partition(expired);
        self.queued = queued.into();
        (running_expired, queued_expired)
    }

    pub fn next_deadline(&self) -> Option<Millis> {
        self.running.iter().chain(self.queued.iter()).filter_map(|c| c.deadline).min()
    }

    /// 清空全部调用，返回进行中的调用（排队中的调用 handler 尚未开始，直接丢弃）。
    pub fn clear(&mut self) -> Vec<Call> {
        self.queued.clear();
        std::mem::take(&mut self.running)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, deadline: Option<Millis>) -> Call {
        Call {
            call_id: id.into(),
            request_id: RequestId::from(id),
            tool: ToolId(1),
            name: "t".into(),
            arguments: Value::Null,
            idempotency_key: None,
            timeout_ms: None,
            deadline,
            waiters: Vec::new(),
            exclusive: None,
            priority: CallPriority::Normal,
        }
    }

    #[test]
    fn expiry_and_order() {
        let mut c = Calls::default();
        c.start(call("a", Some(10)));
        c.enqueue(call("b", Some(5)));
        c.enqueue(call("c", None));
        assert_eq!(c.next_deadline(), Some(5));
        let (r, q) = c.take_expired(5);
        assert!(r.is_empty());
        assert_eq!(q.len(), 1);
        assert_eq!(c.remove_queued(0).map(|x| x.call_id), Some("c".into()));
        let (r, _) = c.take_expired(10);
        assert_eq!(r.len(), 1);
        assert_eq!(c.running_len(), 0);
        assert!(!c.contains("a"));
    }

    #[test]
    fn attach_to_running_or_queued() {
        let mut c = Calls::default();
        c.start(call("a", None));
        c.enqueue(call("b", None));
        assert!(c.is_running("a") && !c.is_running("b"));
        assert!(c.attach("a", RequestId::from("a2")));
        assert!(c.attach("b", RequestId::from("b2")));
        assert!(!c.attach("x", RequestId::from("x2")));
        assert_eq!(c.take_running("a").map(|x| x.waiters), Some(vec![RequestId::from("a2")]));
        assert_eq!(c.take_queued("b").map(|x| x.waiters), Some(vec![RequestId::from("b2")]));
    }

    #[test]
    fn per_tool_concurrency_and_exclusive_groups() {
        let mut c = Calls::default();
        assert!(c.can_start(ToolId(1), 1, Some("doc")));
        let mut a = call("a", None);
        a.exclusive = Some("doc".into());
        c.start(a);
        assert!(!c.can_start(ToolId(1), 1, None), "同一工具达到上限");
        assert!(c.can_start(ToolId(1), 2, None));
        assert!(c.can_start(ToolId(1), 0, None), "0 = 不单独限制");
        assert!(!c.can_start(ToolId(2), 0, Some("doc")), "同组互斥");
        assert!(c.can_start(ToolId(2), 0, Some("sheet")));
        assert!(c.can_start(ToolId(2), 1, None));
    }

    #[test]
    fn enqueue_orders_by_priority_then_arrival() {
        let mut c = Calls::default();
        let with = |id: &str, p: CallPriority| Call { priority: p, ..call(id, None) };
        c.enqueue(with("n1", CallPriority::Normal));
        c.enqueue(with("b1", CallPriority::Background));
        c.enqueue(with("i1", CallPriority::Interactive));
        c.enqueue(with("n2", CallPriority::Normal));
        c.enqueue(with("i2", CallPriority::Interactive));
        let order: Vec<String> = (0..c.queued_len()).filter_map(|i| c.queued_at(i)).map(|x| x.call_id.clone()).collect();
        assert_eq!(order, ["i1", "i2", "n1", "n2", "b1"]);
        assert_eq!(c.lowest_below(CallPriority::Interactive).map(|x| x.call_id.as_str()), Some("b1"));
        assert_eq!(c.lowest_below(CallPriority::Background).map(|x| x.call_id.as_str()), None);
    }
}
