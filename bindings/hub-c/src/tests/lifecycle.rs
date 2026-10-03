//! 单元测试（续）：休眠与自定义唤醒、限额与策略、调用进度、休眠记录持久化与内置工具。

use super::*;

// ---------------------------------------------------------------------------
// 生命周期：休眠 → 列出 dormant → 调用触发自定义唤醒 → 成功
// ---------------------------------------------------------------------------

/// 与 `free_sender` 相同，但不计入 FREED（避免与并行测试的计数互相干扰）。
unsafe extern "C" fn free_sender_quiet(ud: *mut c_void) {
    // SAFETY: Box::into_raw 得到的 Sender。
    drop(unsafe { Box::from_raw(ud as *mut Sender<String>) });
}

unsafe extern "C" fn on_wake(ud: *mut c_void, json: *mut c_char, h: *mut AmHubWake) {
    // SAFETY: 测试传入的 Sender。
    let tx = unsafe { &*(ud as *const Sender<(String, usize)>) };
    // SAFETY: 库分配的字符串。
    let _ = tx.send((unsafe { take(json) }, h as usize));
}

fn start_idle_app(hub: *mut AmHub) -> (NativeClient, Box<dyn std::any::Any>) {
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("sleepy", "会睡觉的 App");
    cfg.host_url = format!("ws://{addr}/app");
    cfg.instance_id = Some("s1".into());
    cfg.lifecycle.mode = app_mcp_native::LifecycleMode::Idle;
    cfg.lifecycle.idle_timeout_ms = 300;
    cfg.lifecycle.wake = Some(app_mcp_native::WakeDescriptor {
        kind: app_mcp_native::WakeKind::AndroidIntent,
        target: Some("dev.example/.WakeReceiver".into()),
        background: true,
    });
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let h = client
        .register_tool(ToolSpec::new("ping", "回显"), Arc::new(Echo))
        .expect("注册");
    client.start();
    (client, Box::new(h))
}

#[test]
fn dormant_app_woken_by_custom_waker() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0","listChangedDebounceMs":20,"wakeTimeoutMs":8000,"leaseTtlMs":0}"#);
    let (etx, erx) = mpsc::channel::<String>();
    let (wtx, wrx) = mpsc::channel::<(String, usize)>();
    // SAFETY: 有效参数；Sender 归库所有（free_sender 释放）或由测试持有且比 Hub 活得久。
    unsafe {
        let st = am_hub_set_event_cb(hub, Some(on_event), Box::into_raw(Box::new(etx)).cast(), Some(free_sender_quiet));
        assert_eq!(st, AmHubStatus::Ok);
        assert_eq!(am_hub_set_waker_cb(hub, Some(on_wake), ud(&wtx), None), AmHubStatus::Ok);
    }
    let (client, _h) = start_idle_app(hub);

    assert!(
        wait_event(&erx, |e| e["type"] == "appDormant" && e["appId"] == "sleepy" && e["instanceId"] == "s1"),
        "应收到 appDormant"
    );
    // SAFETY: 有效参数。
    let apps = query_json(|o| unsafe { am_hub_apps_json(hub, o) });
    let app = apps
        .as_array()
        .and_then(|a| a.iter().find(|a| a["appId"] == "sleepy"))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(app["connected"], false, "{app}");
    assert_eq!(app["dormantInstances"][0]["instanceId"], "s1", "{app}");
    let filter = c(r#"{"apps":["sleepy"],"includeBuiltin":false}"#);
    // SAFETY: 有效参数。
    let tools = query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) });
    assert_eq!(tools[0]["availability"], "dormant", "{tools}");

    // 调用 → Hub 调用自定义 Waker；测试线程扮演厂商：让同进程 App handleWake(令牌) 后完成句柄。
    let (rtx, rrx) = mpsc::channel::<String>();
    call(hub, json!({"name": "sleepy.ping", "arguments": {"x": 1}}), &rtx);
    let (req, h) = wrx.recv_timeout(WAIT).expect("应调用 Waker");
    let req = parse(&req);
    assert_eq!(req["appId"], "sleepy");
    assert_eq!(req["instanceId"], "s1");
    assert_eq!(req["descriptor"]["kind"], "android-intent");
    let token = req["token"].as_str().unwrap_or_default().to_owned();
    assert_eq!(token.len(), 32);
    assert_eq!(req["activationArg"], format!("app-mcp-wake:{token}"));
    assert!(client.handle_wake(req["activationArg"].as_str().unwrap_or_default()));
    // SAFETY: 回调交出的句柄，只消费一次。
    assert_eq!(unsafe { am_hub_waker_complete(h as *mut AmHubWake, true, ptr::null(), ptr::null()) }, AmHubStatus::Ok);
    let out = recv(&rrx);
    assert_eq!(out["result"]["ok"]["echo"], json!({"x": 1}), "{out}");
    assert!(wait_event(&erx, |e| e["type"] == "appWaking" || e["type"] == "appConnected"));
    client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

