//! 运行时线程：驱动核心、维护与 Host 的连接（WebSocket over 回环 TCP / Unix 域套接字 / 命名管道）与计时。
//!
//! 每轮循环先持锁取出核心的全部事件（[`Shared::drain`]），释放锁后执行 I/O 动作、把用户回调
//! 投递到分发线程；然后等待「唤醒 / 定时器到期 / 连接建立 / 收到消息」之一，再把结果交给核心。
//!
//! 休眠（核心进入 `Dormant`）时：连接关闭后 [`drive`] 返回，tokio 运行时被销毁（不再有 I/O 驱动、
//! 定时器），线程在条件变量上阻塞（[`park`]）。期间的用户回调照常投递到分发线程；出现需要 I/O 的动作
//! （`Connect`）或定时器时重建运行时。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::time::Duration;

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use app_mcp_protocol::{ConnectionErrorCode, ConnectionIssue, Endpoint};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message as WsMessage};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::{Action, Job, LogLevel, Shared, epoch, lock_ignore_poison, now_ms};

/// 底层字节流：TCP、Unix 域套接字或命名管道客户端。
trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

type WsStream = WebSocketStream<MaybeTlsStream<Box<dyn Io>>>;
type ConnectFuture = Pin<Box<dyn Future<Output = Result<WsStream, WsError>> + Send>>;

/// 关闭连接时最多等待写端发送 Close 帧的时间。
const CLOSE_GRACE: Duration = Duration::from_secs(1);

/// 一条已建立的连接：读端留在运行时循环里，写端是一个独立任务。
struct Conn {
    tx: UnboundedSender<WsMessage>,
    reader: SplitStream<WsStream>,
    writer: JoinHandle<()>,
}

impl Conn {
    fn new(ws: WsStream) -> Self {
        let (sink, reader) = ws.split();
        let (tx, rx) = unbounded_channel();
        let writer = tokio::spawn(write_loop(sink, rx));
        Self { tx, reader, writer }
    }

    /// 关闭连接：丢弃发送端，写任务发完剩余消息后发送 Close 帧并结束。
    fn close(self) -> JoinHandle<()> {
        drop(self.tx);
        drop(self.reader);
        self.writer
    }
}

async fn write_loop(
    mut sink: SplitSink<WsStream, WsMessage>,
    mut rx: UnboundedReceiver<WsMessage>,
) {
    while let Some(msg) = rx.recv().await {
        if sink.send(msg).await.is_err() {
            // 连接已坏：读端会随之报错并触发断线处理。
            return;
        }
    }
    let _ = tokio::time::timeout(CLOSE_GRACE, sink.close()).await;
}

/// 一次等待的结果。
enum Wakeup {
    Notified,
    Timer,
    Connected(Result<Box<WsStream>, WsError>),
    Incoming(Option<Result<WsMessage, WsError>>),
}

fn deadline(at: u64) -> tokio::time::Instant {
    tokio::time::Instant::from_std(epoch() + Duration::from_millis(at))
}

/// 连接目标。
pub(crate) struct Target {
    pub endpoint: Endpoint,
    pub connect_timeout: Duration,
}

/// [`drive`] 返回的原因。
enum Exit {
    /// 客户端已被丢弃。
    Shutdown,
    /// 核心已休眠、连接已关闭：可以释放运行时。
    Dormant,
}

pub(crate) fn build_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

