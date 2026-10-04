#[cfg(feature = "mcp-server")]
use app_mcp_protocol::Activation;
use app_mcp_protocol::{ErrorKind, Risk, ToolError, ToolsInvokeResult};
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorCode, ReadResourceResult, ResourceContents, ResultType, Tool, ToolAnnotations,
};
use rmcp::ErrorData as McpError;
use serde_json::{Value, json};
use app_mcp_protocol::ResourcesReadResult;

use crate::hub::DEFAULT_MIME;
use crate::names;
use crate::mcp_convert::{self, OutputShape};
#[cfg(feature = "mcp-server")]
use crate::tool_def::ToolDef;
#[cfg(feature = "mcp-server")]
use crate::types::Availability;
use crate::types::{CallOutcome, CallRequest};

use super::*;
use super::{builtin_defs::*, resources::*, results::*, tool_convert::*};

#[test]
fn error_result_is_error_with_kind() {
    let r = error_result(&ToolError::new(ErrorKind::HandlerError, "坏了"));
    assert_eq!(r.is_error, Some(true));
    assert_eq!(r.content[0].as_text().unwrap().text, "HANDLER_ERROR: 坏了");
    assert_eq!(
        r.structured_content.unwrap()["error"]["kind"],
        "HANDLER_ERROR"
    );
}

#[test]
fn success_result_with_hints() {
    let r = success_result(
        "shop",
        ToolsInvokeResult {
            data: json!({"ok": true}),
            state_hints: vec!["cart.state".into()],
            ..ToolsInvokeResult::default()
        },
        OutputShape::Undeclared,
    );
    assert_eq!(r.is_error, Some(false));
    assert_eq!(r.content.len(), 2);
    assert!(
        r.content[1]
            .as_text()
            .unwrap()
            .text
            .contains("app-mcp://shop/cart.state")
    );
    assert_eq!(r.structured_content, Some(json!({"ok": true})));
    let r = success_result(
        "shop",
        ToolsInvokeResult { data: json!([1]), ..ToolsInvokeResult::default() },
        OutputShape::Undeclared,
    );
    assert_eq!(r.structured_content, None);
    assert_eq!(r.content[0].as_text().unwrap().text, "[1]");
    assert_eq!(r.meta, None);
}

/// 后台替代（spec/hub-api.md 3.14）：改调时 MCP 结果的 `_meta` 带实际调用的工具全名，Hub API 结果带 `routed_to`；
/// 与结果状态的键并存。
#[test]
fn routed_result_meta() {
    let inv = |routed_to: Option<&str>, r: Result<ToolsInvokeResult, ToolError>| Invocation {
        call_id: "c".into(),
        app_id: Some("shop".into()),
        instance_id: None,
        overview: None,
        body: Body::App(r),
        output_shape: OutputShape::Undeclared,
        routed_to: routed_to.map(str::to_owned),
        duration_ms: 0,
        woke: false,
        cached_age_ms: None,
    };
    let pending = ToolsInvokeResult { status: crate::ResultStatus::Pending, ..ToolsInvokeResult::default() };
    let r = inv(Some("shop.cart.add"), Ok(pending.clone())).to_mcp().unwrap();
    let meta = r.meta.unwrap();
    assert_eq!(meta.get(mcp_convert::META_ROUTED_TO), Some(&json!("shop.cart.add")));
    assert_eq!(meta.get(mcp_convert::META_STATUS), Some(&json!("pending")));
    let r = inv(Some("shop.cart.add"), Err(ToolError::new(ErrorKind::HandlerError, "x"))).to_mcp().unwrap();
    assert_eq!(r.meta.unwrap().get("dev.appwire/routedTo"), Some(&json!("shop.cart.add")), "错误结果同样标出");
    let meta = inv(None, Ok(ToolsInvokeResult::default())).to_mcp().unwrap().meta.unwrap();
    assert!(meta.get(mcp_convert::META_ROUTED_TO).is_none() && meta.get(mcp_convert::META_STATUS).is_none(), "{meta:?}");
    let o = inv(Some("shop.cart.add"), Ok(pending)).into_outcome().unwrap();
    assert_eq!(o.routed_to.as_deref(), Some("shop.cart.add"));
}

