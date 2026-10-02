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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Duration;

use app_mcp_manifest::Manifest;
use app_mcp_protocol::naming::registration::{self, Registration, kinds};
use app_mcp_protocol::naming::{Address, codes, pipe as names};
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::Instant;

use super::{ChannelIo, ConnectorError, DialedChannel, DiscoveredName, NameEvent};

/// 等待管道出现的退避：首次间隔、倍数上限（spec/naming.md 4.3）。
const FIRST_RETRY: Duration = Duration::from_millis(50);
const MAX_RETRY: Duration = Duration::from_secs(1);
/// HTTP 升级请求的开头（SDK 是 WebSocket 客户端，先发 `GET /app`）。
const UPGRADE_PREFIX: &[u8] = b"GET ";

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

/// 登记目录的变化（目录通知线程 → 连接器）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DirChange {
    /// 这些文件名（不含目录）有变化。
    Files(Vec<String>),
    /// 通知缓冲溢出：须重新枚举目录（F-25）。
    Rescan,
}

/// 登记目录的监视：变化经 `rx` 送达；`guard` 被丢弃时停止监视。
pub(crate) struct DirWatch {
    pub rx: UnboundedReceiver<DirChange>,
    pub guard: Box<dyn Send>,
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
    apps_dir: PathBuf,
    sid: String,
    system: Arc<dyn PipeSystem>,
    /// 已发出 `Installed` 的 appId（目录重新枚举时据此发 `Removed`）。
    known: Arc<Mutex<BTreeSet<String>>>,
}

impl std::fmt::Debug for PipeConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipeConnector").field("apps_dir", &self.apps_dir).finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
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
        &self.apps_dir
    }

    /// 默认登记目录 `%LOCALAPPDATA%\app-mcp\apps`。
    pub fn default_apps_dir() -> Option<PathBuf> {
        let root = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).filter(|p| p.is_absolute())?;
        Some(registration::apps_dir(&root))
    }
}

impl PipeConnector {
    pub(crate) fn with_system(apps_dir: PathBuf, sid: String, system: Arc<dyn PipeSystem>) -> Self {
        Self { apps_dir, sid, system, known: Arc::new(Mutex::new(BTreeSet::new())) }
    }

    fn address_of(&self, reg: &Registration) -> Option<(Address, String)> {
        let address = Address::new(&reg.app_id, None).ok()?;
        let name = names::pipe_name(&self.sid, &address);
        Some((address, name))
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
        let mut stream = pipe.stream;
        let head = first_bytes(stream.as_mut(), address, deadline).await?;
        Ok(DialedChannel { stream: Box::new(Replay { head, pos: 0, inner: stream }), pid: pipe.server_pid })
    }
}

/// 读登记文件（5.3）：`Ok(None)` = 文件不存在。
fn load(dir: &Path, app_id: &str) -> Result<Option<Registration>, String> {
    let path = dir.join(registration::file_name(app_id));
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取 {} 失败：{e}", path.display())),
    };
    let reg = registration::parse(&text, app_id).map_err(|e| format!("{}：{e}", path.display()))?;
    if app_mcp_manifest::is_reserved_app_id(&reg.app_id) {
        return Err(format!("{}：appId「{}」是保留名", path.display(), reg.app_id));
    }
    Ok(Some(reg))
}

/// 登记的程序已不存在（5.4 清除条件）。
fn executable_missing(reg: &Registration) -> bool {
    reg.executable.as_deref().is_some_and(|e| !e.is_empty() && !Path::new(e).exists())
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

/// 文件名 → appId（`<appId>.json`；其他文件如写入中的临时文件为 `None`）。
fn app_id_of(file_name: &str) -> Option<&str> {
    file_name.strip_suffix(".json").filter(|id| app_mcp_protocol::is_valid_app_id(id))
}

/// 扫描登记目录：合法且程序存在的登记。目录不存在时为空。
fn scan(dir: &Path) -> Vec<Registration> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "无法读取 App 登记目录");
            return Vec::new();
        }
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().and_then(app_id_of).map(str::to_owned))
        .collect();
    ids.sort();
    ids.into_iter().filter_map(|id| usable(dir, &id)).collect()
}

/// 读取并筛选一个登记：无效或程序已不存在时记录原因并返回 `None`。
fn usable(dir: &Path, app_id: &str) -> Option<Registration> {
    match load(dir, app_id) {
        Ok(Some(reg)) if executable_missing(&reg) => {
            tracing::warn!(app_id, executable = ?reg.executable, "登记的程序已不存在，忽略该登记");
            None
        }
        Ok(reg) => reg,
        Err(e) => {
            tracing::warn!(app_id, error = %e, "App 登记文件无效，忽略");
            None
        }
    }
}

impl PipeConnector {
    fn discovered(&self, reg: &Registration) -> Option<DiscoveredName> {
        let (address, name) = self.address_of(reg)?;
        Some(DiscoveredName { address, activatable: activatable(reg), running: false, detail: name })
    }

    /// 某些文件变化 / 重新枚举 → 名字事件（安装 / 更新 → `Installed`，删除或失效 → `Removed`）。
    fn changes_to_events(&self, change: DirChange) -> Vec<NameEvent> {
        let ids: BTreeSet<String> = match change {
            DirChange::Files(files) => files.iter().filter_map(|f| app_id_of(f).map(str::to_owned)).collect(),
            DirChange::Rescan => {
                let mut all: BTreeSet<String> = lock(&self.known).clone();
                all.extend(scan(&self.apps_dir).into_iter().map(|r| r.app_id));
                all
            }
        };
        let mut events = Vec::new();
        for id in ids {
            let found = usable(&self.apps_dir, &id).and_then(|reg| self.discovered(&reg));
            let mut known = lock(&self.known);
            match found {
                Some(name) => {
                    known.insert(id);
                    events.push(NameEvent::Installed(name));
                }
                None if known.remove(&id) => {
                    if let Ok(a) = Address::new(&id, None) {
                        events.push(NameEvent::Removed(a));
                    }
                }
                None => {}
            }
        }
        events
    }
}