/// 运行时线程主函数：驱动核心；休眠时销毁运行时并阻塞，需要时重建。
pub(crate) fn run(
    first: tokio::runtime::Runtime,
    shared: Arc<Shared>,
    jobs: Sender<Job>,
    target: Target,
) {
    let mut rt = Some(first);
    let mut carry: Vec<Action> = Vec::new();
    loop {
        let runtime = match rt.take() {
            Some(r) => r,
            None => match build_runtime() {
                Ok(r) => r,
                Err(e) => {
                    log(
                        &shared,
                        &jobs,
                        LogLevel::Error,
                        format!("无法重建运行时：{e}，1 秒后重试"),
                    );
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
            },
        };
        shared.runtime_active.store(true, Ordering::SeqCst);
        let exit = runtime.block_on(drive(
            shared.clone(),
            jobs.clone(),
            &target,
            std::mem::take(&mut carry),
        ));
        drop(runtime);
        shared.runtime_active.store(false, Ordering::SeqCst);
        match exit {
            Exit::Shutdown => return,
            Exit::Dormant => match park(&shared, &jobs) {
                Some(actions) => carry = actions,
                None => return,
            },
        }
    }
}

/// 休眠：没有运行时，在条件变量上阻塞。返回需要运行时处理的动作；客户端被丢弃时返回 `None`。
fn park(shared: &Arc<Shared>, jobs: &Sender<Job>) -> Option<Vec<Action>> {
    loop {
        let seen = *lock_ignore_poison(&shared.park);
        let batch = shared.drain();
        let mut io = Vec::new();
        for action in batch.actions {
            match action {
                Action::Dispatch(job) => {
                    let _ = jobs.send(job);
                }
                other => io.push(other),
            }
        }
        if batch.shutdown {
            return None;
        }
        if batch.timeout.is_some() || io.iter().any(|a| matches!(a, Action::Connect)) {
            return Some(io);
        }
        let mut gen_guard = lock_ignore_poison(&shared.park);
        while *gen_guard == seen {
            gen_guard = shared
                .park_cv
                .wait(gen_guard)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

async fn drive(
    shared: Arc<Shared>,
    jobs: Sender<Job>,
    target: &Target,
    initial: Vec<Action>,
) -> Exit {
    let host_url = target.endpoint.to_string();
    let mut conn: Option<Conn> = None;
    let mut connecting: Option<ConnectFuture> = None;
    let mut closing: Vec<JoinHandle<()>> = Vec::new();
    let mut initial = Some(initial);

    loop {
        let mut batch = shared.drain();
        if let Some(mut first) = initial.take() {
            first.append(&mut batch.actions);
            batch.actions = first;
        }
        for action in batch.actions {
            match action {
                Action::Connect => {
                    if let Some(old) = conn.take() {
                        closing.push(old.close());
                    }
                    connecting = Some(connect(target.endpoint.clone(), target.connect_timeout));
                }
                Action::Send(text) => {
                    if let Some(c) = &conn {
                        let _ = c.tx.send(WsMessage::text(text));
                    }
                }
                Action::Disconnect => {
                    connecting = None;
                    if let Some(old) = conn.take() {
                        closing.push(old.close());
                    }
                }
                Action::Dispatch(job) => {
                    let _ = jobs.send(job);
                }
            }
        }
        closing.retain(|h| !h.is_finished());
        if batch.shutdown {
            break;
        }
        if batch.dormant && conn.is_none() && connecting.is_none() {
            // 休眠：等 Close 帧发出后释放运行时。
            for h in closing {
                let _ = tokio::time::timeout(CLOSE_GRACE, h).await;
            }
            return Exit::Dormant;
        }

        let timer = async {
            match batch.timeout {
                Some(at) => tokio::time::sleep_until(deadline(at)).await,
                None => std::future::pending().await,
            }
        };
        let wakeup = tokio::select! {
            _ = shared.wake.notified() => Wakeup::Notified,
            _ = timer => Wakeup::Timer,
            res = async {
                match connecting.as_mut() {
                    Some(f) => f.await,
                    None => std::future::pending().await,
                }
            }, if connecting.is_some() => {
                Wakeup::Connected(res.map(Box::new))
            }
            msg = async {
                match conn.as_mut() {
                    Some(c) => c.reader.next().await,
                    None => std::future::pending().await,
                }
            }, if conn.is_some() => {
                Wakeup::Incoming(msg)
            }
        };

        match wakeup {
            Wakeup::Notified => {}
            Wakeup::Timer => shared.lock().client.handle_timeout(now_ms()),
            Wakeup::Connected(Ok(ws)) => {
                connecting = None;
                conn = Some(Conn::new(*ws));
                shared.lock().client.handle_connected(now_ms());
            }
            Wakeup::Connected(Err(e)) => {
                connecting = None;
                let issue = connect_issue(&host_url, &e);
                log(&shared, &jobs, LogLevel::Info, issue.to_string());
                shared.lock().client.handle_connect_failed(issue, now_ms());
            }
            Wakeup::Incoming(Some(Ok(WsMessage::Text(text)))) => {
                shared.lock().client.handle_message(text.as_str(), now_ms());
            }
            Wakeup::Incoming(Some(Ok(WsMessage::Close(frame)))) => {
                if let Some(old) = conn.take() {
                    closing.push(old.close());
                }
                lost_connection(&shared, &jobs, closed_issue(&host_url, frame.as_ref()));
            }
            Wakeup::Incoming(None) => {
                if let Some(old) = conn.take() {
                    closing.push(old.close());
                }
                lost_connection(&shared, &jobs, closed_issue(&host_url, None));
            }
            Wakeup::Incoming(Some(Err(e))) => {
                if let Some(old) = conn.take() {
                    closing.push(old.close());
                }
                lost_connection(&shared, &jobs, lost_issue(&host_url, &e));
            }
            // 二进制帧、ping / pong 由 tungstenite 处理或忽略。
            Wakeup::Incoming(Some(Ok(_))) => {}
        }
    }

    // 退出：尽量让 Close 帧发出去。
    if let Some(old) = conn.take() {
        closing.push(old.close());
    }
    for h in closing {
        let _ = tokio::time::timeout(CLOSE_GRACE, h).await;
    }
    Exit::Shutdown
}

/// 建立连接并完成 WebSocket 握手，超时按失败处理。
fn connect(endpoint: Endpoint, timeout: Duration) -> ConnectFuture {
    Box::pin(async move {
        match tokio::time::timeout(timeout, open(&endpoint)).await {
            Ok(result) => result,
            Err(_) => Err(WsError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{} ms 内未能建立连接", timeout.as_millis()),
            ))),
        }
    })
}

async fn open(endpoint: &Endpoint) -> Result<WsStream, WsError> {
    let (io, url): (Box<dyn Io>, &str) = match endpoint {
        Endpoint::WebSocket(url) => {
            if url.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("wss://")) {
                // 进程内只需安装一次；已安装（包括其他库安装的）时忽略错误。
                let _ = rustls::crypto::ring::default_provider().install_default();
            }
            (Box::new(tcp_connect(url).await?), url.as_str())
        }
        Endpoint::Unix(path) => (Box::new(unix_connect(path).await?), app_mcp_protocol::endpoint::IPC_WS_URL),
        Endpoint::Pipe(name) => (Box::new(pipe_connect(name).await?), app_mcp_protocol::endpoint::IPC_WS_URL),
    };
    // IPC 上的 URL 为 ws://，不做 TLS；wss:// 由 tokio-tungstenite 完成 TLS 握手。
    tokio_tungstenite::client_async_tls(url, io)
        .await
        .map(|(ws, _)| ws)
}

async fn tcp_connect(url: &str) -> Result<TcpStream, WsError> {
    let request = url.into_client_request()?;
    let uri = request.uri();
    let host = uri
        .host()
        .map(|h| h.trim_start_matches('[').trim_end_matches(']').to_owned())
        .ok_or(WsError::Url(tokio_tungstenite::tungstenite::error::UrlError::NoHostName))?;
    let secure = url.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("wss://"));
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let socket = TcpStream::connect((host.as_str(), port)).await?;
    socket.set_nodelay(true)?;
    Ok(socket)
}

