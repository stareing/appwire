//! 工具调用与资源读取的唯一实现（spec/hub-api.md 3.2）。
//!
//! 调用顺序：上游 MCP 服务器 → 内置工具 → App 工具。App 工具：路由 → schema 校验 → 审批 → 转发
//! （路由在校验之前，因为 schema 取自目标实例注册的定义）。
//!
//! 结果先表示为 [`Invocation`]，再分别转换为 MCP `CallToolResult`（MCP 出口、格式分派）
//! 或 [`CallOutcome`]（Hub API）。

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use app_mcp_protocol::{ErrorKind, ToolError, ToolsInvokeResult};
use rmcp::model::{CallToolResult, ContentBlock, MetaObject, ResultType};
use rmcp::ErrorData as McpError;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::hub::{overview_info, resource_uri};
use crate::progress::ProgressUpdate;
use crate::names;
use crate::mcp_convert::{self, OutputShape};
use crate::overview::Overview;
use crate::task::CallerKey;
use crate::types::{CallOutcome, CallRequest, HubError};

/// 内置工具名（唯一定义在 [`crate::names`]）。
pub use crate::names::{
    BUILTIN_APP_ID, TOOL_APPS_ACTIVATE, TOOL_APPS_LIST, TOOL_APPS_NAVIGATE, TOOL_APPS_OVERVIEW, TOOL_APPS_PAGE,
    TOOL_APPS_RELEASE, TOOL_APPS_SELECT, TOOL_APPS_TASK_BEGIN, TOOL_APPS_TASK_END, TOOL_APPS_TOOLS,
};

mod builtin;
mod builtin_defs;
mod dispatch;
mod guard;
mod invoke;
mod page_tools;
mod progress_watch;
mod resources;
mod results;
#[cfg(test)]
mod tests;
mod tool_convert;

pub(crate) use builtin_defs::{BuiltinSet, builtin_hub_tools};
#[cfg(feature = "mcp-server")]
pub(crate) use builtin_defs::builtin_tools;
pub(crate) use resources::{first_content, read_resource};
pub use results::result_text;
pub(crate) use results::{error_result, json_result, mcp_error_to_tool, mcp_resource_error_to_tool, success_result};
#[cfg(feature = "mcp-server")]
pub(crate) use results::to_mcp_error;
#[cfg(feature = "mcp-server")]
pub(crate) use tool_convert::to_mcp_tool;
pub(crate) use tool_convert::{
    app_hub_tool, tool_declaration, upstream_annotations, upstream_hub_tool, upstream_tool_declaration,
};
use results::result_value;

/// 静态工具在 App 已连接但没有实例注册时的描述前缀（spec/manifest.md 第 5 节）。
pub const UNAVAILABLE_PREFIX: &str = "[当前不可用] ";

/// 一次调用的输入。
#[derive(Clone, Debug)]
pub(crate) struct CallCtx {
    pub name: String,
    pub arguments: Value,
    /// 调用方键：总览附带、`apps.select`、租约、渐进暴露都记在该调用方的 Agent 任务上（[`crate::task`]）。
    pub caller: CallerKey,
    /// 厂商会话 ID（原样放进 [`ApprovalRequest::session`]）。
    pub session: Option<String>,
    pub instance_id: Option<String>,
    pub timeout: Option<Duration>,
    pub call_id: Option<String>,
    /// 发起调用的 legacy MCP 会话（渐进暴露展开新 App 时只通知该会话）；Hub API 与无会话的 MCP 请求为 `None`。
    pub mcp_session: Option<u64>,
    /// 调用方要接收进度时的出口（MCP 请求带 `progressToken`，spec/hub-api.md 3.12）；合并后的进度发到这里。
    pub progress: Option<ProgressSink>,
    /// Agent 的幂等键（[`CallRequest::idempotency_key`] / MCP 请求 `_meta`），原样转交 App。
    pub idempotency_key: Option<String>,
    /// MCP 出口：发起调用的认证主体（[`ApprovalRequest::principal`]）；Hub API 为 `None`。
    pub principal: Option<String>,
    /// MCP 出口：客户端自报的名称，仅供显示（[`ApprovalRequest::client_name`]）。
    pub client_name: Option<String>,
    /// MCP 请求 `_meta` 出示的任务句柄（`dev.appwire/taskId`）；调用开始时与参数 `taskId` 一并解析为调用方（[`crate::task_handle`]）。
    pub task_id: Option<String>,
}

/// 合并后的进度出口（[`CallCtx::progress`]）。
pub(crate) type ProgressSink = mpsc::UnboundedSender<ProgressUpdate>;

impl CallCtx {
    /// 发起方 Agent 名（第 16 项 N5；按 Agent 的策略规则）；本机主体与 Hub API 为 `None`。
    pub(crate) fn agent(&self) -> Option<&str> {
        self.caller.agent().map(crate::agents::AgentName::as_str)
    }

    pub(crate) fn from_request(req: CallRequest) -> Self {
        Self {
            mcp_session: None,
            caller: CallerKey::api(req.session.as_deref()),
            name: req.name,
            arguments: req.arguments,
            session: req.session,
            instance_id: req.instance_id,
            timeout: req.timeout,
            call_id: req.call_id,
            progress: None,
            idempotency_key: req.idempotency_key,
            principal: None,
            client_name: None,
            task_id: None,
        }
    }
}

/// 调用的结果主体。
pub(crate) enum Body {
    /// 名称无法解析（格式不对或 appId 未知）。
    NotFound(ToolError),
    Builtin(Result<CallToolResult, ToolError>),
    App(Result<ToolsInvokeResult, ToolError>),
    /// 上游结果原样保留；协议错误（`McpError`）在 MCP 出口原样透传。
    Upstream(Result<CallToolResult, McpError>),
}

