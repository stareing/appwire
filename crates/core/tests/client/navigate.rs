//! 导航。

use super::support::*;

// ---------------------------------------------------------------------------
// 导航（spec/protocol.md 3.4，第 4c 项）
// ---------------------------------------------------------------------------

#[test]
fn navigate_unsupported_by_default() {
    let mut h = Harness::new();
    let hello = h.open();
    assert!(hello["params"].get("capabilities").is_none(), "默认不声明导航能力");
    h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    let msgs = sends(&h.request(3, "app/navigate", json!({"page": "cart"})));
    assert_eq!(msgs[0]["error"]["code"], -31001);
    assert_eq!(msgs[0]["error"]["data"], json!({"kind": "NAVIGATION_FAILED", "reason": "unsupported"}));
}

#[test]
fn navigate_event_and_completion() {
    let mut cfg = config();
    cfg.navigation = true;
    let mut h = Harness::with(cfg);
    let hello = h.open();
    assert_eq!(hello["params"]["capabilities"], json!({"navigate": true}));
    h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));

    let ev = h.request(4, "app/navigate", json!({"page": "orders.detail", "params": {"id": "o1"}}));
    let nav = match ev.as_slice() {
        [Event::Navigate { navigate, page, params }] => {
            assert_eq!((page.as_str(), params), ("orders.detail", &json!({"id": "o1"})));
            *navigate
        }
        other => panic!("unexpected {other:?}"),
    };
    h.c.complete_navigate(nav, Ok(())).unwrap();
    assert_eq!(sends(&h.drain())[0], json!({"jsonrpc": "2.0", "id": 4, "result": {"ok": true}}));
    assert_eq!(h.c.complete_navigate(nav, Ok(())), Err(CoreError::UnknownNavigate(nav)));

    // 拒绝；无参数时 params 为 null
    let ev = h.request(5, "app/navigate", json!({"page": "cart"}));
    let Event::Navigate { navigate, params, .. } = ev[0].clone() else { panic!("{ev:?}") };
    assert_eq!(params, Value::Null);
    h.c.complete_navigate(navigate, Err(ToolError::navigation_denied("正在编辑"))).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["error"]["code"], -31002);
    assert_eq!(msgs[0]["error"]["data"], json!({"kind": "NAVIGATION_DENIED", "reason": "app"}));

    // 非法页面名 / 参数 → -32602，不产生事件
    let ev = h.request(6, "app/navigate", json!({"page": "bad page"}));
    assert!(!ev.iter().any(|e| matches!(e, Event::Navigate { .. })));
    assert_eq!(sends(&ev)[0]["error"]["code"], -32602);
    assert_eq!(sends(&h.request(7, "app/navigate", json!({})))[0]["error"]["code"], -32602);

    // 关闭后新请求按不支持回复
    h.c.set_navigation(false);
    assert_eq!(error_kind(&sends(&h.request(8, "app/navigate", json!({"page": "cart"})))[0]), "NAVIGATION_FAILED");
}

#[test]
fn navigate_dropped_on_disconnect() {
    let mut cfg = config();
    cfg.navigation = true;
    let mut h = Harness::with(cfg);
    h.connect();
    let ev = h.request(4, "app/navigate", json!({"page": "cart"}));
    let Event::Navigate { navigate, .. } = ev[0].clone() else { panic!("{ev:?}") };
    h.c.handle_disconnected(h.now);
    h.drain();
    assert_eq!(h.c.complete_navigate(navigate, Ok(())), Err(CoreError::UnknownNavigate(navigate)));
}

