//! 连接会话：握手、目录同步、按参数执行操作、休眠与唤醒回连。

use super::*;

impl Catalog {
    fn apply(&mut self, method_name: &str, params: Value) -> Result<(), String> {
        let bad = |e: serde_json::Error| format!("{method_name} 参数无法解析：{e}");
        match method_name {
            method::TOOLS_SYNC => {
                let p: ToolsSyncParams = serde_json::from_value(params).map_err(bad)?;
                self.tools = p.tools.into_iter().map(|t| (t.name.clone(), t)).collect();
            }
            method::TOOLS_CHANGED => {
                let p: ToolsChangedParams = serde_json::from_value(params).map_err(bad)?;
                for name in p.removed {
                    self.tools.remove(&name);
                }
                self.tools.extend(p.upserted.into_iter().map(|t| (t.name.clone(), t)));
            }
            method::RESOURCES_SYNC => {
                let p: ResourcesSyncParams = serde_json::from_value(params).map_err(bad)?;
                self.resources = p.resources.into_iter().map(|r| (r.name.clone(), r)).collect();
            }
            method::RESOURCES_CHANGED => {
                let p: ResourcesChangedParams = serde_json::from_value(params).map_err(bad)?;
                for name in p.removed {
                    self.resources.remove(&name);
                }
                self.resources
                    .extend(p.upserted.into_iter().map(|r| (r.name.clone(), r)));
            }
            _ => {}
        }
        Ok(())
    }

    /// `--catalog` 打印的一行：完整声明与 Host 按 8.4 计算的 `toolsHash`。
    fn line(&self) -> Value {
        let tools = ToolsSyncParams { tools: self.tools.values().cloned().collect() };
        let resources = ResourcesSyncParams { resources: self.resources.values().cloned().collect() };
        let hash = proto::hash::tools_hash(&tools, &resources);
        json!({
            "type": "catalog",
            "tools": self.tools.iter().map(|(k, v)| (k.clone(), to_value(v))).collect::<serde_json::Map<_, _>>(),
            "resources": self.resources.iter().map(|(k, v)| (k.clone(), to_value(v))).collect::<serde_json::Map<_, _>>(),
            "toolsHash": hash,
        })
    }
}

pub(super) async fn run(mut listener: Listener, opts: Options) -> Result<(), String> {
    let mut host = HostState {
        ops: opts.ops.into_iter(),
        lease_ms: opts.lease_ms,
        reject_sleep_ms: opts.reject_sleep_ms,
        tool_info: opts.tool_info,
        trace: opts.trace,
        catalog: Catalog::default(),
        resume: None,
        wake_token: None,
        connections: 0,
        next_call: 0,
        next_request: 0,
    };
    loop {
        let stream = listener.accept().await?;
        let ws = tokio_tungstenite::accept_async(stream)
            .await
            .map_err(|e| format!("WebSocket 握手失败：{e}"))?;
        host.connections += 1;
        match serve(ws, &mut host).await? {
            ConnEnd::Done => return Ok(()),
            ConnEnd::Slept => match host.ops.next() {
                None => return Ok(()),
                Some(Op::Wake) => {
                    let token = format!("wake-{}", host.connections);
                    emit(
                        &json!({ "type": "wake", "token": token, "arg": format!("app-mcp-wake:{token}") })
                            .to_string(),
                    );
                    host.wake_token = Some(token);
                }
                Some(other) => return Err(format!("休眠后的下一步必须是 --wake，实际为 {other:?}")),
            },
        }
    }
}

