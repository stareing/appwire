//! JSON 辅助：错误与兜底结果的序列化、过滤器与导出格式的解析。

use std::ffi::c_int;

use hub::{CallOutcome, ErrorKind, ToolError, ToolFilter, ToolFormat, format};
use serde_json::{Value, json};

use crate::ffi_util::{AmHubStatus, FfiError, FfiResult};
use crate::handle::STOPPED_MESSAGE;

// ---------------------------------------------------------------------------
// JSON 辅助
// ---------------------------------------------------------------------------

pub(crate) fn error_json(e: &ToolError) -> Value {
    let mut v = json!({ "kind": e.kind, "message": e.message });
    if let Some(d) = &e.details {
        v["details"] = d.clone();
    }
    v
}

/// 以 `CallOutcome` 形式表示的失败结果。
pub(crate) fn outcome_error_json(call_id: &str, e: ToolError) -> String {
    let o = CallOutcome {
        call_id: call_id.to_owned(),
        result: Err(e),
        state_hints: Vec::new(),
        instance_id: None,
        overview: None,
        status: hub::ResultStatus::Done,
        state_resource: None,
        summary: None,
        annotations: None,
        routed_to: None,
        duration_ms: 0,
        woke: false,
        cached_age_ms: None,
    };
    serde_json::to_string(&o).unwrap_or_default()
}

/// dispatch 的兜底结果（Hub 停止）：该格式的错误结果消息。
pub(crate) fn dispatch_fallback(format: ToolFormat, call: &Value) -> String {
    let parsed = format::parse_call(format, call).unwrap_or_else(|p| p);
    let text = format!("{}: {STOPPED_MESSAGE}", error_kind_str(ErrorKind::Cancelled));
    let result = json!({ "content": [{ "type": "text", "text": text }], "isError": true });
    match serde_json::from_value(result) {
        Ok(r) => format::render_result(format, &parsed, &r).to_string(),
        Err(_) => json!({ "error": { "kind": ErrorKind::Cancelled, "message": STOPPED_MESSAGE } })
            .to_string(),
    }
}

pub(crate) fn error_kind_str(k: ErrorKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

pub(crate) fn to_json<T: serde::Serialize>(v: &T) -> FfiResult<String> {
    serde_json::to_string(v)
        .map_err(|e| FfiError::new(AmHubStatus::Internal, format!("序列化失败：{e}")))
}

pub(crate) fn parse_filter(text: Option<&str>) -> FfiResult<ToolFilter> {
    match text.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => serde_json::from_str(t).map_err(|e| FfiError::json("filter_json", e)),
        None => Ok(ToolFilter::default()),
    }
}

pub(crate) fn format_from(v: c_int) -> FfiResult<ToolFormat> {
    Ok(match v {
        0 => ToolFormat::Mcp,
        1 => ToolFormat::OpenAiChat,
        2 => ToolFormat::OpenAiResponses,
        3 => ToolFormat::Anthropic,
        4 => ToolFormat::Gemini,
        _ => return Err(FfiError::invalid_argument(format!("非法的 format：{v}"))),
    })
}
