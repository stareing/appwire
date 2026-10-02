//! macOS：launchd 用户 Agent 的按需套接字（spec/naming.md 4.4）。
//!
//! App 的 LaunchAgent（标签 `dev.appmcp.App.<appId>`）在 `Sockets` 中声明键 `AppMcp` 的 Unix 套接字；launchd 持有监听端，
//! Hub 连接时按需启动作业。作业进程以 `launch_activate_socket("AppMcp")` 取得监听 fd，阻塞在 `accept` 上（无定时器）；
//! 每个接受的连接就是一条通道：与 D-Bus `Open()` 拨入的通道相同，SDK 在其上作为 WebSocket 客户端先发 `app/hello`。
//!
//! - 只有 launchd 启动的进程能取得套接字（`ESRCH`）；用户从 Finder 打开的 GUI 进程登记失败（记录警告），继续走 App 拨 Hub 路径（U-09）。
//! - 实例名字不登记：一个作业只有一个套接字（4.4）。
//! - 拒绝（已有连接 / SDK 已停止）：在连接上写一行 `<CODE>：<说明>` 后断开（与 Windows 相同，[`super::refuse`]）。

use std::sync::Arc;

use app_mcp_protocol::naming::launchd as names;
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

use super::{ChannelSink, Registration};

#[cfg(target_os = "macos")]
pub(crate) struct LaunchdNameServer;

#[cfg(target_os = "macos")]
impl super::NameServer for LaunchdNameServer {
    fn register<'a>(&'a self, request: &'a super::NameRequest, sink: Arc<dyn ChannelSink>) -> super::RegisterFuture<'a> {
        Box::pin(async move { register(request, sink).map(|r| Box::new(r) as Box<dyn Registration>) })
    }
}

/// 已登记的套接字；被丢弃时停止接受任务（关闭本次登记复制的 fd；launchd 交来的原始 fd 留在进程内，见 [`sys`]）。
struct LaunchdRegistration {
    names: Vec<String>,
    tasks: Vec<JoinHandle<()>>,
}

impl Registration for LaunchdRegistration {
    fn names(&self) -> Vec<String> {
        self.names.clone()
    }
}

impl Drop for LaunchdRegistration {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// 取得 launchd 交来的监听套接字并开始接受。必须在运行时内调用。
#[cfg(target_os = "macos")]
fn register(request: &super::NameRequest, sink: Arc<dyn ChannelSink>) -> Result<LaunchdRegistration, String> {
    let fds = sys::activated_sockets(names::SOCKET_KEY)?;
    let listeners = fds
        .into_iter()
        .map(|fd| listener(std::os::unix::net::UnixListener::from(fd)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(serve_all(&request.app_id, listeners, sink))
}

/// std 监听端 → tokio（非阻塞）。
fn listener(std: std::os::unix::net::UnixListener) -> Result<UnixListener, String> {
    std.set_nonblocking(true).map_err(|e| format!("无法把 launchd 套接字设为非阻塞：{e}"))?;
    UnixListener::from_std(std).map_err(|e| format!("无法接管 launchd 套接字：{e}"))
}

/// 在每个监听端上起一个接受任务。
fn serve_all(app_id: &str, listeners: Vec<UnixListener>, sink: Arc<dyn ChannelSink>) -> LaunchdRegistration {
    let tasks = listeners.into_iter().map(|l| tokio::spawn(serve(l, sink.clone()))).collect();
    LaunchdRegistration { names: vec![names::label(app_id)], tasks }
}

/// 接受任务：每个连接核对对端 uid 后交给运行时，或写拒绝行后断开。空闲时阻塞在 `accept` 上（无定时器、不轮询）。
///
/// @security 只接受同一用户的连接（`getpeereid`；套接字文件 `0600` 已限制，这里再核对一次，spec/naming.md 10.1）。
/// @error 监听端出现非暂时性错误时任务结束；之后 Hub 的连接排在队列中直到超时（`ACTIVATION_TIMEOUT`）。
async fn serve(listener: UnixListener, sink: Arc<dyn ChannelSink>) {
    let me = app_mcp_protocol::endpoint::current_uid();
    loop {
        let stream = match listener.accept().await {
            Ok((s, _)) => s,
            Err(e) if transient(&e) => continue,
            Err(_) => return,
        };
        if !stream.peer_cred().is_ok_and(|c| c.uid() == me) {
            continue;
        }
        let Ok(channel) = stream.into_std() else { continue };
        if let Err((refusal, channel)) = sink.try_offer(channel) {
            tokio::spawn(async move {
                let back = channel.set_nonblocking(true).and_then(|()| tokio::net::UnixStream::from_std(channel));
                if let Ok(stream) = back {
                    super::refuse(stream, refusal).await;
                }
            });
        }
    }
}

/// 对端在 `accept` 前放弃等可忽略的错误。
fn transient(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
    )
}

/// `launch_activate_socket(3)` 的错误码 → 说明。
fn activation_error(code: i32) -> String {
    match code {
        names::ACTIVATE_ENOENT => format!(
            "launchd plist 的 Sockets 中没有键 {}（重新 app-mcp-host app install 或按 spec/naming.md 4.4 补全 Agent plist）",
            names::SOCKET_KEY
        ),
        names::ACTIVATE_ESRCH => "本进程不是由 launchd 启动的（用户直接打开的进程不持有按名寻址的套接字，走 App 拨 Hub 路径）".to_owned(),
        other => format!("launch_activate_socket 失败：{}", std::io::Error::from_raw_os_error(other)),
    }
}

/// [`names::activate_socket`] 的失败 → 说明。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn activate_failure(e: names::ActivateSocketError) -> String {
    match e {
        names::ActivateSocketError::Os(code) => activation_error(code),
        names::ActivateSocketError::Count(n) => format!("launchd 交来的套接字数量异常：{n}"),
        names::ActivateSocketError::InvalidKey => format!("套接字键 {:?} 含 NUL", names::SOCKET_KEY),
    }
}

/// launchd 交来的套接字（FFI 在 [`app_mcp_protocol::naming::launchd::activate_socket`]，G-06）。
#[cfg(target_os = "macos")]
mod sys {
    use std::os::fd::OwnedFd;
    use std::sync::OnceLock;