async fn serve(mut ws: WebSocketStream<Box<dyn Io>>, host: &mut HostState) -> Result<ConnEnd, String> {
    let first = host.connections == 1;
    let mut started = false;
    let mut synced = false;
    let mut awaiting_sleep = false;
    let mut slept = false;
    // 当前等待响应的请求：(请求 ID, 类型, 名称)。
    let mut pending: Option<(RequestId, &'static str, String)> = None;
    // `--no-wait` 发出、尚未回复的调用：(请求 ID, 工具名)。
    let mut detached: Vec<(RequestId, String)> = Vec::new();
    let mut ops_done = false;
    let mut timer: Option<(tokio::time::Instant, Timer)> = None;

    loop {
        // 需要发送下一个操作。
        if ops_done && detached.is_empty() {
            close(&mut ws).await;
            return Ok(ConnEnd::Done);
        }
        if started && !ops_done && pending.is_none() && timer.is_none() && !awaiting_sleep && !slept {
            match host.ops.next() {
                None if detached.is_empty() => {
                    close(&mut ws).await;
                    return Ok(ConnEnd::Done);
                }
                None => ops_done = true,
                Some(Op::AwaitSleep) => awaiting_sleep = true,
                Some(Op::Wake) => return Err("--wake 必须在 --await-sleep 之后".to_owned()),
                Some(Op::Invoke { name, args, opts }) => {
                    let id = next_id(host);
                    host.next_call += 1;
                    let call_id = opts.call_id.unwrap_or_else(|| format!("c{}", host.next_call));
                    if let Some(ms) = opts.cancel_after_ms {
                        timer = Some((after(ms), Timer::Cancel(call_id.clone())));
                    }
                    let params = ToolsInvokeParams {
                        call_id,
                        name: name.clone(),
                        arguments: args,
                        timeout_ms: Some(opts.timeout_ms.unwrap_or(5000)),
                        idempotency_key: opts.idempotency_key,
                        priority: Default::default(),
                    };
                    let mut params = to_value(&params);
                    if let Some(p) = opts.priority {
                        params["priority"] = Value::String(p);
                    }
                    let msg = Message::request(id.clone(), method::TOOLS_INVOKE, params);
                    send(&mut ws, &msg).await?;
                    if opts.no_wait {
                        detached.push((id, name));
                        // 不等结果：立即执行下一步（否则要等到下一帧到达才会发）。
                        continue;
                    } else {
                        pending = Some((id, "invoke", name));
                    }
                }
                Some(Op::Catalog { settle_ms }) => timer = Some((after(settle_ms), Timer::Catalog)),
                Some(Op::Delay { ms }) => timer = Some((after(ms), Timer::Delay)),
                Some(Op::Navigate { page, params }) => {
                    let id = next_id(host);
                    let params = proto::NavigateParams { page: page.clone(), params, timeout_ms: None };
                    let msg = Message::request(id.clone(), method::NAVIGATE, to_value(&params));
                    send(&mut ws, &msg).await?;
                    pending = Some((id, "navigate", page));
                }
                Some(Op::Read { name }) => {
                    let id = next_id(host);
                    let params = ResourcesReadParams { name: name.clone() };
                    let msg = Message::request(id.clone(), method::RESOURCES_READ, to_value(&params));
                    send(&mut ws, &msg).await?;
                    pending = Some((id, "read", name));
                }
            }
        }

        let next = match &timer {
            None => ws.next().await,
            Some((at, _)) => {
                let at = *at;
                tokio::select! {
                    f = ws.next() => f,
                    () = tokio::time::sleep_until(at) => {
                        if let Some((_, t)) = timer.take() {
                            fire(&mut ws, host, t).await?;
                        }
                        continue;
                    }
                }
            }
        };
        let frame = match next {
            Some(Ok(f)) => f,
            Some(Err(_)) | None if slept => return Ok(ConnEnd::Slept),
            Some(Err(e)) => return Err(format!("连接出错：{e}")),
            None => return Err("连接在完成前被关闭".to_owned()),
        };
        let text = match frame {
            WsMessage::Text(t) => t.to_string(),
            WsMessage::Close(_) if slept => return Ok(ConnEnd::Slept),
            WsMessage::Close(_) => return Err("连接在完成前被关闭".to_owned()),
            _ => continue,
        };
        let msg = match Message::parse(&text) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("fake_host: 忽略无法解析的消息：{e}");
                continue;
            }
        };
        if host.trace {
            trace_recv(&msg);
        }
        match msg {
            Message::Request(req) => {
                let reply = match req.method.as_str() {
                    method::HELLO => {
                        let tools_current = on_hello(host, first, req.params);
                        Message::result(
                            req.id,
                            to_value(&HelloResult {
                                status: PairingStatus::Paired,
                                token: Some("fake-token".to_owned()),
                                protocol_version: proto::PROTOCOL_VERSION.to_owned(),
                                host_version: "fake".to_owned(),
                                reason: None,
                                tools_current,
                                service: Some(proto::identity::SERVICE_NAME.to_owned()),
                                user: proto::identity::current_user(),
                                pid: Some(std::process::id()),
                                connection_id: Some(format!("fake-{}", std::process::id())),
                                code: None,
                            }),
                        )
                    }
                    method::PING => Message::result(req.id, json!({})),
                    method::SLEEP => {
                        let p: SleepParams = serde_json::from_value(req.params)
                            .map_err(|e| format!("app/sleep 参数无法解析：{e}"))?;
                        let result = if !awaiting_sleep {
                            SleepResult { accepted: false, resume_token: None, retry_after_ms: Some(200) }
                        } else if let Some(ms) = host.reject_sleep_ms.take() {
                            emit_sleep(false, &p);
                            SleepResult { accepted: false, resume_token: None, retry_after_ms: Some(ms) }
                        } else {
                            emit_sleep(true, &p);
                            let token = format!("resume-{}", host.connections);
                            host.resume = Some((token.clone(), p.tools_hash.clone()));
                            awaiting_sleep = false;
                            slept = true;
                            SleepResult { accepted: true, resume_token: Some(token), retry_after_ms: None }
                        };
                        Message::result(req.id, to_value(&result))
                    }
                    other => Message::error(req.id, RpcError::method_not_found(other)),
                };
                send(&mut ws, &reply).await?;
            }
            Message::Notification(n) => {
                if n.method == method::READY {
                    let mut line = json!({
                        "type": "tools",
                        "tools": host.catalog.tools.keys().collect::<Vec<_>>(),
                        "resources": host.catalog.resources.keys().collect::<Vec<_>>(),
                    });
                    if !first {
                        line["synced"] = json!(synced);
                    }
                    if host.tool_info {
                        let info: serde_json::Map<String, Value> = host
                            .catalog
                            .tools
                            .values()
                            .map(|t| {
                                let mut d = json!({ "risk": t.risk });
                                if let Some(a) = &t.annotations {
                                    d["annotations"] = to_value(a);
                                }
                                if let Some(o) = &t.output_schema {
                                    d["outputSchema"] = o.clone();
                                }
                                (t.name.clone(), d)
                            })
                            .collect();
                        line["toolInfo"] = Value::Object(info);
                    }
                    emit(&line.to_string());
                    started = true;
                    send_lease(&mut ws, host.lease_ms).await?;
                } else if n.method == method::TOOLS_PROGRESS {
                    // 进度（spec/protocol.md 3.3）原样打印，供各语言绑定核对 ctx.progress()
                    let mut line = n.params;
                    line["type"] = json!("progress");
                    emit(&line.to_string());
                } else if n.method == method::EVENTS_SYNC || n.method == method::EVENTS_EMIT {
                    emit(&event_line(&n.method, n.params).to_string());
                } else {
                    if n.method == method::TOOLS_SYNC {
                        synced = true;
                    }
                    if let Err(e) = host.catalog.apply(&n.method, n.params) {
                        eprintln!("fake_host: {e}");
                    }
                }
            }
            Message::Response(resp) => {
                if let Some(i) = detached.iter().position(|(id, _)| *id == resp.id) {
                    let (_, name) = detached.remove(i);
                    emit_response("invoke", &name, resp.outcome);
                    continue;
                }
                let Some((id, kind, name)) = pending.take() else {
                    eprintln!("fake_host: 忽略未知响应 {}", resp.id);
                    continue;
                };
                if resp.id != id {
                    eprintln!("fake_host: 忽略未知响应 {}", resp.id);
                    pending = Some((id, kind, name));
                    continue;
                }
                if matches!(timer, Some((_, Timer::Cancel(_)))) {
                    timer = None;
                }
                emit_response(kind, &name, resp.outcome);
                send_lease(&mut ws, host.lease_ms).await?;
            }
        }
    }
}