pub(crate) struct Invocation {
    pub call_id: String,
    pub app_id: Option<String>,
    pub instance_id: Option<String>,
    /// 本次附带的总览（该会话首次接触此 App 或版本变化时）。
    pub overview: Option<Overview>,
    pub body: Body,
    /// App 工具声明的 `outputSchema` 决定的 `structuredContent` 形式。
    pub output_shape: OutputShape,
    /// 改调了后台替代时为实际调用的工具全名（spec/hub-api.md 3.14）。
    pub routed_to: Option<String>,
    /// Hub 收到调用到得出结果的毫秒数（[`HubShared::call`] 填写）。
    pub duration_ms: u64,
    /// 本次 App 工具调用是否经历了唤醒（[`ToolRun::woke`]）。
    pub woke: bool,
}

impl Invocation {
    /// 未路由到实例的结果（内置工具、名称无法解析、调用开始前即失败）；其余字段由调用方补填。
    fn bare(call_id: &str, app_id: Option<&str>, body: Body) -> Self {
        Self {
            call_id: call_id.to_owned(),
            app_id: app_id.map(str::to_owned),
            instance_id: None,
            overview: None,
            body,
            output_shape: OutputShape::Undeclared,
            routed_to: None,
            duration_ms: 0,
            woke: false,
        }
    }

    /// MCP 出口的结果：总览在内容最前面。
    pub(crate) fn to_mcp(&self) -> Result<CallToolResult, McpError> {
        let mut r = match &self.body {
            Body::NotFound(e) | Body::Builtin(Err(e)) | Body::App(Err(e)) => error_result(e),
            Body::Builtin(Ok(r)) | Body::Upstream(Ok(r)) => r.clone(),
            Body::App(Ok(r)) => {
                success_result(self.app_id.as_deref().unwrap_or_default(), r.clone(), self.output_shape)
            }
            Body::Upstream(Err(e)) => return Err(e.clone()),
        };
        if let Some(ov) = &self.overview {
            r.content.insert(0, ContentBlock::text(ov.render()));
        }
        // @compat 上游结果来自旧协议版本时没有 `resultType`；补成 `complete`（CallToolResult 只能是该值），
        // 由 rmcp 在回复旧协议版本的客户端时去掉（docs/plans/12-mcp-stateless.md S2）。
        r.result_type.get_or_insert(ResultType::COMPLETE);
        let meta = r.meta.get_or_insert_with(MetaObject::new);
        if let Some(to) = &self.routed_to {
            meta.insert(mcp_convert::META_ROUTED_TO.to_owned(), json!(to));
        }
        meta.insert(names::META_CALL_ID.to_owned(), json!(self.call_id));
        meta.insert(names::META_DURATION_MS.to_owned(), json!(self.duration_ms));
        if let Some(id) = &self.instance_id {
            meta.insert(names::META_INSTANCE_ID.to_owned(), json!(id));
        }
        if matches!(self.body, Body::App(_)) {
            meta.insert(names::META_WOKE.to_owned(), json!(self.woke));
        }
        Ok(r)
    }

    /// Hub API 的结果。
    pub(crate) fn into_outcome(self) -> Result<CallOutcome, HubError> {
        let mut state_hints = Vec::new();
        let mut app_result = None;
        let result = match self.body {
            Body::NotFound(e) => return Err(HubError(e)),
            Body::Builtin(r) => r.map(|r| result_value(&r)),
            Body::App(r) => r.map(|mut r| {
                state_hints = std::mem::take(&mut r.state_hints);
                let data = std::mem::take(&mut r.data);
                app_result = Some(r);
                data
            }),
            Body::Upstream(Ok(r)) if r.is_error == Some(true) => Err(ToolError::new(
                ErrorKind::HandlerError,
                result_text(&r),
            )),
            Body::Upstream(Ok(r)) => Ok(result_value(&r)),
            Body::Upstream(Err(e)) => Err(mcp_error_to_tool(&e)),
        };
        let app_id = self.app_id.unwrap_or_default();
        let r = app_result.unwrap_or_default();
        Ok(CallOutcome {
            call_id: self.call_id,
            result,
            state_hints,
            instance_id: self.instance_id,
            overview: self.overview.as_ref().map(overview_info),
            status: r.status,
            state_resource: r.state_resource.map(|n| resource_uri(&app_id, &n)),
            summary: r.summary,
            annotations: r.annotations,
            routed_to: self.routed_to,
            duration_ms: self.duration_ms,
            woke: self.woke,
        })
    }
}

pub(crate) type CancelFut<'a> = Pin<&'a mut (dyn Future<Output = ()> + Send)>;

/// 一次 App 工具调用的结果。
pub(crate) struct ToolRun {
    pub result: Result<ToolsInvokeResult, ToolError>,
    /// 实际处理调用的实例。
    pub instance_id: Option<String>,
    pub output_shape: OutputShape,
    /// 调用时目标未连接、经唤醒回连后才送达（休眠实例唤醒、按清单冷启动、页面工具的 App 唤醒）。
    pub woke: bool,
}

fn cancelled() -> ToolError {
    ToolError::new(ErrorKind::Cancelled, "调用已被取消。")
}

/// appId 未知（或被 `hide` 规则整体隐藏，二者对 Agent 不可区分）。
/// appId 未知（或被 `hide` 整体隐藏，与不存在相同）→ `TOOL_NOT_FOUND`。
pub(crate) fn unknown_app(app_id: &str) -> ToolError {
    ToolError::new(
        ErrorKind::ToolNotFound,
        format!("没有 appId 为「{app_id}」的 App。可调用 apps.list 查看可用的 App。"),
    )
}
