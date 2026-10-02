//! 调用队列：进行中的调用与按到达顺序排队的调用。

use std::collections::VecDeque;

use app_mcp_protocol::RequestId;
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

    pub fn enqueue(&mut self, call: Call) {
        self.queued.push_back(call);
    }

    pub fn pop_queued(&mut self) -> Option<Call> {
        self.queued.pop_front()
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
        assert_eq!(c.pop_queued().map(|x| x.call_id), Some("c".into()));
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
}