#[test]
fn waker_failure_maps_error_kind() {
    let hub = start_hub(r#"{"listen":"127.0.0.1:0","listChangedDebounceMs":20,"leaseTtlMs":0}"#);
    let (etx, erx) = mpsc::channel::<String>();
    let (wtx, wrx) = mpsc::channel::<(String, usize)>();
    // SAFETY: 同上。
    unsafe {
        am_hub_set_event_cb(hub, Some(on_event), Box::into_raw(Box::new(etx)).cast(), Some(free_sender_quiet));
        am_hub_set_waker_cb(hub, Some(on_wake), ud(&wtx), None);
    }
    let (client, _h) = start_idle_app(hub);
    assert!(wait_event(&erx, |e| e["type"] == "appDormant"));
    let (rtx, rrx) = mpsc::channel::<String>();
    call(hub, json!({"name": "sleepy.ping"}), &rtx);
    let (_, h) = wrx.recv_timeout(WAIT).expect("应调用 Waker");
    let kind = c("APP_NOT_INSTALLED");
    let msg = c("没装");
    // SAFETY: 回调交出的句柄。
    let st = unsafe { am_hub_waker_complete(h as *mut AmHubWake, false, kind.as_ptr(), msg.as_ptr()) };
    assert_eq!(st, AmHubStatus::Ok);
    let out = recv(&rrx);
    assert_eq!(out["result"]["error"]["kind"], "APP_NOT_INSTALLED", "{out}");
    assert!(out["result"]["error"]["message"].as_str().unwrap_or_default().contains("没装"));
    // 未知类别 → LAUNCH_FAILED；清除回调恢复默认实现
    assert_eq!(parse_error_kind(Some("NOPE")), ErrorKind::LaunchFailed);
    assert_eq!(parse_error_kind(None), ErrorKind::LaunchFailed);
    assert_eq!(parse_error_kind(Some("TIMEOUT")), ErrorKind::Timeout);
    // SAFETY: 有效参数。
    unsafe {
        assert_eq!(am_hub_set_waker_cb(hub, None, ptr::null_mut(), None), AmHubStatus::Ok);
        assert_eq!(am_hub_waker_complete(ptr::null_mut(), true, ptr::null(), ptr::null()), AmHubStatus::InvalidArgument);
    }
    client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

/// 缺少 cargo feature 的错误（spec/hub-api.md 3.10）报 `AM_HUB_ERR_UNSUPPORTED`，其余 I/O 错误仍为 `AM_HUB_ERR_IO`。
#[test]
fn unsupported_io_error_maps_to_own_status() {
    let e = FfiError::io("启动 Hub 失败", &std::io::Error::new(std::io::ErrorKind::Unsupported, "缺少 `upstream`"));
    assert_eq!(e.status, AmHubStatus::Unsupported);
    assert!(e.message.contains("`upstream`"), "{}", e.message);
    let e = FfiError::io("启动 Hub 失败", &std::io::Error::new(std::io::ErrorKind::AddrInUse, "占用"));
    assert_eq!(e.status, AmHubStatus::Io);

    // 端到端：本库以 app-mcp-hub 默认（完整）能力构建时成功；缺少 feature 时为 UNSUPPORTED。
    let cfg = c(r#"{"listen":"127.0.0.1:0","ipcEndpoint":null,"mcpHttp":true}"#);
    let mut hub = ptr::null_mut();
    // SAFETY: 有效参数。
    let st = unsafe { am_hub_start(cfg.as_ptr(), &mut hub) };
    let want = if hub::features::MCP_SERVER { AmHubStatus::Ok } else { AmHubStatus::Unsupported };
    assert_eq!(st, want, "{}", last_error());
    if !hub.is_null() {
        // SAFETY: am_hub_start 成功返回的句柄。
        unsafe { am_hub_free(hub) };
    }
}

/// 以 pending + stateResource + summary + 内容注解完成（第 19 项 R1–R3）。
struct Submit;

impl ToolHandler for Submit {
    fn invoke(&self, call: CallHandle) {
        let _ = call.complete_with(CallResult {
            data_json: Some(r#"{"orderId":"o1"}"#.into()),
            status: ResultStatus::Pending,
            state_resource: Some("order.state".into()),
            summary: Some("已提交".into()),
            annotations: Some(ContentAnnotations { priority: Some(0.5), ..ContentAnnotations::default() }),
            ..CallResult::default()
        });
    }
}

/// 配置 limits / outputValidation（v9）→ 工具注解与 outputSchema、结构化结果、RATE_LIMITED、/status 新字段。
#[test]
fn limits_annotations_and_structured_result() {
    // 非法：限流时 burst = 0
    let bad = c(r#"{"listen":null,"ipcEndpoint":null,"limits":{"toolRateBurst":0}}"#);
    let mut out = ptr::null_mut();
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_start(bad.as_ptr(), &mut out) }, AmHubStatus::InvalidConfig);
    assert!(out.is_null());
    assert!(last_error().contains("toolRateBurst"), "{}", last_error());

    let hub = start_hub(
        r#"{"listen":"127.0.0.1:0","limits":{"toolRatePerMinute":1,"toolRateBurst":1},"outputValidation":"reject"}"#,
    );
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("shop", "商店");
    cfg.host_url = format!("ws://{addr}/app");
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let options = ToolOptions {
        annotations: Some(ToolAnnotations { idempotent_hint: Some(false), ..ToolAnnotations::default() }),
        output_schema_json: Some(r#"{"type":"object","properties":{"orderId":{"type":"string"}}}"#.into()),
        ..ToolOptions::default()
    };
    let _tool = client
        .register_tool_with(ToolSpec::new("order.submit", "下单"), options, Arc::new(Submit))
        .expect("注册");
    client.start();

    let deadline = Instant::now() + WAIT;
    let filter = c(r#"{"apps":["shop"],"onlyAvailable":true,"includeBuiltin":false}"#);
    let tools = loop {
        // SAFETY: 有效参数。
        let t = query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) });
        if t.as_array().map(Vec::len) == Some(1) {
            break t;
        }
        assert!(Instant::now() < deadline, "工具未同步：{t}");
        std::thread::sleep(Duration::from_millis(20));
    };
    // 声明的字段优先，缺少的按 risk（write）推导
    assert_eq!(tools[0]["annotations"]["idempotentHint"], false, "{tools}");
    assert_eq!(tools[0]["annotations"]["readOnlyHint"], false, "{tools}");
    assert_eq!(tools[0]["outputSchema"]["properties"]["orderId"]["type"], "string", "{tools}");

    let (tx, rx) = mpsc::channel();
    call(hub, json!({"name":"shop.order.submit"}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["ok"]["orderId"], "o1", "{o}");
    assert_eq!(o["status"], "pending", "{o}");
    assert_eq!(o["stateResource"], "app-mcp://shop/order.state", "{o}");
    assert_eq!(o["summary"], "已提交", "{o}");
    assert_eq!(o["annotations"], json!({"priority": 0.5}), "{o}");
    // 突发 1：第二次立即调用被限流
    call(hub, json!({"name":"shop.order.submit"}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["error"]["kind"], "RATE_LIMITED", "{o}");
    assert_eq!(o["result"]["error"]["details"]["scope"], "tool", "{o}");

    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    assert_eq!(st["limits"]["toolRatePerMinute"], 1, "{st}");
    assert_eq!(st["limits"]["appRatePerMinute"], 600, "{st}");
    assert_eq!(st["outputValidation"], "reject", "{st}");
    let app_st = &st["apps"][0];
    assert_eq!(app_st["rateLimited"], 1, "{st}");
    assert_eq!(app_st["tooLarge"], 0, "{st}");
    assert_eq!(app_st["tools"][0]["name"], "order.submit", "{st}");
    assert_eq!(app_st["tools"][0]["outputSchema"], true, "{st}");
    assert_eq!(app_st["tools"][0]["annotations"], json!({"idempotentHint": false}), "{st}");

    client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

fn set_policy(hub: *mut AmHub, policy: &str) -> AmHubStatus {
    let p = c(policy);
    // SAFETY: 有效参数。
    unsafe { am_hub_set_policy(hub, p.as_ptr()) }
}

fn tool_names(hub: *mut AmHub) -> Vec<String> {
    let filter = c(r#"{"apps":["notes"],"includeBuiltin":false}"#);
    // SAFETY: 有效参数。
    let t = query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) });
    let mut v: Vec<String> =
        t.as_array().into_iter().flatten().filter_map(|t| t["tool"].as_str().map(str::to_owned)).collect();
    v.sort();
    v
}

/// 配置 policy（v10）→ hide 的工具不在列表中且调用为 TOOL_NOT_FOUND；deny → POLICY_DENIED；am_hub_set_policy 替换，
/// 不合法时保留旧规则；HubStatus.policy 的命中计数与 lastError。
#[test]
fn policy_hide_deny_and_set_policy() {
    let bad = c(r#"{"listen":null,"ipcEndpoint":null,"policy":{"rules":[{"id":"x","action":"hide","app":"a*b"}]}}"#);
    let mut out = ptr::null_mut();
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_start(bad.as_ptr(), &mut out) }, AmHubStatus::InvalidConfig);
    assert!(out.is_null());
    assert!(last_error().contains("policy"), "{}", last_error());

    let hub = start_hub(
        r#"{"listen":"127.0.0.1:0","policy":{"rules":[
            {"id":"hide-delete","action":"hide","app":"notes","tool":"delete"},
            {"id":"deny-add","action":"deny","app":"notes","tool":"add"}]}}"#,
    );
    let app = start_app(hub);
    let deadline = Instant::now() + WAIT;
    while tool_names(hub).len() < 2 {
        assert!(Instant::now() < deadline, "工具未同步");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(tool_names(hub), ["add", "hang"], "hide 的工具不在列表中");

    let (tx, rx) = mpsc::channel();
    call(hub, json!({"name":"notes.delete"}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["error"]["kind"], "TOOL_NOT_FOUND", "{o}");
    call(hub, json!({"name":"notes.add","arguments":{"text":"x"}}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["error"]["kind"], "POLICY_DENIED", "{o}");
    assert_eq!(o["result"]["error"]["details"]["ruleId"], "deny-add", "{o}");
    assert_eq!(o["result"]["error"]["details"]["hook"], "call", "{o}");

    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    assert_eq!(st["policy"]["rules"][0]["id"], "hide-delete", "{st}");
    assert_eq!(st["policy"]["rules"][0]["hits"], 1, "{st}");
    assert_eq!(st["policy"]["rules"][1]["hits"], 1, "{st}");

    // 不是合法 JSON / 未知字段 / 规则不合法（hide 不能写 hooks）：报错，旧规则继续生效
    assert_eq!(set_policy(hub, "{"), AmHubStatus::InvalidJson);
    assert_eq!(set_policy(hub, r#"{"rules":[],"x":1}"#), AmHubStatus::InvalidJson);
    let invalid = r#"{"rules":[{"id":"h","action":"hide","app":"notes","hooks":["call"]}]}"#;
    assert_eq!(set_policy(hub, invalid), AmHubStatus::InvalidConfig);
    assert!(last_error().contains("hooks"), "{}", last_error());
    call(hub, json!({"name":"notes.add","arguments":{"text":"x"}}), &tx);
    assert_eq!(recv(&rx)["result"]["error"]["kind"], "POLICY_DENIED");
    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    assert!(st["policy"]["lastError"]["message"].is_string(), "{st}");

    // 清空：恢复原行为
    assert_eq!(set_policy(hub, "{}"), AmHubStatus::Ok, "{}", last_error());
    assert_eq!(tool_names(hub), ["add", "delete", "hang"]);
    call(hub, json!({"name":"notes.add","arguments":{"text":"x"}}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["ok"]["echo"]["text"], "x", "{o}");
    // SAFETY: NULL 参数。
    assert_eq!(unsafe { am_hub_set_policy(hub, ptr::null()) }, AmHubStatus::InvalidArgument);

    app.client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}

// ---------------------------------------------------------------------------
// v11：调用进度
// ---------------------------------------------------------------------------

/// 报告两次进度（间隔超过 Hub 的合并间隔）后完成。
struct Progressing;

impl ToolHandler for Progressing {
    fn invoke(&self, call: CallHandle) {
        std::thread::spawn(move || {
            let _ = call.report_progress(1.0, Some(2.0), Some("第一步"));
            std::thread::sleep(Duration::from_millis(150));
            let _ = call.report_progress(2.0, None, None);
            std::thread::sleep(Duration::from_millis(150));
            let _ = call.complete(Some(r#"{"done":true}"#), Vec::new());
        });
    }
}

/// user_data = `*const Sender<String>`；进度以 `progress:` 前缀与结果区分。
unsafe extern "C" fn on_progress(ud: *mut c_void, json: *mut c_char) {
    // SAFETY: 测试传入的 Sender。
    let tx = unsafe { &*(ud as *const Sender<String>) };
    // SAFETY: 库分配的字符串。
    let _ = tx.send(format!("progress:{}", unsafe { take(json) }));
}

#[test]
fn call_with_progress_delivers_progress_before_result() {
    let hub = start_hub(r#"{"listen": "127.0.0.1:0", "progressIntervalMs": 10}"#);
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("steps", "步骤");
    cfg.host_url = format!("ws://{addr}/app");
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let _tool = client
        .register_tool(ToolSpec::new("run", "分步执行"), Arc::new(Progressing))
        .expect("注册");
    let _echo = client.register_tool(ToolSpec::new("echo", "回显"), Arc::new(Echo)).expect("注册");
    client.start();
    // 等工具同步完成
    let deadline = Instant::now() + WAIT;
    let filter = c(r#"{"apps":["steps"],"onlyAvailable":true,"includeBuiltin":false}"#);
    loop {
        // SAFETY: 有效参数。
        let t = query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) });
        if t.as_array().map(Vec::len) == Some(2) {
            break;
        }
        assert!(Instant::now() < deadline, "工具未同步：{t}");
        std::thread::sleep(Duration::from_millis(20));
    }

    let (tx, rx) = mpsc::channel::<String>();
    let req = c(&json!({"name": "steps.run", "arguments": {}, "callId": "p1"}).to_string());
    let mut id = ptr::null_mut();
    // SAFETY: 有效参数；tx 比回调活得久。
    let st = unsafe { am_hub_call_with_progress(hub, req.as_ptr(), Some(on_result), Some(on_progress), ud(&tx), &mut id) };
    assert_eq!(st, AmHubStatus::Ok, "{}", last_error());
    // SAFETY: 库分配的字符串。
    assert_eq!(unsafe { take(id) }, "p1");
    let mut got = Vec::new();
    while let Ok(s) = rx.recv_timeout(WAIT) {
        let done = !s.starts_with("progress:");
        got.push(s);
        if done {
            break;
        }
    }
    let (result, progress) = got.split_last().expect("收到结果");
    assert_eq!(parse(result)["result"]["ok"], json!({"done": true}), "{result}");
    let progress: Vec<Value> = progress.iter().map(|s| parse(&s["progress:".len()..])).collect();
    assert_eq!(
        progress,
        vec![
            json!({"callId": "p1", "progress": 1.0, "total": 2.0, "message": "第一步"}),
            json!({"callId": "p1", "progress": 2.0}),
        ]
    );

    // on_progress 为 NULL：等同 am_hub_call。
    let req = c(&json!({"name": "steps.echo", "arguments": {}}).to_string());
    // SAFETY: 有效参数。
    let st = unsafe { am_hub_call_with_progress(hub, req.as_ptr(), Some(on_result), None, ud(&tx), ptr::null_mut()) };
    assert_eq!(st, AmHubStatus::Ok);
    assert!(recv(&rx)["result"]["ok"].is_object());
    // 缺少 cb：参数错误。
    // SAFETY: 同上。
    let st = unsafe { am_hub_call_with_progress(hub, req.as_ptr(), None, Some(on_progress), ud(&tx), ptr::null_mut()) };
    assert_eq!(st, AmHubStatus::InvalidArgument);

    client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}


// ---------------------------------------------------------------------------
// 休眠记录持久化（stateDir）：重启后休眠的 App 仍可列出；HubStatus.dormantStore
// ---------------------------------------------------------------------------

#[test]
fn state_dir_persists_dormant_across_restart() {
    let dir = std::env::temp_dir().join(format!("app-mcp-hub-c-state-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = json!({
        "listen": "127.0.0.1:0", "listChangedDebounceMs": 20, "leaseTtlMs": 0, "stateDir": dir,
    })
    .to_string();

    // 未配置 stateDir：status 不含 dormantStore
    let plain = start_hub(r#"{"listen":null}"#);
    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(plain, o) });
    assert!(st.get("dormantStore").is_none(), "{st}");
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(plain) };

    let hub = start_hub(&config);
    let (etx, erx) = mpsc::channel::<String>();
    // SAFETY: 有效参数；Sender 归库所有。
    unsafe {
        let st = am_hub_set_event_cb(hub, Some(on_event), Box::into_raw(Box::new(etx)).cast(), Some(free_sender_quiet));
        assert_eq!(st, AmHubStatus::Ok);
    }
    let (client, _h) = start_idle_app(hub);
    assert!(wait_event(&erx, |e| e["type"] == "appDormant" && e["appId"] == "sleepy"), "应收到 appDormant");
    let deadline = Instant::now() + WAIT;
    loop {
        // SAFETY: 有效参数。
        let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
        let store = &st["dormantStore"];
        assert_eq!(store["dir"], dir.join("dormant").display().to_string(), "{st}");
        if store["writes"].as_u64().is_some_and(|w| w >= 1) {
            break;
        }
        assert!(Instant::now() < deadline, "休眠记录未写入：{st}");
        std::thread::sleep(Duration::from_millis(20));
    }
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
    client.stop();

    // 重启：从 stateDir 读回休眠实例
    let hub = start_hub(&config);
    // SAFETY: 有效参数。
    let apps = query_json(|o| unsafe { am_hub_apps_json(hub, o) });
    let app = apps
        .as_array()
        .and_then(|a| a.iter().find(|a| a["appId"] == "sleepy"))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(app["dormantInstances"][0]["instanceId"], "s1", "{apps}");
    // SAFETY: 有效参数。
    let st = query_json(|o| unsafe { am_hub_status_json(hub, o) });
    assert_eq!(st["dormantStore"]["loadedInstances"], 1, "{st}");
    assert_eq!(st["dormantStore"]["issues"], json!([]), "{st}");
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
    let _ = std::fs::remove_dir_all(&dir);
}

/// 回显 Agent 给出的幂等键（spec/hub-api.md 3.15）。
struct KeyEcho;

impl ToolHandler for KeyEcho {
    fn invoke(&self, call: CallHandle) {
        let data = json!({ "key": call.idempotency_key() });
        let _ = call.complete(Some(&data.to_string()), Vec::new());
    }
}

/// v13：配置 navigateTimeoutMs；CallRequest.idempotencyKey 原样转交 App、不合法时 INVALID_INPUT；
/// HubTool.surface / page；内置工具 apps.activate / apps.release 与（有页面目录时）apps.page / apps.navigate。
#[test]
fn surface_page_idempotency_key_and_builtins() {
    let bad = c(r#"{"listen":null,"ipcEndpoint":null,"navigateTimeoutMs":-1}"#);
    let mut out = ptr::null_mut();
    // SAFETY: 有效参数。
    assert_eq!(unsafe { am_hub_start(bad.as_ptr(), &mut out) }, AmHubStatus::InvalidJson);
    assert!(out.is_null());

    let hub = start_hub(r#"{"listen":"127.0.0.1:0","navigateTimeoutMs":800}"#);
    // SAFETY: 有效句柄。
    let addr = unsafe { take(am_hub_listen_addr(hub)) };
    let mut cfg = NativeConfig::new("shop", "商店");
    cfg.host_url = format!("ws://{addr}/app");
    let client = NativeClient::new(cfg, None).expect("创建 App");
    let view = ToolOptions {
        surface: app_mcp_native::ToolSurface::View,
        page: Some("cart".into()),
        ..ToolOptions::default()
    };
    let _view = client
        .register_tool_with(ToolSpec::new("cart.checkout", "结算"), view, Arc::new(KeyEcho))
        .expect("注册");
    let _plain = client.register_tool(ToolSpec::new("order.submit", "下单"), Arc::new(KeyEcho)).expect("注册");
    client.start();

    let deadline = Instant::now() + WAIT;
    let filter = c(r#"{"apps":["shop"],"onlyAvailable":true,"includeBuiltin":false}"#);
    let tools = loop {
        // SAFETY: 有效参数。
        let t = query_json(|o| unsafe { am_hub_tools_json(hub, filter.as_ptr(), o) });
        if t.as_array().map(Vec::len) == Some(2) {
            break t;
        }
        assert!(Instant::now() < deadline, "工具未同步：{t}");
        std::thread::sleep(Duration::from_millis(20));
    };
    let by_name = |n: &str| tools.as_array().and_then(|a| a.iter().find(|t| t["name"] == n)).cloned();
    let checkout = by_name("shop.cart.checkout").expect("view 工具");
    assert_eq!((&checkout["surface"], &checkout["page"]), (&json!("view"), &json!("cart")), "{tools}");
    let submit = by_name("shop.order.submit").expect("app 工具");
    assert_eq!(submit["surface"], "app", "{tools}");
    assert!(submit.get("page").is_none(), "无页面时不出现：{tools}");

    // SAFETY: 有效参数（filter 为 NULL = 全部，含内置工具）。
    let all = query_json(|o| unsafe { am_hub_tools_json(hub, ptr::null(), o) });
    let names: Vec<&str> = all.as_array().into_iter().flatten().filter_map(|t| t["name"].as_str()).collect();
    for n in ["apps.list", "apps.select", "apps.overview", "apps.activate", "apps.release", "apps.page", "apps.navigate"] {
        assert!(names.contains(&n), "缺少内置工具 {n}：{names:?}");
    }
    for t in all.as_array().into_iter().flatten().filter(|t| t["name"].as_str().is_some_and(|n| n.starts_with("apps."))) {
        assert!(t.get("surface").is_none() && t.get("page").is_none(), "内置工具不带 surface / page：{t}");
    }

    let (tx, rx) = mpsc::channel();
    call(hub, json!({"name":"shop.order.submit","idempotencyKey":"order-7"}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["ok"]["key"], "order-7", "{o}");
    assert!(o.get("routedTo").is_none(), "未改调时不出现：{o}");
    // v14：调用元信息（App 已连接，不经过唤醒）
    assert!(o["durationMs"].is_u64() && o["woke"] == false, "{o}");
    call(hub, json!({"name":"shop.order.submit"}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["ok"]["key"], Value::Null, "{o}");
    call(hub, json!({"name":"shop.order.submit","idempotencyKey":""}), &tx);
    let o = recv(&rx);
    assert_eq!(o["result"]["error"]["kind"], "INVALID_INPUT", "{o}");
    // v21：priority 可选字段被接受；不认识的取值 → 请求 JSON 不合法
    call(hub, json!({"name":"shop.order.submit","priority":"interactive"}), &tx);
    assert!(recv(&rx)["result"]["ok"].is_object());
    let req = c(&json!({"name":"shop.order.submit","priority":"urgent"}).to_string());
    // SAFETY: 有效参数。
    let st = unsafe { am_hub_call(hub, req.as_ptr(), Some(on_result), ud(&tx), ptr::null_mut()) };
    assert_ne!(st, AmHubStatus::Ok);

    client.stop();
    // SAFETY: 有效句柄。
    unsafe { am_hub_free(hub) };
}
