//! 桥接：Host 的导航请求转给页面（spec/protocol.md 3.4）。

use super::*;

/// 调用的失败（Hub 错误或工具错误）→ (类别, 消息)。
fn failure(r: Result<app_mcp_hub::CallOutcome, app_mcp_hub::HubError>) -> (ErrorKind, String) {
    match r {
        Ok(out) => {
            let e = out.result.expect_err("调用应失败");
            (e.kind, e.message)
        }
        Err(e) => (e.kind(), e.message().to_string()),
    }
}

/// 第 4c 项：Host 的导航请求经插件转给开启导航的页面（spec/protocol.md 3.4）。页面工具的 `page` 让 Hub 记住所在页面，
/// 工具注销后再调用时 Hub 先导航、等工具重新注册再派发。
#[tokio::test(flavor = "multi_thread")]
async fn navigation_is_forwarded_to_page() {
    let fx = Fixture::new("nav", None).await;
    fx.bridge.enable_page_navigation();
    let page = Arc::new(FakePage::default());
    let checkout = |id: u64| {
        json!({ "op": "tool.register", "id": id, "name": "cart.checkout",
                "spec": { "description": "结算", "surface": "view", "page": "cart" } })
    };
    fx.op(&page, "main", "main", json!({ "op": "hello" }));
    assert_eq!(fx.op(&page, "main", "main", checkout(1)), json!({ "ok": true }));
    // 页面 SDK 的 hello 可能晚于 navigation.set 到达：目标跨 hello 保留
    assert_eq!(fx.op(&page, "main", "main", json!({ "op": "navigation.set", "enabled": true })), json!({ "ok": true }));
    fx.op(&page, "main", "main", json!({ "op": "hello" }));
    fx.op(&page, "main", "main", checkout(1));
    fx.connected("nav").await;
    eventually("Hub 看到页面工具", || tool_names(&fx.hub, "nav") == vec!["cart.checkout"]).await;
    fx.op(&page, "main", "main", json!({ "op": "tool.dispose", "id": 1 }));
    eventually("工具注销", || tool_names(&fx.hub, "nav").is_empty()).await;

    // 完成：页面重新注册工具后回复 ok，Hub 再派发调用
    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move { hub.call_tool(CallRequest::new("nav.cart.checkout", json!({}))).await });
    let nav = wait_event(&page, "navigate").await;
    assert_eq!(nav["page"], "cart");
    assert!(nav.get("params").is_none());
    fx.op(&page, "main", "main", checkout(2));
    fx.op(&page, "main", "main", json!({ "op": "navigate.result", "navId": nav["navId"], "ok": true }));
    let call = wait_event(&page, "call").await;
    assert_eq!(call["toolId"], 2);
    fx.op(&page, "main", "main", json!({ "op": "call.result", "callId": call["callId"], "ok": true, "data": { "paid": true } }));
    let out = pending.await.expect("join").expect("调用");
    assert_eq!(out.result.expect("成功")["paid"], true);

    // 拒绝：NAVIGATION_DENIED（reason app），消息来自页面
    fx.op(&page, "main", "main", json!({ "op": "tool.dispose", "id": 2 }));
    eventually("工具注销", || tool_names(&fx.hub, "nav").is_empty()).await;
    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move { hub.call_tool(CallRequest::new("nav.cart.checkout", json!({}))).await });
    let nav = wait_event(&page, "navigate").await;
    fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "navigate.result", "navId": nav["navId"], "ok": false, "kind": "NAVIGATION_DENIED", "message": "需要先登录" }),
    );
    let (kind, message) = failure(pending.await.expect("join"));
    assert_eq!(kind, ErrorKind::NavigationDenied);
    assert!(message.contains("需要先登录"), "{message}");

    // 页面卸载：进行中的导航失败；之后没有导航目标
    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move { hub.call_tool(CallRequest::new("nav.cart.checkout", json!({}))).await });
    let nav = wait_event(&page, "navigate").await;
    assert_eq!(nav["page"], "cart");
    fx.op(&page, "main", "main", json!({ "op": "reset" }));
    let (kind, message) = failure(pending.await.expect("join"));
    assert_eq!(kind, ErrorKind::NavigationFailed, "{message}");
    let (kind, message) = failure(fx.hub.call_tool(CallRequest::new("nav.cart.checkout", json!({}))).await);
    assert_eq!(kind, ErrorKind::NavigationFailed);
    assert!(message.contains("没有页面处理导航"), "{message}");
    // 迟到 / 未知的回复：忽略
    assert_eq!(fx.op(&page, "main", "main", json!({ "op": "navigate.result", "navId": 999, "ok": true })), json!({ "ok": true }));
    drop(fx.bridge);
    shutdown(fx.hub).await;
}

