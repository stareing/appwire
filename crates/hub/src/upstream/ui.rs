//! 上游资源在 Hub 侧的 URI 与 MCP Apps 界面透传（spec/hub-api.md 3.22，第 16 项 N8a）。
//!
//! 上游资源 URI 统一改写为带上游名的 Hub URI（多个上游的同名 URI 互不冲突），读取时按上游名还原：
//! - MCP Apps 界面资源（`ui://…`）→ `ui://<上游名>/<编码后的原 URI>`：规范要求界面资源必须用 `ui://`，不能改成 `app-mcp://`；
//! - 其余资源 → `app-mcp://<上游名>/<编码后的原 URI>`。
//!
//! 上游工具 `_meta` 中指向界面资源的键（`ui.resourceUri` 与已弃用的平铺键 `ui/resourceUri`）同样改写，
//! 其余 `_meta`（含 `ui.visibility`、`ui.csp`）原样保留；渲染、可见性与是否信任界面由 Agent 决定，Hub 不评估 HTML。

#[cfg(feature = "upstream")]
use rmcp::model::{ExtensionCapabilities, JsonObject, Tool};
#[cfg(feature = "upstream")]
use serde_json::{Value, json};

use crate::hub::resource_uri;

use super::{decode_uri_component, encode_uri_component};

/// MCP Apps 扩展标识（modelcontextprotocol/ext-apps，规范 2026-01-26）。
pub const MCP_APPS_EXTENSION: &str = "io.modelcontextprotocol/ui";

/// MCP Apps 界面资源的 MIME 类型。
pub const MCP_APP_MIME: &str = "text/html;profile=mcp-app";

/// 界面资源 URI 前缀（规范：界面资源 MUST 使用 `ui://`）。
pub const UI_URI_SCHEME: &str = "ui://";

/// 工具 `_meta` 中界面资源键：嵌套 `ui.resourceUri`。
#[cfg(feature = "upstream")]
const META_UI: &str = "ui";
#[cfg(feature = "upstream")]
const META_UI_RESOURCE_URI: &str = "resourceUri";
/// @compat 已弃用的平铺键（规范说明 GA 前移除；官方 SDK 仍同时写出）。
#[cfg(feature = "upstream")]
const META_UI_RESOURCE_URI_FLAT: &str = "ui/resourceUri";

/// 上游资源 `original` 在 Hub 侧的 URI。
///
/// @invariant 与 [`parse_upstream_ui_uri`]（界面资源）/ `parse_resource_uri`（其余）互逆。
pub(crate) fn hub_upstream_uri(upstream: &str, original: &str) -> String {
    let encoded = encode_uri_component(original);
    if original.starts_with(UI_URI_SCHEME) {
        format!("{UI_URI_SCHEME}{upstream}/{encoded}")
    } else {
        resource_uri(upstream, &encoded)
    }
}

/// 解析 Hub 侧的界面资源 URI，返回 `(上游名, 编码后的原 URI)`；不是 `ui://<名>/<非空>` 时为 `None`。
pub(crate) fn parse_upstream_ui_uri(uri: &str) -> Option<(&str, &str)> {
    let (name, encoded) = uri.strip_prefix(UI_URI_SCHEME)?.split_once('/')?;
    (!name.is_empty() && !encoded.is_empty()).then_some((name, encoded))
}

/// 还原上游原 URI：Hub 侧的界面资源 URI 只由 [`hub_upstream_uri`] 产生，还原结果不是 `ui://` 时视为伪造。
pub(crate) fn decode_upstream_ui_uri(encoded: &str) -> Option<String> {
    decode_uri_component(encoded).filter(|u| u.starts_with(UI_URI_SCHEME))
}

/// 把上游工具 `_meta` 中的界面资源 URI 改写为 Hub 侧 URI（就地）。值不是 `ui://` 字符串的键不动（如实透传，由 Agent 判断）。
#[cfg(feature = "upstream")]
pub(crate) fn rewrite_tool_ui_meta(upstream: &str, tool: &mut Tool) {
    let Some(meta) = tool.meta.as_mut() else { return };
    if let Some(Value::Object(ui)) = meta.get_mut(META_UI) {
        rewrite_ui_uri(upstream, ui.get_mut(META_UI_RESOURCE_URI));
    }
    rewrite_ui_uri(upstream, meta.get_mut(META_UI_RESOURCE_URI_FLAT));
}

#[cfg(feature = "upstream")]
fn rewrite_ui_uri(upstream: &str, value: Option<&mut Value>) {
    if let Some(Value::String(uri)) = value
        && uri.starts_with(UI_URI_SCHEME)
    {
        *uri = hub_upstream_uri(upstream, uri);
    }
}