/// 第 19 项 R4：调用元信息。callId、durationMs 每种结果都带；instanceId 只在路由到实例时带；woke 只在 App 工具结果中出现。
/// 第 12 项 S2：结果带 `resultType: complete`（上游旧协议结果缺省时补齐）。
#[test]
fn call_meta_keys_and_result_type() {
    let inv = |body: Body, instance: Option<&str>, woke: bool| Invocation {
        call_id: "call-7".into(),
        app_id: Some("shop".into()),
        instance_id: instance.map(str::to_owned),
        overview: None,
        body,
        output_shape: OutputShape::Undeclared,
        routed_to: None,
        duration_ms: 42,
        woke,
        cached_age_ms: None,
    };
    let r = inv(Body::App(Ok(ToolsInvokeResult::default())), Some("shop-1"), true).to_mcp().unwrap();
    assert_eq!(r.result_type, Some(ResultType::COMPLETE));
    let meta = r.meta.unwrap();
    assert_eq!(meta.get(names::META_CALL_ID), Some(&json!("call-7")));
    assert_eq!(meta.get(names::META_INSTANCE_ID), Some(&json!("shop-1")));
    assert_eq!(meta.get(names::META_DURATION_MS), Some(&json!(42)));
    assert_eq!(meta.get(names::META_WOKE), Some(&json!(true)));
    assert!(meta.get(names::META_CACHED).is_none(), "未命中缓存不写 cached");
    let mut hit = inv(Body::App(Ok(ToolsInvokeResult::default())), Some("shop-1"), false);
    hit.cached_age_ms = Some(1500);
    let meta = hit.to_mcp().unwrap().meta.unwrap();
    assert_eq!(meta.get(names::META_CACHED), Some(&json!({"ageMs": 1500})), "命中缓存：dev.appwire/cached");
    assert_eq!(meta.get(names::META_WOKE), Some(&json!(false)));

    let err = ToolError::new(ErrorKind::Timeout, "x");
    let meta = inv(Body::App(Err(err.clone())), None, false).to_mcp().unwrap().meta.unwrap();
    assert_eq!(meta.get(names::META_WOKE), Some(&json!(false)), "失败的 App 工具调用同样带 woke");
    assert!(meta.get(names::META_INSTANCE_ID).is_none());

    for body in [Body::Builtin(Ok(json_result(json!({})))), Body::NotFound(err)] {
        let meta = inv(body, None, false).to_mcp().unwrap().meta.unwrap();
        assert_eq!(meta.get(names::META_CALL_ID), Some(&json!("call-7")));
        assert_eq!(meta.get(names::META_DURATION_MS), Some(&json!(42)));
        assert!(meta.get(names::META_WOKE).is_none() && meta.get(names::META_INSTANCE_ID).is_none(), "{meta:?}");
    }

    // 上游旧协议结果：没有 resultType → 补 complete；原有 _meta 保留
    let mut legacy: CallToolResult =
        serde_json::from_value(json!({"content": [], "_meta": {"x/y": 1}})).unwrap();
    assert_eq!(legacy.result_type, None);
    legacy.is_error = Some(false);
    let r = inv(Body::Upstream(Ok(legacy)), None, false).to_mcp().unwrap();
    assert_eq!(r.result_type, Some(ResultType::COMPLETE));
    let meta = r.meta.unwrap();
    assert_eq!(meta.get("x/y"), Some(&json!(1)));
    assert_eq!(meta.get(names::META_CALL_ID), Some(&json!("call-7")));

    let o = inv(Body::App(Ok(ToolsInvokeResult::default())), Some("shop-1"), true).into_outcome().unwrap();
    assert_eq!((o.call_id.as_str(), o.duration_ms, o.woke), ("call-7", 42, true));
    let v = serde_json::to_value(&o).unwrap();
    assert_eq!((v["durationMs"].clone(), v["woke"].clone()), (json!(42), json!(true)));
    // 旧 JSON（没有新字段）仍可解析
    let mut old = v.clone();
    old.as_object_mut().unwrap().retain(|k, _| k != "durationMs" && k != "woke");
    let o: CallOutcome = serde_json::from_value(old).unwrap();
    assert_eq!((o.duration_ms, o.woke), (0, false));
}

