//! 调用：请求、结果与错误。

use std::time::Duration;

use app_mcp_protocol::{CallPriority, ContentAnnotations, ErrorKind, ResultStatus, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{opt_millis, result_json};
use super::tools::AppOverviewInfo;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CallRequest {
    /// 全名 `<appId>.<tool>`（内置工具为 `apps.list` 等）。
    pub name: String,
    /// 参数对象；`null` 视为 `{}`。
    pub arguments: Value,
    /// 指定实例：该实例必须已连接且注册了此工具，否则返回 `TOOL_NOT_FOUND`。`None` 按路由规则。
    pub instance_id: Option<String>,
    /// 本次调用的等待上限；`None` 用 [`crate::HubConfig::response_timeout`]。
    #[serde(with = "opt_millis")]
    pub timeout: Option<Duration>,
    /// 供 [`crate::Hub::cancel_call`] 使用；`None` 自动生成。
    pub call_id: Option<String>,
    /// 厂商会话 ID：总览的首次附带、`apps.select` 按会话计算；`None` = 默认会话。
    pub session: Option<String>,
    /// Agent 的幂等键：原样转交 App（`ToolsInvokeParams.idempotencyKey`，spec/protocol.md 3.3），1..=256 个字符；
    /// MCP 出口取自请求 `_meta` 的 `dev.appwire/idempotencyKey`（spec/hub-api.md 3.15）。
    pub idempotency_key: Option<String>,
    /// 调用优先级（第 16 项 P6）：原样转交 App（`ToolsInvokeParams.priority`），App SDK 的调用队列先按它、再按到达顺序调度；
    /// MCP 出口取自请求 `_meta` 的 `dev.appwire/priority`（spec/hub-api.md 3.15）。默认 normal。
    pub priority: CallPriority,
    /// 不查只读结果缓存、照常调用并以新结果覆盖（spec/hub-api.md 3.20）；MCP 出口取自请求 `_meta` 的
    /// `dev.appwire/cache: "bypass"`。默认 `false`。
    pub cache_bypass: bool,
}

impl CallRequest {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            name: name.into(),
            arguments,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallOutcome {
    pub call_id: String,
    #[serde(with = "result_json")]
    pub result: Result<Value, ToolError>,
    /// App 声明可能已变化的资源名（不含 appId 前缀）。
    pub state_hints: Vec<String>,
    /// 实际处理调用的实例（App 工具）。
    pub instance_id: Option<String>,
    /// 该会话首次接触此 App（或总览版本变化）时附带。
    pub overview: Option<AppOverviewInfo>,
    /// App 声明的业务状态（spec/protocol.md 3.2；缺省 `done`）。
    #[serde(default)]
    pub status: ResultStatus,
    /// `pending` 时可读取后续状态的资源 URI（`app-mcp://<appId>/<资源名>`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_resource: Option<String>,
    /// App 给出的一句结论。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// App 对结果内容的标注（MCP 内容注解），原样。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ContentAnnotations>,
    /// App 在后台、改调了 view 工具声明的后台替代时为实际调用的工具全名（spec/hub-api.md 3.14）；否则 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routed_to: Option<String>,
    /// Hub 收到调用到得出结果的毫秒数（含审批、唤醒与等待 App）。
    #[serde(default)]
    pub duration_ms: u64,
    /// 本次 App 工具调用是否经历了唤醒（调用时目标未连接，唤醒回连后才送达）。内置工具与上游工具恒为 `false`
    /// （`apps.activate` / `apps.navigate` 的结果自带 `woke`）。
    #[serde(default)]
    pub woke: bool,
    /// 结果来自只读结果缓存（未转发给 App）时距 App 产出的毫秒数；未命中为 `None`（spec/hub-api.md 3.20，
    /// 与 MCP 结果 `_meta` 的 `dev.appwire/cached.ageMs` 相同）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_age_ms: Option<u64>,
}

/// Hub 操作失败：[`ToolError`] 的包装（同一套错误码，spec/protocol.md §4）。
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct HubError(pub ToolError);

impl HubError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self(ToolError::new(kind, message))
    }
    pub fn kind(&self) -> ErrorKind {
        self.0.kind
    }
    pub fn message(&self) -> &str {
        &self.0.message
    }
}

impl From<ToolError> for HubError {
    fn from(e: ToolError) -> Self {
        Self(e)
    }
}
