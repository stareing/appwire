//! Client 状态机的行为测试（spec/protocol.md 第 5 节），全部用确定性时间驱动。

use app_mcp_core::*;
use app_mcp_protocol::ErrorKind;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// 测试辅助：假 Host
// ---------------------------------------------------------------------------

struct Harness {
    c: Client,
    now: Millis,
}

fn config() -> ClientConfig {
    ClientConfig::new("shop", "示例商城", "inst-1", ClientKind::Web)
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
    }
}

fn resource(name: &str) -> ResourceDef {
    ResourceDef { name: name.into(), description: format!("{name} 资源"), mime_type: None, scope: None, realtime: false, annotations: None }
}

/// 从事件中取出所有发送的消息。
fn sends(events: &[Event]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Send(s) => Some(serde_json::from_str(s).expect("sent message is json")),
            _ => None,
        })
        .collect()
}

fn methods(msgs: &[Value]) -> Vec<&str> {
    msgs.iter().map(|m| m["method"].as_str().unwrap_or("<response>")).collect()
}

fn warnings(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::Warning(_))).count()
}

impl Harness {
    fn new() -> Self {
        Self::with(config())
    }

    fn with(cfg: ClientConfig) -> Self {
        Self { c: Client::new(cfg), now: 1_000 }
    }

    fn drain(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Some(e) = self.c.poll_event() {
            out.push(e);
        }
        out
    }

    fn recv(&mut self, msg: Value) -> Vec<Event> {
        self.c.handle_message(&msg.to_string(), self.now);
        self.drain()
    }

    /// 启动并建立连接，返回 `app/hello` 请求。
    fn open(&mut self) -> Value {
        self.c.start(self.now);
        self.drain();
        self.c.handle_connected(self.now);
        let ev = self.drain();
        let msgs = sends(&ev);
        assert_eq!(msgs.len(), 1);
        msgs[0].clone()
    }

    fn hello_result(&mut self, hello: &Value, result: Value) -> Vec<Event> {
        self.recv(json!({"jsonrpc": "2.0", "id": hello["id"], "result": result}))
    }

    /// 完成握手（paired）。
    fn connect(&mut self) -> Vec<Event> {
        let hello = self.open();
        let ev = self.hello_result(
            &hello,
            json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "0.1.0"}),
        );
        assert_eq!(self.c.state(), &ConnectionState::Connected);
        ev
    }

    /// 推进时间并触发到期的定时器。
    fn advance(&mut self, ms: Millis) -> Vec<Event> {
        let target = self.now + ms;
        let mut out = Vec::new();
        while let Some(t) = self.c.poll_timeout() {
            if t > target {
                break;
            }
            self.now = self.now.max(t);
            self.c.handle_timeout(self.now);
            out.extend(self.drain());
        }
        self.now = target;
        self.c.handle_timeout(self.now);
        out.extend(self.drain());
        out
    }

    fn invoke(&mut self, id: i64, call_id: &str, name: &str, timeout_ms: Option<u64>) -> Vec<Event> {
        let mut params = json!({"callId": call_id, "name": name, "arguments": {"x": 1}});
        if let Some(t) = timeout_ms {
            params["timeoutMs"] = json!(t);
        }
        self.recv(json!({"jsonrpc": "2.0", "id": id, "method": "tools/invoke", "params": params}))
    }

    fn request(&mut self, id: i64, method: &str, params: Value) -> Vec<Event> {
        self.recv(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
    }
}

fn invoked(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::InvokeTool { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect()
}

fn cancelled(events: &[Event]) -> Vec<(String, CancelReason)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::CancelTool { call_id, reason } => Some((call_id.clone(), *reason)),
            _ => None,
        })
        .collect()
}

fn error_kind(msg: &Value) -> &str {
    msg["error"]["data"]["kind"].as_str().unwrap_or("")
}

// ---------------------------------------------------------------------------
// 握手
// ---------------------------------------------------------------------------

#[test]
fn handshake_message_order() {
    let mut h = Harness::new();
    h.c.register_tool(tool("cart.add")).unwrap();
    let mut hidden = tool("cart.hidden");
    hidden.enabled = false;
    h.c.register_tool(hidden).unwrap();
    h.c.register_resource(resource("cart.state")).unwrap();
    assert!(h.drain().is_empty(), "未连接时注册不产生事件");

    h.c.start(h.now);
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
    h.c.start(h.now);
    assert!(h.drain().is_empty(), "重复 start 无效果");

    h.c.handle_connected(h.now);
    let ev = h.drain();
    assert_eq!(ev.last(), Some(&Event::StateChanged(ConnectionState::Handshaking)));
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["app/hello"]);
    let hello = &msgs[0];
    assert_eq!(hello["params"]["appId"], "shop");
    assert_eq!(hello["params"]["protocolVersion"], "1");
    assert_eq!(hello["params"]["clientKind"], "web");
    assert_eq!(hello["params"]["instanceId"], "inst-1");
    assert!(hello["params"].get("token").is_none());
    assert!(hello["id"].is_number());

    // 握手期间的注册变更不单独发送
    h.c.register_tool(tool("late")).unwrap();
    assert!(h.drain().is_empty());

    let ev = h.hello_result(
        hello,
        json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "0.1.0"}),
    );
    assert_eq!(ev.first(), Some(&Event::Paired { token: "tk".into() }));
    assert_eq!(ev.last(), Some(&Event::StateChanged(ConnectionState::Connected)));
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["tools/sync", "resources/sync", "app/visibility", "app/ready"]);
    let names: Vec<&str> = msgs[0]["params"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["cart.add", "late"], "只同步已启用的工具，按注册顺序");
    assert_eq!(msgs[0]["params"]["tools"][0]["inputSchema"]["type"], "object");
    assert_eq!(msgs[0]["params"]["tools"][0]["risk"], "read");
    assert_eq!(msgs[1]["params"]["resources"][0]["name"], "cart.state");
    assert_eq!(msgs[2]["params"], json!({"visibility": "visible", "focused": true}));
    assert_eq!(msgs[3]["params"], json!({}));
    assert_eq!(h.c.token(), Some("tk"));
    // 已同步，无后续 changed
    assert!(h.drain().is_empty());
}

#[test]
fn existing_token_is_sent_and_not_repersisted() {
    let mut cfg = config();
    cfg.token = Some("tk".into());
    cfg.launch_token = Some("launch".into());
    let mut h = Harness::with(cfg);
    let hello = h.open();
    assert_eq!(hello["params"]["token"], "tk");
    assert_eq!(hello["params"]["launchToken"], "launch");
    let ev = h.hello_result(
        &hello,
        json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "0.1.0"}),
    );
    assert!(!ev.iter().any(|e| matches!(e, Event::Paired { .. })), "token 未变化时不通知持久化");

    // 重连时不再携带一次性 launchToken
    h.c.handle_disconnected(h.now);
    h.drain();
    h.advance(500);
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain())[0].clone();
    assert_eq!(hello["params"]["token"], "tk");
    assert!(hello["params"].get("launchToken").is_none());
}

