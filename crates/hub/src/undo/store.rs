//! 一个 Agent 任务名下的撤销记录（[`crate::task::AgentTask::undo`]）：纯状态，不读时钟（`now` 由调用方传入）。

use std::collections::VecDeque;
use std::time::Duration;

use serde_json::Value;
use tokio::time::Instant;

/// 一条撤销记录：撤销 `call_id` 即对 `<app_id>.<tool>` 以 `arguments` 发起一次调用。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UndoRecord {
    pub call_id: String,
    pub app_id: String,
    /// 处理原调用的实例（仍在线时逆调用发往它）。
    pub instance_id: Option<String>,
    /// 逆工具的局部名。
    pub tool: String,
    pub arguments: Value,
    pub label: Option<String>,
    /// 登记时刻（有效期从这里算起）。
    pub at: Instant,
}

impl UndoRecord {
    fn live(&self, ttl: Duration, now: Instant) -> bool {
        now.saturating_duration_since(self.at) < ttl
    }
}

/// 任务名下的撤销记录，按登记顺序（最早的在前）。
#[derive(Debug, Default)]
pub(crate) struct TaskUndo {
    records: VecDeque<UndoRecord>,
}

impl TaskUndo {
    fn prune(&mut self, ttl: Duration, now: Instant) {
        self.records.retain(|r| r.live(ttl, now));
    }

    /// 登记一条记录：先丢弃过期的；同一 callId 的旧记录被替换；超过 `max` 条时丢弃最早的。
    ///
    /// @input max 大于 0（撤销开启）；为 0 时不登记。
    pub(crate) fn push(&mut self, record: UndoRecord, ttl: Duration, max: usize) {
        self.prune(ttl, record.at);
        self.records.retain(|r| r.call_id != record.call_id);
        if max == 0 {
            return;
        }
        while self.records.len() >= max {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }

    /// 取出（移除）一条未过期的记录：`call_id` 为 `None` 时取最近登记的一条。不存在 / 已过期 / 已取出 → `None`。
    pub(crate) fn take(&mut self, call_id: Option<&str>, ttl: Duration, now: Instant) -> Option<UndoRecord> {
        self.prune(ttl, now);
        let index = match call_id {
            None => self.records.len().checked_sub(1)?,
            Some(id) => self.records.iter().rposition(|r| r.call_id == id)?,
        };
        self.records.remove(index)
    }

    /// 当前保留的记录数（含尚未被判定的过期记录）。
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }
}
