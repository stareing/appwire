//! 名字服务（spec/naming.md）："被连接方"一侧：在系统名字服务登记本 App 的名字，把 Hub 的每次拨号
//! 变成一条通道交给运行时（[`ChannelSink`]），运行时在通道上跑与 App 拨出连接相同的帧与核心。
//!
//! - [`NameServer`]：平台无关的登记接口；[`platform`] 返回本平台的实现（Linux：D-Bus 会话总线，[`dbus`]；
//!   Windows：每 App 每用户命名管道，[`pipe`]；macOS：launchd 用户 Agent 的按需套接字，[`launchd`]）。
//! - 登记的生命周期：[`crate::NativeClient::start`] 之后由运行时线程登记，`stop` / 客户端被丢弃时注销
//!   （丢弃 [`Registration`]）。登记期间运行时线程保持 tokio 运行时（阻塞在名字服务连接上，无定时器）。

// @why 只有平台实现（Linux D-Bus、Windows 命名管道、macOS launchd）使用这些接口；其他平台 / 关闭 `dbus` 时登记直接报告"不支持"。
#![cfg_attr(
    not(any(windows, target_os = "macos", all(target_os = "linux", not(target_env = "ohos"), feature = "dbus"))),
    allow(dead_code)
)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use app_mcp_core::ConnectionState;

use crate::{Shared, now_ms};

#[cfg(all(target_os = "linux", not(target_env = "ohos"), feature = "dbus"))]
mod dbus;
#[cfg(windows)]
mod pipe;
// @why 接受循环与拒绝在 Linux 上以真实 Unix 套接字测试；`launch_activate_socket` 只在 macOS 编译。
#[cfg(any(target_os = "macos", all(test, unix)))]
mod launchd;

/// 一条由 Hub 拨入的通道（App 一侧的一端）。
#[cfg(unix)]
pub(crate) type Channel = std::os::unix::net::UnixStream;
/// 一条由 Hub 拨入的通道：已连接的命名管道服务端实例（spec/naming.md 4.3，Windows 不传 fd）。
#[cfg(windows)]
pub(crate) type Channel = tokio::net::windows::named_pipe::NamedPipeServer;
/// 本平台没有名字服务通道。
#[cfg(not(any(unix, windows)))]
pub(crate) enum Channel {}

/// 要登记的名字（spec/naming.md 2.1）。
#[derive(Clone, Debug)]
pub(crate) struct NameRequest {
    pub app_id: String,
    /// 登记实例名；`Some` 时另登记实例名字。
    // @why macOS launchd 作业只有一个套接字，不登记实例名字（spec/naming.md 4.4）。
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub instance: Option<String>,
    /// 名字服务地址（D-Bus 地址）；`None` = 按环境。
    // @why Windows 命名管道、macOS launchd 没有"名字服务地址"。
    #[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
    pub address: Option<String>,
}

/// App 拒绝一次拨号的原因（公开类型，见 [`crate::ChannelRefusal`]）。
pub(crate) use crate::ChannelRefusal as Refusal;

/// 接收拨入的通道：接受时交给运行时，拒绝时把通道交还调用方（Windows 在其上写拒绝行后断开）。
pub(crate) trait ChannelSink: Send + Sync + 'static {
    fn try_offer(&self, channel: Channel) -> Result<(), (Refusal, Channel)>;

    /// 同 [`ChannelSink::try_offer`]，拒绝时通道随之关闭。
    #[cfg(unix)]
    fn offer(&self, channel: Channel) -> Result<(), Refusal> {
        self.try_offer(channel).map_err(|(refusal, _)| refusal)
    }
}

/// 已登记的名字；被丢弃时注销（关闭与名字服务的连接，系统随之释放名字）。
pub(crate) trait Registration: Send {
    /// 实际拥有的名字（日志用）。
    fn names(&self) -> Vec<String>;
}

pub(crate) type RegisterFuture<'a> = Pin<Box<dyn Future<Output = Result<Box<dyn Registration>, String>> + Send + 'a>>;

/// 平台名字服务。
pub(crate) trait NameServer: Send + Sync {
    /// 登记名字，之后的拨号交给 `sink`。必须在运行时线程的 tokio 运行时内调用。
    fn register<'a>(&'a self, request: &'a NameRequest, sink: Arc<dyn ChannelSink>) -> RegisterFuture<'a>;
}

/// 本平台的名字服务；不支持时为 `None`。
pub(crate) fn platform() -> Option<&'static dyn NameServer> {
    #[cfg(all(target_os = "linux", not(target_env = "ohos"), feature = "dbus"))]
    {
        static DBUS: dbus::DbusNameServer = dbus::DbusNameServer;
        Some(&DBUS)
    }
    #[cfg(windows)]
    {
        static PIPE: pipe::PipeNameServer = pipe::PipeNameServer;
        Some(&PIPE)
    }
    #[cfg(target_os = "macos")]
    {
        static LAUNCHD: launchd::LaunchdNameServer = launchd::LaunchdNameServer;
        Some(&LAUNCHD)
    }
    #[cfg(not(any(windows, target_os = "macos", all(target_os = "linux", not(target_env = "ohos"), feature = "dbus"))))]
    {
        None
    }
}

/// 名字服务把拨入的通道交给核心（[`app_mcp_core::Client::accept_channel`]）。
///
/// @invariant 只持弱引用：名字服务连接由运行时线程持有，不能反过来让 [`Shared`] 存活。
pub(crate) struct ChannelInbox(pub(crate) std::sync::Weak<Shared>);

impl ChannelSink for ChannelInbox {
    fn try_offer(&self, channel: Channel) -> Result<(), (Refusal, Channel)> {
        let Some(shared) = self.0.upgrade() else { return Err((Refusal::Stopped, channel)) };
        {
            let mut st = shared.lock();
            if st.stopped || *st.client.state() == ConnectionState::Idle {
                return Err((Refusal::Stopped, channel));
            }
            if !st.client.accept_channel(now_ms()) {
                return Err((Refusal::Busy, channel));
            }
            st.channel = Some(channel);
        }
        shared.wake();
        Ok(())
    }
}

/// 写出拒绝行后等待 Hub 读取并关闭的上限（之后 App 断开；只在拒绝路径上计时）。
#[cfg(any(windows, target_os = "macos", all(test, unix)))]
const REFUSAL_LINGER: std::time::Duration = std::time::Duration::from_secs(2);

/// 在通道上写一行 `<CODE>：<说明>` 后等 Hub 读取并关闭（最长 [`REFUSAL_LINGER`]），再断开（spec/naming.md 4.3"拒绝"，
/// Windows 命名管道与 macOS launchd 套接字共用：二者都没有可回错误的方法调用）。
///
/// @why 直接断开会丢弃 Hub 尚未读取的数据，Hub 就只能看到"对端关闭"而不知道原因。
#[cfg(any(windows, target_os = "macos", all(test, unix)))]
pub(crate) async fn refuse<S>(mut stream: S, refusal: Refusal)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use app_mcp_protocol::naming::{codes, pipe as names};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let code = match refusal {
        Refusal::Busy => codes::CHANNEL_LIMIT,
        Refusal::Stopped | Refusal::Invalid(_) => codes::ACTIVATION_DENIED,
    };
    let mut line = names::refusal_line(code, &refusal.to_string());
    line.push('\n');
    if stream.write_all(line.as_bytes()).await.is_err() {
        return;
    }
    let mut sink = [0u8; 256];
    let _ = tokio::time::timeout(REFUSAL_LINGER, async {
        while matches!(stream.read(&mut sink).await, Ok(n) if n > 0) {}
    })
    .await;
}
