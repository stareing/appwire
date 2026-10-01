//! 假 Host：供各语言绑定做集成测试。
//!
//! ```text
//! cargo run -p app-mcp-native --example fake_host -- [--addr 127.0.0.1:0] \
//!     [--invoke <tool> --args '<json>']... [--read <resource>]... \
//!     [--await-sleep] [--wake] ... [--lease-ms <ms>] [--reject-sleep <retryAfterMs>] [--timeout-ms 10000]
//! ```
//!
//! - 绑定后立即向 stdout 打印 `LISTENING <地址>`。
//! - 接受 WebSocket 连接，完成握手（直接 `paired`），回复 `ping`。
//! - 收到 `app/ready` 后打印当前工具与资源列表，然后按命令行顺序依次执行操作，每步打印一行 JSON：
//!   - `--invoke` / `--read`：发送请求，打印结果（`{"type":"invoke"|"read", "name", "result"|"error"}`）。
//!   - `--await-sleep`：等待 SDK 发送 `app/sleep` 并接受（返回 `resumeToken`），打印
//!     `{"type":"sleep","accepted":true,"reason","toolsHash"}`；随后 SDK 关闭连接。
//!     等待期间收到的 `app/sleep` 之外，其他时候收到的 `app/sleep` 一律以 `retryAfterMs: 200` 拒绝（不打印）。
//!   - `--wake`（必须在 `--await-sleep` 之后）：生成一次性唤醒令牌，打印
//!     `{"type":"wake","token","arg":"app-mcp-wake:<token>"}`，测试方把 `arg` 交给 SDK 的 `handleWake`；
//!     然后接受下一个连接，打印 `{"type":"hello","launchToken","resumeToken","wakeReason","toolsCurrent"}`，
//!     恢复令牌与 `toolsHash` 都与休眠时一致时返回 `toolsCurrent: true`（沿用工具快照）。
//!     该连接的 `app/ready` 之后照常打印工具列表，并附带 `"synced"`（本连接是否收到了 `tools/sync`）。
//! - `--lease-ms <ms>`：`app/ready` 之后与每个操作完成后发送 `app/lease { ttlMs }`。
//! - `--reject-sleep <ms>`：`--await-sleep` 期间收到的第一个 `app/sleep` 以 `retryAfterMs: <ms>` 拒绝，打印
//!   `{"type":"sleep","accepted":false,"reason","toolsHash"}`。
//! - 全部完成后关闭连接（最后一步是 `--await-sleep` 时由 SDK 关闭），退出码 0；超时退出码 2；
//!   其他错误退出码 1（信息写 stderr）。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

use app_mcp_protocol::{
    self as proto, HelloParams, HelloResult, LeaseParams, Message, PairingStatus, RequestId,
    ResourcesChangedParams, ResourcesReadParams, ResourcesSyncParams, RpcError, SleepParams,
    SleepResult, ToolsChangedParams, ToolsInvokeParams, ToolsSyncParams, method,
};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;

#[derive(Debug, Clone, PartialEq)]
enum Op {
    Invoke { name: String, args: Value },
    Read { name: String },
    AwaitSleep,
    Wake,
}

#[derive(Debug)]
struct Options {
    addr: String,
    ops: Vec<Op>,
    timeout_ms: u64,
    lease_ms: Option<u64>,
    reject_sleep_ms: Option<u64>,
}

fn parse_u64(flag: &str, text: &str) -> Result<u64, String> {
    text.parse()
        .map_err(|_| format!("{flag} 不是合法整数：{text}"))
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut opts = Options {
        addr: "127.0.0.1:0".to_owned(),
        ops: Vec::new(),
        timeout_ms: 10_000,
        lease_ms: None,
        reject_sleep_ms: None,
    };
    let mut it = args.into_iter();
    while let Some(flag) = it.next() {
        let mut value = |flag: &str| it.next().ok_or_else(|| format!("{flag} 缺少参数值"));
        match flag.as_str() {
            "--addr" => opts.addr = value("--addr")?,
            "--invoke" => opts.ops.push(Op::Invoke {
                name: value("--invoke")?,
                args: json!({}),
            }),
            "--args" => {
                let text = value("--args")?;
                let parsed: Value = serde_json::from_str(&text)
                    .map_err(|e| format!("--args 不是合法 JSON：{e}"))?;
                match opts.ops.last_mut() {
                    Some(Op::Invoke { args, .. }) => *args = parsed,
                    _ => return Err("--args 必须紧跟在 --invoke <tool> 之后".to_owned()),
                }
            }
            "--read" => opts.ops.push(Op::Read {
                name: value("--read")?,
            }),
            "--await-sleep" => opts.ops.push(Op::AwaitSleep),
            "--wake" => {
                if opts.ops.last() != Some(&Op::AwaitSleep) {
                    return Err("--wake 必须紧跟在 --await-sleep 之后".to_owned());
                }
                opts.ops.push(Op::Wake);
            }
            "--lease-ms" => opts.lease_ms = Some(parse_u64("--lease-ms", &value("--lease-ms")?)?),
            "--reject-sleep" => {
                opts.reject_sleep_ms = Some(parse_u64("--reject-sleep", &value("--reject-sleep")?)?)
            }
            "--timeout-ms" => opts.timeout_ms = parse_u64("--timeout-ms", &value("--timeout-ms")?)?,
            other => return Err(format!("未知参数：{other}")),
        }
    }
    Ok(opts)
}