#[test]
fn pending_then_paired() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    let hello = h.open();
    let ev = h.hello_result(&hello, json!({"status": "pending", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    assert_eq!(ev, vec![Event::StateChanged(ConnectionState::PendingPairing)]);

    // 配对等待期间的调用 → UNAUTHORIZED
    let ev = h.invoke(7, "c1", "a", None);
    let msgs = sends(&ev);
    assert_eq!(error_kind(&msgs[0]), "UNAUTHORIZED");
    assert!(invoked(&ev).is_empty());

    let ev = h.recv(json!({"jsonrpc": "2.0", "method": "app/pairingResult", "params": {"status": "paired", "token": "new"}}));
    assert_eq!(ev.first(), Some(&Event::Paired { token: "new".into() }));
    assert_eq!(methods(&sends(&ev)), vec!["tools/sync", "resources/sync", "app/visibility", "app/ready"]);
    assert_eq!(h.c.state(), &ConnectionState::Connected);
}

#[test]
fn pending_then_rejected_does_not_reconnect() {
    let mut h = Harness::new();
    let hello = h.open();
    h.hello_result(&hello, json!({"status": "pending", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    let ev = h.recv(json!({"jsonrpc": "2.0", "method": "app/pairingResult", "params": {"status": "rejected", "reason": "用户拒绝"}}));
    assert_eq!(
        ev,
        vec![Event::Disconnect, Event::StateChanged(ConnectionState::Rejected { reason: "用户拒绝".into(), code: ConnectionErrorCode::Rejected })]
    );
    assert_eq!(h.c.poll_timeout(), None);
    h.c.handle_disconnected(h.now);
    assert!(h.drain().is_empty());
    assert!(h.advance(100_000).is_empty());
    assert!(matches!(h.c.state(), ConnectionState::Rejected { .. }));
}

#[test]
fn hello_rejected_or_error_is_final() {
    let mut h = Harness::new();
    let hello = h.open();
    let ev = h.hello_result(
        &hello,
        json!({"status": "rejected", "reason": "协议版本不兼容", "protocolVersion": "2", "hostVersion": "9"}),
    );
    assert!(ev.contains(&Event::Disconnect));
    assert_eq!(
        h.c.state(),
        &ConnectionState::Rejected { reason: "协议版本不兼容".into(), code: ConnectionErrorCode::Rejected }
    );

    let mut h = Harness::new();
    let hello = h.open();
    let ev = h.recv(json!({"jsonrpc": "2.0", "id": hello["id"], "error": {"code": -32015, "message": "bad version"}}));
    assert!(ev.contains(&Event::Disconnect));
    assert!(matches!(h.c.state(), ConnectionState::Rejected { .. }));
    assert_eq!(h.c.poll_timeout(), None);
}

/// 握手结果中的 Host 身份（spec/protocol.md 1.6）。
#[test]
fn host_identity_mismatch_stops_retrying() {
    let mismatch = |h: &Harness| matches!(h.c.state(), ConnectionState::HostMismatch { .. });
    let paired = |extra: Value| {
        let mut v = json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "0.1.0"});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        v
    };

    // service 不是 app-mcp
    let mut h = Harness::new();
    let hello = h.open();
    let ev = h.hello_result(&hello, paired(json!({"service": "other"})));
    assert!(ev.contains(&Event::Disconnect));
    assert!(mismatch(&h), "{:?}", h.c.state());
    assert_eq!(h.c.state().code(), Some(ConnectionErrorCode::HostNotAppMcp));
    assert_eq!(h.c.poll_timeout(), None, "不自动重试");
    assert!(sends(&ev).is_empty(), "不发送 tools/sync");

    // 不认识 app/hello（标准 -32601）、结果无法解析：同样判定不是 app-mcp
    let mut h = Harness::new();
    let hello = h.open();
    h.recv(json!({"jsonrpc": "2.0", "id": hello["id"], "error": {"code": -32601, "message": "method not found"}}));
    assert!(mismatch(&h));
    let mut h = Harness::new();
    let hello = h.open();
    h.hello_result(&hello, json!({"hello": "world"}));
    assert!(mismatch(&h));

    // 用户不同；wake / connect_now 再试一次
    let mut cfg = config();
    cfg.expected_host_user = Some("1000".into());
    let mut h = Harness::with(cfg);
    let hello = h.open();
    h.hello_result(&hello, paired(json!({"service": "app-mcp", "user": "1001", "pid": 9})));
    let ConnectionState::HostMismatch { reason, code } = h.c.state() else { panic!("{:?}", h.c.state()) };
    assert_eq!(*code, ConnectionErrorCode::HostOtherUser);
    assert!(reason.contains("1001") && reason.contains("pid 9"), "{reason}");
    assert!(h.c.wake(h.now));
    assert!(h.drain().contains(&Event::Connect));
    assert_eq!(h.c.state(), &ConnectionState::Connecting);
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain()).remove(0);
    h.hello_result(&hello, paired(json!({"service": "app-mcp", "user": "1000"})));
    assert_eq!(h.c.state(), &ConnectionState::Connected);

    // 旧 Host 不带身份字段：无法核对，照常连接
    let mut cfg = config();
    cfg.expected_host_user = Some("1000".into());
    let mut h = Harness::with(cfg);
    let hello = h.open();
    h.hello_result(&hello, paired(json!({})));
    assert_eq!(h.c.state(), &ConnectionState::Connected);
}

// ---------------------------------------------------------------------------
// 注册变更
// ---------------------------------------------------------------------------

#[test]
fn changes_are_coalesced_and_cancel_out() {
    let mut h = Harness::new();
    let keep = h.c.register_tool(tool("keep")).unwrap();
    let old = h.c.register_tool(tool("old")).unwrap();
    h.connect();

    let a = h.c.register_tool(tool("a")).unwrap();
    let b = h.c.register_tool(tool("b")).unwrap();
    h.c.unregister_tool(b).unwrap(); // 先加后删：抵消
    h.c.update_tool(a, ToolUpdate { description: Some("新描述".into()), ..Default::default() }).unwrap();
    h.c.unregister_tool(old).unwrap();
    // 删掉再以相同内容加回：抵消
    h.c.unregister_tool(keep).unwrap();
    h.c.register_tool(tool("keep")).unwrap();
    let r = h.c.register_resource(resource("r1")).unwrap();
    let r2 = h.c.register_resource(resource("r2")).unwrap();
    h.c.unregister_resource(r2).unwrap();

    let ev = h.drain();
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["tools/changed", "resources/changed"]);
    assert_eq!(msgs[0]["params"]["upserted"].as_array().unwrap().len(), 1);
    assert_eq!(msgs[0]["params"]["upserted"][0]["name"], "a");
    assert_eq!(msgs[0]["params"]["upserted"][0]["description"], "新描述");
    assert_eq!(msgs[0]["params"]["removed"], json!(["old"]));
    assert_eq!(msgs[1]["params"]["upserted"][0]["name"], "r1");
    assert_eq!(msgs[1]["params"]["removed"], json!([]));

    // 没有新的变更就不再发送
    assert!(h.drain().is_empty());
    h.c.unregister_resource(r).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(methods(&msgs), vec!["resources/changed"]);
    assert_eq!(msgs[0]["params"]["removed"], json!(["r1"]));

    // 先加后删（已连接状态下全部在一次 poll 之前）
    let x = h.c.register_tool(tool("x")).unwrap();
    h.c.unregister_tool(x).unwrap();
    assert!(h.drain().is_empty());
}

#[test]
fn disable_and_enable_tools() {
    let mut h = Harness::new();
    let a = h.c.register_tool(tool("a")).unwrap();
    h.connect();

    h.c.update_tool(a, ToolUpdate { enabled: Some(false), ..Default::default() }).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["method"], "tools/changed");
    assert_eq!(msgs[0]["params"], json!({"upserted": [], "removed": ["a"]}));

    // 禁用期间修改内容：Host 看不到，不发送
    h.c.update_tool(a, ToolUpdate { title: Some(Some("A".into())), ..Default::default() }).unwrap();
    assert!(h.drain().is_empty());

    h.c.update_tool(a, ToolUpdate { enabled: Some(true), ..Default::default() }).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["params"]["upserted"][0]["name"], "a");
    assert_eq!(msgs[0]["params"]["upserted"][0]["title"], "A");
    assert_eq!(msgs[0]["params"]["removed"], json!([]));

    // 禁用后立即启用：抵消
    h.c.update_tool(a, ToolUpdate { enabled: Some(false), ..Default::default() }).unwrap();
    h.c.update_tool(a, ToolUpdate { enabled: Some(true), ..Default::default() }).unwrap();
    assert!(h.drain().is_empty());

    // 注册一个禁用的工具：不发送
    let mut t = tool("b");
    t.enabled = false;
    h.c.register_tool(t).unwrap();
    assert!(h.drain().is_empty());
}