    /// launchd 交来的原始 fd：每个进程只能取一次（再取为 `EALREADY`），因此取到后留在进程内、不关闭；
    /// 每次登记复制一份（[`activated_sockets`]），登记结束只关闭副本。
    static ACTIVATED: OnceLock<Result<Vec<OwnedFd>, String>> = OnceLock::new();

    /// 取得键 `key` 的监听套接字（每次返回新的副本）。
    pub(super) fn activated_sockets(key: &str) -> Result<Vec<OwnedFd>, String> {
        let fds = ACTIVATED
            .get_or_init(|| super::names::activate_socket(key).map_err(super::activate_failure))
            .as_ref()
            .map_err(Clone::clone)?;
        fds.iter()
            .map(|fd| fd.try_clone().map_err(|e| format!("无法复制 launchd 套接字：{e}")))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use app_mcp_protocol::naming::codes;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::super::{Channel, Refusal};
    use super::*;

    /// 替身运行时：按预设接受或拒绝，接受的通道保存下来。
    struct FakeSink {
        refusal: Option<Refusal>,
        accepted: Mutex<Vec<Channel>>,
    }

    impl ChannelSink for FakeSink {
        fn try_offer(&self, channel: Channel) -> Result<(), (Refusal, Channel)> {
            match &self.refusal {
                Some(r) => Err((r.clone(), channel)),
                None => {
                    self.accepted.lock().unwrap().push(channel);
                    Ok(())
                }
            }
        }
    }

    fn socket_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("amcp-ld-{:016x}.sock", rand::random::<u64>()))
    }

    #[tokio::test]
    async fn accepted_connections_become_channels() {
        let path = socket_path();
        let l = listener(std::os::unix::net::UnixListener::bind(&path).unwrap()).unwrap();
        let sink = Arc::new(FakeSink { refusal: None, accepted: Mutex::new(Vec::new()) });
        let reg = serve_all("my-shop", vec![l], sink.clone());
        assert_eq!(reg.names(), ["dev.appmcp.App.my-shop"]);

        let mut hub = tokio::net::UnixStream::connect(&path).await.unwrap();
        hub.write_all(b"ping").await.unwrap();
        let mut got = None;
        for _ in 0..200 {
            if let Some(c) = sink.accepted.lock().unwrap().pop() {
                got = Some(c);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let mut app = got.expect("连接交给了运行时");
        let mut buf = [0u8; 4];
        std::io::Read::read_exact(&mut app, &mut buf).unwrap();
        assert_eq!(&buf, b"ping", "通道就是 Hub 连上的那条连接");

        // 登记被丢弃：接受任务停止，新连接不再被接受（连接排队，不交给运行时）。
        drop(reg);
        tokio::task::yield_now().await;
        let _late = tokio::net::UnixStream::connect(&path).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(sink.accepted.lock().unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn refused_connection_gets_refusal_line() {
        let path = socket_path();
        let l = listener(std::os::unix::net::UnixListener::bind(&path).unwrap()).unwrap();
        let sink = Arc::new(FakeSink { refusal: Some(Refusal::Busy), accepted: Mutex::new(Vec::new()) });
        let _reg = serve_all("shop", vec![l], sink);
        let mut hub = tokio::net::UnixStream::connect(&path).await.unwrap();
        let mut text = String::new();
        let read = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut buf = [0u8; 512];
            loop {
                let n = hub.read(&mut buf).await.unwrap();
                text.push_str(&String::from_utf8_lossy(&buf[..n]));
                if n == 0 || text.contains('\n') {
                    break;
                }
            }
        });
        read.await.expect("拒绝行及时送达");
        assert_eq!(names::parse_refusal(text.trim_end()).0, codes::CHANNEL_LIMIT, "{text}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn activation_errors_explain_cause() {
        assert!(activation_error(2).contains("AppMcp"));
        assert!(activation_error(3).contains("不是由 launchd 启动"));
        assert!(activation_error(37).starts_with("launch_activate_socket 失败"));
        assert!(activate_failure(names::ActivateSocketError::Os(3)).contains("不是由 launchd 启动"));
        assert!(activate_failure(names::ActivateSocketError::Count(0)).contains("数量异常"));
        assert!(activate_failure(names::ActivateSocketError::InvalidKey).contains("NUL"));
    }
}
