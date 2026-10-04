//! 工具弃用声明（spec/protocol.md 3.7）：记录转换、注册 / 更新时的格式校验，以及更新与启停不丢声明。

use std::sync::Arc;

use app_mcp_native as native;

use crate::*;

struct NoopTool;

impl ToolHandler for NoopTool {
    fn invoke(&self, call: Arc<Call>) {
        let _ = call.complete(None, Vec::new());
    }
}

fn tool(name: &str, deprecated: Option<Deprecation>) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: "d".into(),
        input_schema_json: None,
        risk: Some(Risk::Read),
        activation: None,
        title: None,
        enabled: true,
        annotations: None,
        output_schema_json: None,
        surface: None,
        page: None,
        background_tool: None,
        concurrency: 0,
        exclusive: None,
        implements: Vec::new(),
        cache: None,
        deprecated,
    }
}

fn dep(message: &str, replacement: Option<&str>, until: Option<&str>) -> Deprecation {
    Deprecation { message: message.into(), replacement: replacement.map(Into::into), until: until.map(Into::into) }
}

fn offline_client() -> Arc<AppMcpClient> {
    let mut cfg = super::fake_host_config("uniffi-deprecated", "127.0.0.1:9");
    cfg.connect_timeout_ms = Some(1000);
    AppMcpClient::new(cfg, None).expect("client")
}

#[test]
fn deprecation_conversion() {
    let (_, options): (native::ToolSpec, native::ToolOptions) = tool("t.old", None).into();
    assert_eq!(options.deprecated, None, "未声明时为空");
    let full = dep("改用 t.new", Some("t.new"), Some("2027-06-30"));
    let (_, options): (native::ToolSpec, native::ToolOptions) = tool("t.old", Some(full)).into();
    let expected = native::Deprecation { message: "改用 t.new".into(), replacement: Some("t.new".into()), until: Some("2027-06-30".into()) };
    assert_eq!(options.deprecated, Some(expected), "三个字段原样透传");
    let (_, options): (native::ToolSpec, native::ToolOptions) = tool("t.old", Some(dep("即将移除", None, None))).into();
    let d = options.deprecated.expect("只有 message");
    assert_eq!((d.message.as_str(), d.replacement, d.until), ("即将移除", None, None));
}

#[test]
fn invalid_deprecation_is_rejected() {
    let client = offline_client();
    let too_long = "x".repeat(native::MAX_DEPRECATION_MESSAGE_CHARS + 1);
    let bad = [
        dep("", None, None),
        dep("  ", None, None),
        dep(&too_long, None, None),
        dep("m", Some("bad name!"), None),
        dep("m", Some("d.bad"), None),
        dep("m", None, Some("2027-02-30")),
        dep("m", None, Some("2027/06/30")),
    ];
    for d in bad {
        let err = client.register_tool(tool("d.bad", Some(d.clone())), Arc::new(NoopTool)).expect_err("非法声明");
        assert!(matches!(err, AppMcpError::InvalidConfig { .. }), "{d:?}: {err:?}");
    }
    let max = "x".repeat(native::MAX_DEPRECATION_MESSAGE_CHARS);
    let t = client.register_tool(tool("d.ok", Some(dep(&max, Some("d.missing"), Some("2028-02-29")))), Arc::new(NoopTool));
    let t = t.expect("上限长度、未注册的替代工具、闰日均合法");
    let err = t.update(tool("d.ok", Some(dep("m", Some("d.ok"), None)))).expect_err("更新为指向自身");
    assert!(matches!(err, AppMcpError::InvalidConfig { .. }), "{err:?}");
    client.stop();
}

/// toolsHash 含 deprecated 声明：更新替换 / 清除、启停都按声明变化，不被丢掉。
#[test]
fn update_replaces_and_clears_deprecation() {
    let hash_with = |deprecated: Option<Deprecation>| {
        let client = offline_client();
        let _tool = client.register_tool(tool("d.t", deprecated), Arc::new(NoopTool)).expect("tool");
        let h = client.tools_hash();
        client.stop();
        h
    };
    let none = hash_with(None);
    let a = hash_with(Some(dep("改用 d.new", Some("d.new"), None)));
    let b = hash_with(Some(dep("改用 d.new", Some("d.new"), Some("2027-06-30"))));
    assert_ne!(none, a, "deprecated 进 toolsHash");
    assert_ne!(a, b, "until 进 toolsHash");

    let client = offline_client();
    let t = client.register_tool(tool("d.t", Some(dep("改用 d.new", Some("d.new"), None))), Arc::new(NoopTool)).expect("tool");
    assert_eq!(client.tools_hash(), a);
    t.set_enabled(false).expect("disable");
    t.set_enabled(true).expect("enable");
    assert_eq!(client.tools_hash(), a, "启停不丢 deprecated");
    t.update(tool("d.t", Some(dep("改用 d.new", Some("d.new"), Some("2027-06-30"))))).expect("替换");
    assert_eq!(client.tools_hash(), b, "更新替换 deprecated");
    t.update(tool("d.t", None)).expect("清除");
    assert_eq!(client.tools_hash(), none, "更新为空即清除");
    client.stop();
}
