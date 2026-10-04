//! 调用：请求、进度、结果。

use std::time::Duration;

use app_mcp_hub as hub;
use serde_json::Value;

use super::{AppOverviewInfo, CallPriority, ContentAnnotations, HubError, ResultStatus, UndoOffer, parse_json};

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct CallRequest {
    /// 全名 `<appId>.<tool>`。
    pub name: String,
    /// 参数 JSON 对象文本；为空视为 `{}`。
    #[uniffi(default = None)]
    pub arguments_json: Option<String>,
    /// 严格指定实例；为空按路由规则。
    #[uniffi(default = None)]
    pub instance_id: Option<String>,
    /// 等待上限；为空用配置的 `response_timeout_ms`。
    #[uniffi(default = None)]
    pub timeout_ms: Option<u64>,
    /// 供 `cancel_call`；为空自动生成（结果中的 `call_id`）。
    #[uniffi(default = None)]
    pub call_id: Option<String>,
    /// 厂商会话 ID；为空 = 默认会话。
    #[uniffi(default = None)]
    pub session: Option<String>,
    /// Agent 的幂等键（1..=256 个字符），原样转交 App（spec/hub-api.md 3.15）；不合法时调用以 `INVALID_INPUT` 结束。
    #[uniffi(default = None)]
    pub idempotency_key: Option<String>,
    /// 调用优先级（第 16 项 P6）：原样转交 App，App 的调用队列先按它、再按到达顺序调度；为空 = `Normal`。
    #[uniffi(default = None)]
    pub priority: Option<CallPriority>,
    /// 不查只读结果缓存（spec/hub-api.md 3.20）：照常调用 App，以新结果覆盖缓存。
    #[uniffi(default = false)]
    pub cache_bypass: bool,
}

impl CallRequest {
    pub(crate) fn into_hub(self) -> Result<hub::CallRequest, HubError> {
        let arguments = match self.arguments_json.as_deref().map(str::trim) {
            None | Some("") => Value::Object(Default::default()),
            Some(text) => parse_json(text)?,
        };
        Ok(hub::CallRequest {
            name: self.name,
            arguments,
            instance_id: self.instance_id,
            timeout: self.timeout_ms.map(Duration::from_millis),
            call_id: self.call_id,
            session: self.session,
            idempotency_key: self.idempotency_key,
            priority: self.priority.map(Into::into).unwrap_or_default(),
            cache_bypass: self.cache_bypass,
        })
    }
}

/// 工具层面的失败。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ToolErrorInfo {
    /// 错误类别，如 `"USER_REJECTED"`、`"TIMEOUT"`、`"APP_DISCONNECTED"`。
    pub kind: String,
    pub message: String,
    pub details_json: Option<String>,
}

/// 调用进度（[`crate::AppMcpHub::call_tool_with_progress`]，spec/hub-api.md 3.12）：App 报告、Hub 合并后的一条。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ProgressUpdate {
    pub progress: f64,
    /// 总量；未知时为空。
    pub total: Option<f64>,
    /// 说明（截断到 200 字符）。
    pub message: Option<String>,
}

impl From<hub::ProgressUpdate> for ProgressUpdate {
    fn from(u: hub::ProgressUpdate) -> Self {
        Self { progress: u.progress, total: u.total, message: u.message }
    }
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct CallOutcome {
    pub call_id: String,
    /// 成功时的结果 JSON 文本（`error` 为空）。
    pub data_json: Option<String>,
    /// 失败时的错误（`data_json` 为空）。
    pub error: Option<ToolErrorInfo>,
    /// App 声明可能已变化的资源名。
    pub state_hints: Vec<String>,
    pub instance_id: Option<String>,
    /// 该会话首次接触此 App 时附带。
    pub overview: Option<AppOverviewInfo>,
    /// App 声明的业务状态（缺省 `Done`）。
    pub status: ResultStatus,
    /// `Pending` 时可读取后续状态的资源 URI（`app-mcp://<appId>/<资源名>`）。
    #[uniffi(default = None)]
    pub state_resource: Option<String>,
    /// App 给出的一句结论。
    #[uniffi(default = None)]
    pub summary: Option<String>,
    /// App 对结果内容的标注，原样。
    #[uniffi(default = None)]
    pub annotations: Option<ContentAnnotations>,
    /// 改调了 view 工具声明的后台替代时为实际调用的工具全名（spec/hub-api.md 3.14）；否则为空。
    #[uniffi(default = None)]
    pub routed_to: Option<String>,
    /// Hub 收到调用到得出结果的毫秒数（含审批、唤醒与等待 App；第 19 项 R4）。
    #[uniffi(default = 0)]
    pub duration_ms: u64,
    /// 本次 App 工具调用是否经历了唤醒；内置工具与上游工具恒为 false（第 19 项 R4）。
    #[uniffi(default = false)]
    pub woke: bool,
    /// 结果来自只读结果缓存（未转发给 App）时距 App 产出的毫秒数；未命中为空（spec/hub-api.md 3.20）。
    #[uniffi(default = None)]
    pub cached_age_ms: Option<u64>,
    /// 本次调用已登记撤销（spec/hub-api.md 3.23），可用 `apps.undo` 撤销；未登记为空。
    #[uniffi(default = None)]
    pub undo: Option<UndoOffer>,
    /// `apps.undo` 的结果：被撤销调用的 callId；其他调用为空。
    #[uniffi(default = None)]
    pub undo_of: Option<String>,
}

impl From<hub::CallOutcome> for CallOutcome {
    fn from(o: hub::CallOutcome) -> Self {
        let (data_json, error) = match o.result {
            Ok(v) => (Some(v.to_string()), None),
            Err(e) => (
                None,
                Some(ToolErrorInfo {
                    kind: e.kind.as_str().to_owned(),
                    message: e.message,
                    details_json: e.details.map(|d| d.to_string()),
                }),
            ),
        };
        CallOutcome {
            call_id: o.call_id,
            data_json,
            error,
            state_hints: o.state_hints,
            instance_id: o.instance_id,
            overview: o.overview.map(Into::into),
            status: o.status.into(),
            state_resource: o.state_resource,
            summary: o.summary,
            annotations: o.annotations.map(Into::into),
            routed_to: o.routed_to,
            duration_ms: o.duration_ms,
            woke: o.woke,
            cached_age_ms: o.cached_age_ms,
            undo: o.undo.map(Into::into),
            undo_of: o.undo_of,
        }
    }
}