/// 第 14 项 S1 / 第 19 项 R1–R3：注解与输出 schema 随工具同步、可清除；结构化结果原样回给 Host。
#[test]
fn annotations_output_schema_and_structured_result() {
    let mut h = Harness::new();
    let mut t = tool("a");
    t.annotations = Some(ToolAnnotations { open_world_hint: Some(true), ..Default::default() });
    t.output_schema = Some(json!({"type": "object"}));
    let a = h.c.register_tool(t).unwrap();
    let ev = h.connect();
    let sync = sends(&ev).into_iter().find(|m| m["method"] == "tools/sync").unwrap();
    assert_eq!(sync["params"]["tools"][0]["annotations"], json!({"openWorldHint": true}));
    assert_eq!(sync["params"]["tools"][0]["outputSchema"], json!({"type": "object"}));
    // 清除：不再序列化
    h.c.update_tool(a, ToolUpdate { annotations: Some(None), output_schema: Some(None), ..Default::default() }).unwrap();
    let msgs = sends(&h.drain());
    let upserted = &msgs[0]["params"]["upserted"][0];
    assert!(upserted.get("annotations").is_none() && upserted.get("outputSchema").is_none(), "{upserted}");

    h.invoke(10, "c1", "a", None);
    let out = CallOutput {
        status: ResultStatus::Partial,
        summary: Some("只加入了 2 件".into()),
        annotations: Some(ContentAnnotations { priority: Some(1.0), ..Default::default() }),
        ..CallOutput::default()
    };
    h.c.complete_call("c1", Ok(out), h.now).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(
        msgs[0]["result"],
        json!({"data": null, "status": "partial", "summary": "只加入了 2 件", "annotations": {"priority": 1.0}})
    );
}

#[test]
fn registration_errors() {
    let mut h = Harness::new();
    assert_eq!(h.c.register_tool(tool("a b")), Err(CoreError::InvalidName("a b".into())));
    let mut t = tool("a");
    t.input_schema = json!({"type": "string"});
    assert_eq!(h.c.register_tool(t), Err(CoreError::InvalidSchema));
    let a = h.c.register_tool(tool("a")).unwrap();
    let mut dup = tool("a");
    dup.enabled = false;
    assert_eq!(h.c.register_tool(dup), Err(CoreError::DuplicateName("a".into())));
    h.c.register_resource(resource("a")).unwrap();
    assert_eq!(h.c.register_resource(resource("a")), Err(CoreError::DuplicateName("a".into())));
    assert_eq!(
        h.c.update_tool(a, ToolUpdate { input_schema: Some(json!([])), ..Default::default() }),
        Err(CoreError::InvalidSchema)
    );
    assert_eq!(h.c.update_tool(ToolId(999), ToolUpdate::default()), Err(CoreError::UnknownTool(ToolId(999))));
    assert_eq!(h.c.unregister_tool(ToolId(999)), Err(CoreError::UnknownTool(ToolId(999))));
    assert_eq!(h.c.unregister_resource(ResourceId(999)), Err(CoreError::UnknownResource(ResourceId(999))));
    assert_eq!(
        h.c.notify_resource_changed(ResourceId(999), 0),
        Err(CoreError::UnknownResource(ResourceId(999)))
    );
    assert_eq!(h.c.create_scope("x", Some(ScopeId(999))), Err(CoreError::UnknownScope(ScopeId(999))));
    assert_eq!(h.c.dispose_scope(ScopeId(999)), Err(CoreError::UnknownScope(ScopeId(999))));
    h.c.unregister_tool(a).unwrap();
    assert!(h.c.register_tool(tool("a")).is_ok(), "注销后名称可复用");
}

#[test]
fn scope_dispose_is_recursive() {
    let mut h = Harness::new();
    let page = h.c.create_scope("page", None).unwrap();
    let dialog = h.c.create_scope("dialog", Some(page)).unwrap();
    let other = h.c.create_scope("other", None).unwrap();
    let mut t1 = tool("page.t");
    t1.scope = Some(page);
    let mut t2 = tool("dialog.t");
    t2.scope = Some(dialog);
    let mut t3 = tool("other.t");
    t3.scope = Some(other);
    let mut r = resource("dialog.r");
    r.scope = Some(dialog);
    h.c.register_tool(t1).unwrap();
    let t2 = h.c.register_tool(t2).unwrap();
    h.c.register_tool(t3).unwrap();
    h.c.register_resource(r).unwrap();
    h.connect();
    assert_eq!(h.c.scope_name(dialog), Some("dialog"));

    h.c.dispose_scope(page).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(methods(&msgs), vec!["tools/changed", "resources/changed"]);
    assert_eq!(msgs[0]["params"]["removed"], json!(["dialog.t", "page.t"]));
    assert_eq!(msgs[1]["params"]["removed"], json!(["dialog.r"]));
    assert_eq!(h.c.scope_name(dialog), None);
    assert_eq!(h.c.dispose_scope(dialog), Err(CoreError::UnknownScope(dialog)));
    assert_eq!(h.c.unregister_tool(t2), Err(CoreError::UnknownTool(t2)));
    assert!(h.c.create_scope("x", Some(page)).is_err());
    assert!(h.c.create_scope("x", Some(other)).is_ok());
}

