//! 桥接：页面工具与 Rust 工具的往返、注解、输出 schema 与结构化结果。

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn page_tool_roundtrip_with_rust_tool() {
    let fx = Fixture::new("roundtrip", None).await;
    let page = Arc::new(FakePage::default());

    let hello = fx.op(&page, "main", "main", json!({ "op": "hello" }));
    assert_eq!(hello["ok"], true);
    assert_eq!(
        hello["value"]["instanceId"],
        fx.bridge.client().instance_id()
    );
    assert_eq!(hello["value"]["state"]["status"], "idle");
    assert_eq!(
        fx.op(&page, "main", "main", register(1, "page.add")),
        json!({ "ok": true })
    );

    struct Native;
    impl ToolHandler for Native {
        fn invoke(&self, call: CallHandle) {
            let _ = call.complete(Some(r#"{"from":"rust"}"#), vec![]);
        }
    }
    fx.bridge
        .client()
        .register_tool(ToolSpec::new("app.native", "Rust 侧工具"), Arc::new(Native))
        .expect("注册 Rust 工具");
    fx.connected("roundtrip").await;
    eventually("Hub 看到 Rust 与页面工具", || {
        tool_names(&fx.hub, "roundtrip") == vec!["app.native", "page.add"]
    })
    .await;
    // 页面收到连接状态。
    eventually("页面收到 connected", || {
        page.all()
            .iter()
            .any(|e| e["type"] == "state" && e["state"]["status"] == "connected")
    })
    .await;

    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move {
        hub.call_tool(CallRequest::new("roundtrip.page.add", json!({ "a": 41 })))
            .await
    });
    let call = wait_event(&page, "call").await;
    assert_eq!(call["toolId"], 1);
    assert_eq!(call["input"], json!({ "a": 41 }));
    assert!(call.get("idempotencyKey").is_none(), "没有幂等键时不带该字段");
    // 页面在 Hub API 不接收进度时报告进度：无副作用
    let progress = fx.op(&page, "main", "main", json!({ "op": "call.progress", "callId": call["callId"], "progress": 1 }));
    assert_eq!(progress, json!({ "ok": true }));
    let reply = fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "call.result", "callId": call["callId"], "ok": true, "data": { "sum": 42 }, "stateHints": ["cart"] }),
    );
    assert_eq!(reply["ok"], true);
    let out = pending.await.expect("join").expect("调用");
    assert_eq!(out.result.expect("成功")["sum"], 42);
    assert_eq!(out.state_hints, vec!["cart".to_owned()]);

    // Agent 的幂等键原样转给页面（spec/protocol.md 3.3）
    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move {
        let mut req = CallRequest::new("roundtrip.page.add", json!({ "a": 1 }));
        req.idempotency_key = Some("order-7".to_owned());
        hub.call_tool(req).await
    });
    let call = wait_event(&page, "call").await;
    assert_eq!(call["idempotencyKey"], "order-7");
    fx.op(&page, "main", "main", json!({ "op": "call.result", "callId": call["callId"], "ok": true }));
    pending.await.expect("join").expect("带幂等键的调用");

    // 页面 handler 的进度（call.progress）经原生客户端到达 Hub（spec/protocol.md 3.3）
    let hub = fx.hub.clone();
    let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel();
    let pending = tokio::spawn(async move {
        hub.call_tool_with_progress(CallRequest::new("roundtrip.page.add", json!({})), ptx).await
    });
    let call = wait_event(&page, "call").await;
    fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "call.progress", "callId": call["callId"], "progress": 1, "total": 2, "message": "半" }),
    );
    let p = tokio::time::timeout(T, prx.recv()).await.expect("进度").expect("进度");
    assert_eq!((p.progress, p.total, p.message.as_deref()), (1.0, Some(2.0), Some("半")));
    fx.op(&page, "main", "main", json!({ "op": "call.result", "callId": call["callId"], "ok": true, "data": 1 }));
    pending.await.expect("join").expect("调用");
    // 调用结束后的进度：无接收方，忽略
    let late = fx.op(&page, "main", "main", json!({ "op": "call.progress", "callId": call["callId"], "progress": 2 }));
    assert_eq!(late, json!({ "ok": true }));

    let native = fx
        .hub
        .call_tool(CallRequest::new("roundtrip.app.native", json!({})))
        .await
        .expect("调用 Rust 工具");
    assert_eq!(native.result.expect("成功")["from"], "rust");

    // 页面报告的错误类别与详情原样传给 Host；无法识别的类别按 HANDLER_ERROR；详情不是对象时忽略。
    let cases = [
        ("USER_REJECTED", Value::Null, ErrorKind::UserRejected, None),
        ("WHATEVER", Value::Null, ErrorKind::HandlerError, None),
        ("POLICY_DENIED", Value::Null, ErrorKind::PolicyDenied, None),
        (
            "USER_ACTION_REQUIRED",
            json!({ "reason": "login", "uri": "shop://login" }),
            ErrorKind::UserActionRequired,
            Some(json!({ "reason": "login", "uri": "shop://login" })),
        ),
        (
            "USER_ACTION_REQUIRED",
            json!({ "reason": "permission" }),
            ErrorKind::UserActionRequired,
            Some(json!({ "reason": "permission" })),
        ),
        ("USER_ACTION_REQUIRED", json!({}), ErrorKind::UserActionRequired, None),
        ("USER_ACTION_REQUIRED", json!("x"), ErrorKind::UserActionRequired, None),
    ];
    for (kind, details, expected, expected_details) in cases {
        let hub = fx.hub.clone();
        let pending = tokio::spawn(async move {
            hub.call_tool(CallRequest::new("roundtrip.page.add", json!({})))
                .await
        });
        let call = wait_event(&page, "call").await;
        let mut op = json!({ "op": "call.result", "callId": call["callId"], "ok": false, "kind": kind, "message": "不行" });
        if !details.is_null() {
            op["details"] = details;
        }
        fx.op(&page, "main", "main", op);
        let out = pending.await.expect("join").expect("调用");
        let err = out.result.expect_err("失败");
        assert_eq!(err.kind, expected);
        assert_eq!(err.message, "不行");
        assert_eq!(err.details, expected_details, "{kind}");
    }
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

