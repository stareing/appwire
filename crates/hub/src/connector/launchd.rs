//! macOS：launchd 用户 Agent 的按需套接字连接器（spec/naming.md 4.4）。
//!
//! - 发现：只读 App 登记文件 `~/Library/Application Support/app-mcp/apps/<appId>.json`（5.3），从不连接套接字（连接即激活）；
//!   之后经登记目录的 kqueue 通知更新（阻塞在 `kevent` 上的一个线程，只在启用按名寻址时存在），无轮询。
//! - 拨号：`connect` 登记的 `activation.target`（launchd 持有监听端的 Unix 套接字）；作业未运行时 launchd 在连接到来时启动它，
//!   连接先排在监听队列中，App 起来后 `accept`。激活与握手共用 `timeout`，超时 `ACTIVATION_TIMEOUT`。
//! - 身份（10.1 / 10.3）：套接字所在目录须属于当前用户且组 / 其他用户不可写；连接对端（`getpeereid`）须是当前用户或 root
//!   （U-23：launchd 创建的监听端的凭据可能记为 root）。
//! - 拒绝：与 Windows 相同的拒绝行（[`super::greeting`]）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use app_mcp_manifest::Manifest;
use app_mcp_protocol::naming::registration::{Registration, kinds};
use app_mcp_protocol::naming::{Address, codes, launchd as names};
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::time::Instant;

use super::registered::{DirWatch, RegisteredApps, ToName};
use super::{ConnectorError, DialedChannel, DiscoveredName, NameEvent};

#[cfg(target_os = "macos")]
mod kqueue;

/// 登记目录的监视（macOS 为 kqueue；测试用替身）。
pub(crate) trait DirWatcher: Send + Sync + 'static {
    /// 监视 `dir`；不支持时为 `None`。
    fn watch(&self, dir: &Path) -> Result<Option<DirWatch>, ConnectorError>;
}

/// macOS launchd 连接器。
pub struct LaunchdConnector {
    apps: RegisteredApps,
    watcher: Arc<dyn DirWatcher>,
}

impl std::fmt::Debug for LaunchdConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchdConnector").field("apps_dir", &self.apps.dir()).finish_non_exhaustive()
    }
}

#[cfg(target_os = "macos")]
impl LaunchdConnector {
    /// `apps_dir` 为 App 登记目录；`None` = `~/Library/Application Support/app-mcp/apps`（spec/naming.md 5.3）。
    ///
    /// @error 未给目录且没有 `HOME` 时返回 `NAME_NOT_FOUND`。
    pub fn new(apps_dir: Option<PathBuf>) -> Result<Self, ConnectorError> {
        let apps_dir = match apps_dir {
            Some(d) => d,
            None => Self::default_apps_dir()
                .ok_or_else(|| ConnectorError::new(codes::NAME_NOT_FOUND, "环境变量 HOME 未设置，无法确定 App 登记目录"))?,
        };
        Ok(Self::with_watcher(apps_dir, Arc::new(kqueue::Kqueue)))
    }

    /// 本连接器读取的登记目录。
    pub fn apps_dir(&self) -> &Path {
        self.apps.dir()
    }

    /// 默认登记目录 `~/Library/Application Support/app-mcp/apps`。
    pub fn default_apps_dir() -> Option<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from).filter(|p| p.is_absolute())?;
        Some(app_mcp_protocol::naming::registration::apps_dir(&home.join("Library").join("Application Support")))
    }
}

impl LaunchdConnector {
    pub(crate) fn with_watcher(apps_dir: PathBuf, watcher: Arc<dyn DirWatcher>) -> Self {
        Self { apps: RegisteredApps::new(apps_dir), watcher }
    }

    /// 登记 → 发现到的名字（作业标签；不连接套接字就无从得知是否在运行，`running` 恒为否）。
    fn to_name() -> ToName {
        Arc::new(|reg: &Registration| {
            let address = Address::new(&reg.app_id, None).ok()?;
            Some(DiscoveredName { address, activatable: socket_of(reg).is_some(), running: false, detail: names::label(&reg.app_id) })
        })
    }
}

/// 登记的套接字路径：激活方式为 `launchd` 且 `target` 为绝对路径时。
fn socket_of(reg: &Registration) -> Option<&Path> {
    let a = &reg.activation;
    (a.kind == kinds::LAUNCHD).then(|| Path::new(a.target.as_str())).filter(|p| p.is_absolute())
}

/// 连接对端是否可信：当前用户，或 root（launchd 以自己的身份创建监听端时，U-23）。
fn trusted_peer(uid: u32, me: u32) -> bool {
    uid == me || uid == 0
}

