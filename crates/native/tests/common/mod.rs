//! 测试辅助：在独立线程上运行的模拟 Host（tokio-tungstenite 服务器）。
//!
//! - 自动回复 `app/hello`（`paired`，token 为 `tok-<连接序号>`）与 `ping`。
//! - 其余收到的消息（含 hello 请求本身）通过 [`MockHost::next`] 交给测试检查。

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_mcp_native::{ClientListener, LogLevel, StateInfo, StateStatus};
use app_mcp_protocol::{Message, RequestId, RpcError, method};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::Message as WsMessage;

pub const WAIT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum HostEvent {
    Connected(u32),
    Msg(Message),
    Closed,
}

enum HostCmd {
    Send(Message),
    Close,
}

pub struct MockHost {
    pub addr: SocketAddr,
    events: Receiver<HostEvent>,
    cmds: UnboundedSender<HostCmd>,
    next_id: Mutex<u32>,
}

impl MockHost {
    pub fn start() -> Self {
        let (ev_tx, events) = channel();
        let (cmds, mut cmd_rx) = unbounded_channel::<HostCmd>();
        let (addr_tx, addr_rx) = channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                addr_tx.send(listener.local_addr().unwrap()).unwrap();
                let mut n = 0u32;
                loop {
                    let stream = tokio::select! {
                        accepted = listener.accept() => match accepted {
                            Ok((s, _)) => s,
                            Err(_) => return,
                        },
                        cmd = cmd_rx.recv() => match cmd {
                            Some(_) => continue, // 没有连接时丢弃
                            None => return,
                        },
                    };
                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { continue };
                    n += 1;
                    if ev_tx.send(HostEvent::Connected(n)).is_err() {
                        return;
                    }
                    loop {
                        tokio::select! {
                            frame = ws.next() => {
                                let text = match frame {
                                    Some(Ok(WsMessage::Text(t))) => t.to_string(),
                                    Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => break,
                                    Some(Ok(_)) => continue,
                                };
                                let msg = Message::parse(&text).unwrap();
                                if let Message::Request(req) = &msg {
                                    let reply = match req.method.as_str() {
                                        method::HELLO => Some(json!({
                                            "status": "paired", "token": format!("tok-{n}"),
                                            "protocolVersion": "1", "hostVersion": "mock"
                                        })),
                                        method::PING => Some(json!({})),
                                        _ => None,
                                    };
                                    if let Some(result) = reply {
                                        let _ = ws.send(WsMessage::text(Message::result(req.id.clone(), result).to_json())).await;
                                        if req.method == method::PING {
                                            continue;
                                        }
                                    }
                                }
                                if ev_tx.send(HostEvent::Msg(msg)).is_err() {
                                    return;
                                }
                            }
                            cmd = cmd_rx.recv() => match cmd {
                                Some(HostCmd::Send(m)) => { let _ = ws.send(WsMessage::text(m.to_json())).await; }
                                Some(HostCmd::Close) => { let _ = ws.close(None).await; }
                                None => return,
                            },
                        }
                    }
                    if ev_tx.send(HostEvent::Closed).is_err() {
                        return;
                    }
                }
            });
        });
        let addr = addr_rx.recv().unwrap();
        Self {
            addr,
            events,
            cmds,
            next_id: Mutex::new(0),
        }
    }

    pub fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    pub fn next(&self, timeout: Duration) -> Option<HostEvent> {
        match self.events.recv_timeout(timeout) {
            Ok(e) => Some(e),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => panic!("mock host 线程已退出"),
        }
    }

    /// 等到满足条件的事件，跳过其他事件。
    pub fn wait_for<T>(&self, what: &str, mut f: impl FnMut(&HostEvent) -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(ev) = self.next(left) else {
                panic!("等待 {what} 超时")
            };
            if let Some(v) = f(&ev) {
                return v;
            }
        }
    }

    pub fn wait_connected(&self) -> u32 {
        self.wait_for("连接", |e| match e {
            HostEvent::Connected(n) => Some(*n),
            _ => None,
        })
    }

    pub fn wait_closed(&self) {
        self.wait_for("连接关闭", |e| {
            matches!(e, HostEvent::Closed).then_some(())
        })
    }

    pub fn wait_notification(&self, name: &str) -> Value {
        self.wait_for(name, |e| match e {
            HostEvent::Msg(Message::Notification(n)) if n.method == name => Some(n.params.clone()),
            _ => None,
        })
    }

    pub fn wait_request(&self, name: &str) -> Value {
        self.wait_for(name, |e| match e {
            HostEvent::Msg(Message::Request(r)) if r.method == name => Some(r.params.clone()),
            _ => None,
        })
    }

    pub fn wait_response(&self, id: &RequestId) -> Result<Value, RpcError> {
        self.wait_for("响应", |e| match e {
            HostEvent::Msg(Message::Response(r)) if &r.id == id => Some(r.outcome.clone()),
            _ => None,
        })
    }

    /// 等到握手完成（收到 `app/ready`）。
    pub fn wait_ready(&self) {
        self.wait_notification(method::READY);
    }

    pub fn send(&self, msg: Message) {
        self.cmds.send(HostCmd::Send(msg)).unwrap();
    }

    pub fn request(&self, name: &str, params: Value) -> RequestId {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            RequestId::String(format!("h{n}"))
        };
        self.send(Message::request(id.clone(), name, params));
        id
    }

    pub fn invoke(&self, call_id: &str, tool: &str, args: Value) -> RequestId {
        self.request(
            method::TOOLS_INVOKE,
            json!({ "callId": call_id, "name": tool, "arguments": args, "timeoutMs": 5000 }),
        )
    }

    pub fn close(&self) {
        self.cmds.send(HostCmd::Close).unwrap();
    }
}

/// 记录 listener 回调的测试 listener。
#[derive(Default)]
pub struct Recorder {
    pub states: Mutex<Vec<StateInfo>>,
    pub tokens: Mutex<Vec<String>>,
    pub logs: Mutex<Vec<(LogLevel, String)>>,
    pub threads: Mutex<Vec<String>>,
}

impl Recorder {
    pub fn statuses(&self) -> Vec<StateStatus> {
        self.states
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.status)
            .collect()
    }

    fn note_thread(&self) {
        let name = std::thread::current().name().unwrap_or("").to_owned();
        self.threads.lock().unwrap().push(name);
    }
}

impl ClientListener for Recorder {
    fn on_state_changed(&self, state: StateInfo) {
        self.note_thread();
        self.states.lock().unwrap().push(state);
    }
    fn on_paired(&self, token: String) {
        self.note_thread();
        self.tokens.lock().unwrap().push(token);
    }
    fn on_log(&self, level: LogLevel, message: String) {
        self.logs.lock().unwrap().push((level, message));
    }
}

/// 轮询等待条件成立。
pub fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !f() {
        assert!(Instant::now() < deadline, "等待 {what} 超时");
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub fn shared<T>(v: T) -> Arc<Mutex<T>> {
    Arc::new(Mutex::new(v))
}