fn hub_tool(hub: &Hub, name: &str) -> Option<app_mcp_hub::HubTool> {
    hub.tools(&ToolFilter::default())
        .into_iter()
        .find(|t| t.name == name)
}

/// 第 14 项 S1 / 第 19 项 R1–R3：页面与 Rust 工具的注解、输出 schema 与结构化结果经桥接到达 Hub。
#[tokio::test(flavor = "multi_thread")]
async fn annotations_output_schema_and_structured_results() {
    let fx = Fixture::new("annot", None).await;
    let page = Arc::new(FakePage::default());
    fx.op(&page, "main", "main", json!({ "op": "hello" }));
    let spec = json!({
        "description": "下单", "risk": "write",
        "annotations": { "title": "下单", "idempotentHint": true, "openWorldHint": true },
        "outputSchema": { "type": "object", "properties": { "orderId": { "type": "string" } } }
    });
    let reply = fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "tool.register", "id": 1, "name": "page.order", "spec": spec }),
    );
    assert_eq!(reply, json!({ "ok": true }));

    struct Native;
    impl ToolHandler for Native {
        fn invoke(&self, call: CallHandle) {
            let _ = call.complete_with(CallResult {
                summary: Some("没有需要清理的项".to_owned()),
                status: ResultStatus::Noop,
                ..CallResult::default()
            });
        }
    }
    fx.bridge
        .client()
        .register_tool_with(
            ToolSpec::new("app.clean", "清理"),
            ToolOptions {
                annotations: Some(ToolAnnotations {
                    destructive_hint: Some(true),
                    ..ToolAnnotations::default()
                }),
                output_schema_json: Some(r#"{"type":"array"}"#.to_owned()),
                ..ToolOptions::default()
            },
            Arc::new(Native),
        )
        .expect("注册 Rust 工具");
    fx.connected("annot").await;
    eventually("Hub 看到两个工具", || tool_names(&fx.hub, "annot").len() == 2).await;

    let page_tool = hub_tool(&fx.hub, "annot.page.order").expect("页面工具");
    assert_eq!(page_tool.annotations.idempotent_hint, Some(true));
    assert_eq!(page_tool.annotations.open_world_hint, Some(true));
    assert_eq!(page_tool.annotations.title.as_deref(), Some("下单"));
    assert_eq!(page_tool.annotations.read_only_hint, Some(false), "缺少的字段按 risk 推导");
    assert_eq!(
        page_tool.output_schema,
        Some(json!({ "type": "object", "properties": { "orderId": { "type": "string" } } }))
    );
    let rust_tool = hub_tool(&fx.hub, "annot.app.clean").expect("Rust 工具");
    assert_eq!(rust_tool.annotations.destructive_hint, Some(true));
    assert_eq!(rust_tool.output_schema, Some(json!({ "type": "array" })));

    // 结构化结果：pending + stateResource + summary + 内容注解。
    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move {
        hub.call_tool(CallRequest::new("annot.page.order", json!({})))
            .await
    });
    let call = wait_event(&page, "call").await;
    let reply = fx.op(
        &page,
        "main",
        "main",
        json!({
            "op": "call.result", "callId": call["callId"], "ok": true, "data": { "orderId": "o1" },
            "status": "pending", "stateResource": "order.state", "summary": "已提交，等待用户确认",
            "annotations": { "audience": ["user"], "priority": 0.8 }
        }),
    );
    assert_eq!(reply["ok"], true);
    let out = pending.await.expect("join").expect("调用");
    assert_eq!(out.result.expect("成功")["orderId"], "o1");
    assert_eq!(out.status, ResultStatus::Pending);
    assert_eq!(out.summary.as_deref(), Some("已提交，等待用户确认"));
    assert!(
        out.state_resource.as_deref().is_some_and(|r| r.ends_with("order.state")),
        "{:?}",
        out.state_resource
    );
    let ann = out.annotations.expect("内容注解");
    assert_eq!(ann.audience, Some(vec![Audience::User]));
    assert_eq!(ann.priority, Some(0.8));

    let native = fx
        .hub
        .call_tool(CallRequest::new("annot.app.clean", json!({})))
        .await
        .expect("调用 Rust 工具");
    assert_eq!(native.status, ResultStatus::Noop);
    assert_eq!(native.summary.as_deref(), Some("没有需要清理的项"));

    // 信封字段取值不合法（@app-mcp/web 的 isToolResultEnvelope 规则）：与 web / node 一致，整个结果作为 data、状态 done。
    let invalid = [
        json!({ "data": 1, "status": "bogus" }),
        json!({ "data": 1, "summary": null }),
        json!({ "data": 1, "stateResource": 2 }),
        json!({ "data": 1, "stateHints": "x" }),
        json!({ "data": 1, "annotations": [] }),
        json!({ "data": 1, "status": "pending", "summary": 5 }),
    ];
    for envelope in invalid {
        let hub = fx.hub.clone();
        let pending = tokio::spawn(async move {
            hub.call_tool(CallRequest::new("annot.page.order", json!({})))
                .await
        });
        let call = wait_event(&page, "call").await;
        let mut op = json!({ "op": "call.result", "callId": call["callId"], "ok": true });
        for (k, v) in envelope.as_object().into_iter().flatten() {
            op[k] = v.clone();
        }
        assert_eq!(fx.op(&page, "main", "main", op), json!({ "ok": true }), "{envelope}");
        let out = pending.await.expect("join").expect("调用");
        assert_eq!(out.status, ResultStatus::Done, "{envelope}");
        assert_eq!(out.summary, None, "{envelope}");
        assert_eq!(out.result.expect("成功"), envelope);
    }

    // annotations 是对象但字段不合法：调用以 HANDLER_ERROR 结束，页面收到 INVALID_RESULT。
    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move {
        hub.call_tool(CallRequest::new("annot.page.order", json!({})))
            .await
    });
    let call = wait_event(&page, "call").await;
    let reply = fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "call.result", "callId": call["callId"], "ok": true, "data": 1, "annotations": { "priority": "x" } }),
    );
    assert_eq!(reply["code"], "INVALID_RESULT");
    let err = pending.await.expect("join").expect("调用").result.expect_err("失败");
    assert_eq!(err.kind, ErrorKind::HandlerError);

    // 整体更新（页面发送完整定义）：缺少 annotations / outputSchema 即清除声明。
    fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "tool.update", "id": 1, "spec": { "description": "下单", "risk": "read" } }),
    );
    eventually("Hub 看到注解被清除", || {
        hub_tool(&fx.hub, "annot.page.order")
            .is_some_and(|t| t.output_schema.is_none() && t.annotations.idempotent_hint.is_none())
    })
    .await;
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

