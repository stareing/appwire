//! 以 App 登记文件（spec/naming.md 5.3）为发现来源的连接器共用部分：读取 / 校验 / 扫描登记目录、目录变化 → 名字事件、
//! 登记的静态清单。Windows 命名管道（4.3）与 macOS launchd（4.4）都只从登记目录得知有哪些 App，从不为发现而启动进程。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use app_mcp_manifest::Manifest;
use app_mcp_protocol::naming::registration::{self, Registration};
use app_mcp_protocol::naming::{Address, codes};
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::mpsc::UnboundedReceiver;

use super::{ConnectorError, DiscoveredName, NameEvent};

/// 登记目录的变化（目录通知线程 → 连接器）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DirChange {
    /// 这些文件名（不含目录）有变化。
    // @why 只有 Windows（`ReadDirectoryChangesW`）给出文件名；macOS kqueue 只报目录变化。
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    Files(Vec<String>),
    /// 不知道哪些文件变化（通知缓冲溢出 F-25、kqueue 只报目录变化）：须重新枚举目录。
    Rescan,
}

/// 登记目录的监视：变化经 `rx` 送达；`guard` 被丢弃时停止监视。
pub(crate) struct DirWatch {
    pub rx: UnboundedReceiver<DirChange>,
    pub guard: Box<dyn Send>,
}

/// 登记 → 发现到的名字（平台决定名字与能否激活）；不适用时为 `None`。
pub(crate) type ToName = Arc<dyn Fn(&Registration) -> Option<DiscoveredName> + Send + Sync>;

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 一个登记目录及已发出 `Installed` 的 appId（目录重新枚举时据此发 `Removed`）。
#[derive(Clone, Debug)]
pub(crate) struct RegisteredApps {
    dir: PathBuf,
    known: Arc<Mutex<BTreeSet<String>>>,
}

impl RegisteredApps {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir, known: Arc::new(Mutex::new(BTreeSet::new())) }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// 启动扫描：合法且程序存在的登记（在阻塞线程上读目录）。
    pub(crate) async fn discover(&self, to_name: &ToName) -> Result<Vec<DiscoveredName>, ConnectorError> {
        let dir = self.dir.clone();
        let regs = tokio::task::spawn_blocking(move || scan(&dir))
            .await
            .map_err(|e| ConnectorError::new(codes::NAME_NOT_FOUND, format!("扫描登记目录异常：{e}")))?;
        let found: Vec<DiscoveredName> = regs.iter().filter_map(|r| to_name(r)).collect();
        lock(&self.known).extend(found.iter().map(|n| n.address.app_id.clone()));
        Ok(found)
    }

    /// 目录变化 → 名字事件流；流被丢弃时守卫随之丢弃（停止监视）。
    pub(crate) fn events(&self, watch: DirWatch, to_name: ToName) -> BoxStream<'static, NameEvent> {
        let DirWatch { mut rx, guard } = watch;
        let me = self.clone();
        let changes = futures::stream::poll_fn(move |cx| {
            let _keep = &guard;
            rx.poll_recv(cx)
        });
        changes.flat_map(move |c| futures::stream::iter(me.changes_to_events(c, &to_name))).boxed()
    }

    /// 某些文件变化 / 重新枚举 → 名字事件（安装 / 更新 → `Installed`，删除或失效 → `Removed`）。
    pub(crate) fn changes_to_events(&self, change: DirChange, to_name: &ToName) -> Vec<NameEvent> {
        let ids: BTreeSet<String> = match change {
            DirChange::Files(files) => files.iter().filter_map(|f| app_id_of(f).map(str::to_owned)).collect(),
            DirChange::Rescan => {
                let mut all: BTreeSet<String> = lock(&self.known).clone();
                all.extend(scan(&self.dir).into_iter().map(|r| r.app_id));
                all
            }
        };
        let mut events = Vec::new();
        for id in ids {
            let found = usable(&self.dir, &id).and_then(|reg| to_name(&reg));
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

    /// 登记文件 `manifest` 指向的静态清单（未运行也能列出工具，spec/naming.md 5.6）；appId 不一致时忽略。
    pub(crate) fn manifest(&self, app_id: &str) -> Option<Manifest> {
        let path = load(&self.dir, app_id).ok().flatten()?.manifest.filter(|m| !m.is_empty())?;
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

    /// 拨号前读取登记：没有登记或登记的程序已不存在 → `NAME_NOT_FOUND`（调用报 `APP_NOT_INSTALLED`）。
    pub(crate) fn for_dial(&self, address: &Address) -> Result<Registration, ConnectorError> {
        let reg = load(&self.dir, &address.app_id)
            .map_err(|e| ConnectorError::new(codes::NAME_NOT_FOUND, e))?
            .ok_or_else(|| ConnectorError::new(codes::NAME_NOT_FOUND, format!("{address} 没有 App 登记文件")))?;
        if executable_missing(&reg) {
            return Err(ConnectorError::new(
                codes::NAME_NOT_FOUND,
                format!("{address} 登记的程序 {} 已不存在", reg.executable.as_deref().unwrap_or_default()),
            ));
        }
        Ok(reg)
    }
}

/// 读登记文件（5.3）：`Ok(None)` = 文件不存在。
pub(crate) fn load(dir: &Path, app_id: &str) -> Result<Option<Registration>, String> {
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
pub(crate) fn executable_missing(reg: &Registration) -> bool {
    reg.executable.as_deref().is_some_and(|e| !e.is_empty() && !Path::new(e).exists())
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
