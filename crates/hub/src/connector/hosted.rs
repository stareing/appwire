//! 由宿主语言实现的名字服务（spec/naming.md 4.2 Android）：发现与拨号用平台 API 完成（Android：`PackageManager`
//! 读 `<meta-data>` 清单、`bindService` + Binder 换 fd），Rust 一侧只把它适配为 [`super::Connector`]。
//!
//! - 发现：[`HostNameService::discover`] 一次性枚举（只读安装元数据，不启动进程）；之后的变化由宿主在系统事件
//!   （包安装 / 更新 / 卸载广播）中调用 [`HostedConnector::installed`] / [`HostedConnector::removed`] 推送，无轮询。
//! - 拨号：[`HostNameService::dial`] 在阻塞线程上执行，返回 socketpair 的一端（fd）与一个租约号；通道被丢弃
//!   （Hub 宽限到期关闭、对端 EOF、拨号后校验失败）时调用一次 [`HostNameService::release`]（Android：`unbindService`）。
//! - 身份：宿主给出期望的对端 uid（Android：目标包的 uid）时，按 socketpair 对端凭据（`SO_PEERCRED`）核对，
//!   不一致即 `PEER_IDENTITY_MISMATCH`（spec/naming.md 10.3）。

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Duration;

use app_mcp_manifest::Manifest;
use app_mcp_protocol::naming::{Address, codes};
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use super::{ConnectorError, DialedChannel, DiscoveredName, NameEvent};

/// 宿主报告超时之外，Rust 一侧额外等待的时间：宿主自己的超时先生效，这里只兜底回调卡住的情况。
const DIAL_SLACK: Duration = Duration::from_secs(2);

/// 宿主发现的一个 App：名字与安装元数据中的静态清单。
#[derive(Clone, Debug)]
pub struct HostedName {
    pub name: DiscoveredName,
    /// 安装元数据中的静态清单（Android：`<meta-data android:name="dev.appmcp.manifest">` 指向的资源）。
    pub manifest: Option<Manifest>,
}

/// 宿主拨号得到的通道。
#[derive(Debug)]
pub struct HostedChannel {
    /// socketpair 的一端（所有权交给 Hub）。
    pub fd: OwnedFd,
    /// 宿主为这次拨号持有的系统资源（Android：一次 `bindService`）的编号；通道丢弃时以它调用 `release`。
    pub lease: u64,
    /// 期望的对端 uid（Android：目标包的 uid）；`None` = 不核对。
    pub peer_uid: Option<u32>,
}

/// 宿主语言实现的名字服务。方法在 Hub 的阻塞线程上调用，可以阻塞（`dial` 不超过给定的超时）。
///
/// @invariant `discover` 不得启动任何 App 进程；`dial` 失败时宿主自行释放已占用的资源（不再调用 `release`）；
/// 成功返回的每个租约恰好收到一次 `release`。
pub trait HostNameService: Send + Sync + 'static {
    /// 一次性枚举（Hub 启动时）。
    fn discover(&self) -> Result<Vec<HostedName>, ConnectorError>;
    /// 按地址拨号；目标进程未运行时由系统激活。
    fn dial(&self, address: &Address, timeout: Duration) -> Result<HostedChannel, ConnectorError>;
    /// 释放一次成功拨号持有的资源（Android：`unbindService`）。
    fn release(&self, lease: u64);
}

/// [`HostNameService`] → [`super::Connector`]。
pub struct HostedConnector {
    kind: &'static str,
    service: Arc<dyn HostNameService>,
    /// 安装元数据中的清单（[`super::Connector::manifest`]）。
    manifests: Mutex<HashMap<String, Manifest>>,
    events_tx: UnboundedSender<NameEvent>,
    /// 事件流只交给第一次 `watch`（Hub 每个连接器只订阅一次）。
    events_rx: Mutex<Option<UnboundedReceiver<NameEvent>>>,
}

impl std::fmt::Debug for HostedConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedConnector").field("kind", &self.kind).finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl HostedConnector {
    /// `kind` 为发现记录的来源名（如 `"android"`）。
    pub fn new(kind: &'static str, service: Arc<dyn HostNameService>) -> Self {
        let (events_tx, events_rx) = unbounded_channel();
        Self { kind, service, manifests: Mutex::new(HashMap::new()), events_tx, events_rx: Mutex::new(Some(events_rx)) }
    }

    /// 宿主推送：App 安装或更新（Android `PACKAGE_ADDED` / `PACKAGE_REPLACED`）。
    pub fn installed(&self, app: HostedName) {
        self.remember(&app);
        let _ = self.events_tx.send(NameEvent::Installed(app.name));
    }

    /// 宿主推送：App 卸载（Android `PACKAGE_REMOVED` 且非替换 / `PACKAGE_FULLY_REMOVED`）。
    pub fn removed(&self, app_id: &str) {
        lock(&self.manifests).remove(app_id);
        if let Ok(address) = Address::new(app_id, None) {
            let _ = self.events_tx.send(NameEvent::Removed(address));
        }
    }

    fn remember(&self, app: &HostedName) {
        let app_id = &app.name.address.app_id;
        let mut manifests = lock(&self.manifests);
        match &app.manifest {
            Some(m) if m.app_id == *app_id => {
                manifests.insert(app_id.clone(), m.clone());
            }
            Some(m) => {
                tracing::warn!(app_id, manifest_app_id = %m.app_id, "清单的 appId 与发现记录不一致，忽略该清单");
                manifests.remove(app_id);
            }
            None => {
                manifests.remove(app_id);
            }
        }
    }
}