/// 页面回调以 USER_ACTION_REQUIRED 回复（如已发通知请用户点开）：原样带 `reason` / `uri` 到 Hub；转给页面前先把窗口带到前台。
/// 页面工具的 `backgroundTool` 随注册转到原生客户端。
#[tokio::test(flavor = "multi_thread")]
async fn navigation_user_action_and_raise() {
    let fx = Fixture::new("nav-ua", None).await;
    fx.bridge.enable_page_navigation();
    let page = Arc::new(FakePage::default());
    fx.op(&page, "main", "main", json!({ "op": "hello" }));
    let checkout = json!({ "op": "tool.register", "id": 1, "name": "cart.checkout",
        "spec": { "description": "结算", "surface": "view", "page": "cart", "backgroundTool": "cart.checkoutBg" } });
    assert_eq!(fx.op(&page, "main", "main", checkout), json!({ "ok": true }));
    assert_eq!(fx.op(&page, "main", "main", json!({ "op": "navigation.set", "enabled": true })), json!({ "ok": true }));
    fx.connected("nav-ua").await;
    eventually("Hub 看到页面工具", || tool_names(&fx.hub, "nav-ua") == vec!["cart.checkout"]).await;
    fx.op(&page, "main", "main", json!({ "op": "tool.dispose", "id": 1 }));
    eventually("工具注销", || tool_names(&fx.hub, "nav-ua").is_empty()).await;

    let hub = fx.hub.clone();
    let pending = tokio::spawn(async move { hub.call_tool(CallRequest::new("nav-ua.cart.checkout", json!({}))).await });
    let nav = wait_event(&page, "navigate").await;
    assert_eq!(page.raised.load(Ordering::SeqCst), 1);
    fx.op(
        &page,
        "main",
        "main",
        json!({ "op": "navigate.result", "navId": nav["navId"], "ok": false, "kind": "USER_ACTION_REQUIRED",
                "message": "已发通知，请点开", "details": { "reason": "foreground", "uri": "shop://cart" } }),
    );
    let err = match pending.await.expect("join") {
        Ok(out) => out.result.expect_err("调用应失败"),
        Err(e) => panic!("应为工具错误：{e:?}"),
    };
    assert_eq!(err.kind, ErrorKind::UserActionRequired, "{}", err.message);
    assert!(err.message.contains("已发通知"), "{}", err.message);
    let details = err.details.expect("带详情");
    assert_eq!((details["reason"].as_str(), details["uri"].as_str()), (Some("foreground"), Some("shop://cart")));
    drop(fx.bridge);
    shutdown(fx.hub).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_set_requires_page_navigation() {
    let fx = Fixture::new("nav-off", None).await;
    let page = Arc::new(FakePage::default());
    let reply = fx.op(&page, "main", "main", json!({ "op": "navigation.set", "enabled": true }));
    assert_eq!((reply["ok"].clone(), reply["code"].clone()), (json!(false), json!("NAVIGATION_DISABLED")));
    // 关闭总是允许
    assert_eq!(fx.op(&page, "main", "main", json!({ "op": "navigation.set", "enabled": false })), json!({ "ok": true }));
    drop(fx.bridge);
    shutdown(fx.hub).await;
}
