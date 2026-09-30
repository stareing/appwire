//! 运行时线程：驱动核心、维护 WebSocket 连接与计时。
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
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{Error as WsError, Message as WsMessage};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::{Action, Job, LogLevel, Shared, epoch, lock_ignore_poison, now_ms};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
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
    pub url: String,
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
    let host_url = target.url.clone();
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
                    connecting = Some(connect(host_url.clone(), target.connect_timeout));
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
                log(
                    &shared,
                    &jobs,
                    LogLevel::Debug,
                    format!("连接 {host_url} 失败：{e}"),
                );
                shared.lock().client.handle_disconnected(now_ms());
            }
            Wakeup::Incoming(Some(Ok(WsMessage::Text(text)))) => {
                shared.lock().client.handle_message(text.as_str(), now_ms());
            }
            Wakeup::Incoming(Some(Ok(WsMessage::Close(_))) | None) => {
                if let Some(old) = conn.take() {
                    closing.push(old.close());
                }
                log(&shared, &jobs, LogLevel::Info, "Host 关闭了连接".to_owned());
                shared.lock().client.handle_disconnected(now_ms());
            }
            Wakeup::Incoming(Some(Err(e))) => {
                if let Some(old) = conn.take() {
                    closing.push(old.close());
                }
                log(&shared, &jobs, LogLevel::Info, format!("连接中断：{e}"));
                shared.lock().client.handle_disconnected(now_ms());
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

/// 建立连接（ws:// 或 wss://），超时按失败处理。
fn connect(url: String, timeout: Duration) -> ConnectFuture {
    Box::pin(async move {
        if url.len() >= 6 && url[..6].eq_ignore_ascii_case("wss://") {
            // 进程内只需安装一次；已安装（包括其他库安装的）时忽略错误。
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        match tokio::time::timeout(timeout, tokio_tungstenite::connect_async(url.as_str())).await {
            Ok(result) => result.map(|(ws, _)| ws),
            Err(_) => Err(WsError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{} ms 内未能建立连接", timeout.as_millis()),
            ))),
        }
    })
}

fn log(shared: &Shared, jobs: &Sender<Job>, level: LogLevel, message: String) {
    if let Some(listener) = shared.listener.clone() {
        let _ = jobs.send(Box::new(move || listener.on_log(level, message)));
    }
}