/// 输出一行到 stdout 并 flush。
fn emit(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let opts = match parse_args(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("fake_host: {e}");
            return ExitCode::from(1);
        }
    };
    let listener = match TcpListener::bind(&opts.addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("fake_host: 无法绑定 {}：{e}", opts.addr);
            return ExitCode::from(1);
        }
    };
    let local: SocketAddr = match listener.local_addr() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("fake_host: 无法获取监听地址：{e}");
            return ExitCode::from(1);
        }
    };
    emit(&format!("LISTENING {local}"));

    let timeout = Duration::from_millis(opts.timeout_ms);
    match tokio::time::timeout(timeout, run(listener, opts)).await {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            eprintln!("fake_host: {e}");
            ExitCode::from(1)
        }
        Err(_) => {
            eprintln!("fake_host: 超过 {} ms 仍未完成", timeout.as_millis());
            ExitCode::from(2)
        }
    }
}

/// 当前 SDK 同步过来的工具与资源（已按名称排序）。休眠期间作为快照保留。
#[derive(Default)]
struct Catalog {
    tools: BTreeSet<String>,
    resources: BTreeSet<String>,
}

impl Catalog {
    fn apply(&mut self, method_name: &str, params: Value) -> Result<(), String> {
        let bad = |e: serde_json::Error| format!("{method_name} 参数无法解析：{e}");
        match method_name {
            method::TOOLS_SYNC => {
                let p: ToolsSyncParams = serde_json::from_value(params).map_err(bad)?;
                self.tools = p.tools.into_iter().map(|t| t.name).collect();
            }
            method::TOOLS_CHANGED => {
                let p: ToolsChangedParams = serde_json::from_value(params).map_err(bad)?;
                for name in p.removed {
                    self.tools.remove(&name);
                }
                self.tools.extend(p.upserted.into_iter().map(|t| t.name));
            }
            method::RESOURCES_SYNC => {
                let p: ResourcesSyncParams = serde_json::from_value(params).map_err(bad)?;
                self.resources = p.resources.into_iter().map(|r| r.name).collect();
            }
            method::RESOURCES_CHANGED => {
                let p: ResourcesChangedParams = serde_json::from_value(params).map_err(bad)?;
                for name in p.removed {
                    self.resources.remove(&name);
                }
                self.resources
                    .extend(p.upserted.into_iter().map(|r| r.name));
            }
            _ => {}
        }
        Ok(())
    }
}

/// 跨连接的 Host 状态。
struct HostState {
    ops: std::vec::IntoIter<Op>,
    lease_ms: Option<u64>,
    /// 还需要拒绝的休眠次数（`--reject-sleep`）。
    reject_sleep_ms: Option<u64>,
    catalog: Catalog,
    /// 休眠被接受时发放的恢复令牌与当时的 toolsHash。
    resume: Option<(String, String)>,
    /// 等待回连的唤醒令牌。
    wake_token: Option<String>,
    connections: u32,
    next_call: u32,
    next_request: i64,
}

/// 一个连接结束的方式。
enum ConnEnd {
    /// 全部操作完成。
    Done,
    /// 休眠被接受、SDK 已断开，等待唤醒回连。
    Slept,
}

async fn run(listener: TcpListener, opts: Options) -> Result<(), String> {
    let mut host = HostState {
        ops: opts.ops.into_iter(),
        lease_ms: opts.lease_ms,
        reject_sleep_ms: opts.reject_sleep_ms,
        catalog: Catalog::default(),
        resume: None,
        wake_token: None,
        connections: 0,
        next_call: 0,
        next_request: 0,
    };
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|e| format!("accept 失败：{e}"))?;
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

