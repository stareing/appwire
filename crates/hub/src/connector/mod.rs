//! 名字服务连接器（spec/naming.md）：Hub 一侧的"发现 / 拨号 / 激活"抽象。
//!
//! - 发现只读名字服务的列表与事件（[`Connector::discover`]、[`Connector::watch`]），从不为发现而启动进程（5.1）。
//! - 拨号（[`Connector::dial`]）按地址打开一条通道；名字的所有者未运行时由系统激活（D-Bus 服务激活等）。
//!   得到的通道交给 App 连接服务，其上跑与本地 IPC 相同的帧与消息（SDK 先发 `app/hello`）。
//! - 连接器只做机制：何时拨号、何时关闭由 Hub 的路由与生命周期决定（7.2）。
//!
//! 平台实现：Linux D-Bus 会话总线（[`DbusConnector`]，cargo feature `dbus`）；由宿主语言实现发现与拨号的
//! [`HostedConnector`]（Unix，Android 经 hub-uniffi 的 Kotlin 实现，spec/naming.md 4.2）。

use std::fmt;

use app_mcp_manifest::Manifest;
use app_mcp_protocol::naming::Address;
use futures::stream::BoxStream;
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(all(target_os = "linux", not(target_env = "ohos"), feature = "dbus"))]
mod dbus;
#[cfg(all(target_os = "linux", not(target_env = "ohos"), feature = "dbus"))]
pub use dbus::DbusConnector;
#[cfg(unix)]
mod hosted;
#[cfg(unix)]
pub use hosted::{HostNameService, HostedChannel, HostedConnector, HostedName, naming_code};

/// 发现到的一个名字（App 的默认名字或一个登记实例）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredName {
    pub address: Address,
    /// 可被系统激活（只有默认名字可以，spec/naming.md 2.1）。
    pub activatable: bool,
    /// 所有者正在运行。
    pub running: bool,
    /// 平台名字（日志 / `apps.list` 用，如 D-Bus 总线名）。
    pub detail: String,
}

/// 名字服务事件（D-Bus `NameOwnerChanged` 等）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameEvent {
    /// 名字出现（所有者开始运行）。
    Appeared(Address),
    /// 名字消失（所有者退出 / 注销）。
    Vanished(Address),
    /// App 安装或更新（Android 包变更广播）：新增 / 更新发现记录（静态清单经 [`Connector::manifest`] 读取）。
    Installed(DiscoveredName),
    /// App 卸载：移除发现记录与来自安装元数据的清单（spec/naming.md 5.4"卸载事件"）。
    Removed(Address),
}

/// 通道两端的字节流。
pub trait ChannelIo: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> ChannelIo for T {}

/// 拨号得到的通道。
pub struct DialedChannel {
    pub stream: Box<dyn ChannelIo>,
    /// 对端进程号（操作系统提供时）。
    pub pid: Option<u32>,
}

impl fmt::Debug for DialedChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DialedChannel").field("pid", &self.pid).finish_non_exhaustive()
    }
}

/// 连接器错误：`code` 为 spec/naming.md 第 12 节的错误码（[`app_mcp_protocol::naming::codes`]）。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{code}：{message}")]
pub struct ConnectorError {
    pub code: &'static str,
    pub message: String,
}

impl ConnectorError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

/// 一个平台名字服务。
///
/// @invariant 实现不得为发现而激活任何进程；`dial` 返回后不保留对通道的任何引用（通道归调用方，关闭即释放）。
#[async_trait::async_trait]
pub trait Connector: Send + Sync + fmt::Debug + 'static {
    /// 来源名（发现记录的 `source` 细分，如 `"dbus"`）。
    fn kind(&self) -> &'static str;

    /// 一次性枚举名字（启动扫描，spec/naming.md 5.1）。
    async fn discover(&self) -> Result<Vec<DiscoveredName>, ConnectorError>;

    /// 名字出现 / 消失的事件流（无事件时不唤醒）；不支持时返回空流。
    async fn watch(&self) -> Result<BoxStream<'static, NameEvent>, ConnectorError>;

    /// 安装元数据中该 App 的静态清单（Android `<meta-data>` 清单资源）；没有时为 `None`（默认）。
    /// Hub 在发现 / 安装事件后读取，使未运行的 App 也能按清单列出工具（spec/naming.md 5.6）。
    fn manifest(&self, _app_id: &str) -> Option<Manifest> {
        None
    }

    /// 按地址拨号；名字的所有者未运行时由系统激活。`timeout` 为激活与 `Open` 合计的上限。
    async fn dial(&self, address: &Address, timeout: std::time::Duration) -> Result<DialedChannel, ConnectorError>;
}