/// 打印一个请求的结果行（`{"type", "name", "result" | "error"}`）。
fn emit_response(kind: &str, name: &str, outcome: Result<Value, proto::RpcError>) {
    let mut line = BTreeMap::new();
    line.insert("type", json!(kind));
    line.insert("name", json!(name));
    match outcome {
        Ok(result) => line.insert("result", result),
        Err(err) => line.insert("error", to_value(&err)),
    };
    emit(&to_value(&line).to_string());
}

/// 全部操作完成：关闭连接，尽量等对端确认关闭（避免对端看到异常断开）。
async fn close(ws: &mut WebSocketStream<Box<dyn Io>>) {
    let _ = ws.close(None).await;
    let _ = tokio::time::timeout(Duration::from_millis(500), async {
        while let Some(Ok(_)) = ws.next().await {}
    })
    .await;
}

/// 处理 hello，返回 `toolsCurrent`。非首个连接打印一行 hello 信息。
fn on_hello(host: &mut HostState, first: bool, params: Value) -> bool {
    let hello: HelloParams = serde_json::from_value(params).unwrap_or_default();
    let tools_current = match (&host.resume, &hello.resume_token, &hello.tools_hash) {
        (Some((token, hash)), Some(t), Some(h)) => token == t && hash == h,
        _ => false,
    };
    // 恢复令牌只能用一次。
    host.resume = None;
    if !tools_current {
        // SDK 会完整同步。
        host.catalog = Catalog::default();
    }
    if !first {
        let launch_ok = hello.launch_token.is_some() && hello.launch_token == host.wake_token;
        if hello.launch_token.is_some() && !launch_ok {
            eprintln!("fake_host: 回连携带了未知的 launchToken {:?}", hello.launch_token);
        }
        host.wake_token = None;
        emit(
            &json!({
                "type": "hello",
                "launchToken": hello.launch_token,
                "resumeToken": hello.resume_token,
                "wakeReason": hello.wake_reason,
                "toolsCurrent": tools_current,
            })
            .to_string(),
        );
    }
    tools_current
}