/// 第 16 项 N4：页面工具的 `implements` 经桥接到达 Hub；格式不合法时登记被拒绝；整体更新缺省即清除。
#[tokio::test(flavor = "multi_thread")]
async fn page_tool_implements() {
    let fx = Fixture::new("intent", None).await;
    let page = Arc::new(FakePage::default());
    fx.op(&page, "main", "main", json!({ "op": "hello" }));
    let spec = json!({ "description": "打开链接", "implements": ["link.open@1"],
        "inputSchema": { "type": "object", "properties": { "url": { "type": "string" } } } });
    let reply = fx.op(&page, "main", "main", json!({ "op": "tool.register", "id": 1, "name": "web.open", "spec": spec }));
    assert_eq!(reply, json!({ "ok": true }));
    let bad = json!({ "op": "tool.register", "id": 2, "name": "web.bad",
        "spec": { "description": "x", "implements": ["no-version"] } });
    assert_eq!(fx.op(&page, "main", "main", bad)["ok"], json!(false));
    fx.connected("intent").await;
    eventually("Hub 看到页面工具", || tool_names(&fx.hub, "intent") == vec!["web.open"]).await;
    let tool = hub_tool(&fx.hub, "intent.web.open").expect("页面工具");
    assert_eq!(tool.implements, vec!["link.open@1".to_owned()]);

    fx.op(&page, "main", "main", json!({ "op": "tool.update", "id": 1, "spec": { "description": "打开链接" } }));
    eventually("Hub 看到 implements 被清除", || {
        hub_tool(&fx.hub, "intent.web.open").is_some_and(|t| t.implements.is_empty())
    })
    .await;
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

/// 第 16 项 O3：页面工具与资源的 `cache` 经桥接到达 Hub：第二次调用命中缓存（页面只收到一次调用）；越界的声明登记被拒绝；
/// 整体更新缺省即清除，之后调用照常转给页面。
#[tokio::test(flavor = "multi_thread")]
async fn page_tool_and_resource_cache() {
    let fx = Fixture::new("cached", None).await;
    let page = Arc::new(FakePage::default());
    fx.op(&page, "main", "main", json!({ "op": "hello" }));
    let spec = json!({ "description": "列表", "risk": "read", "cache": { "ttlMs": 60000 } });
    let reply = fx.op(&page, "main", "main", json!({ "op": "tool.register", "id": 1, "name": "feed.list", "spec": spec }));
    assert_eq!(reply, json!({ "ok": true }));
    let bad = json!({ "op": "tool.register", "id": 2, "name": "feed.bad",
        "spec": { "description": "x", "risk": "read", "cache": { "ttlMs": 0 } } });
    assert_eq!(fx.op(&page, "main", "main", bad)["ok"], json!(false));
    let res = json!({ "op": "resource.register", "id": 3, "name": "feed", "description": "订阅",
        "cache": { "ttlMs": 60000, "scope": "shared" } });
    assert_eq!(fx.op(&page, "main", "main", res), json!({ "ok": true }));
    let bad_res = json!({ "op": "resource.register", "id": 4, "name": "feed2", "description": "订阅",
        "cache": { "ttlMs": 86400001 } });
    assert_eq!(fx.op(&page, "main", "main", bad_res)["ok"], json!(false));
    fx.connected("cached").await;
    eventually("Hub 看到页面工具", || tool_names(&fx.hub, "cached") == vec!["feed.list"]).await;

    let calls = |page: &FakePage| page.all().iter().filter(|e| e["type"] == "call").count();
    let call_once = |hub: Arc<Hub>| tokio::spawn(async move { hub.call_tool(CallRequest::new("cached.feed.list", json!({}))).await });
    let pending = call_once(fx.hub.clone());
    let call = wait_event(&page, "call").await;
    fx.op(&page, "main", "main", json!({ "op": "call.result", "callId": call["callId"], "ok": true, "data": { "n": 1 } }));
    let first = pending.await.expect("join").expect("调用");
    assert_eq!(first.cached_age_ms, None);
    let second = fx.hub.call_tool(CallRequest::new("cached.feed.list", json!({}))).await.expect("命中");
    assert!(second.cached_age_ms.is_some(), "第二次命中缓存");
    assert_eq!(second.result.as_ref().ok(), first.result.as_ref().ok());
    assert_eq!(calls(&page), 0, "页面只收到一次调用（第一次已由 wait_event 取走）");

    fx.op(&page, "main", "main", json!({ "op": "tool.update", "id": 1, "spec": { "description": "列表", "risk": "read" } }));
    eventually("更新后不再命中", || fx.hub.status().cache.is_some_and(|c| c.entries == 0)).await;
    let pending = call_once(fx.hub.clone());
    let call = wait_event(&page, "call").await;
    fx.op(&page, "main", "main", json!({ "op": "call.result", "callId": call["callId"], "ok": true, "data": { "n": 2 } }));
    assert_eq!(pending.await.expect("join").expect("调用").cached_age_ms, None);
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}