#[test]
fn tool_surface_and_page_sync() {
    let mut h = Harness::new();
    let mut def = tool("cart.checkout");
    def.surface = ToolSurface::View;
    def.page = Some("cart".into());
    let t = h.c.register_tool(def).unwrap();
    let ev = h.connect();
    let sync = sends(&ev).into_iter().find(|m| m["method"] == "tools/sync").unwrap();
    assert_eq!((sync["params"]["tools"][0]["surface"].as_str(), sync["params"]["tools"][0]["page"].as_str()), (Some("view"), Some("cart")));
    h.c.update_tool(t, ToolUpdate { page: Some(None), surface: Some(ToolSurface::App), ..Default::default() }).unwrap();
    let msgs = sends(&h.drain());
    let up = &msgs[0]["params"]["upserted"][0];
    assert!(up.get("surface").is_none() && up.get("page").is_none(), "{up}");
    assert_eq!(
        h.c.update_tool(t, ToolUpdate { page: Some(Some("bad page".into())), ..Default::default() }),
        Err(CoreError::InvalidName("bad page".into()))
    );
    let mut bad = tool("x");
    bad.page = Some(String::new());
    assert_eq!(h.c.register_tool(bad), Err(CoreError::InvalidName(String::new())));
}

#[test]
fn tool_background_tool_sync() {
    let mut h = Harness::new();
    let mut def = tool("cart.view_add");
    def.surface = ToolSurface::View;
    def.background_tool = Some("cart.add".into());
    let t = h.c.register_tool(def).unwrap();
    let ev = h.connect();
    let sync = sends(&ev).into_iter().find(|m| m["method"] == "tools/sync").unwrap();
    assert_eq!(sync["params"]["tools"][0]["backgroundTool"], "cart.add");
    h.c.update_tool(t, ToolUpdate { background_tool: Some(None), ..Default::default() }).unwrap();
    let msgs = sends(&h.drain());
    assert!(msgs[0]["params"]["upserted"][0].get("backgroundTool").is_none(), "清除");
    assert_eq!(
        h.c.update_tool(t, ToolUpdate { background_tool: Some(Some("bad name".into())), ..Default::default() }),
        Err(CoreError::InvalidName("bad name".into()))
    );
    let mut bad = tool("y");
    bad.background_tool = Some(String::new());
    assert_eq!(h.c.register_tool(bad), Err(CoreError::InvalidName(String::new())));
}

/// 不可见且不能自行回到前台：立即以 USER_ACTION_REQUIRED（foreground）回复，不交给导航回调。
#[test]
fn navigate_refused_fast_when_hidden() {
    let mut cfg = config();
    cfg.navigation = true;
    let mut h = Harness::with(cfg);
    h.connect();
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    h.drain();
    let ev = h.request(4, "app/navigate", json!({"page": "cart"}));
    assert!(!ev.iter().any(|e| matches!(e, Event::Navigate { .. })), "不交给回调");
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["id"], 4);
    assert_eq!(msgs[0]["error"]["code"], -32019);
    assert_eq!(msgs[0]["error"]["data"], json!({"kind": "USER_ACTION_REQUIRED", "reason": "foreground"}));
    assert!(msgs[0]["error"]["message"].as_str().unwrap().contains("cart"));
    // 冻结同样拒绝；非法页面名仍是 -32602
    h.c.set_visibility(Visibility::Frozen, false, h.now);
    h.drain();
    assert_eq!(error_kind(&sends(&h.request(5, "app/navigate", json!({"page": "cart"})))[0]), "USER_ACTION_REQUIRED");
    assert_eq!(sends(&h.request(6, "app/navigate", json!({"page": "bad page"})))[0]["error"]["code"], -32602);

    // 能自行回到前台 / App 自行处理：交给回调，回调可以用 USER_ACTION_REQUIRED（带 uri）完成
    h.c.set_navigate_in_background(true);
    let ev = h.request(7, "app/navigate", json!({"page": "cart"}));
    let Event::Navigate { navigate, .. } = ev[0].clone() else { panic!("{ev:?}") };
    let e = ToolError::user_action_required("点通知继续", Some("foreground"), Some("shop://cart"));
    h.c.complete_navigate(navigate, Err(e)).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(
        msgs[0]["error"]["data"],
        json!({"kind": "USER_ACTION_REQUIRED", "reason": "foreground", "uri": "shop://cart"})
    );

    // 回到可见：照常交给回调
    h.c.set_navigate_in_background(false);
    h.c.set_visibility(Visibility::Visible, true, h.now);
    h.drain();
    let ev = h.request(8, "app/navigate", json!({"page": "cart"}));
    assert!(matches!(ev.as_slice(), [Event::Navigate { .. }]), "{ev:?}");
}
