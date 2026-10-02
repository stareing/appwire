//! 策略回调：审批策略与审批 / 配对处理器及其请求。

use std::time::Duration;

use app_mcp_protocol::{Risk, ToolAnnotations};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::opt_millis;
use super::tools::risk_rank;

/// 审批策略。默认不审批（与原 Host 一致）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ApprovalPolicy {
    /// 风险不低于此等级的工具调用前询问 [`ApprovalHandler`]；`None` = 不审批。
    pub require_at_or_above: Option<Risk>,
    /// 等待审批的上限；`None` 用 `response_timeout`。超时视为拒绝。spec 之外的补充字段。
    #[serde(with = "opt_millis")]
    pub timeout: Option<Duration>,
}

impl ApprovalPolicy {
    pub fn requires(&self, risk: Risk) -> bool {
        self.require_at_or_above
            .is_some_and(|min| risk_rank(risk) >= risk_rank(min))
    }
}

/// 厂商 UI 接管调用确认。
#[async_trait::async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// 返回 `false` → 调用以 `USER_REJECTED` 结束。超时视为拒绝。
    async fn approve(&self, req: ApprovalRequest) -> bool;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub call_id: String,
    pub app_id: String,
    pub app_name: String,
    pub tool: String,
    pub title: Option<String>,
    pub description: String,
    pub risk: Risk,
    pub arguments: Value,
    pub session: Option<String>,
    /// 工具的 MCP 注解（与 [`HubTool::annotations`](super::HubTool::annotations) 相同），供厂商按声明决定是否确认。
    #[serde(default)]
    pub annotations: ToolAnnotations,
    /// MCP 出口：发起调用的认证主体（取自传输层凭据，现在恒为 `local`，第 16 项 N5 按 Agent 发令牌后细分）；Hub API 为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    /// MCP 出口：客户端自报的名称（`clientInfo.name`；legacy 取自 `initialize`，无会话请求取自请求 `_meta`）。
    ///
    /// @security 自报、不可信，**仅供显示**，不得据此做授权决定（MCP 规范 S-F6）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
}

/// 厂商 UI 接管 App 配对。
#[async_trait::async_trait]
pub trait PairingHandler: Send + Sync {
    /// 未知 App（无静态清单，或 Origin 不在白名单）首次连接时询问。
    /// 返回 `false` → 握手结果为 `rejected`。超时（`pairing_timeout`）视为拒绝。
    async fn pair(&self, req: PairingRequest) -> bool;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingRequest {
    pub app_id: String,
    pub app_name: String,
    pub origin: Option<String>,
    pub client_kind: String,
    /// spec 之外的补充字段。
    pub instance_id: String,
}
