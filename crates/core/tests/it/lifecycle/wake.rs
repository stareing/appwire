//! 唤醒、快速恢复、握手超时与 toolsHash。

use super::support::*;

// ---------------------------------------------------------------------------
// 唤醒与快速恢复
// ---------------------------------------------------------------------------

#[test]
fn wake_with_tools_current_skips_sync() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    let sleep = h.go_dormant();

    assert!(!h.c.handle_wake("--unrelated", h.now));
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    assert!(h.c.handle_wake("myapp://app-mcp/wake?token=wk-1", h.now));
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Waking)]);
    let hello = h.link();
    let p = &hello["params"];
    assert_eq!(p["launchToken"], "wk-1");
    assert_eq!(p["resumeToken"], "resume-1");
    assert_eq!(p["toolsHash"], sleep["params"]["toolsHash"]);
    assert_eq!(p["wakeReason"], "os-activation");
    assert_eq!(p["token"], "tk");

    let ev = h.paired(&hello, true);
    assert_eq!(methods(&sends(&ev)), vec!["app/visibility", "app/ready"], "toolsCurrent 跳过全量同步");
    assert_eq!(h.c.resume_token(), None, "恢复令牌只用一次");

    // 恢复后增量追踪照常工作，调用照常处理
    h.c.register_tool(tool("b")).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(methods(&msgs), vec!["tools/changed"]);
    let ev = h.invoke("c1", "a");
    assert!(ev.iter().any(|e| matches!(e, Event::InvokeTool { .. })));
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    // 再次休眠（处理过调用：合并窗口）
    let t = h.now;
    let (sleep2, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t + MERGE);
    h.accept_sleep(&sleep2, "resume-2");
    assert_eq!(h.c.state(), &ConnectionState::Dormant);

    // 之后的普通回连不再带 launchToken / wakeReason os-activation
    assert!(h.c.wake(h.now));
    h.drain();
    let hello = h.link();
    assert!(hello["params"].get("launchToken").is_none());
    assert_eq!(hello["params"]["wakeReason"], "app");
    assert_eq!(hello["params"]["resumeToken"], "resume-2");
}

#[test]
fn registration_change_while_dormant_forces_full_sync() {
    let mut h = Harness::new(LifecycleMode::Idle);
    let a = h.c.register_tool(tool("a")).unwrap();
    h.connect();
    let sleep = h.go_dormant();
    // 休眠期间的注册变更不唤醒、不产生事件
    h.c.register_tool(tool("b")).unwrap();
    h.c.update_tool(a, ToolUpdate { description: Some("新描述".into()), ..Default::default() }).unwrap();
    assert!(h.drain().is_empty());
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    assert_eq!(h.c.poll_timeout(), None);

    assert!(h.c.handle_wake("app-mcp-wake:wk", h.now));
    h.drain();
    let hello = h.link();
    assert_ne!(hello["params"]["toolsHash"], sleep["params"]["toolsHash"], "摘要反映休眠期间的变更");
    assert_eq!(hello["params"]["toolsHash"], json!(h.c.tools_hash()));
    // Host 发现摘要不一致 → toolsCurrent: false → 完整同步
    let ev = h.paired(&hello, false);
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["tools/sync", "resources/sync", "app/visibility", "app/ready"]);
    assert_eq!(msgs[0]["params"]["tools"].as_array().unwrap().len(), 2);
}

#[test]
fn tools_current_without_resume_token_still_syncs() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.start(h.now);
    h.drain();
    let hello = h.link();
    let ev = h.paired(&hello, true);
    assert_eq!(methods(&sends(&ev)), vec!["tools/sync", "resources/sync", "app/visibility", "app/ready"]);
    assert!(ev.iter().any(|e| matches!(e, Event::Warning(_))));
}

#[test]
fn wake_in_backoff_reconnects_immediately() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.c.handle_disconnected(h.now);
    h.drain();
    assert!(matches!(h.c.state(), ConnectionState::Backoff { .. }));
    assert!(h.c.handle_wake("app-mcp-wake:abc", h.now));
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
    let hello = h.link();
    assert_eq!(hello["params"]["launchToken"], "abc");
    assert_eq!(hello["params"]["wakeReason"], "os-activation");
}

