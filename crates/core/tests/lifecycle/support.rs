//! 测试辅助。

pub(crate) use app_mcp_core::*;
pub(crate) use serde_json::{Value, json};

pub(crate) struct Harness {
    pub(crate) c: Client,
    pub(crate) now: Millis,
    pub(crate) next_id: i64,
}

pub(crate) fn config(mode: LifecycleMode) -> ClientConfig {
    let mut c = ClientConfig::new("shop", "示例商城", "inst-1", ClientKind::Native);
    c.lifecycle.mode = mode;
    c
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
    }
}

pub(crate) fn resource(name: &str) -> ResourceDef {
    ResourceDef { name: name.into(), description: format!("{name} 资源"), mime_type: None, scope: None, realtime: false, annotations: None }
}

pub(crate) fn sends(events: &[Event]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Send(s) => Some(serde_json::from_str(s).expect("json")),
            _ => None,
        })
        .collect()
}

pub(crate) fn methods(msgs: &[Value]) -> Vec<&str> {
    msgs.iter().map(|m| m["method"].as_str().unwrap_or("<response>")).collect()
}

pub(crate) fn find<'a>(msgs: &'a [Value], method: &str) -> Option<&'a Value> {
    msgs.iter().find(|m| m["method"] == method)
}

pub(crate) const IDLE: Millis = 60_000;
/// 调用 / 读取后的合并窗口默认值（spec/lifecycle.md 第 13 节 B1）。
pub(crate) const MERGE: Millis = 2_000;

impl Harness {
    pub(crate) fn new(mode: LifecycleMode) -> Self {
        Self::with(config(mode))
    }

    pub(crate) fn with(cfg: ClientConfig) -> Self {
        Self { c: Client::new(cfg), now: 1_000, next_id: 0 }
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

    pub(crate) fn respond(&mut self, req: &Value, result: Value) -> Vec<Event> {
        self.recv(json!({"jsonrpc": "2.0", "id": req["id"], "result": result}))
    }

    /// 连接建立（驱动层已处理 Connect），返回 hello。
    pub(crate) fn link(&mut self) -> Value {
        self.c.handle_connected(self.now);
        let msgs = sends(&self.drain());
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["method"], "app/hello");
        msgs[0].clone()
    }

    pub(crate) fn paired(&mut self, hello: &Value, tools_current: bool) -> Vec<Event> {
        let mut r = json!({"status": "paired", "token": "tk", "protocolVersion": "1", "hostVersion": "h"});
        if tools_current {
            r["toolsCurrent"] = json!(true);
        }
        let ev = self.respond(hello, r);
        assert_eq!(self.c.state(), &ConnectionState::Connected);
        ev
    }

    /// start + 连接 + 握手，返回 hello。
    pub(crate) fn connect(&mut self) -> Value {
        self.c.start(self.now);
        let ev = self.drain();
        assert!(ev.contains(&Event::Connect), "{ev:?}");
        let hello = self.link();
        self.paired(&hello, false);
        hello
    }

    pub(crate) fn advance(&mut self, ms: Millis) -> Vec<Event> {
        let target = self.now + ms;
        let mut out = Vec::new();
        while let Some(t) = self.c.poll_timeout() {
            if t > target {
                break;
            }
            self.now = self.now.max(t);
            self.c.handle_timeout(self.now);
            let ev = self.drain();
            // 自动回复心跳
            for m in sends(&ev) {
                if m["method"] == "ping" {
                    self.respond(&m, json!({}));
                }
            }
            out.extend(ev);
        }
        self.now = target;
        self.c.handle_timeout(self.now);
        out.extend(self.drain());
        out
    }

    /// 推进时间直到发出 app/sleep，返回该请求与发出时刻。
    pub(crate) fn wait_sleep(&mut self, limit: Millis) -> Option<(Value, Millis)> {
        let end = self.now + limit;
        while self.now < end {
            let step = self.c.poll_timeout().map_or(end, |t| t.min(end)).max(self.now + 1) - self.now;
            let ev = self.advance(step);
            if let Some(m) = find(&sends(&ev), "app/sleep") {
                return Some((m.clone(), self.now));
            }
        }
        None
    }

    pub(crate) fn invoke(&mut self, call_id: &str, name: &str) -> Vec<Event> {
        self.next_id += 1;
        self.recv(json!({"jsonrpc": "2.0", "id": 1000 + self.next_id, "method": "tools/invoke",
            "params": {"callId": call_id, "name": name, "arguments": {}}}))
    }

    pub(crate) fn request(&mut self, method: &str, params: Value) -> Vec<Event> {
        self.next_id += 1;
        self.recv(json!({"jsonrpc": "2.0", "id": 1000 + self.next_id, "method": method, "params": params}))
    }

    pub(crate) fn notify(&mut self, method: &str, params: Value) -> Vec<Event> {
        self.recv(json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// 休眠握手被接受。
    pub(crate) fn accept_sleep(&mut self, sleep: &Value, resume: &str) -> Vec<Event> {
        self.respond(sleep, json!({"accepted": true, "resumeToken": resume}))
    }

    /// 从已连接进入 Dormant（idle 模式），返回 sleep 请求。
    pub(crate) fn go_dormant(&mut self) -> Value {
        let (sleep, _) = self.wait_sleep(10 * IDLE).expect("应发出 app/sleep");
        let ev = self.accept_sleep(&sleep, "resume-1");
        assert!(ev.contains(&Event::Disconnect));
        assert_eq!(self.c.state(), &ConnectionState::Dormant);
        sleep
    }
}

pub(crate) fn realtime(name: &str) -> ResourceDef {
    ResourceDef { realtime: true, ..resource(name) }
}
