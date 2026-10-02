//! 按调用方记账的入口（第 16 项 P3，[`crate::usage`]）：记账事件与唤醒准入（唤醒策略 + 唤醒计数）。

use app_mcp_protocol::ToolError;

use crate::agents::AgentName;
use crate::task::CallerKey;
use crate::usage::UsageEvent;

use super::{HubShared, lock};

impl HubShared {
    /// 为调用方记一次事件。
    pub(crate) fn record_usage(&self, caller: &CallerKey, app_id: &str, event: UsageEvent) {
        let agent = caller.agent().map(AgentName::as_str);
        lock(&self.usage).record(&caller.usage_subject(), agent, app_id, event);
    }

    /// 唤醒准入：策略 `wake` 执行点（按调用方的 Agent 匹配）通过后，为调用方记一次唤醒。
    ///
    /// @error 策略拒绝 → `POLICY_DENIED`（不记账）。
    pub(crate) fn admit_wake(&self, app_id: &str, tool: Option<&str>, caller: &CallerKey) -> Result<(), ToolError> {
        self.check_wake_policy(app_id, tool, caller.agent().map(AgentName::as_str))?;
        self.record_usage(caller, app_id, UsageEvent::Wake);
        Ok(())
    }
}
