//! 注册变更。

use super::support::*;

// ---------------------------------------------------------------------------
// 注册变更
// ---------------------------------------------------------------------------

#[test]
fn changes_are_coalesced_and_cancel_out() {
    let mut h = Harness::new();
    let keep = h.c.register_tool(tool("keep")).unwrap();
    let old = h.c.register_tool(tool("old")).unwrap();
    h.connect();

    let a = h.c.register_tool(tool("a")).unwrap();
    let b = h.c.register_tool(tool("b")).unwrap();
    h.c.unregister_tool(b).unwrap(); // 先加后删：抵消
    h.c.update_tool(a, ToolUpdate { description: Some("新描述".into()), ..Default::default() }).unwrap();
    h.c.unregister_tool(old).unwrap();
    // 删掉再以相同内容加回：抵消
    h.c.unregister_tool(keep).unwrap();
    h.c.register_tool(tool("keep")).unwrap();
    let r = h.c.register_resource(resource("r1")).unwrap();
    let r2 = h.c.register_resource(resource("r2")).unwrap();
    h.c.unregister_resource(r2).unwrap();

    let ev = h.drain();
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["tools/changed", "resources/changed"]);
    assert_eq!(msgs[0]["params"]["upserted"].as_array().unwrap().len(), 1);
    assert_eq!(msgs[0]["params"]["upserted"][0]["name"], "a");
    assert_eq!(msgs[0]["params"]["upserted"][0]["description"], "新描述");
    assert_eq!(msgs[0]["params"]["removed"], json!(["old"]));
    assert_eq!(msgs[1]["params"]["upserted"][0]["name"], "r1");
    assert_eq!(msgs[1]["params"]["removed"], json!([]));

    // 没有新的变更就不再发送
    assert!(h.drain().is_empty());
    h.c.unregister_resource(r).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(methods(&msgs), vec!["resources/changed"]);
    assert_eq!(msgs[0]["params"]["removed"], json!(["r1"]));

    // 先加后删（已连接状态下全部在一次 poll 之前）
    let x = h.c.register_tool(tool("x")).unwrap();
    h.c.unregister_tool(x).unwrap();
    assert!(h.drain().is_empty());
}

#[test]
fn disable_and_enable_tools() {
    let mut h = Harness::new();
    let a = h.c.register_tool(tool("a")).unwrap();
    h.connect();

    h.c.update_tool(a, ToolUpdate { enabled: Some(false), ..Default::default() }).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["method"], "tools/changed");
    assert_eq!(msgs[0]["params"], json!({"upserted": [], "removed": ["a"]}));

    // 禁用期间修改内容：Host 看不到，不发送
    h.c.update_tool(a, ToolUpdate { title: Some(Some("A".into())), ..Default::default() }).unwrap();
    assert!(h.drain().is_empty());

    h.c.update_tool(a, ToolUpdate { enabled: Some(true), ..Default::default() }).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["params"]["upserted"][0]["name"], "a");
    assert_eq!(msgs[0]["params"]["upserted"][0]["title"], "A");
    assert_eq!(msgs[0]["params"]["removed"], json!([]));

    // 禁用后立即启用：抵消
    h.c.update_tool(a, ToolUpdate { enabled: Some(false), ..Default::default() }).unwrap();
    h.c.update_tool(a, ToolUpdate { enabled: Some(true), ..Default::default() }).unwrap();
    assert!(h.drain().is_empty());

    // 注册一个禁用的工具：不发送
    let mut t = tool("b");
    t.enabled = false;
    h.c.register_tool(t).unwrap();
    assert!(h.drain().is_empty());
}

