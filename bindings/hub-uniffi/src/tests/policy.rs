//! 端到端单元测试（续）：策略挂点（spec/hub-api.md 3.13）。

use super::*;

fn rule(id: &str, action: PolicyAction, tool: &str) -> PolicyRule {
    PolicyRule { id: id.into(), action, app: "notes".into(), tool: Some(tool.into()), annotations: None, hooks: None, agent: None }
}

/// 策略挂点（spec/hub-api.md 3.13）：配置的 hide / deny、运行中替换、不合法规则保留旧规则、状态中的命中计数。
#[test]
fn policy_hide_deny_and_replace() {
    let bad = AppMcpHub::start(HubConfig {
        enable_listen: false,
        enable_ipc: false,
        policy: Some(PolicyConfig { rules: vec![rule("bad id!", PolicyAction::Hide, "x")] }),
        ..Default::default()
    });
    assert!(matches!(bad, Err(HubError::InvalidConfig { .. })), "不合法的规则 id 被拒");

    let hub = AppMcpHub::start(HubConfig {
        listen: Some("127.0.0.1:0".into()),
        enable_ipc: false,
        policy: Some(PolicyConfig {
            rules: vec![rule("hide-clear", PolicyAction::Hide, "notes.clear"), rule("deny-add", PolicyAction::Deny, "notes.add")],
        }),
        ..Default::default()
    })
    .expect("启动 Hub");
    let mut cfg = native::NativeConfig::new("notes", "笔记");
    cfg.host_url = format!("ws://{}/app", hub.listen_addr().expect("监听地址"));
    let app = native::NativeClient::new(cfg, None).expect("App");
    app.register_tool(native::ToolSpec::new("notes.add", "添加"), Arc::new(AddNote)).expect("注册");
    app.register_tool(native::ToolSpec::new("notes.clear", "清空"), Arc::new(ClearNotes)).expect("注册");
    app.register_tool(native::ToolSpec::new("echo", "回显"), Arc::new(AddNote)).expect("注册");
    app.start();

    let deadline = Instant::now() + Duration::from_secs(10);
    let names = |hub: &AppMcpHub| -> Vec<String> {
        let filter = ToolFilter { apps: Some(vec!["notes".into()]), include_builtin: false, ..Default::default() };
        let mut v: Vec<String> = hub.tools(filter).into_iter().map(|t| t.tool).collect();
        v.sort();
        v
    };
    while names(&hub).len() < 2 {
        assert!(Instant::now() < deadline, "App 未连上");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(names(&hub), ["echo", "notes.add"], "hide 的工具不在列表中");

    assert_eq!(call_error_kind(&hub, "notes.notes.clear").0.as_deref(), Some("TOOL_NOT_FOUND"));
    let (kind, details) = call_error_kind(&hub, "notes.notes.add");
    assert_eq!(kind.as_deref(), Some("POLICY_DENIED"));
    let details = details.expect("details");
    assert_eq!((details["ruleId"].as_str(), details["hook"].as_str()), (Some("deny-add"), Some("call")));
    assert_eq!(call_error_kind(&hub, "notes.echo").0, None, "未命中规则的工具照常调用");

    let st = hub.policy().expect("policy");
    let hits: Vec<(String, u64)> = st.rules.iter().map(|r| (r.rule.id.clone(), r.hits)).collect();
    assert_eq!(hits, [("hide-clear".to_owned(), 1), ("deny-add".to_owned(), 1)]);
    assert_eq!(st.last_error, None);
    assert_eq!(hub.status().expect("status").policy.map(|p| p.rules.len()), Some(2));

    // 不合法：hide 不能写 hooks → 报错，之前的规则继续生效
    let mut invalid = rule("hide-echo", PolicyAction::Hide, "echo");
    invalid.hooks = Some(vec![PolicyHook::Call]);
    let e = hub.set_policy(PolicyConfig { rules: vec![invalid] });
    assert!(matches!(&e, Err(HubError::Tool { kind, .. }) if kind == "INVALID_INPUT"), "{e:?}");
    assert_eq!(call_error_kind(&hub, "notes.notes.add").0.as_deref(), Some("POLICY_DENIED"));
    assert!(hub.policy().expect("policy").last_error.is_some(), "失败记入 last_error");

    // 替换：只隐藏 echo（按注解匹配的规则也能经 FFI 往返）
    let mut by_annotation = rule("deny-destructive", PolicyAction::Deny, "*");
    by_annotation.annotations = Some(AnnotationMatch { destructive_hint: Some(true), ..Default::default() });
    by_annotation.hooks = Some(vec![PolicyHook::Call, PolicyHook::Wake]);
    hub.set_policy(PolicyConfig { rules: vec![rule("hide-echo", PolicyAction::Hide, "echo"), by_annotation.clone()] })
        .expect("替换");
    assert_eq!(names(&hub), ["notes.add", "notes.clear"]);
    assert_eq!(call_error_kind(&hub, "notes.notes.add").0, None);
    let st = hub.policy().expect("policy");
    assert_eq!(st.last_error, None, "成功加载后清除");
    assert_eq!(st.rules[1].rule, by_annotation);
    app.stop();
    hub.shutdown();
}