#[test]
fn wake_during_sleep_handshake_rewakes_after_accept() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    assert!(h.c.handle_wake("app-mcp-wake:late", h.now));
    let ev = h.accept_sleep(&sleep, "r1");
    assert_eq!(
        ev,
        vec![
            Event::Disconnect,
            Event::StateChanged(ConnectionState::Dormant),
            Event::Connect,
            Event::StateChanged(ConnectionState::Waking),
        ]
    );
    let hello = h.link();
    assert_eq!(hello["params"]["launchToken"], "late");
    assert_eq!(hello["params"]["resumeToken"], "r1");

    // hold 同理
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    let _hold = h.c.hold(h.now);
    let ev = h.accept_sleep(&sleep, "r1");
    assert!(ev.contains(&Event::StateChanged(ConnectionState::Waking)));
    assert_eq!(h.link()["params"]["wakeReason"], "app");
}

/// 回归：已连接时收到唤醒令牌（Android WakeWorker 重排后迟到、前台广播）不应残留，
/// 否则下一次空闲休眠被接受后会立即回连（2026-10-01 真机：DORMANT 后 11 ms 回连）。
#[test]
fn wake_token_while_connected_does_not_rewake_after_next_sleep() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.advance(IDLE / 2);
    assert!(h.c.handle_wake("app-mcp-wake:stale", h.now), "是本 SDK 的唤醒参数");
    assert!(h.drain().is_empty(), "已连接：不断开、不回连");
    // 重新开始空闲计时：从收到唤醒起满 IDLE 才休眠
    let woke_at = h.now;
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, woke_at + IDLE);
    let ev = h.accept_sleep(&sleep, "r1");
    assert_eq!(ev, vec![Event::Disconnect, Event::StateChanged(ConnectionState::Dormant)]);
    assert_eq!(h.c.poll_timeout(), None);

    // 之后的正常唤醒不携带过期令牌
    assert!(h.c.wake_with_reason(WakeReason::Visible, h.now));
    h.drain();
    let hello = h.link();
    assert!(hello["params"].get("launchToken").is_none_or(Value::is_null), "{hello}");
    assert_eq!(hello["params"]["wakeReason"], "visible");
}

#[test]
fn handle_wake_before_start_connects_even_on_demand() {
    let mut cfg = config(LifecycleMode::OnDemand);
    cfg.lifecycle.residency = Residency::ExitWhenIdle;
    let mut h = Harness::with(cfg);
    assert!(h.c.handle_wake("shop://app-mcp/wake?token=cold", h.now));
    h.c.start(h.now);
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
    let hello = h.link();
    assert_eq!(hello["params"]["launchToken"], "cold");
    assert_eq!(hello["params"]["wakeReason"], "os-activation");
    h.paired(&hello, false);
    // 由唤醒冷启动 + exit-when-idle → 休眠后 IdleExit
    let (sleep, _) = h.wait_sleep(IDLE).unwrap();
    let ev = h.accept_sleep(&sleep, "r");
    assert_eq!(ev.last(), Some(&Event::IdleExit));
}

#[test]
fn residency_controls_idle_exit() {
    // exit-when-idle 但不是唤醒启动：不发 IdleExit
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.residency = Residency::ExitWhenIdle;
    let mut h = Harness::with(cfg);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    assert!(!h.accept_sleep(&sleep, "r").contains(&Event::IdleExit));

    // 配置带 launch token（Host 冷启动）→ 视为唤醒启动
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.residency = Residency::ExitWhenIdle;
    cfg.launch_token = Some("lt".into());
    let mut h = Harness::with(cfg);
    let hello = h.connect();
    assert_eq!(hello["params"]["wakeReason"], "os-activation");
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    assert!(h.accept_sleep(&sleep, "r").contains(&Event::IdleExit));

    // exit-always
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.residency = Residency::ExitAlways;
    let mut h = Harness::with(cfg);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(h.accept_sleep(&sleep, "r").last(), Some(&Event::IdleExit));

    // keep
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    assert!(!h.accept_sleep(&sleep, "r").contains(&Event::IdleExit));
}