/// 第 14 项 S1 / 第 19 项 R1–R3：注解与输出 schema 随工具同步、可清除；结构化结果原样回给 Host。
#[test]
fn annotations_output_schema_and_structured_result() {
    let mut h = Harness::new();
    let mut t = tool("a");
    t.annotations = Some(ToolAnnotations { open_world_hint: Some(true), ..Default::default() });
    t.output_schema = Some(json!({"type": "object"}));
    let a = h.c.register_tool(t).unwrap();
    let ev = h.connect();
    let sync = sends(&ev).into_iter().find(|m| m["method"] == "tools/sync").unwrap();
    assert_eq!(sync["params"]["tools"][0]["annotations"], json!({"openWorldHint": true}));
    assert_eq!(sync["params"]["tools"][0]["outputSchema"], json!({"type": "object"}));
    // 清除：不再序列化
    h.c.update_tool(a, ToolUpdate { annotations: Some(None), output_schema: Some(None), ..Default::default() }).unwrap();
    let msgs = sends(&h.drain());
    let upserted = &msgs[0]["params"]["upserted"][0];
    assert!(upserted.get("annotations").is_none() && upserted.get("outputSchema").is_none(), "{upserted}");

    h.invoke(10, "c1", "a", None);
    let out = CallOutput {
        status: ResultStatus::Partial,
        summary: Some("只加入了 2 件".into()),
        annotations: Some(ContentAnnotations { priority: Some(1.0), ..Default::default() }),
        ..CallOutput::default()
    };
    h.c.complete_call("c1", Ok(out), h.now).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(
        msgs[0]["result"],
        json!({"data": null, "status": "partial", "summary": "只加入了 2 件", "annotations": {"priority": 1.0}})
    );
}

#[test]
fn registration_errors() {
    let mut h = Harness::new();
    assert_eq!(h.c.register_tool(tool("a b")), Err(CoreError::InvalidName("a b".into())));
    let mut t = tool("a");
    t.input_schema = json!({"type": "string"});
    assert_eq!(h.c.register_tool(t), Err(CoreError::InvalidSchema));
    let a = h.c.register_tool(tool("a")).unwrap();
    let mut dup = tool("a");
    dup.enabled = false;
    assert_eq!(h.c.register_tool(dup), Err(CoreError::DuplicateName("a".into())));
    h.c.register_resource(resource("a")).unwrap();
    assert_eq!(h.c.register_resource(resource("a")), Err(CoreError::DuplicateName("a".into())));
    assert_eq!(
        h.c.update_tool(a, ToolUpdate { input_schema: Some(json!([])), ..Default::default() }),
        Err(CoreError::InvalidSchema)
    );
    assert_eq!(h.c.update_tool(ToolId(999), ToolUpdate::default()), Err(CoreError::UnknownTool(ToolId(999))));
    assert_eq!(h.c.unregister_tool(ToolId(999)), Err(CoreError::UnknownTool(ToolId(999))));
    assert_eq!(h.c.unregister_resource(ResourceId(999)), Err(CoreError::UnknownResource(ResourceId(999))));
    assert_eq!(
        h.c.notify_resource_changed(ResourceId(999), 0),
        Err(CoreError::UnknownResource(ResourceId(999)))
    );
    assert_eq!(h.c.create_scope("x", Some(ScopeId(999))), Err(CoreError::UnknownScope(ScopeId(999))));
    assert_eq!(h.c.dispose_scope(ScopeId(999)), Err(CoreError::UnknownScope(ScopeId(999))));
    h.c.unregister_tool(a).unwrap();
    assert!(h.c.register_tool(tool("a")).is_ok(), "注销后名称可复用");
}

#[test]
fn scope_dispose_is_recursive() {
    let mut h = Harness::new();
    let page = h.c.create_scope("page", None).unwrap();
    let dialog = h.c.create_scope("dialog", Some(page)).unwrap();
    let other = h.c.create_scope("other", None).unwrap();
    let mut t1 = tool("page.t");
    t1.scope = Some(page);
    let mut t2 = tool("dialog.t");
    t2.scope = Some(dialog);
    let mut t3 = tool("other.t");
    t3.scope = Some(other);
    let mut r = resource("dialog.r");
    r.scope = Some(dialog);
    h.c.register_tool(t1).unwrap();
    let t2 = h.c.register_tool(t2).unwrap();
    h.c.register_tool(t3).unwrap();
    h.c.register_resource(r).unwrap();
    h.connect();
    assert_eq!(h.c.scope_name(dialog), Some("dialog"));

    h.c.dispose_scope(page).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(methods(&msgs), vec!["tools/changed", "resources/changed"]);
    assert_eq!(msgs[0]["params"]["removed"], json!(["dialog.t", "page.t"]));
    assert_eq!(msgs[1]["params"]["removed"], json!(["dialog.r"]));
    assert_eq!(h.c.scope_name(dialog), None);
    assert_eq!(h.c.dispose_scope(dialog), Err(CoreError::UnknownScope(dialog)));
    assert_eq!(h.c.unregister_tool(t2), Err(CoreError::UnknownTool(t2)));
    assert!(h.c.create_scope("x", Some(page)).is_err());
    assert!(h.c.create_scope("x", Some(other)).is_ok());
}