/// Hub 作为上游的 MCP 客户端、以及在有上游声明该扩展时作为服务器，声明的 MCP Apps 扩展能力。
#[cfg(feature = "upstream")]
pub(crate) fn mcp_apps_extension() -> ExtensionCapabilities {
    let settings: JsonObject = match json!({ "mimeTypes": [MCP_APP_MIME] }) {
        Value::Object(m) => m,
        _ => JsonObject::new(),
    };
    ExtensionCapabilities::from([(MCP_APPS_EXTENSION.to_owned(), settings)])
}

/// 上游声明的扩展能力中是否含 MCP Apps。
#[cfg(feature = "upstream")]
pub(crate) fn declares_mcp_apps(extensions: Option<&ExtensionCapabilities>) -> bool {
    extensions.is_some_and(|e| e.contains_key(MCP_APPS_EXTENSION))
}

#[cfg(all(test, feature = "upstream"))]
mod tests {
    use rmcp::model::MetaObject;
    use serde_json::Map;

    use super::*;
    use crate::hub::parse_resource_uri;

    fn tool_with_meta(meta: Value) -> Tool {
        let Value::Object(m) = meta else { unreachable!() };
        let mut t = Tool::new("w", "d", Map::new());
        t.meta = Some(MetaObject::from(m));
        t
    }

    #[test]
    fn ui_uri_roundtrip() {
        let hub = hub_upstream_uri("files", "ui://widget/view.html?x=1");
        assert!(hub.starts_with("ui://files/"), "{hub}");
        let (name, encoded) = parse_upstream_ui_uri(&hub).expect("解析");
        assert_eq!(name, "files");
        assert_eq!(decode_upstream_ui_uri(encoded).as_deref(), Some("ui://widget/view.html?x=1"));
    }

    /// 非界面资源仍是 `app-mcp://<名>/<编码>`，与改写前一致。
    #[test]
    fn other_resources_keep_app_mcp_scheme() {
        let hub = hub_upstream_uri("files", "demo://greeting");
        assert_eq!(hub, format!("app-mcp://files/{}", encode_uri_component("demo://greeting")));
        assert_eq!(parse_resource_uri(&hub).map(|(a, _)| a), Some("files"));
        assert_eq!(parse_upstream_ui_uri(&hub), None);
    }

    /// 每条拒绝规则一例：缺上游名、缺路径、无分隔、非 ui 前缀、还原后不是 `ui://`、非法编码。
    #[test]
    fn rejects_malformed_ui_uris() {
        for uri in ["ui:///abc", "ui://files/", "ui://files", "app-mcp://files/x"] {
            assert_eq!(parse_upstream_ui_uri(uri), None, "{uri}");
        }
        assert_eq!(decode_upstream_ui_uri(&encode_uri_component("file:///etc/passwd")), None);
        assert_eq!(decode_upstream_ui_uri("%zz"), None);
    }

    #[test]
    fn rewrites_nested_and_flat_keys_only() {
        let mut t = tool_with_meta(json!({
            "ui": {"resourceUri": "ui://w/v.html", "visibility": ["model", "app"], "csp": {"connectDomains": ["a.example"]}},
            "ui/resourceUri": "ui://w/v.html",
            "other": "ui://w/v.html",
        }));
        rewrite_tool_ui_meta("up", &mut t);
        let meta = t.meta.as_ref().expect("meta");
        let expected = hub_upstream_uri("up", "ui://w/v.html");
        assert_eq!(meta["ui"]["resourceUri"], expected);
        assert_eq!(meta["ui/resourceUri"], expected);
        assert_eq!(meta["ui"]["visibility"], json!(["model", "app"]));
        assert_eq!(meta["ui"]["csp"], json!({"connectDomains": ["a.example"]}));
        assert_eq!(meta["other"], "ui://w/v.html");
    }

    /// 值不是 `ui://` 字符串时不动；没有 `_meta` 时不新增。
    #[test]
    fn leaves_non_ui_values_untouched() {
        let mut t = tool_with_meta(json!({"ui": {"resourceUri": "https://x/y"}, "ui/resourceUri": 3}));
        rewrite_tool_ui_meta("up", &mut t);
        let meta = t.meta.as_ref().expect("meta");
        assert_eq!(meta["ui"]["resourceUri"], "https://x/y");
        assert_eq!(meta["ui/resourceUri"], 3);
        let mut bare = Tool::new("b", "d", Map::new());
        rewrite_tool_ui_meta("up", &mut bare);
        assert!(bare.meta.is_none());
    }

    #[test]
    fn extension_declaration() {
        let ext = mcp_apps_extension();
        assert_eq!(ext[MCP_APPS_EXTENSION]["mimeTypes"], json!([MCP_APP_MIME]));
        assert!(declares_mcp_apps(Some(&ext)));
        assert!(!declares_mcp_apps(Some(&ExtensionCapabilities::new())));
        assert!(!declares_mcp_apps(None));
    }
}