#[async_trait::async_trait]
impl super::Connector for PipeConnector {
    fn kind(&self) -> &'static str {
        "pipe"
    }

    async fn discover(&self) -> Result<Vec<DiscoveredName>, ConnectorError> {
        let dir = self.apps_dir.clone();
        let regs = tokio::task::spawn_blocking(move || scan(&dir))
            .await
            .map_err(|e| ConnectorError::new(codes::NAME_NOT_FOUND, format!("扫描登记目录异常：{e}")))?;
        let found: Vec<DiscoveredName> = regs.iter().filter_map(|r| self.discovered(r)).collect();
        lock(&self.known).extend(found.iter().map(|n| n.address.app_id.clone()));
        Ok(found)
    }

    async fn watch(&self) -> Result<BoxStream<'static, NameEvent>, ConnectorError> {
        let Some(DirWatch { mut rx, guard }) = self.system.watch(&self.apps_dir)? else {
            return Ok(futures::stream::empty().boxed());
        };
        let me = Self {
            apps_dir: self.apps_dir.clone(),
            sid: self.sid.clone(),
            system: self.system.clone(),
            known: self.known.clone(),
        };
        // 守卫随事件流存活：流被丢弃（Hub 关闭）时停止监视线程。
        let changes = futures::stream::poll_fn(move |cx| {
            let _keep = &guard;
            rx.poll_recv(cx)
        });
        Ok(changes.flat_map(move |c| futures::stream::iter(me.changes_to_events(c))).boxed())
    }

    /// 登记文件 `manifest` 指向的静态清单（未运行也能列出工具，spec/naming.md 5.6）；appId 不一致时忽略。
    fn manifest(&self, app_id: &str) -> Option<Manifest> {
        let path = load(&self.apps_dir, app_id).ok().flatten()?.manifest.filter(|m| !m.is_empty())?;
        match app_mcp_manifest::load_file(&path) {
            Ok(loaded) if loaded.manifest.app_id == app_id => Some(loaded.manifest),
            Ok(loaded) => {
                tracing::warn!(app_id, manifest_app_id = %loaded.manifest.app_id, "登记的清单 appId 不一致，忽略");
                None
            }
            Err(e) => {
                tracing::warn!(app_id, path = %path, error = %e, "无法读取登记的清单");
                None
            }
        }
    }

    async fn dial(&self, address: &Address, timeout: Duration) -> Result<DialedChannel, ConnectorError> {
        let deadline = Instant::now() + timeout;
        let reg = load(&self.apps_dir, &address.app_id)
            .map_err(|e| ConnectorError::new(codes::NAME_NOT_FOUND, e))?
            .ok_or_else(|| ConnectorError::new(codes::NAME_NOT_FOUND, format!("{address} 没有 App 登记文件")))?;
        if executable_missing(&reg) {
            return Err(ConnectorError::new(
                codes::NAME_NOT_FOUND,
                format!("{address} 登记的程序 {} 已不存在", reg.executable.as_deref().unwrap_or_default()),
            ));
        }
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

/// 读到 App 发来的第一段数据：HTTP 升级请求的开头 → 返回已读字节（之后回放）；其他内容 → App 的拒绝行。
async fn first_bytes(stream: &mut dyn ChannelIo, address: &Address, deadline: Instant) -> Result<Vec<u8>, ConnectorError> {
    let mut head = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = match tokio::time::timeout_at(deadline, stream.read(&mut buf)).await {
            Err(_) => {
                return Err(ConnectorError::new(codes::ACTIVATION_TIMEOUT, format!("{address} 打开通道后没有及时发送握手")));
            }
            Ok(Ok(n)) => n,
            // 管道断开（ERROR_BROKEN_PIPE 等）与 EOF 同样处理。
            Ok(Err(_)) => 0,
        };
        head.extend_from_slice(&buf[..n]);
        if head.len() >= UPGRADE_PREFIX.len() && head.starts_with(UPGRADE_PREFIX) {
            return Ok(head);
        }
        let complete = n == 0 || head.contains(&b'\n') || head.len() >= names::MAX_REFUSAL_LINE;
        if !complete {
            continue;
        }
        if head.is_empty() {
            return Err(ConnectorError::new(codes::ACTIVATION_DENIED, format!("{address} 在发送握手前关闭了通道")));
        }
        if UPGRADE_PREFIX.starts_with(&head) {
            return Err(ConnectorError::new(codes::ACTIVATION_DENIED, format!("{address} 的握手在开头处中断")));
        }
        let line = String::from_utf8_lossy(&head);
        let (code, detail) = names::parse_refusal(line.lines().next().unwrap_or_default());
        return Err(ConnectorError::new(code, format!("{address} 拒绝了通道：{detail}")));
    }
}

/// 先回放已读的开头，再读管道。
struct Replay {
    head: Vec<u8>,
    pos: usize,
    inner: Box<dyn ChannelIo>,
}

impl AsyncRead for Replay {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        if let Some(rest) = this.head.get(this.pos..).filter(|r| !r.is_empty()) {
            let n = rest.len().min(buf.remaining());
            buf.put_slice(&rest[..n]);
            this.pos += n;
            if this.pos == this.head.len() {
                this.head = Vec::new();
                this.pos = 0;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Replay {
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
