//! 端到端单元测试（续）：标准意图（spec/intents.md 第 4 节）经绑定层：真实 App 声明 `implements` →
//! `HubTool.implements`、Agent 会话 `apps.intents`、机主默认表的设置 / 读取 / 拒绝与 `status().intents`。

use std::collections::HashMap;

use super::*;

/// 注册两个实现 `message.send@1` 的工具（`mail.a_send`、`mail.b_send`）并连接。
fn start_mail_app(hub: &AppMcpHub) -> native::NativeClient {
    let mut cfg = native::NativeConfig::new("mail", "邮件");
    cfg.host_url = format!("ws://{}/app", hub.listen_addr().expect("监听地址"));
    let app = native::NativeClient::new(cfg, None).expect("App");
    let schema = json!({
        "type": "object",
        "properties": {"to": {"type": "array", "items": {"type": "string"}}, "text": {"type": "string"}},
        "required": ["to", "text"]
    });
    for name in ["a_send", "b_send"] {
        let mut spec = native::ToolSpec::new(name, "发邮件");
        spec.input_schema_json = Some(schema.to_string());
        let options = native::ToolOptions { implements: vec!["message.send@1".into()], ..Default::default() };
        app.register_tool_with(spec, options, Arc::new(ClearNotes)).expect("注册");
    }
    app.start();
    app
}

/// Agent 会话中 `apps.intents {intent: "message.send"}` 的实现者（工具全名, 是否默认）。
fn implementations(hub: &AppMcpHub) -> Vec<(String, bool)> {
    let req = CallRequest { session: Some("agent".into()), ..req("apps.intents", json!({"intent": "message.send"})) };
    let out = wait(hub.call_tool(req)).expect("调用");
    assert!(out.error.is_none(), "{out:?}");
    let data: Value = serde_json::from_str(out.data_json.as_deref().unwrap_or("null")).unwrap_or_default();
    let entry = data["intents"]
        .as_array()
        .and_then(|a| a.iter().find(|e| e["intent"] == "message.send@1"))
        .unwrap_or_else(|| panic!("缺少 message.send@1：{data}"));
    assert_eq!(entry["known"], true, "{entry}");
    entry["implementations"]
        .as_array()
        .map(|a| a.iter().map(|i| (i["tool"].as_str().unwrap_or_default().to_owned(), i["default"] == true)).collect())
        .unwrap_or_default()
}

#[test]
fn implements_lists_and_defaults_reorder() {
    let hub = start_hub(None);
    let (tx, rx) = mpsc::channel();
    hub.set_event_listener(Some(Arc::new(Events(Mutex::new(tx)))));
    let app = start_mail_app(&hub);
    wait_for(&rx, |e| matches!(e, HubEvent::AppConnected { .. }));

    let mail_tools = || hub.tools(ToolFilter { apps: Some(vec!["mail".into()]), ..Default::default() });
    // 连接事件之后工具列表才送达。
    let deadline = Instant::now() + Duration::from_secs(10);
    while !mail_tools().iter().any(|t| t.name == "mail.b_send") {
        assert!(Instant::now() < deadline, "等待工具送达超时");
        std::thread::sleep(Duration::from_millis(10));
    }
    let tools = mail_tools();
    let b = tools.iter().find(|t| t.name == "mail.b_send").unwrap_or_else(|| panic!("{tools:?}"));
    assert_eq!(b.implements, ["message.send@1"], "HubTool.implements 透传");
    assert_eq!(hub.intents().expect("intents"), IntentsStatus::default());

    let unset = vec![("mail.a_send".to_owned(), false), ("mail.b_send".to_owned(), false)];
    assert_eq!(implementations(&hub), unset, "无默认时按全名排序");

    let defaults = HashMap::from([("message.send".to_owned(), "mail.b_send".to_owned())]);
    hub.set_intent_defaults(defaults.clone()).expect("设置默认");
    assert_eq!(implementations(&hub), [("mail.b_send".to_owned(), true), ("mail.a_send".to_owned(), false)], "默认排首位");
    let st = hub.intents().expect("intents");
    assert_eq!((&st.defaults, st.last_error.as_deref()), (&defaults, None));
    assert_eq!(hub.status().expect("status").intents, Some(st), "status().intents 同 intents()");

    // 不合法的默认表整体拒绝（INVALID_INPUT），之前的继续生效，原因记入 last_error。
    let bad = HashMap::from([("Bad Verb".to_owned(), "mail.a_send".to_owned())]);
    match hub.set_intent_defaults(bad) {
        Err(HubError::Tool { kind, .. }) => assert_eq!(kind, "INVALID_INPUT"),
        other => panic!("应拒绝：{other:?}"),
    }
    let st = hub.intents().expect("intents");
    assert_eq!(st.defaults, defaults, "旧值保留");
    assert!(st.last_error.as_deref().is_some_and(|e| e.contains("Bad Verb")), "{st:?}");
    assert_eq!(implementations(&hub)[0], ("mail.b_send".to_owned(), true));

    hub.set_intent_defaults(HashMap::new()).expect("清空");
    assert_eq!(hub.intents().expect("intents"), IntentsStatus::default(), "成功后清除 last_error");
    app.stop();
    hub.shutdown();
    assert!(matches!(hub.intents(), Err(HubError::Shutdown)));
    assert!(matches!(hub.set_intent_defaults(HashMap::new()), Err(HubError::Shutdown)));
}

/// `HubConfig.intent_defaults` 经绑定生效；不合法时 Hub 照常启动、空表并记下原因。
#[test]
fn config_intent_defaults_apply_or_record_error() {
    let start = |defaults: HashMap<String, String>| {
        AppMcpHub::start(HubConfig {
            enable_listen: false,
            enable_ipc: false,
            intent_defaults: Some(defaults),
            ..Default::default()
        })
        .expect("启动 Hub")
    };
    let good = HashMap::from([("link.open@1".to_owned(), "web.open".to_owned())]);
    let hub = start(good.clone());
    assert_eq!(hub.intents().expect("intents"), IntentsStatus { defaults: good, last_error: None });
    hub.shutdown();
    let hub = start(HashMap::from([("link.open".to_owned(), "nodot".to_owned())]));
    let st = hub.intents().expect("intents");
    assert!(st.defaults.is_empty() && st.last_error.as_deref().is_some_and(|e| e.contains("nodot")), "{st:?}");
    hub.shutdown();
}