// ---------------------------------------------------------------------------
// 调用
// ---------------------------------------------------------------------------

#[test]
fn call_success_and_handler_error() {
    let mut h = Harness::new();
    let a = h.c.register_tool(tool("a")).unwrap();
    h.connect();

    let ev = h.invoke(10, "c1", "a", Some(5_000));
    assert_eq!(
        ev,
        vec![Event::InvokeTool { call_id: "c1".into(), tool: a, name: "a".into(), arguments: json!({"x": 1}), idempotency_key: None }]
    );
    h.c.complete_call(
        "c1",
        Ok(CallOutput { data: json!({"ok": true}), state_hints: vec!["cart.state".into()], ..CallOutput::default() }),
        h.now,
    )
    .unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs, vec![json!({"jsonrpc": "2.0", "id": 10, "result": {"data": {"ok": true}, "stateHints": ["cart.state"]}})]);
    assert_eq!(h.c.poll_timeout().map(|t| t > h.now + 5_000), Some(true), "已完成调用的超时不再计时");

    // 完成后重复完成
    assert_eq!(
        h.c.complete_call("c1", Ok(CallOutput::default()), h.now),
        Err(CoreError::UnknownCall("c1".into()))
    );

    h.invoke(11, "c2", "a", None);
    let err = ToolError::new(ErrorKind::HandlerError, "库存不足").with_details(json!({"sku": "x"}));
    h.c.complete_call("c2", Err(err), h.now).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["id"], 11);
    assert_eq!(msgs[0]["error"]["code"], -32006);
    assert_eq!(msgs[0]["error"]["message"], "库存不足");
    assert_eq!(msgs[0]["error"]["data"], json!({"kind": "HANDLER_ERROR", "sku": "x"}));

    // 无返回值
    h.invoke(12, "c3", "a", None);
    h.c.complete_call("c3", Ok(CallOutput::default()), h.now).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["result"], json!({"data": null}));
}

#[test]
fn tool_not_found_and_disabled() {
    let mut h = Harness::new();
    let mut t = tool("off");
    t.enabled = false;
    h.c.register_tool(t).unwrap();
    h.connect();

    let ev = h.invoke(1, "c1", "missing", None);
    assert!(invoked(&ev).is_empty());
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["id"], 1);
    assert_eq!(msgs[0]["error"]["code"], -32001);
    assert_eq!(error_kind(&msgs[0]), "TOOL_NOT_FOUND");

    let ev = h.invoke(2, "c2", "off", None);
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["error"]["code"], -32002);
    assert_eq!(error_kind(&msgs[0]), "TOOL_DISABLED");
}

#[test]
fn calls_are_serialized_by_default() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();

    assert_eq!(invoked(&h.invoke(1, "c1", "a", None)), vec!["c1"]);
    assert!(h.invoke(2, "c2", "a", None).is_empty());
    assert!(h.invoke(3, "c3", "a", None).is_empty());
    assert_eq!((h.c.running_call_count(), h.c.queued_call_count()), (1, 2));

    // 排队中的调用不能提前完成
    assert!(h.c.complete_call("c2", Ok(CallOutput::default()), h.now).is_err());

    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    let ev = h.drain();
    assert_eq!(sends(&ev)[0]["id"], 1);
    assert_eq!(invoked(&ev), vec!["c2"]);
    h.c.complete_call("c2", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c3"]);
}

#[test]
fn max_concurrent_calls() {
    let mut cfg = config();
    cfg.max_concurrent_calls = 2;
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.invoke(2, "c2", "a", None);
    assert!(h.invoke(3, "c3", "a", None).is_empty());
    assert_eq!(h.c.running_call_count(), 2);
    h.c.complete_call("c2", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c3"]);
}

#[test]
fn timeout_includes_queue_time() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    let start = h.now;
    h.invoke(1, "c1", "a", None);
    h.advance(400);
    h.invoke(2, "c2", "a", Some(1_000));
    assert_eq!(h.c.poll_timeout(), Some(start + 1_400));

    let ev = h.advance(1_000);
    assert!(cancelled(&ev).is_empty(), "排队中的调用从未启动，不需要中止 handler");
    let msgs = sends(&ev);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["id"], 2);
    assert_eq!(error_kind(&msgs[0]), "TIMEOUT");
    assert_eq!(h.c.queued_call_count(), 0);
    assert_eq!(h.c.running_call_count(), 1);

    // 第一个调用照常完成
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    assert!(invoked(&h.drain()).is_empty());
}

#[test]
fn running_call_timeout() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", Some(3_000));
    h.invoke(2, "c2", "a", None);
    assert!(h.advance(2_999).iter().all(|e| !matches!(e, Event::CancelTool { .. })));
    let ev = h.advance(1);
    assert_eq!(cancelled(&ev), vec![("c1".into(), CancelReason::Timeout)]);
    let msgs = sends(&ev);
    assert_eq!(error_kind(&msgs[0]), "TIMEOUT");
    assert_eq!(msgs[0]["error"]["code"], -32005);
    assert_eq!(invoked(&ev), vec!["c2"], "超时释放并发名额");

    assert_eq!(
        h.c.complete_call("c1", Ok(CallOutput::default()), h.now),
        Err(CoreError::UnknownCall("c1".into()))
    );
    assert!(h.drain().is_empty(), "超时后的完成结果不发送");
}

#[test]
fn cancel_running_and_queued() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.invoke(2, "c2", "a", None);
    h.invoke(3, "c3", "a", None);

    // 取消排队中的调用：直接移出
    let ev = h.recv(json!({"jsonrpc": "2.0", "method": "tools/cancel", "params": {"callId": "c2"}}));
    assert!(cancelled(&ev).is_empty());
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["id"], 2);
    assert_eq!(error_kind(&msgs[0]), "CANCELLED");

    // 取消进行中的调用
    let ev = h.recv(json!({"jsonrpc": "2.0", "method": "tools/cancel", "params": {"callId": "c1", "reason": "用户中止"}}));
    assert_eq!(cancelled(&ev), vec![("c1".into(), CancelReason::Requested)]);
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["id"], 1);
    assert_eq!(msgs[0]["error"]["code"], -32007);
    assert_eq!(invoked(&ev), vec!["c3"]);

    assert_eq!(
        h.c.complete_call("c1", Ok(CallOutput::default()), h.now),
        Err(CoreError::UnknownCall("c1".into()))
    );
    assert!(h.drain().is_empty());

    // 未知调用的取消：警告
    let ev = h.recv(json!({"jsonrpc": "2.0", "method": "tools/cancel", "params": {"callId": "nope"}}));
    assert_eq!(warnings(&ev), 1);
    assert!(sends(&ev).is_empty());
}

#[test]
fn unregistered_tool_calls_still_complete() {
    let mut h = Harness::new();
    let a = h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.invoke(2, "c2", "a", None);
    h.c.unregister_tool(a).unwrap();
    h.drain();
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    let ev = h.drain();
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["id"], 1);
    assert!(msgs[0].get("result").is_some());
    // 排队中的调用在启动时发现工具已注销
    assert_eq!(msgs[1]["id"], 2);
    assert_eq!(error_kind(&msgs[1]), "TOOL_NOT_FOUND");
    assert!(invoked(&ev).is_empty());
}

