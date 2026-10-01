//! 假 Host：供各语言绑定做集成测试。
//!
//! ```text
//! cargo run -p app-mcp-native --example fake_host -- [--addr 127.0.0.1:0 | --ipc <端点>] \
//!     [--invoke <tool> --args '<json>']... [--read <resource>]... \
//!     [--await-sleep] [--wake] ... [--lease-ms <ms>] [--reject-sleep <retryAfterMs>] [--timeout-ms 10000]
//! ```
//!
//! - 绑定后立即向 stdout 打印 `LISTENING <地址>`。默认监听 TCP（`--addr`，打印 `host:port`）；
//!   `--ipc unix:<绝对路径>`（Unix）/ `--ipc pipe:\\.\pipe\<名称>`（Windows）改为监听本地 IPC，
//!   打印端点字符串本身（如 `LISTENING unix:/tmp/x/hub.sock`），可直接作为 SDK 的 Host 地址。
//!   `--addr` 与 `--ipc` 互斥。IPC 端点由测试方指定，不要用平台默认端点（常驻 Host 可能正在用）。
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
use std::process::ExitCode;
use std::time::Duration;

use app_mcp_protocol::{
    self as proto, Endpoint, HelloParams, HelloResult, LeaseParams, Message, PairingStatus, RequestId,
    ResourcesChangedParams, ResourcesReadParams, ResourcesSyncParams, RpcError, SleepParams,
    SleepResult, ToolsChangedParams, ToolsInvokeParams, ToolsSyncParams, method,
};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
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
    /// TCP 监听地址；`None` 时为 [`DEFAULT_ADDR`]（未指定 `--ipc` 时）。
    addr: Option<String>,
    /// 本地 IPC 端点（`--ipc`）。
    ipc: Option<Endpoint>,
    ops: Vec<Op>,
    timeout_ms: u64,
    lease_ms: Option<u64>,
    reject_sleep_ms: Option<u64>,
}

const DEFAULT_ADDR: &str = "127.0.0.1:0";

