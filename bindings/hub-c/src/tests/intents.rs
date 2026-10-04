//! 单元测试（续）：标准意图（v23，spec/intents.md）：App 声明 implements → apps.intents 列出、机主默认排首位、
//! am_hub_set_intent_defaults 拒绝不合法的默认表并保留旧值、am_hub_intents_json 与 status.intents。

use super::*;

/// 注册两个都实现 `link.open@1` 的工具（`notes.browse`、`notes.open`）的 App。
fn start_intent_app(hub: *mut AmHub) -> App {
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("notes", "笔记");
    cfg.host_url = format!("ws://{addr}/app");
    cfg.instance_id = Some("n1".into());
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let mut handles: Vec<Box<dyn std::any::Any>> = Vec::new();
    for name in ["open", "browse"] {
        let mut spec = ToolSpec::new(name, "打开链接");
        spec.input_schema_json =
            Some(json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}).to_string());
        let options = ToolOptions { implements: vec!["link.open@1".into()], ..ToolOptions::default() };
        handles.push(Box::new(client.register_tool_with(spec, options, Arc::new(Echo)).expect("注册")));
    }
    client.start();
    App { client, _handles: handles }
}

fn intents_of(hub: *mut AmHub, tx: &Sender<String>, rx: &Receiver<String>) -> Value {
    call(hub, json!({"name":"apps.intents","arguments":{"intent":"link.open"},"session":"s1"}), tx);
    recv(rx)
}

/// 实现者的工具全名（按 apps.intents 给出的顺序）与 default 标记。
fn implementations(outcome: &Value) -> Vec<(String, bool)> {
    outcome["result"]["ok"]["intents"][0]["implementations"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|i| (i["tool"].as_str().unwrap_or_default().to_owned(), i["default"] == json!(true)))
                .collect()
        })
        .unwrap_or_default()
}

fn intents_status(hub: *mut AmHub) -> Value {
    // SAFETY: 有效参数。
    query_json(|o| unsafe { am_hub_intents_json(hub, o) })
}

fn set_defaults(hub: *mut AmHub, json: &str) -> AmHubStatus {
    let text = c(json);
    // SAFETY: 有效参数。
    unsafe { am_hub_set_intent_defaults(hub, text.as_ptr()) }
}

#[test]
fn app_implements_reach_apps_intents_and_defaults_order_them() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0"}"#);
    let _app = start_intent_app(hub);

    // HubTool.implements（am_hub_tools_json）；appConnected 先于工具同步到达，轮询到工具出现为止。
    let deadline = Instant::now() + WAIT;
    let implements = loop {
        // SAFETY: 有效参数。
        let tools = query_json(|o| unsafe { am_hub_tools_json(hub, ptr::null(), o) });
        let open = tools.as_array().and_then(|t| t.iter().find(|t| t["name"] == "notes.open")).cloned();
        if let Some(tool) = open {
            break tool["implements"].clone();
        }
        assert!(Instant::now() < deadline, "等待 App 工具超时");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(implements, json!(["link.open@1"]));

    // 无默认：按工具全名排序
    let (tx, rx) = mpsc::channel::<String>();
    let outcome = intents_of(hub, &tx, &rx);
    assert_eq!(
        implementations(&outcome),
        vec![("notes.browse".to_owned(), false), ("notes.open".to_owned(), false)],
        "{outcome}"
    );
    assert_eq!(intents_status(hub), json!({"defaults": {}}));

    // 设默认：默认工具排首位并标 default
    assert_eq!(set_defaults(hub, r#"{"link.open":"notes.open"}"#), AmHubStatus::Ok, "{}", last_error());
    let outcome = intents_of(hub, &tx, &rx);
    assert_eq!(
        implementations(&outcome),
        vec![("notes.open".to_owned(), true), ("notes.browse".to_owned(), false)],
        "{outcome}"
    );

    // 不合法的默认表：INVALID_CONFIG，旧值保留，原因记入 lastError（am_hub_intents_json 与 status.intents 一致）
    assert_eq!(set_defaults(hub, r#"{"link":"notes.open","link.open":"bad"}"#), AmHubStatus::InvalidConfig);
    assert!(last_error().contains("link"), "{}", last_error());
    let status = intents_status(hub);
    assert_eq!(status["defaults"], json!({"link.open": "notes.open"}), "{status}");
    assert!(status["lastError"].as_str().is_some_and(|e| !e.is_empty()), "{status}");
    // SAFETY: 有效参数。
    let hub_status = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    assert_eq!(hub_status["intents"], status);
    assert_eq!(implementations(&intents_of(hub, &tx, &rx))[0], ("notes.open".to_owned(), true));

    // 不是字符串到字符串的对象 / 非法 JSON：INVALID_JSON；NULL：INVALID_ARGUMENT
    assert_eq!(set_defaults(hub, r#"{"link.open":1}"#), AmHubStatus::InvalidJson);
    assert_eq!(set_defaults(hub, "{"), AmHubStatus::InvalidJson);
    // SAFETY: NULL 参数是合法输入（报错）。
    unsafe {
        assert_eq!(am_hub_set_intent_defaults(hub, ptr::null()), AmHubStatus::InvalidArgument);
        assert_eq!(am_hub_set_intent_defaults(ptr::null_mut(), c("{}").as_ptr()), AmHubStatus::InvalidArgument);
        let mut out = ptr::null_mut();
        assert_eq!(am_hub_intents_json(ptr::null(), &mut out), AmHubStatus::InvalidArgument);
        assert!(out.is_null());
    }

    // 成功替换后清除 lastError；"{}" 清空
    assert_eq!(set_defaults(hub, "{}"), AmHubStatus::Ok);
    assert_eq!(intents_status(hub), json!({"defaults": {}}));
    // SAFETY: 测试结束。
    unsafe { am_hub_free(hub) };
}