#[test]
fn disconnect_cancels_all_calls() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.c.register_resource(resource("r")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", Some(10_000));
    h.invoke(2, "c2", "a", None);
    let ev = h.request(3, "resources/read", json!({"name": "r"}));
    let read = match &ev[0] {
        Event::ReadResource { read, .. } => *read,
        other => panic!("unexpected {other:?}"),
    };

    h.c.handle_disconnected(h.now);
    let ev = h.drain();
    assert!(sends(&ev).is_empty(), "断线时不发送任何响应");
    assert_eq!(cancelled(&ev), vec![("c1".into(), CancelReason::Disconnected)]);
    assert_eq!(
        ev.last(),
        Some(&Event::StateChanged(ConnectionState::Backoff { retry_at: h.now + 500, reason: None, code: None }))
    );
    assert_eq!((h.c.running_call_count(), h.c.queued_call_count()), (0, 0));
    assert_eq!(
        h.c.complete_call("c1", Ok(CallOutput::default()), h.now),
        Err(CoreError::UnknownCall("c1".into()))
    );
    assert_eq!(h.c.complete_read(read, Ok(json!(1))), Err(CoreError::UnknownRead(read)));
    assert!(h.drain().is_empty());
    assert_eq!(h.c.poll_timeout(), Some(h.now + 500), "只剩重连定时器");
}

#[test]
fn stop_cancels_and_stays_stopped() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.c.stop(h.now);
    let ev = h.drain();
    assert_eq!(cancelled(&ev), vec![("c1".into(), CancelReason::Stopped)]);
    assert!(ev.contains(&Event::Disconnect));
    assert_eq!(ev.last(), Some(&Event::StateChanged(ConnectionState::Stopped)));
    assert_eq!(h.c.poll_timeout(), None);
    h.c.handle_disconnected(h.now);
    h.c.start(h.now);
    assert!(h.drain().is_empty());
    assert_eq!(h.c.state(), &ConnectionState::Stopped);

    // 未连接时停止不产生 Disconnect
    let mut h = Harness::new();
    h.c.stop(h.now);
    assert_eq!(h.drain(), vec![Event::StateChanged(ConnectionState::Stopped)]);
}

#[test]
fn duplicate_call_id_attaches_or_is_rejected_when_dedup_off() {
    // 去重开启（默认）：执行中的重复请求挂到同一次执行上，完成时两者得到同一结果
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    let ev = h.invoke(2, "c1", "a", None);
    assert!(sends(&ev).is_empty() && invoked(&ev).is_empty(), "不执行、不立即回复：{ev:?}");
    assert_eq!(warnings(&ev), 1, "命中记一条警告日志");
    h.c.complete_call("c1", Ok(CallOutput { data: json!(5), ..CallOutput::default() }), h.now).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs.iter().map(|m| m["id"].clone()).collect::<Vec<_>>(), vec![json!(2), json!(1)]);
    assert!(msgs.iter().all(|m| m["result"] == json!({"data": 5})));

    // 排队中的重复请求同样挂接
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.invoke(2, "c2", "a", None);
    let ev = h.invoke(3, "c2", "a", None);
    assert!(sends(&ev).is_empty() && invoked(&ev).is_empty(), "{ev:?}");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c2".to_owned()], "c2 只执行一次");

    // 关闭去重：旧行为
    let mut cfg = config();
    cfg.call_dedup = CallDedupPolicy::OFF;
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    let msgs = sends(&h.invoke(2, "c1", "a", None));
    assert_eq!(msgs[0]["id"], 2);
    assert_eq!(msgs[0]["error"]["code"], -32602);
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(invoked(&h.invoke(3, "c1", "a", None)), vec!["c1".to_owned()], "关闭时完成后再次执行");
}

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

// ---------------------------------------------------------------------------
// 心跳
// ---------------------------------------------------------------------------

fn find_ping(events: &[Event]) -> Option<Value> {
    sends(events).into_iter().find(|m| m["method"] == "ping")
}

#[test]
fn heartbeat_ping_and_response() {
    let mut h = Harness::new();
    h.connect();
    let t0 = h.now;
    assert_eq!(h.c.poll_timeout(), Some(t0 + 15_000));
    assert!(find_ping(&h.advance(14_999)).is_none());
    let ping = find_ping(&h.advance(1)).expect("ping sent");
    assert!(ping["id"].is_number());
    assert!(ping.get("params").is_none());
    assert_eq!(h.c.poll_timeout(), Some(t0 + 25_000), "等待响应超时");

    // 其他消息不会重置心跳
    h.now += 5_000;
    h.recv(json!({"jsonrpc": "2.0", "id": "h1", "method": "ping"}));
    assert_eq!(h.c.poll_timeout(), Some(t0 + 25_000));

    let ev = h.recv(json!({"jsonrpc": "2.0", "id": ping["id"], "result": {}}));
    assert!(ev.is_empty());
    assert_eq!(h.c.poll_timeout(), Some(t0 + 30_000));
    assert!(find_ping(&h.advance(10_000)).is_some());
    assert_eq!(h.c.state(), &ConnectionState::Connected);
}

#[test]
fn heartbeat_timeout_visible() {
    let mut h = Harness::new();
    h.connect();
    h.advance(15_000);
    assert!(h.advance(9_999).iter().all(|e| *e != Event::Disconnect));
    let ev = h.advance(1);
    assert_eq!(warnings(&ev), 1);
    assert!(ev.contains(&Event::Disconnect));
    let Some(Event::StateChanged(ConnectionState::Backoff { retry_at, code, .. })) = ev.last() else {
        panic!("{ev:?}")
    };
    assert_eq!((*retry_at, *code), (h.now + 500, Some(ConnectionErrorCode::HeartbeatTimeout)));

    // 核心自行完成断开处理，随后按退避重连
    let ev = h.advance(500);
    assert_eq!(ev, vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
}

#[test]
fn heartbeat_timeout_hidden_is_longer() {
    let mut h = Harness::new();
    h.connect();
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    let msgs = sends(&h.drain());
    assert_eq!(msgs[0]["method"], "app/visibility");
    assert_eq!(msgs[0]["params"], json!({"visibility": "hidden", "focused": false}));
    // 值未变化：不发送
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    assert!(h.drain().is_empty());

    h.advance(15_000);
    assert!(!h.advance(10_000).contains(&Event::Disconnect));
    assert!(!h.advance(109_999).contains(&Event::Disconnect));
    assert!(h.advance(1).contains(&Event::Disconnect));
}

#[test]
fn visibility_before_connect_is_reported_in_handshake() {
    let mut h = Harness::new();
    h.c.set_visibility(Visibility::Frozen, false, h.now);
    assert!(h.drain().is_empty());
    let ev = h.connect();
    let vis = sends(&ev).into_iter().find(|m| m["method"] == "app/visibility").unwrap();
    assert_eq!(vis["params"], json!({"visibility": "frozen", "focused": false}));
}

#[test]
fn host_ping_and_activate() {
    let mut h = Harness::new();
    let hello = h.open();
    // 握手期间也响应 ping
    let msgs = sends(&h.request(1, "ping", Value::Null));
    assert_eq!(msgs[0], json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
    h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));

    let ev = h.request(2, "app/activate", json!({"mode": "foreground"}));
    assert_eq!(sends(&ev)[0], json!({"jsonrpc": "2.0", "id": 2, "result": {}}));
    assert_eq!(warnings(&ev), 1);
    let msgs = sends(&h.request(3, "app/activate", json!({"mode": "sideways"})));
    assert_eq!(msgs[0]["error"]["code"], -32602);
}