/// 宿主回传的错误码字符串 → 本库常量；未知码按"系统拒绝"处理。
pub fn naming_code(code: &str) -> &'static str {
    codes::ALL.iter().copied().find(|c| *c == code).unwrap_or(codes::ACTIVATION_DENIED)
}

#[async_trait::async_trait]
impl super::Connector for HostedConnector {
    fn kind(&self) -> &'static str {
        self.kind
    }

    async fn discover(&self) -> Result<Vec<DiscoveredName>, ConnectorError> {
        let service = self.service.clone();
        let found = tokio::task::spawn_blocking(move || service.discover())
            .await
            .map_err(|e| ConnectorError::new(codes::NAME_NOT_FOUND, format!("发现回调异常：{e}")))??;
        Ok(found
            .into_iter()
            .map(|app| {
                self.remember(&app);
                app.name
            })
            .collect())
    }

    async fn watch(&self) -> Result<BoxStream<'static, NameEvent>, ConnectorError> {
        match lock(&self.events_rx).take() {
            Some(mut rx) => Ok(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)).boxed()),
            None => Ok(futures::stream::empty().boxed()),
        }
    }

    fn manifest(&self, app_id: &str) -> Option<Manifest> {
        lock(&self.manifests).get(app_id).cloned()
    }

    async fn dial(&self, address: &Address, timeout: Duration) -> Result<DialedChannel, ConnectorError> {
        let (service, target) = (self.service.clone(), address.clone());
        let (tx, rx) = oneshot::channel();
        // @why 结果经 oneshot 交回：这次拨号被放弃（超时，或 Hub 的等待先结束而丢弃了本 future）时接收端已不在，
        // 宿主迟到交回的通道由阻塞线程自己释放，不会留下绑定（spec/naming.md 7.1）。
        tokio::task::spawn_blocking(move || {
            if let Err(Ok(late)) = tx.send(service.dial(&target, timeout)) {
                drop(late.fd);
                service.release(late.lease);
            }
        });
        match tokio::time::timeout(timeout + DIAL_SLACK, rx).await {
            Ok(Ok(result)) => into_dialed(address, result?, self.service.clone()),
            Ok(Err(_)) => Err(ConnectorError::new(codes::ACTIVATION_DENIED, format!("拨号 {address} 的回调异常结束"))),
            Err(_) => Err(ConnectorError::new(
                codes::ACTIVATION_TIMEOUT,
                format!("{} 秒内没有得到 {address} 的通道", timeout.as_secs_f32()),
            )),
        }
    }
}

/// 宿主交回的 fd → Hub 的通道。从这里开始租约由 [`Lease`] 持有：任何失败路径都会释放它。
fn into_dialed(
    address: &Address,
    channel: HostedChannel,
    service: Arc<dyn HostNameService>,
) -> Result<DialedChannel, ConnectorError> {
    let lease = Lease { service, id: channel.lease };
    let io_err = |e: std::io::Error| ConnectorError::new(codes::ACTIVATION_DENIED, format!("{address} 的通道不可用：{e}"));
    let std_stream = std::os::unix::net::UnixStream::from(channel.fd);
    std_stream.set_nonblocking(true).map_err(io_err)?;
    let stream = tokio::net::UnixStream::from_std(std_stream).map_err(io_err)?;
    let cred = stream.peer_cred().map_err(io_err)?;
    // @security 通道对端（创建 socketpair 的进程）必须是宿主给出的那个 uid（spec/naming.md 10.3）。
    if let Some(expected) = channel.peer_uid
        && cred.uid() != expected
    {
        return Err(ConnectorError::new(
            codes::PEER_IDENTITY_MISMATCH,
            format!("{address} 的通道来自 uid {}（pid {:?}），与登记的 uid {expected} 不一致，已拒绝", cred.uid(), cred.pid()),
        ));
    }
    let pid = cred.pid().and_then(|p| u32::try_from(p).ok());
    Ok(DialedChannel { stream: Box::new(LeasedStream { inner: stream, _lease: lease }), pid })
}

/// 一次拨号的系统资源；被丢弃时释放（恰好一次）。
struct Lease {
    service: Arc<dyn HostNameService>,
    id: u64,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let (service, id) = (self.service.clone(), self.id);
        let release = move || service.release(id);
        // @why 宿主回调可能做跨进程调用（unbindService）：在运行时内时移到阻塞线程，不占用工作线程。
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => drop(handle.spawn_blocking(release)),
            Err(_) => release(),
        }
    }
}

/// 带租约的通道：读写转给 socketpair，丢弃时释放租约（Hub 关闭通道 = 宿主解绑）。
struct LeasedStream {
    inner: tokio::net::UnixStream,
    _lease: Lease,
}

impl AsyncRead for LeasedStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for LeasedStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_codes_map_back_to_constants() {
        assert_eq!(naming_code("HUB_NOT_TRUSTED"), codes::HUB_NOT_TRUSTED);
        assert_eq!(naming_code("NAME_NOT_FOUND"), codes::NAME_NOT_FOUND);
        assert_eq!(naming_code("CHANNEL_LIMIT"), codes::CHANNEL_LIMIT);
        assert_eq!(naming_code("whatever"), codes::ACTIVATION_DENIED);
    }
}