/// 连接 Unix 域套接字，并确认监听方与本进程是同一用户（防止他人抢占路径冒充 Host）。
#[cfg(unix)]
async fn unix_connect(path: &std::path::Path) -> Result<tokio::net::UnixStream, WsError> {
    app_mcp_protocol::endpoint::check_unix_socket_path(path)
        .map_err(|issue| WsError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, issue)))?;
    let stream = tokio::net::UnixStream::connect(path).await?;
    let cred = stream.peer_cred()?;
    let me = app_mcp_protocol::endpoint::current_uid();
    if cred.uid() != me {
        return Err(WsError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} 的监听方属于其他用户（uid {}），拒绝连接", path.display(), cred.uid()),
        )));
    }
    Ok(stream)
}

#[cfg(not(unix))]
async fn unix_connect(path: &std::path::Path) -> Result<TcpStream, WsError> {
    Err(WsError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("本平台不支持 Unix 域套接字端点 unix:{}", path.display()),
    )))
}

/// 连接命名管道，并确认管道所有者是当前用户（Hub 创建管道时把所有者设为自己；他人无法伪造）。
#[cfg(windows)]
async fn pipe_connect(
    name: &str,
) -> Result<tokio::net::windows::named_pipe::NamedPipeClient, WsError> {
    use std::os::windows::io::AsRawHandle;
    use app_mcp_protocol::endpoint::win;
    /// 所有实例都在使用中（Hub 正在创建下一个实例）。
    const ERROR_PIPE_BUSY: i32 = 231;
    let client = loop {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(name) {
            Ok(c) => break c,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e.into()),
        }
    };
    let owner = win::handle_owner_sid(client.as_raw_handle())?;
    let me = win::current_user_sid()?;
    if owner != me {
        return Err(WsError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("命名管道 {name} 的所有者（{owner}）不是当前用户，拒绝连接"),
        )));
    }
    Ok(client)
}