// ---------------------------------------------------------------------------
// 重连
// ---------------------------------------------------------------------------

fn retry_delay(h: &mut Harness) -> Millis {
    match h.c.state() {
        ConnectionState::Backoff { retry_at, .. } => retry_at - h.now,
        other => panic!("not in backoff: {other:?}"),
    }
}

#[test]
fn exponential_backoff_and_reset() {
    let mut h = Harness::new();
    h.c.start(h.now);
    h.drain();
    let mut delays = Vec::new();
    for _ in 0..9 {
        // Connecting 状态下连接失败
        h.c.handle_disconnected(h.now);
        h.drain();
        let d = retry_delay(&mut h);
        delays.push(d);
        assert_eq!(h.c.poll_timeout(), Some(h.now + d));
        let ev = h.advance(d);
        assert_eq!(ev, vec![Event::Connect, Event::StateChanged(ConnectionState::Connecting)]);
    }
    assert_eq!(delays, vec![500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000, 30_000]);

    // 握手中断线也继续累加
    h.c.handle_connected(h.now);
    h.drain();
    h.c.handle_disconnected(h.now);
    h.drain();
    assert_eq!(retry_delay(&mut h), 30_000);
    h.advance(30_000);

    // 成功握手后清零
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain())[0].clone();
    h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    assert_eq!(h.c.state(), &ConnectionState::Connected);
    h.c.handle_disconnected(h.now);
    h.drain();
    assert_eq!(retry_delay(&mut h), 500);
}

#[test]
fn reconnect_resyncs_full_state() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.c.handle_disconnected(h.now);
    // 断线期间的变更
    h.c.register_tool(tool("b")).unwrap();
    let ev = h.drain();
    assert!(sends(&ev).is_empty());
    h.advance(500);
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain())[0].clone();
    let ev = h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["tools/sync", "resources/sync", "app/visibility", "app/ready"]);
    assert_eq!(msgs[0]["params"]["tools"].as_array().unwrap().len(), 2);
    assert!(h.drain().is_empty());
}

// ---------------------------------------------------------------------------
// 异常输入
// ---------------------------------------------------------------------------

#[test]
fn malformed_messages_warn_without_panicking() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    for text in [
        "not json",
        "[]",
        "{}",
        r#"{"jsonrpc":"2.0"}"#,
        r#"{"jsonrpc":"2.0","id":1}"#,
        r#"{"jsonrpc":"2.0","method":5}"#,
        r#"{"jsonrpc":"2.0","id":999,"result":{}}"#,
        r#"{"jsonrpc":"2.0","id":"x","result":{}}"#,
        r#"{"jsonrpc":"2.0","method":"tools/cancel","params":42}"#,
        r#"{"jsonrpc":"2.0","method":"some/notification"}"#,
        r#"{"jsonrpc":"2.0","method":"app/pairingResult","params":{"status":"paired"}}"#,
        "\u{0}\u{ffff}",
    ] {
        h.c.handle_message(text, h.now);
        let ev = h.drain();
        assert_eq!(warnings(&ev), 1, "{text}");
        assert!(sends(&ev).is_empty(), "{text}");
    }
    assert_eq!(h.c.state(), &ConnectionState::Connected);

    // 参数错误的请求 → -32602
    for params in [json!(null), json!({"name": "a"}), json!({"callId": 1, "name": "a"}), json!([1, 2])] {
        let msgs = sends(&h.request(9, "tools/invoke", params));
        assert_eq!(msgs[0]["error"]["code"], -32602);
        assert_eq!(msgs[0]["id"], 9);
    }
    let msgs = sends(&h.request(10, "resources/read", json!({})));
    assert_eq!(msgs[0]["error"]["code"], -32602);
    assert_eq!(h.c.running_call_count(), 0);
}

#[test]
fn unknown_method_returns_method_not_found() {
    let mut h = Harness::new();
    h.connect();
    let ev = h.request(42, "tools/frobnicate", json!({}));
    let msgs = sends(&ev);
    assert_eq!(msgs[0]["id"], 42);
    assert_eq!(msgs[0]["error"]["code"], -32601);

    // 握手期间同样
    let mut h = Harness::new();
    h.open();
    let msgs = sends(&h.recv(json!({"jsonrpc": "2.0", "id": "s", "method": "nope"})));
    assert_eq!(msgs[0], json!({"jsonrpc": "2.0", "id": "s", "error": {"code": -32601, "message": "method not found: nope"}}));
}

#[test]
fn messages_while_disconnected_are_ignored() {
    let mut h = Harness::new();
    h.c.handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, 0);
    let ev = h.drain();
    assert_eq!(warnings(&ev), 1);
    assert!(sends(&ev).is_empty());
    h.c.handle_connected(0);
    assert_eq!(warnings(&h.drain()), 1, "未请求连接时的 handle_connected 被忽略");
    h.c.handle_timeout(1_000_000);
    assert!(h.drain().is_empty());
    assert_eq!(h.c.state(), &ConnectionState::Idle);
}

#[test]
fn tool_name_with_app_id_prefix_registers_but_warns() {
    let mut h = Harness::new();
    h.c.register_tool(tool("cart.add")).unwrap();
    assert_eq!(warnings(&h.drain()), 0, "含 . 的局部名是正常的");
    h.c.register_tool(tool("shop.info")).unwrap();
    let ev = h.drain();
    let msg = ev
        .iter()
        .find_map(|e| match e {
            Event::Warning(w) => Some(w.clone()),
            _ => None,
        })
        .expect("以 appId. 开头应产生警告");
    assert!(msg.contains("shop.shop.info") && msg.contains("\"info\""), "{msg}");
    // 协议语义不变：按原样注册，同步时名称不改写。
    let ev = h.connect();
    let sync = sends(&ev).into_iter().find(|m| m["method"] == "tools/sync").unwrap();
    let names: Vec<&str> =
        sync["params"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"shop.info"), "{names:?}");
}

// ---------------------------------------------------------------------------
// 诊断：错误码、连接 ID、app/diagnostic（spec/protocol.md 第 10 节）
// ---------------------------------------------------------------------------