/// 第 19 项 R3：无返回值 → "已完成"、不填 structuredContent；有摘要时摘要代替。
#[test]
fn success_result_without_value() {
    let r = success_result("shop", ToolsInvokeResult::default(), OutputShape::Object);
    assert_eq!(r.content.len(), 1);
    assert_eq!(r.content[0].as_text().unwrap().text, "已完成");
    assert_eq!(r.structured_content, None);
    let r = success_result(
        "shop",
        ToolsInvokeResult { summary: Some("已加入购物车".into()), ..ToolsInvokeResult::default() },
        OutputShape::Undeclared,
    );
    assert_eq!(r.content.len(), 1);
    assert_eq!(r.content[0].as_text().unwrap().text, "已加入购物车");
}

/// 第 19 项 R1 + 第 14 项 S2：状态说明在前、_meta 带状态；内容标注只加在 App 的内容块上。
#[test]
fn success_result_status_and_annotations() {
    let r = success_result(
        "shop",
        ToolsInvokeResult {
            data: json!({"orderId": "o1"}),
            status: app_mcp_protocol::ResultStatus::Pending,
            state_resource: Some("order.state".into()),
            summary: Some("已提交，等待付款".into()),
            annotations: Some(app_mcp_protocol::ContentAnnotations { priority: Some(0.5), ..Default::default() }),
            state_hints: vec!["cart.state".into()],
        },
        OutputShape::Object,
    );
    let texts: Vec<&str> = r.content.iter().map(|c| c.as_text().unwrap().text.as_str()).collect();
    assert_eq!(texts.len(), 4, "{texts:?}");
    assert!(texts[0].contains("尚未完成") && texts[0].contains("app-mcp://shop/order.state"));
    assert_eq!(texts[1], "已提交，等待付款");
    assert_eq!(texts[2], r#"{"orderId":"o1"}"#);
    assert!(texts[3].contains("app-mcp://shop/cart.state"));
    let annotated: Vec<bool> = r.content.iter().map(|c| c.as_text().unwrap().annotations.is_some()).collect();
    assert_eq!(annotated, [false, true, true, false]);
    let meta = r.meta.unwrap();
    assert_eq!(meta.get("dev.appwire/status"), Some(&json!("pending")));
    assert_eq!(meta.get("dev.appwire/stateResource"), Some(&json!("app-mcp://shop/order.state")));
    assert_eq!(r.structured_content, Some(json!({"orderId": "o1"})));
    // noop 且无返回值：只有状态说明，不出现"已完成"
    let r = success_result(
        "shop",
        ToolsInvokeResult { status: app_mcp_protocol::ResultStatus::Noop, ..ToolsInvokeResult::default() },
        OutputShape::Undeclared,
    );
    assert_eq!(r.content.len(), 1);
    assert!(r.content[0].as_text().unwrap().text.contains("没有做任何改动"));
    // 非对象结果按声明包装
    let r = success_result(
        "shop",
        ToolsInvokeResult { data: json!(["a"]), ..ToolsInvokeResult::default() },
        OutputShape::Wrapped,
    );
    assert_eq!(r.structured_content, Some(json!({"result": ["a"]})));
}

#[cfg(feature = "mcp-server")]
#[test]
fn tool_conversion() {
    use app_mcp_protocol::ToolInfo;
    let info: ToolInfo = serde_json::from_value(json!({
        "name": "orders.search", "description": "搜索", "inputSchema": {"type": "object"}, "risk": "read", "title": "搜"
    }))
    .unwrap();
    let info = ToolDef::from_info(info);
    let t = to_mcp_tool("shop", &info, Availability::NotRegistered);
    assert_eq!(t.name, "shop.orders.search");
    assert_eq!(t.description.as_deref(), Some("[当前不可用] 搜索"));
    assert_eq!(t.title.as_deref(), Some("搜"));
    assert_eq!(t.annotations.unwrap().read_only_hint, Some(true));
    assert!(t.output_schema.is_none());
    // 声明的注解逐字段优先，缺少的按 risk 推导；非对象 outputSchema 包装
    let declared: ToolInfo = serde_json::from_value(json!({
        "name": "orders.cancel", "description": "取消", "inputSchema": {"type": "object"}, "risk": "destructive",
        "annotations": {"idempotentHint": true, "openWorldHint": false, "title": "取消订单"},
        "outputSchema": {"type": "array"}
    }))
    .unwrap();
    let declared = ToolDef::from_info(declared);
    let t = to_mcp_tool("shop", &declared, Availability::Available);
    assert_eq!(
        serde_json::to_value(t.annotations.unwrap()).unwrap(),
        json!({"title": "取消订单", "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false})
    );
    assert_eq!(
        Value::Object((*t.output_schema.unwrap()).clone()),
        json!({"type": "object", "properties": {"result": {"type": "array"}}, "required": ["result"]})
    );
    let d = tool_declaration(&declared);
    assert_eq!((d.risk, d.output_schema, d.effective.destructive_hint), (Risk::Destructive, true, Some(true)));
    assert_eq!(d.annotations.unwrap().destructive_hint, None, "声明原样");
    let hd = app_hub_tool("shop", &declared, Availability::Available);
    assert_eq!(hd.annotations.idempotent_hint, Some(true));
    assert_eq!(hd.output_schema, Some(json!({"type": "array"})));
    let h = app_hub_tool("shop", &info, Availability::Available);
    assert_eq!(h.name, "shop.orders.search");
    assert_eq!(h.tool, "orders.search");
    assert_eq!(h.activation, Activation::Foreground);
}

#[test]
fn resource_text_conversion() {
    let c = resource_contents(
        "u",
        None,
        ResourcesReadResult {
            contents: json!({"a": 1}),
            mime_type: None,
        },
    );
    assert_eq!(
        c,
        ResourceContents::text(r#"{"a":1}"#, "u").with_mime_type(DEFAULT_MIME)
    );
    let c = resource_contents(
        "u",
        Some("text/markdown"),
        ResourcesReadResult {
            contents: json!("# hi"),
            mime_type: None,
        },
    );
    assert_eq!(
        c,
        ResourceContents::text("# hi", "u").with_mime_type("text/markdown")
    );
    let rc = first_content("u", ReadResourceResult::new(vec![c]));
    assert_eq!(rc.text.as_deref(), Some("# hi"));
    assert_eq!(rc.mime_type.as_deref(), Some("text/markdown"));
}

#[test]
fn builtins_and_upstream_risk() {
    let b = builtin_hub_tools(BuiltinSet::default());
    let names: Vec<&str> = b.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "apps.list", "apps.select", "apps.overview", "apps.activate", "apps.release", "apps.calls", "apps.cancel",
            "apps.events.subscribe", "apps.events.unsubscribe", "apps.events", "apps.search", "apps.intents"
        ]
    );
    // 调用对象（P5）：apps.calls 只读，apps.cancel 非只读、幂等
    assert_eq!((b[5].risk, b[5].annotations.read_only_hint), (Risk::Read, Some(true)));
    assert_eq!((b[6].risk, b[6].annotations.read_only_hint, b[6].annotations.idempotent_hint), (Risk::Write, Some(false), Some(true)));
    // 事件（N3 + P4）：订阅 / 退订非只读、幂等；取件会移出事件，非只读、非幂等；均为任务级（可带 taskId）
    let ann = |t: &crate::types::HubTool| (t.annotations.read_only_hint, t.annotations.idempotent_hint);
    assert_eq!((ann(&b[7]), ann(&b[8]), ann(&b[9])), ((Some(false), Some(true)), (Some(false), Some(true)), (Some(false), Some(false))));
    // 检索（O1）：只读、幂等、风险 read；任务级（可带 taskId）
    assert_eq!((b[10].risk, ann(&b[10])), (Risk::Read, (Some(true), Some(true))));
    // 标准意图（N4）：只读、幂等、风险 read；任务级（可带 taskId）
    assert_eq!((b[11].risk, ann(&b[11])), (Risk::Read, (Some(true), Some(true))));
    assert_eq!(b[11].annotations.destructive_hint, Some(false));
    for name in ["apps.events.subscribe", "apps.events.unsubscribe", "apps.events", "apps.search", "apps.intents"] {
        assert!(builtin_schema(name).unwrap()["properties"].get("taskId").is_some(), "{name} 带 taskId");
    }
    // 启用对象锁时另有 apps.lock / apps.unlock（非只读、幂等，风险 write）；Hub API 形式不带 taskId
    let locks = builtin_hub_tools(BuiltinSet { locks: true, ..BuiltinSet::default() });
    let lock = locks.iter().find(|t| t.name == "apps.lock").expect("apps.lock");
    assert_eq!((lock.risk, lock.annotations.read_only_hint, lock.annotations.idempotent_hint), (Risk::Write, Some(false), Some(true)));
    assert!(lock.input_schema["properties"].get("taskId").is_none());
    assert!(locks.iter().any(|t| t.name == "apps.unlock"));
    assert_eq!(b[0].tool, "list");
    assert_eq!(b[0].risk, Risk::Read);
    // apps.activate / apps.release 改变 App 状态：非只读，风险按注解推导为 write
    assert_eq!((b[3].risk, b[3].annotations.read_only_hint, b[3].annotations.idempotent_hint), (Risk::Write, Some(false), Some(true)));
    assert!(b.iter().all(|t| t.surface.is_none() && t.page.is_none()));
    let b = builtin_hub_tools(BuiltinSet { apps_tools: true, ..BuiltinSet::default() });
    assert_eq!(b.len(), 13);
    assert_eq!(b[3].name, "apps.tools");
    // 有页面目录时另有 apps.page 与 apps.navigate
    let names: Vec<String> = builtin_hub_tools(BuiltinSet { apps_page: true, ..BuiltinSet::default() }).into_iter().map(|t| t.name).collect();
    assert!(names.contains(&"apps.page".to_owned()) && names.contains(&"apps.navigate".to_owned()));
    assert!(builtin_schema("apps.navigate").is_some(), "未列出时也可调用");
    assert!(builtin_schema("apps.select").is_some());
    // 未列出时 apps.tools 仍可调用
    assert!(builtin_schema("apps.tools").is_some());
    assert!(builtin_schema("apps.nope").is_none());
    let t = Tool::new("rm", "删除", obj(json!({"type": "object"})))
        .with_annotations(ToolAnnotations::new().destructive(true));
    assert_eq!(upstream_risk(&t), Risk::Destructive);
    let t = Tool::new("ls", "列出", obj(json!({"type": "object"})));
    assert_eq!(upstream_risk(&t), Risk::Write);
    assert_eq!(upstream_hub_tool("files", &t).name, "files.ls");
    assert_eq!(b[0].annotations.read_only_hint, Some(true));
    // 上游注解原样；没有注解时为空
    assert_eq!(upstream_hub_tool("files", &t).annotations, app_mcp_protocol::ToolAnnotations::default());
    let rm = Tool::new("rm", "删除", obj(json!({"type": "object"})))
        .with_annotations(ToolAnnotations::new().destructive(true).idempotent(true));
    let h = upstream_hub_tool("files", &rm);
    assert_eq!((h.annotations.destructive_hint, h.annotations.idempotent_hint), (Some(true), Some(true)));
    let d = upstream_tool_declaration(&rm);
    assert_eq!((d.name.as_str(), d.risk, d.output_schema), ("rm", Risk::Destructive, false));
    assert_eq!(d.annotations, Some(d.effective.clone()));
}

#[test]
fn hub_is_send_sync_and_futures_are_send() {
    fn send_sync<T: Send + Sync>() {}
    fn send<F: Send>(_: &F) {}
    send_sync::<crate::Hub>();
    #[cfg(feature = "mcp-server")]
    send_sync::<crate::McpSession>();
    send_sync::<crate::HubEvent>();
    // 仅做类型检查，不运行。
    let _check = |hub: &crate::Hub| {
        send(&hub.call_tool(CallRequest::default()));
        send(&hub.dispatch(crate::ToolFormat::Anthropic, Value::Null));
        send(&hub.read_resource(""));
    };
}

#[test]
fn values_and_errors() {
    assert_eq!(result_value(&json_result(json!({"a": 1}))), json!({"a": 1}));
    let r = CallToolResult::success(vec![ContentBlock::text("[1,2]")]);
    assert_eq!(result_value(&r), json!([1, 2]));
    let r = CallToolResult::success(vec![ContentBlock::text("hi")]);
    assert_eq!(result_value(&r), json!("hi"));
    let r = CallToolResult::success(vec![ContentBlock::text("a"), ContentBlock::text("b")]);
    assert_eq!(result_text(&r), "a\n\nb");
    let e = to_mcp_error(
        &ToolError::new(ErrorKind::AppDisconnected, "断了").with_details(json!({"x": 1})),
    );
    let back = mcp_error_to_tool(&e);
    assert_eq!(back.kind, ErrorKind::AppDisconnected);
    assert_eq!(back.details, Some(json!({"x": 1})));
    let e = McpError::internal_error("x", None);
    assert_eq!(mcp_error_to_tool(&e).kind, ErrorKind::HandlerError);
}

/// S3 回归：上游不带 `data.kind` 的 -32000…-32019 不得按本协议码反查（-32004 曾被当成 USER_REJECTED）。
#[test]
fn upstream_codes_are_not_reverse_mapped() {
    for code in [-32004, -32001, -32010, -32016] {
        let back = mcp_error_to_tool(&McpError::new(ErrorCode(code), "上游", None));
        assert_eq!(back.kind, ErrorKind::HandlerError, "{code}");
        assert_eq!(back.details, Some(json!({ "upstreamCode": code })));
    }
    let e = McpError::new(ErrorCode(-32004), "上游", Some(json!({ "details": { "x": 1 } })));
    assert_eq!(mcp_error_to_tool(&e).details, Some(json!({ "x": 1, "upstreamCode": -32004 })), "保留上游 details");
    let e = McpError::new(ErrorCode(-32004), "上游", Some(json!({ "details": "原因" })));
    assert_eq!(mcp_error_to_tool(&e).details, Some(json!({ "upstream": "原因", "upstreamCode": -32004 })));
}

#[test]
fn not_found_codes_by_context() {
    let legacy = McpError::resource_not_found("无", None);
    assert_eq!(mcp_error_to_tool(&legacy).kind, ErrorKind::ResourceNotFound);
    assert_eq!(mcp_resource_error_to_tool(&legacy).kind, ErrorKind::ResourceNotFound);
    let modern = McpError::invalid_params("无", None);
    assert_eq!(mcp_resource_error_to_tool(&modern).kind, ErrorKind::ResourceNotFound, "资源读取的 -32602");
    assert_eq!(mcp_error_to_tool(&modern).kind, ErrorKind::HandlerError, "工具调用的 -32602 不是资源不存在");
    let tagged = McpError::new(ErrorCode(-32099), "带类别", Some(json!({ "kind": ErrorKind::UserRejected })));
    assert_eq!(mcp_error_to_tool(&tagged).kind, ErrorKind::UserRejected, "data.kind 优先");
}
