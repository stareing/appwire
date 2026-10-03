//! 资源读取（MCP 出口与 Hub API 共用）：App 资源、上游资源与内容大小上限。

use std::sync::Arc;

use app_mcp_protocol::ErrorKind;
use rmcp::model::{ErrorCode, ReadResourceRequestParams, ReadResourceResult, ResourceContents, ResultType};
use rmcp::{ErrorData as McpError, Peer, RoleClient, ServiceError};
use serde_json::{Value, json};

use crate::hub::{DEFAULT_MIME, HubShared, parse_resource_uri};
use crate::limits::Payload;
use crate::names::BUILTIN_APP_ID;
use crate::task::CallerKey;
use crate::types::ResourceContent;
use crate::upstream::decode_uri_component;

use super::results::to_mcp_error;

/// 读取资源（MCP 出口与 Hub API 共用）。上游的协议错误原样返回。
pub(crate) async fn read_resource(
    shared: &Arc<HubShared>,
    uri: &str,
    caller: &CallerKey,
) -> Result<ReadResourceResult, McpError> {
    let Some((app_id, name)) = parse_resource_uri(uri) else {
        return Err(McpError::resource_not_found(
            format!("无法识别的资源 URI：{uri}"),
            None,
        ));
    };
    let _activity = shared.session_request(caller);
    if app_id == BUILTIN_APP_ID {
        return crate::hub_state::read_hub_state(shared, name, uri, caller);
    }
    if shared.app_hidden_hit(app_id) {
        return Err(McpError::resource_not_found(format!("资源「{uri}」不存在"), Some(json!({ "kind": ErrorKind::ResourceNotFound }))));
    }
    if let Some(peer) = shared.upstream_peer(app_id) {
        return read_upstream_resource(shared, app_id, name, uri, peer).await;
    }
    let (info, result) = shared
        .read_app_resource(app_id, name, shared.selected_for(caller, app_id), caller)
        .await
        .map_err(|e| to_mcp_error(&e))?;
    let contents = resource_contents(uri, info.mime_type.as_deref(), result);
    check_resource_size(shared, app_id, uri, std::slice::from_ref(&contents))?;
    Ok(ReadResourceResult::new(vec![contents]))
}

async fn read_upstream_resource(
    shared: &HubShared,
    name: &str,
    encoded: &str,
    uri: &str,
    peer: Option<Peer<RoleClient>>,
) -> Result<ReadResourceResult, McpError> {
    let peer = peer.ok_or_else(|| {
        McpError::new(
            ErrorCode(ErrorKind::AppDisconnected.code() as i32),
            format!("上游 MCP 服务器「{name}」当前未连接。"),
            Some(json!({ "kind": ErrorKind::AppDisconnected })),
        )
    })?;
    let original = decode_uri_component(encoded).ok_or_else(|| {
        McpError::resource_not_found(format!("无法识别的资源 URI：{uri}"), None)
    })?;
    let fut = peer.read_resource(ReadResourceRequestParams::new(original.clone()));
    let mut result = match tokio::time::timeout(shared.config.response_timeout, fut).await {
        Ok(Ok(r)) => r,
        Ok(Err(ServiceError::McpError(e))) => return Err(e),
        Ok(Err(e)) => {
            return Err(McpError::internal_error(
                format!("读取上游资源失败：{e}"),
                None,
            ));
        }
        Err(_) => return Err(McpError::internal_error("读取上游资源超时", None)),
    };
    for c in &mut result.contents {
        let u = match c {
            ResourceContents::TextResourceContents { uri, .. } => uri,
            ResourceContents::BlobResourceContents { uri, .. } => uri,
            #[allow(unreachable_patterns)]
            _ => continue,
        };
        if *u == original {
            *u = uri.to_owned();
        }
    }
    check_resource_size(shared, name, uri, &result.contents)?;
    // @compat 同工具结果：旧协议版本的上游不带 `resultType`，补成 `complete`（docs/plans/12-mcp-stateless.md S2）。
    result.result_type.get_or_insert(ResultType::COMPLETE);
    Ok(result)
}

/// 资源内容（文本 / base64）的大小上限（spec/hub-api.md 3.11）。
fn check_resource_size(shared: &HubShared, app_id: &str, uri: &str, contents: &[ResourceContents]) -> Result<(), McpError> {
    let size: usize = contents
        .iter()
        .map(|c| match c {
            ResourceContents::TextResourceContents { text, .. } => text.len(),
            ResourceContents::BlobResourceContents { blob, .. } => blob.len(),
            #[allow(unreachable_patterns)]
            _ => 0,
        })
        .sum();
    shared
        .guard_payload(app_id, Payload::Resource, size, &format!("资源「{uri}」"))
        .map_err(|e| to_mcp_error(&e))
}

/// Hub API：取第一段内容。
pub(crate) fn first_content(uri: &str, r: ReadResourceResult) -> ResourceContent {
    let empty = ResourceContent {
        uri: uri.to_owned(),
        mime_type: None,
        text: None,
        blob: None,
    };
    match r.contents.into_iter().next() {
        Some(ResourceContents::TextResourceContents {
            uri, mime_type, text, ..
        }) => ResourceContent {
            uri,
            mime_type,
            text: Some(text),
            blob: None,
        },
        Some(ResourceContents::BlobResourceContents {
            uri, mime_type, blob, ..
        }) => ResourceContent {
            uri,
            mime_type,
            text: None,
            blob: Some(blob),
        },
        #[allow(unreachable_patterns)]
        _ => empty,
    }
}

pub(crate) fn resource_contents(
    uri: &str,
    default_mime: Option<&str>,
    result: app_mcp_protocol::ResourcesReadResult,
) -> ResourceContents {
    let mime = result
        .mime_type
        .as_deref()
        .or(default_mime)
        .unwrap_or(DEFAULT_MIME)
        .to_owned();
    let is_json = mime == DEFAULT_MIME || mime.ends_with("+json");
    let text = match (&result.contents, is_json) {
        (Value::String(s), false) => s.clone(),
        (v, _) => serde_json::to_string(v).unwrap_or_default(),
    };
    ResourceContents::text(text, uri).with_mime_type(mime)
}
