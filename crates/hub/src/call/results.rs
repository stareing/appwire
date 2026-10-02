//! 结果转换：MCP `CallToolResult` 的构造、纯文本与值的提取、错误码互转。

use app_mcp_protocol::{ErrorKind, ToolError, ToolsInvokeResult};
use rmcp::model::{CallToolResult, ContentBlock, ErrorCode, MetaObject, TextContent};
use rmcp::ErrorData as McpError;
use serde_json::{Map, Value, json};

use crate::hub::resource_uri;
use crate::mcp_convert::{self, OutputShape};

pub(crate) fn json_result(v: Value) -> CallToolResult {
    let text = serde_json::to_string(&v).unwrap_or_default();
    let mut r = CallToolResult::success(vec![ContentBlock::text(text)]);
    if v.is_object() {
        r.structured_content = Some(v);
    }
    r
}

/// 业务错误：以 `is_error: true` 的结果返回，模型能看到原因。
pub(crate) fn error_result(e: &ToolError) -> CallToolResult {
    let mut structured = json!({ "kind": e.kind, "message": e.message });
    if let Some(d) = &e.details {
        structured["details"] = d.clone();
    }
    let mut r = CallToolResult::error(vec![ContentBlock::text(format!(
        "{}: {}",
        e.kind, e.message
    ))]);
    r.structured_content = Some(json!({ "error": structured }));
    r
}

/// App 工具的成功结果（spec/hub-api.md 3.2）：状态说明（非 `done`）→ 摘要 → 返回值 JSON（无返回值、无摘要且 `done` 时为
/// "已完成"）→ 资源变化提示。App 的内容标注只加在 App 给出的内容块（摘要、返回值）上。
pub(crate) fn success_result(app_id: &str, r: ToolsInvokeResult, shape: OutputShape) -> CallToolResult {
    let state_uri = r.state_resource.as_deref().map(|n| resource_uri(app_id, n));
    let mut app_texts = Vec::new();
    if let Some(s) = &r.summary {
        app_texts.push(TextContent::new(s.clone()));
    }
    if !r.data.is_null() {
        app_texts.push(TextContent::new(serde_json::to_string(&r.data).unwrap_or_default()));
    }
    if app_texts.is_empty() && r.status.is_done() {
        app_texts.push(TextContent::new(mcp_convert::DONE_TEXT));
    }
    let annotations = r.annotations.as_ref().map(mcp_convert::content_annotations);
    let mut content: Vec<ContentBlock> = mcp_convert::status_note(r.status, state_uri.as_deref())
        .map(ContentBlock::text)
        .into_iter()
        .collect();
    content.extend(app_texts.into_iter().map(|t| {
        ContentBlock::Text(match &annotations {
            Some(a) => t.with_annotations(a.clone()),
            None => t,
        })
    }));
    if !r.state_hints.is_empty() {
        let uris: Vec<String> = r
            .state_hints
            .iter()
            .map(|h| resource_uri(app_id, h))
            .collect();
        content.push(ContentBlock::text(format!(
            "[app-mcp] 以下资源的内容可能已因本次调用而变化，如需最新状态请重新读取：{}",
            uris.join("、")
        )));
    }
    let mut result = CallToolResult::success(content);
    result.structured_content = shape.structured(&r.data);
    if !r.status.is_done() {
        let mut meta = MetaObject::new();
        meta.insert(mcp_convert::META_STATUS.to_owned(), json!(r.status));
        if let Some(uri) = state_uri {
            meta.insert(mcp_convert::META_STATE_RESOURCE.to_owned(), json!(uri));
        }
        result.meta = Some(meta);
    }
    result
}

/// 结果内容的纯文本：文本块以空行连接，非文本块序列化为 JSON。
pub fn result_text(r: &CallToolResult) -> String {
    r.content
        .iter()
        .map(|c| match c.as_text() {
            Some(t) => t.text.clone(),
            None => serde_json::to_string(c).unwrap_or_default(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Hub API 的结果值：结构化内容优先，其次单段文本（能解析为 JSON 时解析），否则内容数组。
pub(super) fn result_value(r: &CallToolResult) -> Value {
    if let Some(v) = &r.structured_content {
        return v.clone();
    }
    match r.content.as_slice() {
        [] => Value::Null,
        [one] => match one.as_text() {
            Some(t) => serde_json::from_str(&t.text).unwrap_or_else(|_| Value::String(t.text.clone())),
            None => serde_json::to_value(one).unwrap_or(Value::Null),
        },
        many => serde_json::to_value(many).unwrap_or(Value::Null),
    }
}

/// MCP 协议错误 → 协议错误类别（工具调用路径）。
///
/// `data.kind`（本 Hub 与 AppWire 上游经 [`to_mcp_error`] 写入）优先；没有时只认 MCP 标准码 `-32002` → `RESOURCE_NOT_FOUND`，
/// 其余一律 `HANDLER_ERROR`，原码放 `details.upstreamCode`。
///
/// @compat MCP 2026-07-28：接收方不得对 -32000…-32019（-32002 除外）假定含义，故不按数值反查 [`ErrorKind::from_code`]
/// （docs/plans/12-mcp-stateless.md 第 5 节，S3）。
pub(crate) fn mcp_error_to_tool(e: &McpError) -> ToolError {
    upstream_error_to_tool(e, &[ErrorCode::RESOURCE_NOT_FOUND])
}

/// 同 [`mcp_error_to_tool`]，用于资源读取：`-32602`（MCP 2026-07-28 的资源不存在）同样视为 `RESOURCE_NOT_FOUND`。
pub(crate) fn mcp_resource_error_to_tool(e: &McpError) -> ToolError {
    upstream_error_to_tool(e, &[ErrorCode::RESOURCE_NOT_FOUND, ErrorCode::INVALID_PARAMS])
}

fn upstream_error_to_tool(e: &McpError, not_found_codes: &[ErrorCode]) -> ToolError {
    let data = e.data.as_ref();
    let details = data.and_then(|d| d.get("details")).cloned();
    let from_data = data
        .and_then(|d| d.get("kind"))
        .and_then(|k| serde_json::from_value::<ErrorKind>(k.clone()).ok());
    if let Some(kind) = from_data {
        return ToolError { kind, message: e.message.to_string(), details };
    }
    let kind = if not_found_codes.contains(&e.code) { ErrorKind::ResourceNotFound } else { ErrorKind::HandlerError };
    let mut details = match details {
        Some(Value::Object(m)) => m,
        Some(other) => Map::from_iter([("upstream".to_owned(), other)]),
        None => Map::new(),
    };
    details.insert("upstreamCode".to_owned(), json!(e.code.0));
    ToolError { kind, message: e.message.to_string(), details: Some(Value::Object(details)) }
}

/// 把协议错误转为 MCP 协议错误（资源读取等非工具调用路径）。
pub(crate) fn to_mcp_error(e: &ToolError) -> McpError {
    let mut data = json!({ "kind": e.kind });
    if let Some(d) = &e.details {
        data["details"] = d.clone();
    }
    match e.kind {
        ErrorKind::ResourceNotFound => McpError::resource_not_found(e.message.clone(), Some(data)),
        kind => McpError::new(ErrorCode(kind.code() as i32), e.message.clone(), Some(data)),
    }
}
