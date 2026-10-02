//! Windows：每 App 每用户命名管道连接器（spec/naming.md 4.3）。
//!
//! - 发现：只读 App 登记文件 `%LOCALAPPDATA%\app-mcp\apps\<appId>.json`（5.3），不枚举 `\\.\pipe\`（U-06），从不启动进程；
//!   之后经目录变更通知（`ReadDirectoryChangesW`）更新，无轮询。
//! - 拨号：作为管道客户端打开 `\\.\pipe\appmcp-<SID>-<appId>`；管道不存在时按登记的激活方式拉起 App（`exec` 直接运行
//!   程序并追加 `--app-mcp-activation`；`uri` / `aumid` 复用 [`crate::SystemWaker`]），随后在本次激活窗口内以有界退避
//!   （50 ms 起、×2、上限 1 s）等待管道出现——不是常驻轮询；超时 `ACTIVATION_TIMEOUT`。
//! - 身份（10.3）：管道所有者 SID 必须是当前用户；服务端进程映像必须与登记文件的 `executable` 一致，否则
//!   `PEER_IDENTITY_MISMATCH`。
//! - 拒绝：App 不接受这条通道时在管道上写一行 `<CODE>：<说明>` 后断开（SDK 总是先发 HTTP 升级请求，二者不会混淆）；
//!   Hub 在交出通道前读到第一段数据即可区分，读到的升级请求原样回放给 App 连接服务。
//!
//! 平台相关部分（打开管道、激活进程、目录通知）在 [`PipeSystem`] 之后；其余逻辑在各平台都能编译与测试。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use app_mcp_manifest::Manifest;
use app_mcp_protocol::naming::registration::{Registration, kinds};
use app_mcp_protocol::naming::{Address, codes, pipe as names};
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::time::Instant;

pub(crate) use super::registered::{DirChange, DirWatch};
use super::registered::{RegisteredApps, ToName};
use super::{ChannelIo, ConnectorError, DialedChannel, DiscoveredName, NameEvent};

/// 等待管道出现的退避：首次间隔、倍数上限（spec/naming.md 4.3）。
const FIRST_RETRY: Duration = Duration::from_millis(50);
const MAX_RETRY: Duration = Duration::from_secs(1);

/// 打开管道的结果。
pub(crate) enum PipeOpen {
    Opened(OpenedPipe),
    /// 管道不存在（App 未运行或未登记名字）。
    Absent,
    /// 管道存在但所有实例都在使用（App 正在创建下一个实例）。
    Busy,
}

/// 已打开的管道（所有者 SID 已核对）。
pub(crate) struct OpenedPipe {
    pub stream: Box<dyn ChannelIo>,
    /// 服务端进程号（`GetNamedPipeServerProcessId`）。
    pub server_pid: Option<u32>,
    /// 服务端进程映像路径（`QueryFullProcessImageNameW`）；取不到时为 `None`。
    pub server_image: Option<String>,
}

/// 一次激活发出后的进程（只有 `exec` 能观察到）。
pub(crate) trait Launched: Send {
    /// 进程已以失败状态退出时返回说明。
    fn failure(&mut self) -> Option<String>;
}

/// 平台操作。Windows 实现为 `WinPipes`；测试用替身。
#[async_trait::async_trait]
pub(crate) trait PipeSystem: Send + Sync + 'static {
    /// 打开管道并核对所有者 SID。
    fn open(&self, name: &str) -> Result<PipeOpen, ConnectorError>;
    /// 按登记的激活方式拉起 App（只发出激活，不等待管道）。
    async fn activate(&self, reg: &Registration) -> Result<Box<dyn Launched>, ConnectorError>;
    /// 监视登记目录；不支持时为 `None`。
    fn watch(&self, dir: &Path) -> Result<Option<DirWatch>, ConnectorError>;
}

/// Windows 命名管道连接器。
pub struct PipeConnector {
    apps: RegisteredApps,
    sid: String,
    system: Arc<dyn PipeSystem>,
}