#[cfg(not(windows))]
async fn pipe_connect(name: &str) -> Result<TcpStream, WsError> {
    Err(WsError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("本平台不支持命名管道端点 pipe:{name}"),
    )))
}

/// 建立连接失败的原因：按系统错误归类错误码（spec/protocol.md 10.1）。
fn connect_issue(host_url: &str, e: &WsError) -> ConnectionIssue {
    // 已归类的问题（如 IPC_PATH_TOO_LONG）原样使用，说明中已带建议。
    if let WsError::Io(io) = e
        && let Some(issue) = io.get_ref().and_then(|inner| inner.downcast_ref::<ConnectionIssue>())
    {
        return ConnectionIssue::new(issue.code, format!("连接 {host_url} 失败：{}", issue.message));
    }
    let code = match e {
        WsError::Io(io) => app_mcp_protocol::diagnostic::connect_error_code(io.kind()),
        _ => ConnectionErrorCode::ConnectFailed,
    };
    ConnectionIssue::new(code, format!("连接 {host_url} 失败：{e}"))
}

/// 已建立的连接断开：记日志（带断开前的连接 ID），核心进入带错误码的 `Backoff`。
fn lost_connection(shared: &Shared, jobs: &Sender<Job>, issue: ConnectionIssue) {
    log(shared, jobs, LogLevel::Info, issue.to_string());
    shared.lock().client.handle_disconnected_with(issue, now_ms());
}

/// 对端正常关闭（Close 帧或读到连接结束）的原因（spec/protocol.md 10.1 `CONNECTION_CLOSED`）。
fn closed_issue(host_url: &str, frame: Option<&CloseFrame>) -> ConnectionIssue {
    let detail = match frame {
        Some(f) if !f.reason.is_empty() => format!("（关闭码 {}：{}）", u16::from(f.code), f.reason),
        Some(f) => format!("（关闭码 {}）", u16::from(f.code)),
        None => String::new(),
    };
    ConnectionIssue::new(ConnectionErrorCode::ConnectionClosed, format!("Host 关闭了连接 {host_url}{detail}，稍后重连"))
}

/// 连接因错误中断（未经关闭握手）的原因（spec/protocol.md 10.1 `CONNECTION_LOST`）。
fn lost_issue(host_url: &str, e: &WsError) -> ConnectionIssue {
    ConnectionIssue::new(ConnectionErrorCode::ConnectionLost, format!("与 {host_url} 的连接中断：{e}，稍后重连"))
}

/// 记录日志；连接期间以 `[连接 ID] ` 开头。调用时不得持有 `shared` 的锁。
fn log(shared: &Shared, jobs: &Sender<Job>, level: LogLevel, message: String) {
    if let Some(listener) = shared.listener.clone() {
        let message = format!("{}{message}", crate::cid_prefix(&shared.lock().client));
        let _ = jobs.send(Box::new(move || listener.on_log(level, message)));
    }
}
