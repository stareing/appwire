//! 桥接：连接 ID、scope 与资源、按 WebView 的会话、取消转发、非法 op 与过滤。

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn connection_id_reaches_page_and_bridge_drop_stops_client() {
    let fx = Fixture::new("cid", None).await;
    let page = Arc::new(FakePage::default());

    let hello = fx.op(&page, "main", "main", json!({ "op": "hello" }));
    assert!(hello["value"].get("connectionId").is_none(), "未连接时不带连接 ID");
    fx.op(&page, "main", "main", register(1, "page.add"));
    fx.connected("cid").await;

    let hub_cid = fx
        .hub
        .apps()
        .into_iter()
        .find(|a| a.app_id == "cid")
        .and_then(|a| a.instances.into_iter().next())
        .and_then(|i| i.connection_id)
        .expect("Hub 分配了连接 ID");
    assert_eq!(fx.bridge.client().connection_id().as_deref(), Some(hub_cid.as_str()));
    let mut connected = None;
    eventually("页面收到带连接 ID 的 connected", || {
        connected = page
            .all()
            .into_iter()
            .find(|e| e["type"] == "state" && e["state"]["status"] == "connected");
        connected.is_some()
    })
    .await;
    assert_eq!(connected.unwrap_or(Value::Null)["connectionId"], hub_cid.as_str());
    let hello = fx.op(&page, "main", "main", json!({ "op": "hello" }));
    assert_eq!(hello["value"]["connectionId"], hub_cid.as_str());

    // Sessions 与客户端之间的引用环由 Bridge 的 Drop 断开：丢弃 Bridge 后客户端停止、App 从 Hub 消失。
    let Fixture { hub, bridge, sessions } = fx;
    drop(bridge);
    eventually("丢弃 Bridge 后 App 断开", || {
        !hub.apps().iter().any(|a| a.app_id == "cid" && a.connected)
    })
    .await;
    drop(sessions);
    shutdown(hub).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn scopes_resources_and_updates() {
    let fx = Fixture::new("scopes", None).await;
    let page = Arc::new(FakePage::default());
    fx.connected("scopes").await;

    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "scope.create", "id": 10, "name": "cart" })
        )["ok"],
        true
    );
    let mut in_scope = register(11, "cart.clear");
    in_scope["scopeId"] = json!(10);
    assert_eq!(fx.op(&page, "main", "main", in_scope)["ok"], true);
    assert_eq!(
        fx.op(&page, "main", "main", register(12, "todo.add"))["ok"],
        true
    );
    eventually("两个页面工具", || {
        tool_names(&fx.hub, "scopes").len() == 2
    })
    .await;

    // 更新：整体替换定义。
    let update = json!({ "op": "tool.update", "id": 12, "spec": { "description": "新的说明", "title": "添加待办" } });
    assert_eq!(fx.op(&page, "main", "main", update)["ok"], true);
    eventually("更新生效", || {
        fx.hub.tools(&ToolFilter::default()).iter().any(|t| {
            t.tool == "todo.add"
                && t.description == "新的说明"
                && t.title.as_deref() == Some("添加待办")
        })
    })
    .await;

    // 注销 scope：其下工具一并注销。
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "scope.dispose", "id": 10 })
        )["ok"],
        true
    );
    eventually("scope 注销", || {
        tool_names(&fx.hub, "scopes") == vec!["todo.add"]
    })
    .await;
    let unknown = fx.op(&page, "main", "main", {
        let mut op = register(13, "x.y");
        op["scopeId"] = json!(10);
        op
    });
    assert_eq!(unknown["code"], "UNKNOWN_SCOPE");

    // 资源：读取转给页面。
    let res = json!({ "op": "resource.register", "id": 20, "name": "cart", "description": "购物车", "mimeType": "application/json" });
    assert_eq!(fx.op(&page, "main", "main", res)["ok"], true);
    let mut uri = String::new();
    eventually("资源出现", || {
        if let Some(r) = fx
            .hub
            .resources()
            .into_iter()
            .find(|r| r.app_id == "scopes" && r.name == "scopes.cart")
        {
            uri = r.uri;
            true
        } else {
            false
        }
    })
    .await;
    let hub = fx.hub.clone();
    let uri1 = uri.clone();
    let reading = tokio::spawn(async move { hub.read_resource(&uri1).await });
    let read = wait_event(&page, "read").await;
    assert_eq!(read["resourceId"], 20);
    let reply = fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "read.result", "readId": read["readId"], "ok": true, "data": { "items": 3 } }),
    );
    assert_eq!(reply["ok"], true);
    let content = reading.await.expect("join").expect("读取");
    let text = content.text.unwrap_or_default();
    assert!(text.contains("\"items\":3"), "{text}");
    // 读取失败：类别与详情（如 USER_ACTION_REQUIRED 的 reason / uri）原样传给 Host（app_mcp.h v12 / ReadHandle::fail_with_details）。
    let hub = fx.hub.clone();
    let reading = tokio::spawn(async move { hub.read_resource(&uri).await });
    let read = wait_event(&page, "read").await;
    fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "read.result", "readId": read["readId"], "ok": false, "kind": "USER_ACTION_REQUIRED",
                "message": "登录已过期", "details": { "reason": "login", "uri": "shop://login" } }),
    );
    let err = reading.await.expect("join").expect_err("读取失败").0;
    assert_eq!((err.kind, err.message.as_str()), (ErrorKind::UserActionRequired, "登录已过期"));
    assert_eq!(err.details, Some(json!({ "reason": "login", "uri": "shop://login" })));
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "resource.notify", "id": 20 })
        )["ok"],
        true
    );

    // 工具 dispose。
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "tool.dispose", "id": 12 })
        )["ok"],
        true
    );
    eventually("工具注销", || tool_names(&fx.hub, "scopes").is_empty()).await;
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