/// `connect` 的失败 → 第 12 节的码。
fn connect_error(address: &Address, socket: &Path, e: &std::io::Error) -> ConnectorError {
    use std::io::ErrorKind;
    let label = names::label(&address.app_id);
    match e.kind() {
        ErrorKind::NotFound => ConnectorError::new(
            codes::NAME_NOT_FOUND,
            format!("{address} 的套接字 {} 不存在：launchd 作业 {label} 未载入（launchctl bootstrap gui/$UID <plist>，或重新 app install）", socket.display()),
        ),
        ErrorKind::ConnectionRefused => ConnectorError::new(
            codes::ACTIVATION_DENIED,
            format!("{address} 的套接字 {} 无人监听：launchd 作业 {label} 未载入或已被卸下", socket.display()),
        ),
        ErrorKind::PermissionDenied => {
            ConnectorError::new(codes::BIND_PERMISSION_DENIED, format!("无权连接 {address} 的套接字 {}：{e}", socket.display()))
        }
        _ => ConnectorError::new(codes::ACTIVATION_DENIED, format!("连接 {address} 的套接字 {} 失败：{e}", socket.display())),
    }
}

#[async_trait::async_trait]
impl super::Connector for LaunchdConnector {
    fn kind(&self) -> &'static str {
        "launchd"
    }

    async fn discover(&self) -> Result<Vec<DiscoveredName>, ConnectorError> {
        self.apps.discover(&Self::to_name()).await
    }

    async fn watch(&self) -> Result<BoxStream<'static, NameEvent>, ConnectorError> {
        // 守卫随事件流存活：流被丢弃（Hub 关闭）时停止监视线程。
        match self.watcher.watch(self.apps.dir())? {
            Some(watch) => Ok(self.apps.events(watch, Self::to_name())),
            None => Ok(futures::stream::empty().boxed()),
        }
    }

    fn manifest(&self, app_id: &str) -> Option<Manifest> {
        self.apps.manifest(app_id)
    }

    async fn dial(&self, address: &Address, timeout: Duration) -> Result<DialedChannel, ConnectorError> {
        let deadline = Instant::now() + timeout;
        if address.instance.is_some() {
            return Err(ConnectorError::new(
                codes::NAME_NOT_FOUND,
                format!("{address}：macOS 上不登记实例名字（一个 launchd 作业只有一个套接字，spec/naming.md 4.4）"),
            ));
        }
        let reg = self.apps.for_dial(address)?;
        let Some(socket) = socket_of(&reg) else {
            return Err(ConnectorError::new(
                codes::ACTIVATION_DENIED,
                format!("{address} 登记的激活方式「{}」（{}）不能在 macOS 上按名拨号", reg.activation.kind, reg.activation.target),
            ));
        };
        if let Err(issue) = app_mcp_protocol::endpoint::check_unix_socket_path(socket) {
            return Err(ConnectorError::new(codes::NAME_NOT_FOUND, issue.to_string()));
        }
        let me = app_mcp_protocol::endpoint::current_uid();
        if let Some(dir) = socket.parent() {
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::metadata(dir).map_err(|e| connect_error(address, socket, &e))?;
            if let Some(why) = names::socket_dir_issue(meta.uid(), meta.mode(), me) {
                return Err(ConnectorError::new(
                    codes::PEER_IDENTITY_MISMATCH,
                    format!("{address} 的套接字目录 {} {why}，已拒绝", dir.display()),
                ));
            }
        }
        let stream = match tokio::time::timeout_at(deadline, tokio::net::UnixStream::connect(socket)).await {
            Err(_) => {
                return Err(ConnectorError::new(codes::ACTIVATION_TIMEOUT, format!("{} 秒内未能连接 {address}", timeout.as_secs_f32())));
            }
            Ok(Err(e)) => return Err(connect_error(address, socket, &e)),
            Ok(Ok(s)) => s,
        };
        let cred = stream.peer_cred().map_err(|e| {
            ConnectorError::new(codes::PEER_IDENTITY_MISMATCH, format!("无法取得 {address} 套接字对端的凭据：{e}"))
        })?;
        if !trusted_peer(cred.uid(), me) {
            return Err(ConnectorError::new(
                codes::PEER_IDENTITY_MISMATCH,
                format!("{address} 的套接字由 uid {} 监听，不是当前用户，已拒绝", cred.uid()),
            ));
        }
        // 对端进程号为 launchd（1）时不是 App 进程，不记录。
        let pid = cred.pid().and_then(|p| u32::try_from(p).ok()).filter(|p| *p > 1);
        tracing::debug!(%address, socket = %socket.display(), "已连接 launchd 套接字，等待 App 握手");
        let stream = super::greeting::expect_upgrade(Box::new(stream), address, deadline).await?;
        Ok(DialedChannel { stream, pid })
    }
}

#[cfg(test)]
mod tests;