fn parse_u64(flag: &str, text: &str) -> Result<u64, String> {
    text.parse()
        .map_err(|_| format!("{flag} 不是合法整数：{text}"))
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut opts = Options {
        addr: None,
        ipc: None,
        ops: Vec::new(),
        timeout_ms: 10_000,
        lease_ms: None,
        reject_sleep_ms: None,
    };
    let mut it = args.into_iter();
    while let Some(flag) = it.next() {
        let mut value = |flag: &str| it.next().ok_or_else(|| format!("{flag} 缺少参数值"));
        match flag.as_str() {
            "--addr" => opts.addr = Some(value("--addr")?),
            "--ipc" => {
                let text = value("--ipc")?;
                let endpoint = Endpoint::parse(&text)?;
                if !endpoint.is_ipc() {
                    return Err(format!("--ipc 需要 unix: 或 pipe: 端点：{text}"));
                }
                opts.ipc = Some(endpoint);
            }
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
    if opts.addr.is_some() && opts.ipc.is_some() {
        return Err("--addr 与 --ipc 不能同时使用".to_owned());
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
    let (listener, local) = match Listener::bind(&opts).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("fake_host: {e}");
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

/// 一个已接受连接的字节流（TCP / Unix 域套接字 / 命名管道）。
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// 监听位置：TCP（默认）或本地 IPC（`--ipc`）。IPC 上跑的仍是同一套 WebSocket 帧（spec/protocol.md 1.2）。
///
/// @why 不复用 app-mcp-hub 的 IPC 监听：那是 Hub 的 crate 内部实现（单实例判断、残留清理、对端鉴权），
///      native 不依赖 Hub；这里只需在测试指定的全新路径上监听，端点解析、长度检查与管道安全描述符
///      复用 app-mcp-protocol。
enum Listener {
    Tcp(TcpListener),
    #[cfg(unix)]
    Unix(UnixSocket),
    #[cfg(windows)]
    Pipe(PipeListener),
}

impl Listener {
    /// 绑定并返回 `LISTENING` 行要打印的位置（TCP 为 `host:port`，IPC 为端点字符串）。
    async fn bind(opts: &Options) -> Result<(Self, String), String> {
        let Some(endpoint) = &opts.ipc else {
            let addr = opts.addr.as_deref().unwrap_or(DEFAULT_ADDR);
            let listener = TcpListener::bind(addr)
                .await
                .map_err(|e| format!("无法绑定 {addr}：{e}"))?;
            let local = listener
                .local_addr()
                .map_err(|e| format!("无法获取监听地址：{e}"))?;
            return Ok((Self::Tcp(listener), local.to_string()));
        };
        let listener = match endpoint {
            #[cfg(unix)]
            Endpoint::Unix(path) => Self::Unix(UnixSocket::bind(path)?),
            #[cfg(windows)]
            Endpoint::Pipe(name) => Self::Pipe(PipeListener::bind(name)?),
            other => return Err(format!("本平台不支持端点 {other}")),
        };
        Ok((listener, endpoint.to_string()))
    }

    async fn accept(&mut self) -> Result<Box<dyn Io>, String> {
        let accept_err = |e: std::io::Error| format!("accept 失败：{e}");
        match self {
            Self::Tcp(l) => Ok(Box::new(l.accept().await.map_err(accept_err)?.0)),
            #[cfg(unix)]
            Self::Unix(s) => Ok(Box::new(s.listener.accept().await.map_err(accept_err)?.0)),
            #[cfg(windows)]
            Self::Pipe(p) => Ok(Box::new(p.accept().await.map_err(accept_err)?)),
        }
    }
}

/// Unix 域套接字监听；Drop 时删除套接字文件。
#[cfg(unix)]
struct UnixSocket {
    listener: tokio::net::UnixListener,
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl UnixSocket {
    /// @error 路径过长（IPC_PATH_TOO_LONG）、路径已存在或目录不可写时返回错误；不删除已有文件。
    fn bind(path: &std::path::Path) -> Result<Self, String> {
        proto::endpoint::check_unix_socket_path(path).map_err(|i| i.to_string())?;
        let listener = tokio::net::UnixListener::bind(path)
            .map_err(|e| format!("无法绑定 unix:{}：{e}", path.display()))?;
        Ok(Self { listener, path: path.to_owned() })
    }
}

#[cfg(unix)]
impl Drop for UnixSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Windows 命名管道监听：始终保留一个等待连接的实例（SDK 断开后可回连，如 `--wake`）。
///
/// @security 安全描述符与 Hub 相同（只允许当前用户、所有者为当前用户；SDK 连接时校验所有者），拒绝远程客户端。
#[cfg(windows)]
struct PipeListener {
    name: String,
    security: proto::endpoint::win::PipeSecurity,
    next: tokio::net::windows::named_pipe::NamedPipeServer,
}

#[cfg(windows)]
impl PipeListener {
    /// @error 名称过长（IPC_PATH_TOO_LONG）或同名管道已存在时返回错误。
    fn bind(name: &str) -> Result<Self, String> {
        proto::endpoint::check_pipe_name(name).map_err(|i| i.to_string())?;
        let security = proto::endpoint::win::PipeSecurity::current_user()
            .map_err(|e| format!("无法创建管道安全描述符：{e}"))?;
        let next = Self::create(name, &security, true)
            .map_err(|e| format!("无法创建命名管道 {name}：{e}"))?;
        Ok(Self { name: name.to_owned(), security, next })
    }

    fn create(
        name: &str,
        security: &proto::endpoint::win::PipeSecurity,
        first: bool,
    ) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        let mut opts = tokio::net::windows::named_pipe::ServerOptions::new();
        opts.first_pipe_instance(first).reject_remote_clients(true);
        security.with_attributes(|sa| {
            // SAFETY: sa 指向有效的 SECURITY_ATTRIBUTES，在调用期间有效。
            unsafe { opts.create_with_security_attributes_raw(name, sa) }
        })
    }

    async fn accept(&mut self) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
        self.next.connect().await?;
        let next = Self::create(&self.name, &self.security, false)?;
        Ok(std::mem::replace(&mut self.next, next))
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

async fn run(mut listener: Listener, opts: Options) -> Result<(), String> {
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
        assert_eq!(o.addr.as_deref(), Some("127.0.0.1:9"));
        assert_eq!(o.ipc, None);
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

    #[test]
    fn parses_ipc_endpoint() {
        let o = parse_args(args(&["--ipc", "unix:/tmp/x/hub.sock"])).unwrap();
        assert_eq!(o.ipc, Some(Endpoint::unix("/tmp/x/hub.sock")));
        assert_eq!(o.addr, None);
        let o = parse_args(args(&["--ipc", r"pipe:\\.\pipe\fake-1"])).unwrap();
        assert_eq!(o.ipc, Some(Endpoint::Pipe(r"\\.\pipe\fake-1".to_owned())));
        // 非 IPC 端点、非法端点、与 --addr 同用都拒绝。
        assert!(parse_args(args(&["--ipc", "ws://127.0.0.1:1/app"])).is_err());
        assert!(parse_args(args(&["--ipc", "unix:relative.sock"])).is_err());
        assert!(parse_args(args(&["--addr", "127.0.0.1:0", "--ipc", "unix:/tmp/a.sock"])).is_err());
    }

    /// 经本地 IPC 完成握手并调用一次工具（Unix 套接字放在临时目录、Windows 用每进程独立管道名，不碰默认端点）。
    #[tokio::test(flavor = "current_thread")]
    async fn serves_over_ipc() {
        use app_mcp_protocol::endpoint::IPC_WS_URL;
        #[cfg(unix)]
        let dir = std::env::temp_dir().join(format!("fake-host-test-{}", std::process::id()));
        #[cfg(unix)]
        let endpoint = {
            std::fs::create_dir_all(&dir).unwrap();
            format!("unix:{}", dir.join("h.sock").display())
        };
        #[cfg(windows)]
        let endpoint = format!(r"pipe:\\.\pipe\fake-host-test-{}", std::process::id());
        let opts = parse_args(args(&["--ipc", &endpoint, "--invoke", "t"])).unwrap();
        let (listener, shown) = Listener::bind(&opts).await.unwrap();
        assert_eq!(shown, endpoint);
        let host = tokio::spawn(async move { run(listener, opts).await });

        #[cfg(unix)]
        let stream = tokio::net::UnixStream::connect(dir.join("h.sock")).await.unwrap();
        #[cfg(windows)]
        let stream = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(&endpoint["pipe:".len()..])
            .unwrap();
        let (mut ws, _) = tokio_tungstenite::client_async(IPC_WS_URL, stream).await.unwrap();
        let hello = Message::request(RequestId::Number(1), method::HELLO, json!({}));
        ws.send(WsMessage::text(hello.to_json())).await.unwrap();
        let reply = ws.next().await.unwrap().unwrap().into_text().unwrap();
        assert!(reply.contains("fake-"), "{reply}");
        ws.send(WsMessage::text(Message::notification(method::READY, json!({})).to_json()))
            .await
            .unwrap();
        let invoke = Message::parse(&ws.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
        let Message::Request(req) = invoke else { panic!("应为 tools/invoke 请求") };
        assert_eq!(req.method, method::TOOLS_INVOKE);
        ws.send(WsMessage::text(Message::result(req.id, json!({"data": 1})).to_json()))
            .await
            .unwrap();
        // fake host 完成后关闭连接。
        while let Some(Ok(_)) = ws.next().await {}
        host.await.unwrap().unwrap();
        #[cfg(unix)]
        {
            assert!(!dir.join("h.sock").exists(), "退出后应删除套接字文件");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}