/// 页面资源的 `realtime` 传到 Hub：只有 realtime 资源的订阅算未休眠原因（spec/lifecycle.md 第 13 节 B3）。
#[tokio::test(flavor = "multi_thread")]
async fn page_resource_realtime_reaches_hub() {
    let fx = Fixture::new("realtime", None).await;
    let page = Arc::new(FakePage::default());
    fx.connected("realtime").await;

    let plain = json!({ "op": "resource.register", "id": 1, "name": "cart", "description": "购物车",
                        "annotations": { "audience": ["user"], "priority": 0.5 } });
    assert_eq!(fx.op(&page, "main", "main", plain)["ok"], true);
    let scope = json!({ "op": "scope.create", "id": 2, "name": "orders" });
    assert_eq!(fx.op(&page, "main", "main", scope)["ok"], true);
    let realtime = json!({ "op": "resource.register", "id": 3, "scopeId": 2, "name": "order",
                           "description": "订单状态", "realtime": true });
    assert_eq!(fx.op(&page, "main", "main", realtime)["ok"], true);

    let uri = |name: &str| -> Option<String> {
        fx.hub
            .resources()
            .into_iter()
            .find(|r| r.app_id == "realtime" && r.name == format!("realtime.{name}"))
            .map(|r| r.uri)
    };
    eventually("资源出现", || {
        uri("cart").is_some() && uri("order").is_some()
    })
    .await;
    // 资源的内容标注随登记到达 Hub；未声明的不带。
    let annotations = |name: &str| {
        fx.hub
            .resources()
            .into_iter()
            .find(|r| r.name == format!("realtime.{name}"))
            .and_then(|r| r.annotations)
    };
    assert_eq!(
        annotations("cart").map(|a| serde_json::to_value(a).unwrap_or_default()),
        Some(json!({ "audience": ["user"], "priority": 0.5 }))
    );
    assert_eq!(annotations("order"), None);
    let bad = json!({ "op": "resource.register", "id": 9, "name": "bad", "description": "x",
                      "annotations": { "audience": ["bot"] } });
    assert_ne!(fx.op(&page, "main", "main", bad)["ok"], true, "非法内容标注被拒绝");
    let subscribed_realtime = || {
        fx.hub
            .status()
            .apps
            .into_iter()
            .filter(|a| a.app_id == "realtime")
            .flat_map(|a| a.instances)
            .filter_map(|i| i.power)
            .any(|p| {
                p.awake_reasons
                    .contains(&app_mcp_hub::AwakeReason::Subscription)
            })
    };

    let cart = uri("cart").unwrap_or_default();
    let order = uri("order").unwrap_or_default();
    fx.hub.subscribe(&cart).expect("订阅 cart");
    fx.hub.subscribe(&order).expect("订阅 order");
    eventually("realtime 订阅算未休眠原因", subscribed_realtime).await;
    // 只剩普通资源的订阅：不算。
    fx.hub.unsubscribe(&order);
    eventually("普通订阅不算未休眠原因", || {
        !subscribed_realtime()
    })
    .await;

    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_are_per_webview_and_end_with_window() {
    let fx = Fixture::new("windows", None).await;
    let main = Arc::new(FakePage::default());
    let other = Arc::new(FakePage::default());
    fx.connected("windows").await;

    fx.op(&main, "main", "main", json!({ "op": "hello" }));
    fx.op(&main, "main", "main", register(1, "main.tool"));
    fx.op(
        &other,
        "settings",
        "settings-window",
        json!({ "op": "hello" }),
    );
    fx.op(
        &other,
        "settings",
        "settings-window",
        register(1, "settings.tool"),
    );
    eventually("两个窗口的工具", || {
        tool_names(&fx.hub, "windows").len() == 2
    })
    .await;
    assert_eq!(fx.sessions.count(), 2);

    // 进行中的调用：窗口销毁时以 APP_DISCONNECTED 失败。
    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move {
        hub.call_tool(CallRequest::new("windows.settings.tool", json!({})))
            .await
    });
    wait_event(&other, "call").await;
    fx.sessions.end_window("settings-window");
    let out = pending.await.expect("join").expect("调用");
    assert_eq!(
        out.result.expect_err("失败").kind,
        ErrorKind::AppDisconnected
    );
    eventually("只剩主窗口工具", || {
        tool_names(&fx.hub, "windows") == vec!["main.tool"]
    })
    .await;
    assert_eq!(fx.sessions.count(), 1);

    // 页面刷新（hello）：旧登记作废，新页面可以用同一个 id / 名称重新登记。
    fx.op(&main, "main", "main", json!({ "op": "hello" }));
    eventually("刷新后注销", || {
        tool_names(&fx.hub, "windows").is_empty()
    })
    .await;
    assert_eq!(
        fx.op(&main, "main", "main", register(1, "main.tool"))["ok"],
        true
    );
    eventually("重新登记", || {
        tool_names(&fx.hub, "windows") == vec!["main.tool"]
    })
    .await;

    // reset（页面卸载）同样注销。
    assert_eq!(
        fx.op(&main, "main", "main", json!({ "op": "reset" })),
        json!({ "ok": true })
    );
    eventually("reset 后注销", || {
        tool_names(&fx.hub, "windows").is_empty()
    })
    .await;
    assert_eq!(fx.sessions.count(), 0);

    // 事件无法送达（WebView 已不存在）：会话随之注销。
    fx.op(&main, "main", "main", register(2, "gone.tool"));
    eventually("登记", || {
        tool_names(&fx.hub, "windows") == vec!["gone.tool"]
    })
    .await;
    main.gone.store(true, Ordering::SeqCst);
    let out = fx
        .hub
        .call_tool(CallRequest::new("windows.gone.tool", json!({})))
        .await
        .expect("调用");
    assert_eq!(
        out.result.expect_err("失败").kind,
        ErrorKind::AppDisconnected
    );
    eventually("送达失败后注销", || {
        tool_names(&fx.hub, "windows").is_empty()
    })
    .await;
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_is_forwarded_to_page() {
    let fx = Fixture::new("cancel", None).await;
    let page = Arc::new(FakePage::default());
    fx.connected("cancel").await;
    fx.op(&page, "main", "main", register(1, "slow.op"));
    eventually("登记", || tool_names(&fx.hub, "cancel").len() == 1).await;

    let hub = fx.hub.clone();
    let mut req = CallRequest::new("cancel.slow.op", json!({}));
    req.call_id = Some("call-to-cancel".into());
    let pending = tokio::spawn(async move { hub.call_tool(req).await });
    let call = wait_event(&page, "call").await;
    fx.hub.cancel_call("call-to-cancel");
    let cancel = wait_event(&page, "cancel").await;
    assert_eq!(cancel["callId"], call["callId"]);
    assert_eq!(cancel["kind"], "CANCELLED");
    let _ = pending.await;
    // 取消后页面的结果被忽略。
    let late = json!({ "op": "call.result", "callId": call["callId"], "ok": true, "data": null });
    assert_eq!(fx.op(&page, "main", "main", late), json!({ "ok": true }));
    fx.bridge.client().stop();
    shutdown(fx.hub).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_bad_ops_and_filtered_webviews() {
    let accept: Arc<bridge::AcceptFn> = Arc::new(|label: &str| label != "untrusted");
    let fx = Fixture::new("errors", Some(accept)).await;
    let page = Arc::new(FakePage::default());

    assert_eq!(
        fx.op(&page, "untrusted", "w", json!({ "op": "hello" }))["code"],
        "FORBIDDEN"
    );
    assert_eq!(
        fx.op(&page, "main", "main", json!({ "op": "nope" }))["code"],
        "INVALID_OP"
    );
    assert_eq!(
        fx.op(&page, "main", "main", json!("not an object"))["code"],
        "INVALID_OP"
    );
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "tool.register", "id": 1.5, "name": "a", "spec": { "description": "x" } })
        )["code"],
        "INVALID_OP"
    );
    assert_eq!(
        fx.op(&page, "main", "main", register(1, "bad name!"))["code"],
        "INVALID_NAME"
    );
    assert_eq!(
        fx.op(&page, "main", "main", register(2, "dup.tool"))["ok"],
        true
    );
    assert_eq!(
        fx.op(&page, "other", "other", register(1, "dup.tool"))["code"],
        "DUPLICATE_NAME"
    );
    assert_eq!(
        fx.op(&page, "main", "main", register(2, "another.tool"))["code"],
        "DUPLICATE_ID"
    );
    let bad_schema = json!({ "op": "tool.register", "id": 3, "name": "s.t", "spec": { "description": "x", "inputSchema": { "type": "string" } } });
    assert_eq!(
        fx.op(&page, "main", "main", bad_schema)["code"],
        "INVALID_SCHEMA"
    );

    // 生命周期操作转给原生客户端。
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "lifecycle.hold", "holdId": 7 })
        ),
        json!({ "ok": true })
    );
    assert_eq!(
        fx.op(
            &page,
            "main",
            "main",
            json!({ "op": "lifecycle.release", "holdId": 7 })
        ),
        json!({ "ok": true })
    );
    assert_eq!(
        fx.op(&page, "main", "main", json!({ "op": "lifecycle.wake" }))["ok"],
        true
    );
    assert!(fx.op(&page, "main", "main", json!({ "op": "lifecycle.sleep" }))["value"].is_boolean());

    // 停止后登记失败。
    fx.bridge.client().stop();
    assert_eq!(
        fx.op(&page, "main", "main", register(9, "late.tool"))["code"],
        "STOPPED"
    );
    shutdown(fx.hub).await;
}
