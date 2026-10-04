//! 呈现与记录的单元测试（T-09：每条规则与兜底路径）。

use app_mcp_protocol::schema_compat::{ChangeLevel, SchemaChange};
use app_mcp_protocol::{Deprecation, ErrorKind, Risk, ToolInfo, ToolSurface};
use serde_json::json;

use super::log::{MAX_SCHEMA_CHANGES, SchemaChangeLog, SchemaChangeRecord};
use super::present::{REFETCH_HINT, invalid_arguments};
use crate::tool_def::ToolDef;

fn info(deprecated: Option<Deprecation>, description: &str) -> ToolInfo {
    ToolInfo {
        name: "send".into(),
        description: description.into(),
        input_schema: json!({"type": "object", "properties": {"to": {"type": "string"}}}),
        risk: Risk::Write,
        activation: None,
        title: None,
        annotations: None,
        output_schema: None,
        surface: ToolSurface::App,
        page: None,
        background_tool: None,
        implements: Vec::new(),
        cache: None,
        deprecated,
    }
}

fn deprecation(replacement: Option<&str>) -> Deprecation {
    Deprecation { message: "旧版发送".into(), replacement: replacement.map(str::to_owned), until: None }
}

fn change(level: ChangeLevel) -> SchemaChange {
    serde_json::from_value(json!({"level": level, "path": "/inputSchema/required", "message": "新增必填参数"})).unwrap()
}

#[test]
fn invalid_arguments_appends_hint_and_schema_hash() {
    let def = ToolDef::from_info(info(None, "发消息"));
    let e = invalid_arguments("chat", "send", &def, "to: 类型应为 string");
    assert_eq!(e.kind, ErrorKind::InvalidInput);
    assert_eq!(e.message, format!("参数不符合工具「chat.send」的 inputSchema：to: 类型应为 string。{REFETCH_HINT}。"));
    assert!(e.message.contains("请重新获取（apps.tools / tools/list）后再调用"));
    assert_eq!(e.details, Some(json!({"schemaHash": def.schema_hash()})));
}

#[test]
fn record_level_is_highest_and_empty_is_skipped() {
    assert_eq!(SchemaChangeRecord::new("a", "t", Vec::new(), 1), None, "无不兼容变化不记录");
    let warn = SchemaChangeRecord::new("a", "t", vec![change(ChangeLevel::Warning)], 1).unwrap();
    assert_eq!(warn.level, ChangeLevel::Warning);
    let mixed = vec![change(ChangeLevel::Warning), change(ChangeLevel::Breaking)];
    let r = SchemaChangeRecord::new("a", "t", mixed, 7).unwrap();
    assert_eq!((r.level, r.at, r.changes.len()), (ChangeLevel::Breaking, 7, 2), "有任一 breaking 即为 breaking");
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["appId"], "a");
    assert_eq!(v["level"], "breaking");
    assert_eq!(v["changes"][1]["path"], "/inputSchema/required");
}

#[test]
fn log_keeps_latest_records_oldest_first() {
    let mut log = SchemaChangeLog::default();
    for i in 0..(MAX_SCHEMA_CHANGES as u64 + 3) {
        log.push(SchemaChangeRecord::new("a", "t", vec![change(ChangeLevel::Warning)], i).unwrap());
    }
    let all = log.snapshot();
    assert_eq!(all.len(), MAX_SCHEMA_CHANGES);
    assert_eq!((all[0].at, all[MAX_SCHEMA_CHANGES - 1].at), (3, MAX_SCHEMA_CHANGES as u64 + 2), "满时丢最旧的");
}

#[cfg(feature = "mcp-server")]
mod mcp {
    use super::*;
    use crate::names::{META_DEPRECATED, META_SCHEMA_HASH};
    use crate::schema_evolution::{DEPRECATED_PREFIX, mcp_description, mcp_tool_meta};
    use crate::types::Availability;

    #[test]
    fn description_prefixes_in_order() {
        let plain = ToolDef::from_info(info(None, "发消息"));
        assert_eq!(mcp_description("chat", &plain, Availability::Available), "发消息");
        assert_eq!(mcp_description("chat", &plain, Availability::NotRegistered), "[当前不可用] 发消息");
        let dep = ToolDef::from_info(info(Some(deprecation(Some("send2"))), "发消息"));
        assert_eq!(mcp_description("chat", &dep, Availability::Dormant), "[已弃用] 旧版发送（改用 chat.send2） 发消息");
        assert_eq!(
            mcp_description("chat", &dep, Availability::NotRegistered),
            "[当前不可用] [已弃用] 旧版发送（改用 chat.send2） 发消息",
            "不可用在前、弃用在后"
        );
        let no_rep = ToolDef::from_info(info(Some(deprecation(None)), ""));
        assert_eq!(mcp_description("chat", &no_rep, Availability::Available), format!("{DEPRECATED_PREFIX}旧版发送"), "无 replacement、无描述");
    }

    #[test]
    fn tool_meta_has_hash_and_deprecation() {
        let plain = ToolDef::from_info(info(None, "d"));
        let meta = mcp_tool_meta(&plain);
        assert_eq!(meta.get(META_SCHEMA_HASH), Some(&json!(plain.schema_hash())));
        assert!(meta.get(META_DEPRECATED).is_none(), "未弃用不带");
        let dep = ToolDef::from_info(info(Some(deprecation(Some("send2"))), "d"));
        let meta = mcp_tool_meta(&dep);
        assert_eq!(meta.get(META_DEPRECATED), Some(&json!({"message": "旧版发送", "replacement": "send2"})));
    }
}