#[test]
fn reject_codes_come_from_host() {
    let mut h = Harness::new();
    let hello = h.open();
    h.hello_result(
        &hello,
        json!({"status": "rejected", "reason": "来源不允许", "code": "ORIGIN_NOT_ALLOWED", "protocolVersion": "1", "hostVersion": "0.1.0"}),
    );
    assert_eq!(
        h.c.state(),
        &ConnectionState::Rejected { reason: "来源不允许".into(), code: ConnectionErrorCode::OriginNotAllowed }
    );

    // 不认识的码（新版本）→ REJECTED
    let mut h = Harness::new();
    let hello = h.open();
    h.hello_result(&hello, json!({"status": "pending", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    h.recv(json!({"jsonrpc": "2.0", "method": "app/pairingResult", "params": {"status": "rejected", "code": "SOMETHING_NEW"}}));
    assert_eq!(h.c.state().code(), Some(ConnectionErrorCode::Rejected));

    let mut h = Harness::new();
    let hello = h.open();
    h.hello_result(&hello, json!({"status": "pending", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    h.recv(json!({"jsonrpc": "2.0", "method": "app/pairingResult", "params": {"status": "rejected", "reason": "拒绝", "code": "PAIRING_REJECTED"}}));
    assert_eq!(h.c.state().code(), Some(ConnectionErrorCode::PairingRejected));
    assert_eq!(h.c.state().reason(), Some("拒绝"));
}

#[test]
fn connect_failure_and_handshake_timeout_carry_codes() {
    let mut h = Harness::new();
    h.c.start(h.now);
    h.drain();
    h.c.handle_connect_failed(
        ConnectionIssue::new(ConnectionErrorCode::HostNotRunning, "连接 unix:/x 失败：No such file"),
        h.now,
    );
    let ev = h.drain();
    assert_eq!(
        ev.last(),
        Some(&Event::StateChanged(ConnectionState::Backoff {
            retry_at: h.now + 500,
            reason: Some("连接 unix:/x 失败：No such file".into()),
            code: Some(ConnectionErrorCode::HostNotRunning),
        }))
    );

    // 握手超时
    let mut h = Harness::new();
    h.open();
    let ev = h.advance(10_000);
    assert!(ev.contains(&Event::Disconnect));
    assert_eq!(h.c.state().code(), Some(ConnectionErrorCode::HandshakeTimeout));
}

#[test]
fn disconnect_with_issue_carries_code() {
    let mut h = Harness::new();
    h.connect();
    h.c.handle_disconnected_with(ConnectionIssue::new(ConnectionErrorCode::ConnectionClosed, "Host 关闭了连接"), h.now);
    let ev = h.drain();
    assert_eq!(
        ev.last(),
        Some(&Event::StateChanged(ConnectionState::Backoff {
            retry_at: h.now + 500,
            reason: Some("Host 关闭了连接".into()),
            code: Some(ConnectionErrorCode::ConnectionClosed),
        }))
    );
    // 已在 Backoff：再次报告断开不改变状态
    h.c.handle_disconnected_with(ConnectionIssue::new(ConnectionErrorCode::ConnectionLost, "x"), h.now);
    assert!(h.drain().is_empty());
    assert_eq!(h.c.state().code(), Some(ConnectionErrorCode::ConnectionClosed));

    // 无原因的断开仍不带码（向后兼容）
    let mut h = Harness::new();
    h.connect();
    h.c.handle_disconnected(h.now);
    assert_eq!(h.c.state().code(), None);
}

#[test]
fn connection_id_from_hello() {
    let mut h = Harness::new();
    let hello = h.open();
    assert_eq!(h.c.connection_id(), None);
    h.hello_result(
        &hello,
        json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "0.1.0", "connectionId": "3f9a1c-12"}),
    );
    assert_eq!(h.c.connection_id(), Some("3f9a1c-12"));
    h.c.handle_disconnected(h.now);
    assert_eq!(h.c.connection_id(), None, "断开后清除");
    // 旧 Host 不带
    let mut h = Harness::new();
    h.connect();
    assert_eq!(h.c.connection_id(), None);
}

#[test]
fn reported_issues_are_sent_after_handshake() {
    let mut h = Harness::new();
    h.c.report_issue("BLOCKED_CSP", "CSP 不允许");
    h.c.report_issue("BLOCKED_LOCAL_NETWORK_ACCESS", "LNA 1");
    h.c.report_issue("BLOCKED_LOCAL_NETWORK_ACCESS", "LNA 2");
    let ev = h.connect();
    let msgs = sends(&ev);
    assert_eq!(
        methods(&msgs),
        vec!["tools/sync", "resources/sync", "app/visibility", "app/ready", "app/diagnostic", "app/diagnostic"]
    );
    assert_eq!(msgs[4]["params"], json!({"code": "BLOCKED_CSP", "message": "CSP 不允许", "count": 1}));
    assert_eq!(msgs[5]["params"], json!({"code": "BLOCKED_LOCAL_NETWORK_ACCESS", "message": "LNA 2", "count": 2}));

    // 已上报的不再发送；已连接时直接发出
    h.c.handle_disconnected(h.now);
    h.drain();
    let mut h2 = Harness::new();
    h2.connect();
    h2.c.report_issue("BLOCKED_CSP", "x");
    assert_eq!(methods(&sends(&h2.drain())), vec!["app/diagnostic"]);

    // 不同的码最多积累 MAX_PENDING_DIAGNOSTICS 个
    let mut h3 = Harness::new();
    for i in 0..(MAX_PENDING_DIAGNOSTICS + 5) {
        h3.c.report_issue(&format!("CODE_{i}"), "m");
    }
    let ev = h3.connect();
    let n = methods(&sends(&ev)).iter().filter(|m| **m == "app/diagnostic").count();
    assert_eq!(n, MAX_PENDING_DIAGNOSTICS);
}

// ---------------------------------------------------------------------------
// 调用去重（spec/protocol.md 3.3，第 16 项 U1 / N7a）
// ---------------------------------------------------------------------------

/// U1 复现：handler 已完成、结果还没发出连接就断了（结果随连接丢弃，Host 只能报"结果未知"）；
/// 同一 callId 在回连后再次到达时不能再执行一次，而是重放首次结果。
#[test]
fn u1_same_call_id_after_reconnect_runs_once() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    assert_eq!(invoked(&h.invoke(1, "c1", "a", None)), vec!["c1".to_owned()]);
    h.c.complete_call("c1", Ok(CallOutput { data: json!({"orderId": "o1"}), ..CallOutput::default() }), h.now).unwrap();
    // 驱动层还没取走响应，连接已断：响应被丢弃
    h.c.handle_disconnected(h.now);
    let ev = h.drain();
    assert!(sends(&ev).is_empty(), "断线时丢弃未发出的响应");

    h.advance(500);
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain())[0].clone();
    h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    let ev = h.invoke(7, "c1", "a", None);
    assert!(invoked(&ev).is_empty(), "同一 callId 不再执行 handler");
    let msgs = sends(&ev);
    assert_eq!(msgs, vec![json!({"jsonrpc": "2.0", "id": 7, "result": {"data": {"orderId": "o1"}}})]);
}

/// 执行中断线：首次执行被中断（handler 已收到取消），回连后同一 callId 得到"中断、结果未知"，不再执行。
#[test]
fn interrupted_call_replays_interruption() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.c.handle_disconnected(h.now);
    assert_eq!(cancelled(&h.drain()), vec![("c1".into(), CancelReason::Disconnected)]);
    h.advance(500);
    h.c.handle_connected(h.now);
    let hello = sends(&h.drain())[0].clone();
    h.hello_result(&hello, json!({"status": "paired", "protocolVersion": "1", "hostVersion": "0.1.0"}));
    let ev = h.invoke(2, "c1", "a", None);
    assert!(invoked(&ev).is_empty());
    let msgs = sends(&ev);
    assert_eq!(error_kind(&msgs[0]), "CANCELLED");
    assert_eq!(msgs[0]["error"]["data"]["interrupted"], true);
    assert_eq!(msgs[0]["error"]["data"]["callId"], "c1");
}

/// 已开始执行的超时 / 取消记为首次结果；排队中被取消、超时的调用 handler 没执行过，同一 callId 可以重新执行。
#[test]
fn dedup_records_only_started_calls() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "run", "a", Some(100));
    h.invoke(2, "queued", "a", None);
    h.recv(json!({"jsonrpc": "2.0", "method": "tools/cancel", "params": {"callId": "queued"}}));
    h.advance(100);
    assert_eq!(invoked(&h.drain()), Vec::<String>::new());
    let ev = h.invoke(3, "run", "a", None);
    assert!(invoked(&ev).is_empty());
    assert_eq!(error_kind(&sends(&ev)[0]), "TIMEOUT", "超时结果被重放");
    assert_eq!(warnings(&ev), 1, "重放记一条警告日志");
    assert_eq!(invoked(&h.invoke(4, "queued", "a", None)), vec!["queued".to_owned()], "排队中取消的可重新执行");
    h.recv(json!({"jsonrpc": "2.0", "method": "tools/cancel", "params": {"callId": "queued"}}));
    assert_eq!(error_kind(&sends(&h.invoke(5, "queued", "a", None))[0]), "CANCELLED", "执行中取消记为首次结果");
}