async fn serve(mut ws: WebSocketStream<TcpStream>, host: &mut HostState) -> Result<ConnEnd, String> {
    let first = host.connections == 1;
    let mut started = false;
    let mut synced = false;
    let mut awaiting_sleep = false;
    let mut slept = false;
    // 当前等待响应的请求：(请求 ID, 类型, 名称)。
    let mut pending: Option<(RequestId, &'static str, String)> = None;

    loop {
        // 需要发送下一个操作。
        if started && pending.is_none() && !awaiting_sleep && !slept {
            match host.ops.next() {
                None => {
                    let _ = ws.close(None).await;
                    // 尽量等对端确认关闭，避免对端看到异常断开。
                    let _ = tokio::time::timeout(Duration::from_millis(500), async {
                        while let Some(Ok(_)) = ws.next().await {}
                    })
                    .await;
                    return Ok(ConnEnd::Done);
                }
                Some(Op::AwaitSleep) => awaiting_sleep = true,
                Some(Op::Wake) => return Err("--wake 必须在 --await-sleep 之后".to_owned()),
                Some(Op::Invoke { name, args }) => {
                    let id = next_id(host);
                    host.next_call += 1;
                    let params = ToolsInvokeParams {
                        call_id: format!("c{}", host.next_call),
                        name: name.clone(),
                        arguments: args,
                        timeout_ms: Some(5000),
                    };
                    let msg = Message::request(id.clone(), method::TOOLS_INVOKE, to_value(&params));
                    send(&mut ws, &msg).await?;
                    pending = Some((id, "invoke", name));
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

        let frame = match ws.next().await {
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
                        "tools": host.catalog.tools.iter().collect::<Vec<_>>(),
                        "resources": host.catalog.resources.iter().collect::<Vec<_>>(),
                    });
                    if !first {
                        line["synced"] = json!(synced);
                    }
                    emit(&line.to_string());
                    started = true;
                    send_lease(&mut ws, host.lease_ms).await?;
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
                let Some((id, kind, name)) = pending.take() else {
                    eprintln!("fake_host: 忽略未知响应 {}", resp.id);
                    continue;
                };
                if resp.id != id {
                    eprintln!("fake_host: 忽略未知响应 {}", resp.id);
                    pending = Some((id, kind, name));
                    continue;
                }
                let mut line = BTreeMap::new();
                line.insert("type", json!(kind));
                line.insert("name", json!(name));
                match resp.outcome {
                    Ok(result) => line.insert("result", result),
                    Err(err) => line.insert("error", to_value(&err)),
                };
                emit(&to_value(&line).to_string());
                send_lease(&mut ws, host.lease_ms).await?;
            }
        }
    }
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
            let msg = Message::notification(method::LEASE, to_value(&LeaseParams { ttl_ms }));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_interleaved_ops() {
        let o = parse_args(args(&[
            "--addr",
            "127.0.0.1:9",
            "--invoke",
            "a",
            "--args",
            r#"{"x":1}"#,
            "--read",
            "r",
            "--invoke",
            "b",
            "--timeout-ms",
            "50",
        ]))
        .unwrap();
        assert_eq!(o.addr, "127.0.0.1:9");
        assert_eq!(o.timeout_ms, 50);
        assert_eq!(
            o.ops,
            vec![
                Op::Invoke {
                    name: "a".into(),
                    args: json!({"x": 1})
                },
                Op::Read { name: "r".into() },
                Op::Invoke {
                    name: "b".into(),
                    args: json!({})
                },
            ]
        );
    }

    #[test]
    fn parses_lifecycle_ops() {
        let o = parse_args(args(&[
            "--invoke", "a", "--await-sleep", "--wake", "--read", "r", "--await-sleep",
            "--lease-ms", "100", "--reject-sleep", "50",
        ]))
        .unwrap();
        assert_eq!(o.lease_ms, Some(100));
        assert_eq!(o.reject_sleep_ms, Some(50));
        assert_eq!(
            o.ops,
            vec![
                Op::Invoke { name: "a".into(), args: json!({}) },
                Op::AwaitSleep,
                Op::Wake,
                Op::Read { name: "r".into() },
                Op::AwaitSleep,
            ]
        );
        assert!(parse_args(args(&["--wake"])).is_err());
        assert!(parse_args(args(&["--invoke", "a", "--wake"])).is_err());
        assert!(parse_args(args(&["--lease-ms", "x"])).is_err());
    }

    #[test]
    fn rejects_bad_args() {
        assert!(parse_args(args(&["--args", "{}"])).is_err());
        assert!(parse_args(args(&["--invoke", "a", "--args", "nope"])).is_err());
        assert!(parse_args(args(&["--invoke"])).is_err());
        assert!(parse_args(args(&["--bogus"])).is_err());
        assert!(parse_args(args(&["--timeout-ms", "x"])).is_err());
    }
}
