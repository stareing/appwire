//! 热路径优化的等价性（TASKS 第 10 项）：`tools/invoke` 参数移出（不重建、不深拷贝）、结果直接序列化为文本、
//! 去重表存文本、`toolsHash` 缓存，与原实现的线上字节 / 事件逐一相同。

use app_mcp_core::*;
use app_mcp_protocol::{Message, RequestId, ResourcesReadResult, ToolsInvokeResult};
use serde_json::{Value, json};

fn config() -> ClientConfig {
    ClientConfig::new("shop", "示例商城", "inst-1", ClientKind::Native)
}

fn tool(name: &str) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: format!("{name} 的描述"),
        input_schema: json!({"type": "object", "properties": {}}),
        risk: Risk::Read,
        activation: None,
        title: None,
        enabled: true,
        scope: None,
        annotations: None,
        output_schema: None,
        surface: ToolSurface::App,
        page: None,
        background_tool: None,
        concurrency: 0,
        exclusive: None,
    }
}

fn drain(c: &mut Client) -> Vec<Event> {
    std::iter::from_fn(|| c.poll_event()).collect()
}

fn send_texts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Send(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn connected(cfg: ClientConfig) -> Client {
    let mut c = Client::new(cfg);
    c.register_tool(tool("a")).unwrap();
    c.start(0);
    drain(&mut c);
    c.handle_connected(0);
    let hello: Value = serde_json::from_str(&send_texts(&drain(&mut c))[0]).unwrap();
    let reply = json!({"jsonrpc": "2.0", "id": hello["id"],
        "result": {"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "h"}});
    c.handle_message(&reply.to_string(), 0);
    drain(&mut c);
    assert_eq!(c.state(), &ConnectionState::Connected);
    c
}

/// 原实现的回复文本：`Message::result(id, to_value(ToolsInvokeResult)).to_json()`。
fn legacy_reply(id: RequestId, r: &ToolsInvokeResult) -> String {
    Message::result(id, serde_json::to_value(r).unwrap()).to_json()
}

fn invoke_text(id: Value, call_id: &str, arguments: &Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/invoke",
        "params": {"callId": call_id, "name": "a", "arguments": arguments}})
    .to_string()
}

fn sample_args() -> Value {
    json!({"z": [1, 2.5, -3, "é\"\\\n", null, true], "a": {"nested": {"k": "v"}}, "big": 18446744073709551615u64})
}

/// 结果直接序列化：所有字段齐全（含注解、pending 状态）与仅 data 两种情况，数字 / 字符串 ID，回复文本与原实现逐字节相同。
#[test]
fn invoke_reply_bytes_match_legacy_serialization() {
    let full = CallOutput {
        data: json!({"b": [1, {"y": 2, "x": 1}], "a": "文本\u{1}", "f": 0.1}),
        state_hints: vec!["cart.state".into(), "orders".into()],
        annotations: Some(ContentAnnotations {
            audience: Some(vec![Audience::User]),
            priority: Some(0.5),
            last_modified: Some("2026-10-02T00:00:00Z".into()),
        }),
        status: ResultStatus::Pending,
        state_resource: Some("job.status".into()),
        summary: Some("已提交".into()),
    };
    let plain = CallOutput { data: json!(null), ..Default::default() };
    for (id, out) in [(json!(7), &full), (json!("req-\"x\""), &full), (json!(-1), &plain)] {
        let mut c = connected(config());
        c.handle_message(&invoke_text(id.clone(), "c1", &json!({})), 0);
        drain(&mut c);
        c.complete_call("c1", Ok(out.clone()), 0).unwrap();
        let got = send_texts(&drain(&mut c));
        let rid: RequestId = serde_json::from_value(id).unwrap();
        let want = legacy_reply(
            rid,
            &ToolsInvokeResult {
                data: out.data.clone(),
                state_hints: out.state_hints.clone(),
                annotations: out.annotations.clone(),
                status: out.status,
                state_resource: out.state_resource.clone(),
                summary: out.summary.clone(),
            },
        );
        assert_eq!(got, vec![want]);
    }
}

/// 去重重放（存文本）：重复的 callId 得到与首次逐字节相同的结果（只有 ID 不同）。
#[test]
fn dedup_replay_is_byte_identical() {
    let mut c = connected(config());
    c.handle_message(&invoke_text(json!(1), "c1", &json!({})), 0);
    drain(&mut c);
    c.complete_call("c1", Ok(CallOutput { data: json!({"n": 1, "s": "x"}), ..Default::default() }), 0).unwrap();
    let first = send_texts(&drain(&mut c));
    c.handle_message(&invoke_text(json!(2), "c1", &json!({})), 0);
    let replay = send_texts(&drain(&mut c));
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0], first[0].replacen(r#"{"id":1,"#, r#"{"id":2,"#, 1));
}

/// 执行中到达的重复请求（挂在同一次执行上）与原请求得到相同结果文本。
#[test]
fn attached_waiter_gets_same_reply() {
    let mut c = connected(config());
    c.handle_message(&invoke_text(json!(1), "c1", &json!({})), 0);
    c.handle_message(&invoke_text(json!(2), "c1", &json!({})), 0);
    drain(&mut c);
    c.complete_call("c1", Ok(CallOutput { data: json!([1, 2]), ..Default::default() }), 0).unwrap();
    let texts = send_texts(&drain(&mut c));
    assert_eq!(texts.len(), 2);
    assert_eq!(texts[0], r#"{"id":2,"jsonrpc":"2.0","result":{"data":[1,2]}}"#);
    assert_eq!(texts[1], r#"{"id":1,"jsonrpc":"2.0","result":{"data":[1,2]}}"#);
}

/// 资源读取结果直接序列化，与原实现逐字节相同。
#[test]
fn resource_read_reply_bytes_match_legacy() {
    let mut c = connected(config());
    let r = c
        .register_resource(ResourceDef {
            name: "cart.state".into(),
            description: "购物车".into(),
            mime_type: Some("application/json".into()),
            scope: None,
            realtime: false,
            annotations: None,
        })
        .unwrap();
    drain(&mut c);
    let text = json!({"jsonrpc": "2.0", "id": 5, "method": "resources/read", "params": {"name": "cart.state"}});
    c.handle_message(&text.to_string(), 0);
    let read = drain(&mut c)
        .into_iter()
        .find_map(|e| match e {
            Event::ReadResource { read, resource, .. } if resource == r => Some(read),
            _ => None,
        })
        .unwrap();
    let contents = json!({"items": [{"b": 1, "a": 2}], "total": 3});
    c.complete_read(read, Ok(contents.clone())).unwrap();
    let want = Message::result(
        RequestId::Number(5),
        serde_json::to_value(ResourcesReadResult { contents, mime_type: Some("application/json".into()) }).unwrap(),
    )
    .to_json();
    assert_eq!(send_texts(&drain(&mut c)), vec![want]);
}

/// 参数从消息中移出（不经 `from_value` 重建）：各种形态的参数、缺省参数、null 的可选字段与未知字段，事件与 serde 解析的结果相同。
#[test]
fn invoke_params_are_moved_out_intact() {
    let cases = [
        (json!({"callId": "c1", "name": "a", "arguments": sample_args(), "timeoutMs": 50, "idempotencyKey": "k", "extra": 1}),
            Some(sample_args()), Some("k")),
        (json!({"callId": "c1", "name": "a"}), Some(Value::Null), None),
        (json!({"callId": "c1", "name": "a", "arguments": null, "timeoutMs": null, "idempotencyKey": null}), Some(Value::Null), None),
    ];
    for (params, args, idem) in cases {
        let mut c = connected(config());
        let text = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/invoke", "params": params});
        c.handle_message(&text.to_string(), 0);
        let ev = drain(&mut c);
        let want = Event::InvokeTool {
            call_id: "c1".into(),
            tool: ToolId(1),
            name: "a".into(),
            arguments: args.clone().unwrap(),
            idempotency_key: idem.map(str::to_owned),
        };
        assert!(ev.contains(&want), "{params}: {ev:?}");
    }
}

/// 不合法的参数仍由 serde 给出原有回复：`timeoutMs` 为小数 / 负数、`idempotencyKey` / `callId` 类型不对。
/// serde 另外接受的形态（参数写成数组，按字段顺序）照旧接受。
#[test]
fn invalid_invoke_param_types_keep_serde_errors() {
    let mut c = connected(config());
    c.handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/invoke","params":["c1","a",{"x":1}]}"#, 0);
    let ev = drain(&mut c);
    assert!(ev.iter().any(|e| matches!(e, Event::InvokeTool { call_id, arguments, .. } if call_id == "c1" && arguments == &json!({"x": 1}))), "{ev:?}");

    for params in [
        json!({"callId": "c1", "name": "a", "timeoutMs": 1.5}),
        json!({"callId": "c1", "name": "a", "timeoutMs": -1}),
        json!({"callId": "c1", "name": "a", "idempotencyKey": 7}),
        json!({"callId": 1, "name": "a"}),
    ] {
        let mut c = connected(config());
        let text = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/invoke", "params": params});
        c.handle_message(&text.to_string(), 0);
        let ev = drain(&mut c);
        assert!(!ev.iter().any(|e| matches!(e, Event::InvokeTool { .. })), "{params}");
        let reply: Value = serde_json::from_str(&send_texts(&ev)[0]).unwrap();
        let serde_err = serde_json::from_value::<app_mcp_protocol::ToolsInvokeParams>(params.clone()).unwrap_err();
        assert_eq!(reply["error"]["code"], -32602, "{params}");
        assert_eq!(reply["error"]["message"], format!("tools/invoke 的参数无效：{serde_err}"), "{params}");
    }
}

/// 其余输入保持原有回复：缺字段、重复键（后者为准）、`id` 为 null（按通知处理）、未连接。
#[test]
fn malformed_invokes_keep_generic_behaviour() {
    // 缺 callId：-32602，错误文本与通用路径相同。
    let mut c = connected(config());
    c.handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/invoke","params":{"name":"a"}}"#, 0);
    let texts = send_texts(&drain(&mut c));
    let reply: Value = serde_json::from_str(&texts[0]).unwrap();
    assert_eq!(reply["error"]["code"], -32602);
    assert!(reply["error"]["message"].as_str().unwrap().starts_with("tools/invoke 的参数无效："), "{reply}");

    // 参数对象内重复键：通用路径解析为 Value，后者为准。
    let mut c = connected(config());
    c.handle_message(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/invoke","params":{"callId":"x","callId":"y","name":"a","arguments":{}}}"#,
        0,
    );
    let ev = drain(&mut c);
    assert!(ev.iter().any(|e| matches!(e, Event::InvokeTool { call_id, .. } if call_id == "y")), "{ev:?}");

    // id 为 null：按通知处理（未知通知警告），不执行。
    let mut c = connected(config());
    c.handle_message(r#"{"jsonrpc":"2.0","id":null,"method":"tools/invoke","params":{"callId":"x","name":"a"}}"#, 0);
    let ev = drain(&mut c);
    assert!(!ev.iter().any(|e| matches!(e, Event::InvokeTool { .. })));
    assert!(ev.iter().any(|e| matches!(e, Event::Warning(w) if w.contains("未知通知"))), "{ev:?}");

    // 握手中（未连接）：UNAUTHORIZED。
    let mut c = Client::new(config());
    c.register_tool(tool("a")).unwrap();
    c.start(0);
    drain(&mut c);
    c.handle_connected(0);
    drain(&mut c);
    c.handle_message(&invoke_text(json!(9), "c1", &json!({})), 0);
    let ev = drain(&mut c);
    assert!(!ev.iter().any(|e| matches!(e, Event::InvokeTool { .. })));
    let reply: Value = serde_json::from_str(&send_texts(&ev)[0]).unwrap();
    assert_eq!(reply["id"], 9);
    assert_eq!(reply["error"]["data"]["kind"], "UNAUTHORIZED", "{reply}");
}

/// 参数移交（不再深拷贝）：排队的调用开始执行时仍拿到完整参数。
#[test]
fn queued_call_keeps_arguments_until_started() {
    let mut c = connected(config());
    c.handle_message(&invoke_text(json!(1), "c1", &json!({"first": 1})), 0);
    c.handle_message(&invoke_text(json!(2), "c2", &sample_args()), 0);
    drain(&mut c);
    assert_eq!(c.queued_call_count(), 1);
    c.complete_call("c1", Ok(CallOutput::default()), 0).unwrap();
    let ev = drain(&mut c);
    let args = ev.iter().find_map(|e| match e {
        Event::InvokeTool { call_id, arguments, .. } if call_id == "c2" => Some(arguments.clone()),
        _ => None,
    });
    assert_eq!(args, Some(sample_args()));
}

/// 休眠 → 唤醒一轮，返回 `app/sleep` 与随后 `app/hello` 中的 `toolsHash`（两处都经缓存）。
fn sleep_wake_hashes(c: &mut Client) -> (String, String) {
    let parse = |ev: &[Event], method: &str| {
        send_texts(ev)
            .iter()
            .filter_map(|t| serde_json::from_str::<Value>(t).ok())
            .find(|m| m["method"] == method)
            .expect(method)
    };
    assert!(c.sleep(0));
    let sleep = parse(&drain(c), "app/sleep");
    let accept = json!({"jsonrpc": "2.0", "id": sleep["id"], "result": {"accepted": true, "resumeToken": "r"}});
    c.handle_message(&accept.to_string(), 0);
    drain(c);
    assert_eq!(c.state(), &ConnectionState::Dormant);
    assert!(c.wake(0));
    drain(c);
    c.handle_connected(0);
    let hello = parse(&drain(c), "app/hello");
    let reply = json!({"jsonrpc": "2.0", "id": hello["id"],
        "result": {"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "h", "toolsCurrent": true}});
    c.handle_message(&reply.to_string(), 0);
    drain(c);
    assert_eq!(c.state(), &ConnectionState::Connected);
    let h = |m: &Value| m["params"]["toolsHash"].as_str().unwrap_or_default().to_owned();
    (h(&sleep), h(&hello))
}

/// `toolsHash` 缓存：每种注册变更（注册、更新、禁用 / 启用、scope 销毁、资源、注销）之后，`app/sleep` 与快速恢复的
/// `app/hello` 中的摘要都与同样操作、从不缓存的对照 Client 重新计算的值相同。
#[test]
fn tools_hash_cache_tracks_every_registry_change() {
    let mut cfg = config();
    cfg.lifecycle.mode = LifecycleMode::Idle;
    let mut c = connected(cfg.clone());
    // 对照：同样的注册操作，从不休眠，`tools_hash()` 每次重新计算。
    let mut mirror = Client::new(cfg);
    mirror.register_tool(tool("a")).unwrap();
    let a = ToolId(1);
    let res = ResourceDef { name: "r".into(), description: "资源".into(), mime_type: None, scope: None, realtime: false, annotations: None };

    let mut seen: Vec<String> = Vec::new();
    let mut verify = |c: &mut Client, mirror: &Client, what: &str| {
        let want = mirror.tools_hash();
        assert_eq!(sleep_wake_hashes(c), (want.clone(), want.clone()), "{what}");
        assert_eq!(c.tools_hash(), want, "{what}");
        // 第二轮没有变更：仍相同（缓存命中）。
        assert_eq!(sleep_wake_hashes(c), (want.clone(), want.clone()), "{what}（无变更）");
        seen.push(want);
    };
    verify(&mut c, &mirror, "初始");

    let update = || ToolUpdate { description: Some("新描述".into()), ..Default::default() };
    c.update_tool(a, update()).unwrap();
    mirror.update_tool(a, update()).unwrap();
    verify(&mut c, &mirror, "更新");

    let disable = |on| ToolUpdate { enabled: Some(on), ..Default::default() };
    c.update_tool(a, disable(false)).unwrap();
    mirror.update_tool(a, disable(false)).unwrap();
    verify(&mut c, &mirror, "禁用");
    c.update_tool(a, disable(true)).unwrap();
    mirror.update_tool(a, disable(true)).unwrap();
    verify(&mut c, &mirror, "启用");

    let (s1, s2) = (c.create_scope("s", None).unwrap(), mirror.create_scope("s", None).unwrap());
    c.register_tool(ToolDef { scope: Some(s1), ..tool("b") }).unwrap();
    mirror.register_tool(ToolDef { scope: Some(s2), ..tool("b") }).unwrap();
    verify(&mut c, &mirror, "scope 内注册");
    c.dispose_scope(s1).unwrap();
    mirror.dispose_scope(s2).unwrap();
    verify(&mut c, &mirror, "scope 销毁");

    let (r1, r2) = (c.register_resource(res.clone()).unwrap(), mirror.register_resource(res).unwrap());
    verify(&mut c, &mirror, "注册资源");
    c.unregister_resource(r1).unwrap();
    mirror.unregister_resource(r2).unwrap();
    c.unregister_tool(a).unwrap();
    mirror.unregister_tool(a).unwrap();
    verify(&mut c, &mirror, "全部注销");

    // 变化确实反映在摘要上：更新、禁用、scope 内注册、注册资源各产生新值；启用 / scope 销毁回到旧值；
    // 禁用唯一的工具与全部注销都等于空注册表（禁用的工具不同步给 Host）。
    assert_eq!(seen[3], seen[1], "启用后回到更新后的定义");
    assert_eq!(seen[5], seen[3], "scope 销毁后回到之前的定义");
    assert_eq!(seen[2], seen[7], "禁用唯一的工具 = 空注册表");
    let distinct: std::collections::BTreeSet<_> = seen.iter().collect();
    assert_eq!(distinct.len(), 5, "{seen:?}");
}