impl std::fmt::Debug for PipeConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipeConnector").field("apps_dir", &self.apps.dir()).finish_non_exhaustive()
    }
}

#[cfg(windows)]
impl PipeConnector {
    /// `apps_dir` 为 App 登记目录；`None` = `%LOCALAPPDATA%\app-mcp\apps`（spec/naming.md 5.3）。
    ///
    /// @error 取不到当前用户 SID、或未给目录且没有 `LOCALAPPDATA` 时返回 `NAME_NOT_FOUND`。
    pub fn new(apps_dir: Option<PathBuf>) -> Result<Self, ConnectorError> {
        let sid = app_mcp_protocol::endpoint::win::current_user_sid()
            .map_err(|e| ConnectorError::new(codes::NAME_NOT_FOUND, format!("无法取得当前用户 SID：{e}")))?;
        let apps_dir = match apps_dir {
            Some(d) => d,
            None => Self::default_apps_dir().ok_or_else(|| {
                ConnectorError::new(codes::NAME_NOT_FOUND, "环境变量 LOCALAPPDATA 未设置，无法确定 App 登记目录")
            })?,
        };
        Ok(Self::with_system(apps_dir, sid, Arc::new(win::WinPipes)))
    }

    /// 本连接器读取的登记目录。
    pub fn apps_dir(&self) -> &Path {
        self.apps.dir()
    }

    /// 默认登记目录 `%LOCALAPPDATA%\app-mcp\apps`。
    pub fn default_apps_dir() -> Option<PathBuf> {
        let root = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).filter(|p| p.is_absolute())?;
        Some(app_mcp_protocol::naming::registration::apps_dir(&root))
    }
}

impl PipeConnector {
    pub(crate) fn with_system(apps_dir: PathBuf, sid: String, system: Arc<dyn PipeSystem>) -> Self {
        Self { apps: RegisteredApps::new(apps_dir), sid, system }
    }

    /// 登记 → 发现到的名字（管道名；不能枚举管道，`running` 恒为否）。
    fn to_name(&self) -> ToName {
        let sid = self.sid.clone();
        Arc::new(move |reg: &Registration| {
            let address = Address::new(&reg.app_id, None).ok()?;
            let detail = names::pipe_name(&sid, &address);
            Some(DiscoveredName { address, activatable: activatable(reg), running: false, detail })
        })
    }

    async fn finish(&self, address: &Address, reg: &Registration, pipe: OpenedPipe, deadline: Instant) -> Result<DialedChannel, ConnectorError> {
        // @security 服务端进程必须是登记的程序（spec/naming.md 10.3：名字抢注）。
        if let Some(expected) = reg.executable.as_deref().filter(|e| !e.is_empty()) {
            let matches = pipe.server_image.as_deref().is_some_and(|img| names::same_executable(img, expected));
            if !matches {
                return Err(ConnectorError::new(
                    codes::PEER_IDENTITY_MISMATCH,
                    format!(
                        "{address} 的管道由进程 {:?}（{}）持有，与登记的程序 {expected} 不一致，已拒绝",
                        pipe.server_pid,
                        pipe.server_image.as_deref().unwrap_or("映像未知")
                    ),
                ));
            }
        }
        let stream = super::greeting::expect_upgrade(pipe.stream, address, deadline).await?;
        Ok(DialedChannel { stream, pid: pipe.server_pid })
    }
}

/// 激活方式本平台能否执行。
fn activatable(reg: &Registration) -> bool {
    let a = &reg.activation;
    match a.kind.as_str() {
        kinds::EXEC => !a.target.is_empty() || reg.executable.as_deref().is_some_and(|e| !e.is_empty()),
        kinds::URI | kinds::AUMID => !a.target.is_empty(),
        _ => false,
    }
}