/// 到期时刻：现在之后 `ms` 毫秒。
fn after(ms: u64) -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_millis(ms)
}

/// 执行到期的定时动作。
async fn fire<S>(ws: &mut S, host: &HostState, timer: Timer) -> Result<(), String>
where
    S: futures::Sink<WsMessage> + Unpin,
    S::Error: std::fmt::Display,
{
    match timer {
        Timer::Cancel(call_id) => {
            if host.trace {
                emit(&json!({ "type": "cancel", "callId": call_id }).to_string());
            }
            let params = ToolsCancelParams { call_id, reason: None };
            send(ws, &Message::notification(method::TOOLS_CANCEL, to_value(&params))).await
        }
        Timer::Catalog => {
            emit(&host.catalog.line().to_string());
            Ok(())
        }
        Timer::Delay => Ok(()),
    }
}

/// `--trace`：打印 SDK 发来的请求与通知（`ping` 除外，是否发心跳取决于传输）。
fn trace_recv(msg: &Message) {
    let (method_name, params) = match msg {
        Message::Request(r) => (&r.method, &r.params),
        Message::Notification(n) => (&n.method, &n.params),
        Message::Response(_) => return,
    };
    if method_name == method::PING {
        return;
    }
    emit(&json!({ "type": "recv", "method": method_name, "params": params }).to_string());
}

/// 事件（第 16 项 N3，spec/protocol.md 3.5）：`events/sync` → `{"type":"events","events"}`，`events/emit` →
/// `{"type":"event","name","eventId","payload"?}`；参数原样合并（不是对象时放在 `params` 下）。
pub(super) fn event_line(method_name: &str, params: Value) -> Value {
    let kind = if method_name == method::EVENTS_SYNC { "events" } else { "event" };
    match params {
        Value::Object(mut map) => {
            map.insert("type".to_owned(), json!(kind));
            Value::Object(map)
        }
        other => json!({ "type": kind, "params": other }),
    }
}

fn emit_sleep(accepted: bool, p: &SleepParams) {
    emit(
        &json!({ "type": "sleep", "accepted": accepted, "reason": p.reason, "toolsHash": p.tools_hash })
            .to_string(),
    );
}

async fn send_lease<S>(ws: &mut S, lease_ms: Option<u64>) -> Result<(), String>
where
    S: futures::Sink<WsMessage> + Unpin,
    S::Error: std::fmt::Display,
{
    match lease_ms {
        Some(ttl_ms) => {
            let msg = Message::notification(method::LEASE, to_value(&LeaseParams { ttl_ms, ..Default::default() }));
            send(ws, &msg).await
        }
        None => Ok(()),
    }
}

fn next_id(host: &mut HostState) -> RequestId {
    host.next_request += 1;
    RequestId::String(format!("fake-{}", host.next_request))
}

fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

async fn send<S>(ws: &mut S, msg: &Message) -> Result<(), String>
where
    S: futures::Sink<WsMessage> + Unpin,
    S::Error: std::fmt::Display,
{
    ws.send(WsMessage::text(msg.to_json()))
        .await
        .map_err(|e| format!("发送失败：{e}"))
}