#[test]
fn accepted_sleep_with_running_call_cancels_it() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    let (sleep, _) = h.wait_sleep(3 * IDLE).unwrap();
    // 竞态：休眠握手中到达的调用照常执行
    let ev = h.invoke("c1", "a");
    assert!(ev.iter().any(|e| matches!(e, Event::InvokeTool { .. })));
    let ev = h.accept_sleep(&sleep, "r");
    assert!(ev.contains(&Event::CancelTool { call_id: "c1".into(), reason: CancelReason::Disconnected }));
    assert!(ev.iter().any(|e| matches!(e, Event::Warning(_))));
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
}

#[test]
fn stop_from_dormant() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.go_dormant();
    h.c.stop(h.now);
    assert_eq!(h.drain(), vec![Event::StateChanged(ConnectionState::Stopped)]);
    assert!(h.c.handle_wake("app-mcp-wake:x", h.now), "是本 SDK 的唤醒参数");
    assert_eq!(h.c.state(), &ConnectionState::Stopped);
    assert!(!h.c.wake(h.now));
}

// ---------------------------------------------------------------------------
// 握手超时
// ---------------------------------------------------------------------------

#[test]
fn handshake_timeout_reconnects() {
    let mut h = Harness::new(LifecycleMode::Persistent);
    h.c.start(h.now);
    h.drain();
    h.link();
    assert_eq!(h.c.poll_timeout(), Some(h.now + 10_000));
    let ev = h.advance(10_000);
    assert!(ev.contains(&Event::Disconnect));
    assert!(matches!(h.c.state(), ConnectionState::Backoff { .. }));

    // pending（等待用户确认）不受握手超时限制
    let mut h = Harness::new(LifecycleMode::Persistent);
    h.c.start(h.now);
    h.drain();
    let hello = h.link();
    h.respond(&hello, json!({"status": "pending", "protocolVersion": "1", "hostVersion": "h"}));
    assert_eq!(h.c.poll_timeout(), None);

    // 0 表示不限
    let mut cfg = config(LifecycleMode::Persistent);
    cfg.handshake_timeout_ms = 0;
    let mut h = Harness::with(cfg);
    h.c.start(h.now);
    h.drain();
    h.link();
    assert_eq!(h.c.poll_timeout(), None);
}

// ---------------------------------------------------------------------------
// toolsHash
// ---------------------------------------------------------------------------

#[test]
fn tools_hash_fixed_vector() {
    // 与 crates/protocol/src/hash.rs 的固定向量相同的定义。
    let mut h = Harness::new(LifecycleMode::Idle);
    assert_eq!(h.c.tools_hash(), "69c61b185225ee82", "空注册表");
    h.c.register_tool(ToolDef {
        name: "todo.add".into(),
        description: "添加待办".into(),
        input_schema: json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
        risk: Risk::Write,
        activation: None,
        title: None,
        enabled: true,
        scope: None,
        annotations: None,
        output_schema: None,
        surface: ToolSurface::App,
        page: None,
        background_tool: None,
        implements: Vec::new(),
        cache: None,
        deprecated: None,
        concurrency: 0,
        exclusive: None,
    })
    .unwrap();
    let checkout = h
        .c
        .register_tool(ToolDef {
            name: "cart.checkout".into(),
            description: "结算".into(),
            input_schema: json!({"type": "object"}),
            risk: Risk::Payment,
            activation: Some(Activation::Foreground),
            title: Some("Checkout".into()),
            enabled: true,
            scope: None,
            annotations: None,
            output_schema: None,
            surface: ToolSurface::App,
            page: None,
            background_tool: None,
            implements: Vec::new(),
            cache: None,
            deprecated: None,
            concurrency: 0,
            exclusive: None,
        })
        .unwrap();
    h.c.register_resource(ResourceDef {
        name: "cart.state".into(),
        description: "购物车".into(),
        mime_type: None,
        scope: None,
        realtime: false,
        annotations: None,
        cache: None,
    })
    .unwrap();
    assert_eq!(h.c.tools_hash(), "ba703035ddca2f91");
    // 禁用的工具不计入
    h.c.update_tool(checkout, ToolUpdate { enabled: Some(false), ..Default::default() }).unwrap();
    assert_ne!(h.c.tools_hash(), "ba703035ddca2f91");
    h.c.update_tool(checkout, ToolUpdate { enabled: Some(true), ..Default::default() }).unwrap();
    assert_eq!(h.c.tools_hash(), "ba703035ddca2f91");
}
