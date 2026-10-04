//! 测试辅助。

pub(crate) use app_mcp_core::*;
pub(crate) use app_mcp_protocol::ErrorKind;
pub(crate) use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// 测试辅助：假 Host
// ---------------------------------------------------------------------------

pub(crate) struct Harness {
    pub(crate) c: Client,
    pub(crate) now: Millis,
}

pub(crate) fn config() -> ClientConfig {
    ClientConfig::new("shop", "示例商城", "inst-1", ClientKind::Web)
}

pub(crate) fn tool(name: &str) -> ToolDef {
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
        implements: Vec::new(),
        cache: None,
        deprecated: None,
        concurrency: 0,
        exclusive: None,
    }
}

pub(crate) fn resource(name: &str) -> ResourceDef {
    ResourceDef { name: name.into(), description: format!("{name} 资源"), mime_type: None, scope: None, realtime: false, annotations: None, cache: None }
}

/// 从事件中取出所有发送的消息。
pub(crate) fn sends(events: &[Event]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Send(s) => Some(serde_json::from_str(s).expect("sent message is json")),
            _ => None,
        })
        .collect()
}

pub(crate) fn methods(msgs: &[Value]) -> Vec<&str> {
    msgs.iter().map(|m| m["method"].as_str().unwrap_or("<response>")).collect()
}

pub(crate) fn warnings(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::Warning(_))).count()
}

impl Harness {
    pub(crate) fn new() -> Self {
        Self::with(config())
    }

    pub(crate) fn with(cfg: ClientConfig) -> Self {
        Self { c: Client::new(cfg), now: 1_000 }
    }

    pub(crate) fn drain(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Some(e) = self.c.poll_event() {
            out.push(e);
        }
        out
    }

    pub(crate) fn recv(&mut self, msg: Value) -> Vec<Event> {
        self.c.handle_message(&msg.to_string(), self.now);
        self.drain()
    }

    /// 启动并建立连接，返回 `app/hello` 请求。
    pub(crate) fn open(&mut self) -> Value {
        self.c.start(self.now);
        self.drain();
        self.c.handle_connected(self.now);
        let ev = self.drain();
        let msgs = sends(&ev);
        assert_eq!(msgs.len(), 1);
        msgs[0].clone()
    }

    pub(crate) fn hello_result(&mut self, hello: &Value, result: Value) -> Vec<Event> {
        self.recv(json!({"jsonrpc": "2.0", "id": hello["id"], "result": result}))
    }

    /// 完成握手（paired）。
    pub(crate) fn connect(&mut self) -> Vec<Event> {
        let hello = self.open();
        let ev = self.hello_result(
            &hello,
            json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "0.1.0"}),
        );
        assert_eq!(self.c.state(), &ConnectionState::Connected);
        ev
    }

    /// 推进时间并触发到期的定时器。
    pub(crate) fn advance(&mut self, ms: Millis) -> Vec<Event> {
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

    pub(crate) fn invoke(&mut self, id: i64, call_id: &str, name: &str, timeout_ms: Option<u64>) -> Vec<Event> {
        let mut params = json!({"callId": call_id, "name": name, "arguments": {"x": 1}});
        if let Some(t) = timeout_ms {
            params["timeoutMs"] = json!(t);
        }
        self.recv(json!({"jsonrpc": "2.0", "id": id, "method": "tools/invoke", "params": params}))
    }

    pub(crate) fn request(&mut self, id: i64, method: &str, params: Value) -> Vec<Event> {
        self.recv(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
    }
}

pub(crate) fn invoked(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::InvokeTool { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect()
}

pub(crate) fn cancelled(events: &[Event]) -> Vec<(String, CancelReason)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::CancelTool { call_id, reason } => Some((call_id.clone(), *reason)),
            _ => None,
        })
        .collect()
}

pub(crate) fn error_kind(msg: &Value) -> &str {
    msg["error"]["data"]["kind"].as_str().unwrap_or("")
}