#[async_trait::async_trait]
impl super::Connector for PipeConnector {
    fn kind(&self) -> &'static str {
        "pipe"
    }

    async fn discover(&self) -> Result<Vec<DiscoveredName>, ConnectorError> {
        self.apps.discover(&self.to_name()).await
    }

    async fn watch(&self) -> Result<BoxStream<'static, NameEvent>, ConnectorError> {
        // 守卫随事件流存活：流被丢弃（Hub 关闭）时停止监视线程。
        match self.system.watch(self.apps.dir())? {
            Some(watch) => Ok(self.apps.events(watch, self.to_name())),
            None => Ok(futures::stream::empty().boxed()),
        }
    }

    /// 登记文件 `manifest` 指向的静态清单（未运行也能列出工具，spec/naming.md 5.6）；appId 不一致时忽略。
    fn manifest(&self, app_id: &str) -> Option<Manifest> {
        self.apps.manifest(app_id)
    }

    async fn dial(&self, address: &Address, timeout: Duration) -> Result<DialedChannel, ConnectorError> {
        let deadline = Instant::now() + timeout;
        let reg = self.apps.for_dial(address)?;
        let name = names::pipe_name(&self.sid, address);
        // 实例名字不能激活（spec/naming.md 2.1），只打开一次。
        let can_activate = address.instance.is_none();
        let mut launched: Option<Box<dyn Launched>> = None;
        let mut delay = FIRST_RETRY;
        loop {
            match self.system.open(&name)? {
                PipeOpen::Opened(pipe) => return self.finish(address, &reg, pipe, deadline).await,
                PipeOpen::Absent if !can_activate => {
                    return Err(ConnectorError::new(codes::NAME_NOT_FOUND, format!("实例 {address} 未运行（管道 {name} 不存在）")));
                }
                PipeOpen::Absent if launched.is_none() => {
                    if !activatable(&reg) {
                        return Err(ConnectorError::new(
                            codes::ACTIVATION_DENIED,
                            format!("{address} 未运行，且登记的激活方式「{}」不能由 Hub 执行", reg.activation.kind),
                        ));
                    }
                    tracing::info!(%address, kind = %reg.activation.kind, "管道不存在，激活 App");
                    launched = Some(self.system.activate(&reg).await?);
                }
                PipeOpen::Absent | PipeOpen::Busy => {
                    if let Some(why) = launched.as_mut().and_then(|l| l.failure()) {
                        return Err(ConnectorError::new(codes::ACTIVATION_DENIED, format!("{address} 激活后进程异常退出：{why}")));
                    }
                }
            }
            if Instant::now() + delay > deadline {
                return Err(ConnectorError::new(
                    codes::ACTIVATION_TIMEOUT,
                    format!("{} 秒内管道 {name} 没有出现（App 启动失败或过慢）", timeout.as_secs_f32()),
                ));
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(MAX_RETRY);
        }
    }
}

/// `FILE_NOTIFY_INFORMATION` 记录序列 → 文件名（`ReadDirectoryChangesW` 的输出，逐条检查边界）。
///
/// 记录：`NextEntryOffset: u32`、`Action: u32`、`FileNameLength: u32`（字节）、`FileName: [u16]`；`NextEntryOffset = 0` 为最后一条。
pub(crate) fn parse_notify(bytes: &[u8]) -> Vec<String> {
    let u32_at = |off: usize| -> Option<u32> {
        let b = bytes.get(off..off.checked_add(4)?)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut names = Vec::new();
    let mut off = 0usize;
    while let (Some(next), Some(len)) = (u32_at(off), u32_at(off + 8)) {
        let start = off + 12;
        let Some(raw) = start.checked_add(len as usize).and_then(|end| bytes.get(start..end)) else { break };
        let wide: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        names.push(String::from_utf16_lossy(&wide));
        if next == 0 {
            break;
        }
        match off.checked_add(next as usize) {
            Some(n) if n < bytes.len() => off = n,
            _ => break,
        }
    }
    names
}

#[cfg(windows)]
mod win;

#[cfg(test)]
mod tests;