/// Agent 幂等键（spec/protocol.md 3.3，第 4f 项 j）：原样进入 `InvokeTool`；同一工具的同一幂等键以新 callId 到达时——
/// 执行中挂到同一次执行、完成后重放首次结果；其他工具或没有键的调用照常执行。去重关闭时只透传。
#[test]
fn idempotency_key_passthrough_and_dedup() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.c.register_tool(tool("b")).unwrap();
    h.connect();
    let invoke_keyed = |h: &mut Harness, id: i64, call: &str, name: &str, key: &str| {
        h.request(id, "tools/invoke", json!({"callId": call, "name": name, "arguments": {}, "idempotencyKey": key}))
    };
    let ev = invoke_keyed(&mut h, 1, "c1", "a", "k1");
    let keys: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            Event::InvokeTool { idempotency_key, .. } => Some(idempotency_key.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(keys, vec![Some("k1".to_owned())], "幂等键原样交给 handler");
    let ev = invoke_keyed(&mut h, 2, "c2", "a", "k1");
    assert!(invoked(&ev).is_empty(), "执行中：挂到同一次执行");
    assert_eq!(warnings(&ev), 1);
    h.c.complete_call("c1", Ok(CallOutput { data: json!({"n": 1}), ..CallOutput::default() }), h.now).unwrap();
    let msgs = sends(&h.drain());
    assert_eq!(msgs.len(), 2, "两个请求都收到首次结果");
    assert!(msgs.iter().all(|m| m["result"]["data"] == json!({"n": 1})));
    let ev = invoke_keyed(&mut h, 3, "c3", "a", "k1");
    assert!(invoked(&ev).is_empty(), "完成后：重放首次结果");
    assert_eq!(sends(&ev)[0]["result"]["data"], json!({"n": 1}));
    assert_eq!(invoked(&invoke_keyed(&mut h, 4, "c4", "b", "k1")), vec!["c4".to_owned()], "其他工具不受影响");
    h.c.complete_call("c4", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(invoked(&h.invoke(5, "c5", "a", None)), vec!["c5".to_owned()], "没有键时只按 callId");

    let mut cfg = config();
    cfg.call_dedup = CallDedupPolicy::OFF;
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    assert_eq!(invoked(&invoke_keyed(&mut h, 1, "c1", "a", "k")), vec!["c1".to_owned()]);
    assert_eq!(invoked(&invoke_keyed(&mut h, 2, "c2", "a", "k")), Vec::<String>::new(), "并发上限 1：排队");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    assert_eq!(invoked(&h.drain()), vec!["c2".to_owned()], "去重关闭时同一幂等键照常执行");
}

#[test]
fn dedup_expires_and_is_bounded() {
    let mut cfg = config();
    cfg.call_dedup = CallDedupPolicy { ttl_ms: 1_000, max_entries: 1 };
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    for (id, call) in [(1, "c1"), (2, "c2")] {
        h.invoke(id, call, "a", None);
        h.c.complete_call(call, Ok(CallOutput::default()), h.now).unwrap();
        h.drain();
    }
    assert_eq!(invoked(&h.invoke(3, "c1", "a", None)), vec!["c1".to_owned()], "超出条数上限的最早记录被淘汰");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert!(invoked(&h.invoke(4, "c1", "a", None)).is_empty());
    h.advance(1_000);
    assert_eq!(invoked(&h.invoke(5, "c1", "a", None)), vec!["c1".to_owned()], "过期后重新执行");
}

// ---------------------------------------------------------------------------
// 进度（spec/protocol.md 3.3，第 16 项 O2）
// ---------------------------------------------------------------------------

#[test]
fn progress_is_sent_for_running_calls_only() {
    let mut h = Harness::new();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke(1, "c1", "a", None);
    h.invoke(2, "c2", "a", None);
    h.c.report_progress("c1", 1.0, Some(4.0), Some("第 1 步".into()), h.now).unwrap();
    h.c.report_progress("c1", 2.0, Some(f64::NAN), None, h.now).unwrap();
    let ev = h.drain();
    assert_eq!(
        sends(&ev),
        vec![
            json!({"jsonrpc": "2.0", "method": "tools/progress", "params": {"callId": "c1", "progress": 1.0, "total": 4.0, "message": "第 1 步"}}),
            json!({"jsonrpc": "2.0", "method": "tools/progress", "params": {"callId": "c1", "progress": 2.0}}),
        ]
    );
    h.c.report_progress("c1", f64::INFINITY, None, None, h.now).unwrap();
    let ev = h.drain();
    assert!(sends(&ev).is_empty());
    assert_eq!(warnings(&ev), 1);
    assert_eq!(h.c.report_progress("c2", 1.0, None, None, h.now), Err(CoreError::UnknownCall("c2".into())), "排队中");
    assert_eq!(h.c.report_progress("x", 1.0, None, None, h.now), Err(CoreError::UnknownCall("x".into())));
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(h.c.report_progress("c1", 3.0, None, None, h.now), Err(CoreError::UnknownCall("c1".into())), "已完成");
}

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
