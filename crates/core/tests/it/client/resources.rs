//! 资源。

use super::support::*;

// ---------------------------------------------------------------------------
// 资源
// ---------------------------------------------------------------------------

/// 第 14 项：资源的内容标注随 `resources/sync` / `resources/changed` 同步；未声明时不序列化。
#[test]
fn resource_annotations_are_synced() {
    let mut h = Harness::new();
    let annotated = ResourceDef {
        annotations: Some(ContentAnnotations { priority: Some(0.5), ..Default::default() }),
        ..resource("a")
    };
    h.c.register_resource(annotated).unwrap();
    h.c.register_resource(resource("b")).unwrap();
    let ev = h.connect();
    let sync = sends(&ev).into_iter().find(|m| m["method"] == "resources/sync").unwrap();
    assert_eq!(sync["params"]["resources"][0]["annotations"], json!({"priority": 0.5}));
    assert!(sync["params"]["resources"][1].get("annotations").is_none());
    let c = ResourceDef {
        annotations: Some(ContentAnnotations { audience: Some(vec![Audience::User]), ..Default::default() }),
        ..resource("c")
    };
    h.c.register_resource(c).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["params"]["upserted"][0]["annotations"], json!({"audience": ["user"]}));
}

#[test]
fn resource_read() {
    let mut h = Harness::new();
    let mut def = resource("cart.state");
    def.mime_type = Some("application/json".into());
    let r = h.c.register_resource(def).unwrap();
    h.connect();

    let ev = h.request(5, "resources/read", json!({"name": "cart.state"}));
    let read = match ev.as_slice() {
        [Event::ReadResource { read, resource, name }] => {
            assert_eq!(*resource, r);
            assert_eq!(name, "cart.state");
            *read
        }
        other => panic!("unexpected {other:?}"),
    };
    h.c.complete_read(read, Ok(json!({"items": []}))).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(
        msgs[0],
        json!({"jsonrpc": "2.0", "id": 5, "result": {"contents": {"items": []}, "mimeType": "application/json"}})
    );
    assert_eq!(h.c.complete_read(read, Ok(json!(1))), Err(CoreError::UnknownRead(read)));

    // 读取失败
    let ev = h.request(6, "resources/read", json!({"name": "cart.state"}));
    let Event::ReadResource { read, .. } = ev[0].clone() else { panic!() };
    h.c.complete_read(read, Err(ToolError::new(ErrorKind::HandlerError, "读取失败"))).unwrap();
    assert_eq!(error_kind(&sends(&h.drain())[0]), "HANDLER_ERROR");

    let msgs = sends(&h.request(7, "resources/read", json!({"name": "nope"})));
    assert_eq!(msgs[0]["error"]["code"], -32013);
    assert_eq!(error_kind(&msgs[0]), "RESOURCE_NOT_FOUND");
}

#[test]
fn resource_subscription_and_throttle() {
    let mut h = Harness::new();
    let r = h.c.register_resource(resource("cart.state")).unwrap();
    let other = h.c.register_resource(resource("other")).unwrap();
    h.connect();

    // 未订阅：不发送
    h.c.notify_resource_changed(r, h.now).unwrap();
    assert!(h.drain().is_empty());

    let msgs = sends(&h.request(1, "resources/subscribe", json!({"name": "cart.state"})));
    assert_eq!(msgs[0], json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
    assert!(h.c.is_subscribed(r));
    let msgs = sends(&h.request(2, "resources/subscribe", json!({"name": "nope"})));
    assert_eq!(error_kind(&msgs[0]), "RESOURCE_NOT_FOUND");

    let t0 = h.now;
    h.c.notify_resource_changed(r, t0).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs, vec![json!({"jsonrpc": "2.0", "method": "resources/updated", "params": {"name": "cart.state"}})]);

    // 节流期内多次变化合并为一次
    h.now = t0 + 10;
    h.c.notify_resource_changed(r, h.now).unwrap();
    h.now = t0 + 50;
    h.c.notify_resource_changed(r, h.now).unwrap();
    h.c.notify_resource_changed(other, h.now).unwrap(); // 未订阅
    assert!(h.drain().is_empty());
    assert_eq!(h.c.poll_timeout(), Some(t0 + 100));
    let ev = h.advance(50);
    assert_eq!(methods(&sends(&ev)), vec!["resources/updated"]);
    assert!(h.advance(1_000).iter().all(|e| !matches!(e, Event::Send(s) if s.contains("resources/updated"))));

    // 节流期过后立即发送
    h.c.notify_resource_changed(r, h.now).unwrap();
    assert_eq!(sends(&h.drain()).len(), 1);

    // 取消订阅
    let msgs = sends(&h.request(3, "resources/unsubscribe", json!({"name": "cart.state"})));
    assert_eq!(msgs[0]["result"], json!({}));
    h.now += 1_000;
    h.c.notify_resource_changed(r, h.now).unwrap();
    assert!(h.drain().is_empty());

    // 断线后订阅清空
    h.request(4, "resources/subscribe", json!({"name": "cart.state"}));
    h.c.handle_disconnected(h.now);
    h.drain();
    assert!(!h.c.is_subscribed(r));
}

#[test]
fn resource_ops_during_handshake_are_unauthorized() {
    let mut h = Harness::new();
    h.c.register_resource(resource("r")).unwrap();
    h.open();
    let msgs = sends(&h.request(1, "resources/read", json!({"name": "r"})));
    assert_eq!(error_kind(&msgs[0]), "UNAUTHORIZED");
    assert_eq!(msgs[0]["error"]["code"], -32014);
}
