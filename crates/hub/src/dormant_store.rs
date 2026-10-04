//! 休眠记录持久化（spec/lifecycle.md 第 9 节；spec/hub-api.md 3.5「持久化」）：配置了 [`crate::HubConfig::state_dir`] 时，
//! 把每个 App 的休眠实例记录写到 `<state_dir>/dormant/<appId>.json`，Hub 重启后读回，重启前休眠的 App 仍可列出、可唤醒。
//!
//! - 内容只有声明：工具 / 资源定义、唤醒描述、`toolsHash`、恢复令牌、页面目录中 SDK 上报过的页面工具与时间戳；没有调用参数 / 结果。
//! - 工具定义按内容去重保存一份（`tools` 表 + 下标），读回时同一定义只建一个 [`SharedTool`]，与内存中的共享方式一致。
//! - 原子写（临时文件 + 改名，Unix 0600）；单个文件不超过 [`MAX_STORE_FILE_BYTES`]，读回最多 [`MAX_STORE_FILES`] 个文件。
//! - 读回时过期（`sleptAt` 早于 `dormant_ttl`）的实例丢弃；文件损坏、版本未知、超出上限 → 跳过并记 warn 日志与
//!   [`StoreIssue`]（`HubStatus.dormant_store`、`app-mcp-host doctor`），不删除、不中断启动。
//!
//! 本模块的转换函数是纯函数；文件 I/O 只在 [`DormantStore`] 的方法中。

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use app_mcp_protocol::{AppOverview, ClientKind, ResourceInfo, ToolInfo, Visibility, WakeDescriptor, is_valid_app_id};
use serde::{Deserialize, Serialize};

use crate::registry::DormantInstance;
use crate::tool_def::{SharedTool, ToolDef};

/// 文件格式版本。读到其他版本（如更新的 Host 写的）时跳过，不覆盖判断之外的内容。
pub const STORE_VERSION: u32 = 1;

/// `<state_dir>` 下存放休眠记录的子目录。
pub const DORMANT_DIR: &str = "dormant";

/// 单个文件的大小上限（写入超出时不写并删除旧文件；读到超出的文件时跳过）。
///
/// @why 一个 App 的快照受协议消息大小约束（工具同步一条消息），4 MiB 足够多个实例共享定义后的总和；上限防止失控的 App
/// 撑大磁盘与启动时的读取。
pub const MAX_STORE_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// 启动时最多读回的文件数（按修改时间从新到旧）。
pub const MAX_STORE_FILES: usize = 1024;

/// 一个 App 的休眠记录文件。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredApp {
    pub version: u32,
    pub app_id: String,
    pub saved_at_ms: u64,
    /// 去重后的工具定义表。
    pub tools: Vec<ToolInfo>,
    pub instances: Vec<StoredInstance>,
    /// 页面目录中 SDK 上报过的页面工具（`tools` 的下标）。
    #[serde(default)]
    pub page_tools: Vec<u32>,
}

/// 一个休眠实例。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredInstance {
    pub instance_id: String,
    pub app_name: String,
    pub client_kind: ClientKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overview: Option<AppOverview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    /// 工具（`StoredApp.tools` 的下标）。
    pub tools: Vec<u32>,
    #[serde(default)]
    pub resources: Vec<ResourceInfo>,
    /// @security 恢复令牌只用于快速恢复（跳过工具同步），不授予调用权限；文件仅当前用户可读。
    pub resume_token: String,
    pub tools_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<WakeDescriptor>,
    pub slept_at_ms: u64,
    pub connected_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_at_ms: Option<u64>,
}

/// 读回的一个 App。
#[derive(Debug)]
pub(crate) struct RestoredApp {
    pub app_id: String,
    /// 未过期的实例，按休眠前的活跃顺序（旧 → 新）。
    pub instances: Vec<DormantInstance>,
    pub page_tools: Vec<SharedTool>,
    /// 因过期丢弃的实例数。
    pub expired: usize,
}

/// 读取 / 写入中被跳过的文件或失败（`HubStatus.dormant_store.issues`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreIssue {
    /// 文件名（相对于休眠记录目录）。
    pub file: String,
    /// 中文说明。
    pub reason: String,
}

/// 持久化状态（`HubStatus.dormant_store`，spec/hub-api.md 3.9）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DormantStoreStatus {
    /// 休眠记录目录（`<state_dir>/dormant`）。
    pub dir: String,
    /// 启动时读回的实例数。
    pub loaded_instances: u64,
    /// 启动时因过期丢弃的实例数。
    pub expired_instances: u64,
    /// 启动以来成功写入 / 删除文件的次数。
    pub writes: u64,
    /// 启动时跳过的文件（损坏、版本未知、超出上限）。
    pub issues: Vec<StoreIssue>,
    /// 最近一次写入失败。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)).unwrap_or(0)
}

fn from_unix_ms(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

/// 共享定义去重表：同一 `Arc` 只转换一次，内容相同的不同 `Arc` 也只存一份。
#[derive(Default)]
struct ToolTable {
    infos: Vec<ToolInfo>,
    by_ptr: HashMap<*const ToolDef, u32>,
    by_text: HashMap<String, u32>,
}

impl ToolTable {
    fn index(&mut self, t: &SharedTool) -> u32 {
        if let Some(i) = self.by_ptr.get(&Arc::as_ptr(t)) {
            return *i;
        }
        let info = t.to_info();
        let text = serde_json::to_string(&info).unwrap_or_default();
        let i = *self.by_text.entry(text).or_insert_with(|| {
            self.infos.push(info);
            (self.infos.len() - 1) as u32
        });
        self.by_ptr.insert(Arc::as_ptr(t), i);
        i
    }
}

/// 内存中的休眠记录 → 文件内容。`dormant` 为空时返回 `None`（应删除文件）。
pub(crate) fn snapshot(
    app_id: &str,
    dormant: &[DormantInstance],
    page_tools: &[SharedTool],
    now: SystemTime,
) -> Option<StoredApp> {
    if dormant.is_empty() {
        return None;
    }
    let mut table = ToolTable::default();
    let mut ordered: Vec<&DormantInstance> = dormant.iter().collect();
    ordered.sort_by_key(|d| d.recency);
    let instances = ordered
        .into_iter()
        .map(|d| StoredInstance {
            instance_id: d.instance_id.clone(),
            app_name: d.app_name.clone(),
            client_kind: d.client_kind,
            app_version: d.app_version.clone(),
            title: d.title.clone(),
            url: d.url.clone(),
            overview: d.overview.clone(),
            visibility: d.visibility,
            tools: d.tools.values().map(|t| table.index(t)).collect(),
            resources: d.resources.values().cloned().collect(),
            resume_token: d.resume_token.clone(),
            tools_hash: d.tools_hash.clone(),
            wake: d.wake.clone(),
            slept_at_ms: unix_ms(d.slept_at),
            connected_at_ms: unix_ms(d.connected_at),
            last_active_at_ms: d.last_active_at.map(unix_ms),
        })
        .collect();
    let page_tools = page_tools.iter().map(|t| table.index(t)).collect();
    Some(StoredApp {
        version: STORE_VERSION,
        app_id: app_id.to_owned(),
        saved_at_ms: unix_ms(now),
        tools: table.infos,
        instances,
        page_tools,
    })
}

/// 文件内容 → 休眠记录（校验后）。`expired_before` 之前休眠的实例丢弃。`recency` 由注册表登记时重新分配。
///
/// @security 文件来自磁盘，按不可信输入校验：版本、appId、工具下标范围、工具 / 资源名称与 schema（与 SDK 上报同一套过滤）。
pub(crate) fn restore(stored: StoredApp, expired_before: SystemTime) -> Result<RestoredApp, String> {
    if stored.version != STORE_VERSION {
        return Err(format!("版本 {} 不受支持（本 Host 读取版本 {STORE_VERSION}）", stored.version));
    }
    if !is_valid_app_id(&stored.app_id) {
        return Err(format!("appId「{}」不合法", stored.app_id));
    }
    let app_id = stored.app_id;
    let defs: Vec<Option<SharedTool>> = stored
        .tools
        .into_iter()
        .map(|t| crate::registry::sanitize_tools(&app_id, vec![t]).pop())
        .collect();
    let pick = |i: &u32| -> Result<Option<SharedTool>, String> {
        defs.get(*i as usize).cloned().ok_or_else(|| format!("工具下标 {i} 超出定义表（{} 个）", defs.len()))
    };
    let page_tools = stored.page_tools.iter().map(pick).collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect();
    let mut instances = Vec::new();
    let mut expired = 0;
    for (seq, s) in stored.instances.into_iter().enumerate() {
        if s.instance_id.is_empty() {
            return Err("实例 ID 为空".to_owned());
        }
        let slept_at = from_unix_ms(s.slept_at_ms);
        if slept_at < expired_before {
            expired += 1;
            continue;
        }
        let tools = s
            .tools
            .iter()
            .map(pick)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .map(|t| (t.name.clone(), t))
            .collect();
        let resources = crate::registry::sanitize_resources(&app_id, s.resources)
            .into_iter()
            .map(|r| (r.name.clone(), r))
            .collect();
        instances.push(DormantInstance {
            instance_id: s.instance_id,
            app_name: s.app_name,
            client_kind: s.client_kind,
            app_version: s.app_version,
            title: s.title,
            url: s.url,
            overview: s.overview,
            visibility: s.visibility,
            tools,
            resources,
            resume_token: s.resume_token,
            tools_hash: s.tools_hash,
            wake: s.wake,
            slept_at,
            connected_at: from_unix_ms(s.connected_at_ms),
            last_active_at: s.last_active_at_ms.map(from_unix_ms),
            recency: seq as u64,
        });
    }
    Ok(RestoredApp { app_id, instances, page_tools, expired })
}

/// 文件名对应的 appId（`<appId>.json`）；其他文件（临时文件等）返回 `None`。
fn app_id_of(file_name: &str) -> Option<&str> {
    file_name.strip_suffix(".json").filter(|id| is_valid_app_id(id))
}

/// 读取结果。
#[derive(Debug, Default)]
pub(crate) struct LoadReport {
    pub apps: Vec<RestoredApp>,
    pub issues: Vec<StoreIssue>,
    /// 全部实例都已过期、已删除的文件数。
    pub removed: usize,
}

/// 休眠记录目录（`<state_dir>/dormant`）的读写。
#[derive(Clone, Debug)]
pub(crate) struct DormantStore {
    dir: PathBuf,
}

impl DormantStore {
    pub fn new(state_dir: &Path) -> Self {
        Self { dir: state_dir.join(DORMANT_DIR) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, app_id: &str) -> PathBuf {
        self.dir.join(format!("{app_id}.json"))
    }

    /// 写入（`None` = 删除）一个 App 的记录。超出大小上限时删除旧文件并返回错误。
    ///
    /// @side-effect 首次写入时创建目录（Unix 0700，须属于当前用户）。
    pub fn save(&self, app_id: &str, content: Option<&StoredApp>) -> io::Result<()> {
        if !is_valid_app_id(app_id) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("appId「{app_id}」不合法，不持久化")));
        }
        let path = self.path(app_id);
        let Some(content) = content else {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        };
        let text = serde_json::to_vec(content).map_err(io::Error::other)?;
        if text.len() as u64 > MAX_STORE_FILE_BYTES {
            let _ = std::fs::remove_file(&path);
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!(
                    "App「{app_id}」的休眠记录 {} 字节超出上限 {MAX_STORE_FILE_BYTES}，不持久化（Hub 重启后需等 App 回连才能列出）",
                    text.len()
                ),
            ));
        }
        crate::instance::prepare_dir(&self.dir)?;
        crate::instance::write_atomic(&path, &text)
    }

    /// 读回全部记录：过期实例丢弃（全部过期的文件删除），问题文件跳过并记入 `issues`。目录不存在时为空。
    pub fn load(&self, now: SystemTime, ttl: Duration) -> LoadReport {
        let mut report = LoadReport::default();
        let expired_before = now.checked_sub(ttl).unwrap_or(UNIX_EPOCH);
        let mut files = match self.list() {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return report,
            Err(e) => {
                report.issues.push(StoreIssue { file: String::new(), reason: format!("目录无法读取：{e}") });
                return report;
            }
        };
        // 新的优先：超出数量上限时丢弃最旧的。
        files.sort_by_key(|f| std::cmp::Reverse(f.1));
        for (i, (name, _, len)) in files.into_iter().enumerate() {
            let issue = |reason: String| StoreIssue { file: name.clone(), reason };
            if i >= MAX_STORE_FILES {
                report.issues.push(issue(format!("超出读取上限 {MAX_STORE_FILES} 个文件，已跳过")));
                continue;
            }
            match self.read_one(&name, len, expired_before) {
                Ok(app) if app.instances.is_empty() => {
                    let _ = std::fs::remove_file(self.dir.join(&name));
                    report.removed += 1;
                    if app.expired == 0 {
                        tracing::debug!(file = %name, "休眠记录文件没有实例，已删除");
                    }
                }
                Ok(app) => report.apps.push(app),
                Err(reason) => {
                    tracing::warn!(file = %self.dir.join(&name).display(), %reason, "跳过休眠记录文件");
                    report.issues.push(issue(reason));
                }
            }
        }
        report
    }

    /// 目录中的记录文件：(文件名, 修改时间, 大小)。
    fn list(&self) -> io::Result<Vec<(String, SystemTime, u64)>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else { continue };
            if app_id_of(&name).is_none() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            out.push((name, meta.modified().unwrap_or(UNIX_EPOCH), meta.len()));
        }
        Ok(out)
    }

    fn read_one(&self, name: &str, len: u64, expired_before: SystemTime) -> Result<RestoredApp, String> {
        if len > MAX_STORE_FILE_BYTES {
            return Err(format!("文件 {len} 字节超出上限 {MAX_STORE_FILE_BYTES}"));
        }
        let bytes = std::fs::read(self.dir.join(name)).map_err(|e| format!("无法读取：{e}"))?;
        // 先只读版本：版本未知时不按当前格式解析（更新的 Host 写的文件格式可能不同）。
        #[derive(Deserialize)]
        struct Header {
            version: u32,
        }
        let header: Header = serde_json::from_slice(&bytes).map_err(|e| format!("内容损坏：{e}"))?;
        if header.version != STORE_VERSION {
            return Err(format!("版本 {} 不受支持（本 Host 读取版本 {STORE_VERSION}）", header.version));
        }
        let stored: StoredApp = serde_json::from_slice(&bytes).map_err(|e| format!("内容损坏：{e}"))?;
        if Some(stored.app_id.as_str()) != app_id_of(name) {
            return Err(format!("文件名与内容中的 appId「{}」不一致", stored.app_id));
        }
        restore(stored, expired_before)
    }
}

/// 离线检查休眠记录目录（`app-mcp-host doctor` 在 Host 未运行时也可用）：返回各文件的状态，不修改任何文件。
pub fn inspect(state_dir: &Path, now: SystemTime, ttl: Duration) -> io::Result<Vec<StoreFileInfo>> {
    let store = DormantStore::new(state_dir);
    let expired_before = now.checked_sub(ttl).unwrap_or(UNIX_EPOCH);
    let mut out: Vec<StoreFileInfo> = store
        .list()?
        .into_iter()
        .map(|(name, _, len)| match store.read_one(&name, len, expired_before) {
            Ok(app) => StoreFileInfo {
                file: name,
                app_id: Some(app.app_id),
                instances: app.instances.len() as u64,
                expired: app.expired as u64,
                bytes: len,
                problem: None,
            },
            Err(reason) => StoreFileInfo { file: name, bytes: len, problem: Some(reason), ..Default::default() },
        })
        .collect();
    out.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(out)
}

/// [`inspect`] 中一个文件的状态。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreFileInfo {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// 未过期的实例数。
    pub instances: u64,
    /// 已过期的实例数（下次 Host 启动时丢弃）。
    pub expired: u64,
    pub bytes: u64,
    /// 损坏 / 版本未知 / 超出上限的说明；`None` = 可读回。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use app_mcp_protocol::{Risk, ToolSurface, WakeKind};
    use serde_json::json;

    fn temp_dir(tag: &str) -> PathBuf {
        let n: u64 = rand::random();
        std::env::temp_dir().join(format!("app-mcp-store-{tag}-{}-{n:x}", std::process::id()))
    }

    fn tool(name: &str, page: Option<&str>) -> SharedTool {
        Arc::new(ToolDef::from_info(ToolInfo {
            name: name.into(),
            description: format!("{name} 描述"),
            input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
            risk: Risk::Read,
            activation: None,
            title: None,
            annotations: None,
            output_schema: None,
            surface: ToolSurface::App,
            page: page.map(str::to_owned),
            background_tool: None,
            implements: Vec::new(),
        }))
    }

    fn resource(name: &str) -> ResourceInfo {
        serde_json::from_value(json!({"name": name, "description": "资源"})).unwrap()
    }

    fn instance(id: &str, tools: &[SharedTool], slept_at: SystemTime, recency: u64) -> DormantInstance {
        DormantInstance {
            instance_id: id.into(),
            app_name: "示例".into(),
            client_kind: ClientKind::Native,
            app_version: Some("1.0".into()),
            title: None,
            url: None,
            overview: None,
            visibility: Some(Visibility::Hidden),
            tools: tools.iter().map(|t| (t.name.clone(), t.clone())).collect::<BTreeMap<_, _>>(),
            resources: [("cart".to_owned(), resource("cart"))].into(),
            resume_token: format!("rt-{id}"),
            tools_hash: "h".into(),
            wake: Some(WakeDescriptor { kind: WakeKind::Uri, target: Some("demo".into()), background: true }),
            slept_at,
            connected_at: slept_at - Duration::from_secs(5),
            last_active_at: None,
            recency,
        }
    }

    /// 毫秒精度的时刻（文件只存毫秒）。
    fn now_ms() -> SystemTime {
        from_unix_ms(unix_ms(SystemTime::now()))
    }

    #[test]
    fn round_trip_shares_definitions() {
        let dir = temp_dir("rt");
        let store = DormantStore::new(&dir);
        let now = now_ms();
        let a = tool("a", None);
        let p = tool("page.x", Some("orders"));
        // 内容相同、不是同一个 Arc 的定义也只存一份
        let a2 = Arc::new((*a).clone());
        let older = instance("i1", &[a.clone(), p.clone()], now - Duration::from_secs(60), 1);
        let newer = instance("i2", &[a2], now, 2);
        let stored = snapshot("demo", &[newer.clone(), older.clone()], std::slice::from_ref(&p), now).unwrap();
        assert_eq!(stored.tools.len(), 2, "去重后两个定义");
        store.save("demo", Some(&stored)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("dormant/demo.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let report = store.load(now, Duration::from_secs(3600));
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        let app = report.apps.into_iter().next().unwrap();
        assert_eq!(app.app_id, "demo");
        let ids: Vec<&str> = app.instances.iter().map(|d| d.instance_id.as_str()).collect();
        assert_eq!(ids, vec!["i1", "i2"], "按休眠前的活跃顺序");
        let (r1, r2) = (&app.instances[0], &app.instances[1]);
        assert!(Arc::ptr_eq(&r1.tools["a"], &r2.tools["a"]), "读回后共享同一定义");
        assert!(Arc::ptr_eq(&r1.tools["page.x"], &app.page_tools[0]));
        assert_eq!(*r1.tools["a"], *a);
        assert_eq!(r1.snapshot_hash(), older.snapshot_hash(), "快照摘要不变（快速恢复依赖它）");
        assert_eq!((r1.resume_token.as_str(), r1.slept_at, r1.wake.clone()), ("rt-i1", older.slept_at, older.wake.clone()));
        assert_eq!(r1.resources.keys().collect::<Vec<_>>(), vec!["cart"]);

        // 删除
        store.save("demo", None).unwrap();
        assert!(store.load(now, Duration::from_secs(3600)).apps.is_empty());
        store.save("demo", None).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn expired_instances_dropped_and_empty_files_removed() {
        let dir = temp_dir("exp");
        let store = DormantStore::new(&dir);
        let now = now_ms();
        let day = Duration::from_secs(24 * 3600);
        let t = tool("a", None);
        let one = std::slice::from_ref(&t);
        let mixed = [instance("old", one, now - day - Duration::from_secs(1), 1), instance("new", one, now, 2)];
        store.save("mixed", snapshot("mixed", &mixed, &[], now).as_ref()).unwrap();
        let stale = [instance("old", &[t], now - 2 * day, 1)];
        store.save("stale", snapshot("stale", &stale, &[], now).as_ref()).unwrap();

        let report = store.load(now, day);
        assert_eq!(report.removed, 1);
        assert!(!dir.join("dormant/stale.json").exists(), "全部过期的文件删除");
        let app = &report.apps[0];
        assert_eq!((app.app_id.as_str(), app.instances.len(), app.expired), ("mixed", 1, 1));
        assert_eq!(app.instances[0].instance_id, "new");
        assert!(snapshot("x", &[], &[], now).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_unknown_version_and_oversized_files_are_skipped() {
        let dir = temp_dir("bad");
        let store = DormantStore::new(&dir);
        let now = now_ms();
        let good = snapshot("good", &[instance("i", &[tool("a", None)], now, 1)], &[], now).unwrap();
        store.save("good", Some(&good)).unwrap();
        let d = dir.join(DORMANT_DIR);
        std::fs::write(d.join("broken.json"), b"{ not json").unwrap();
        std::fs::write(d.join("future.json"), br#"{"version": 99, "whatever": true}"#).unwrap();
        let mut renamed = good.clone();
        renamed.app_id = "other".into();
        std::fs::write(d.join("mismatch.json"), serde_json::to_vec(&renamed).unwrap()).unwrap();
        let mut bad_index = good.clone();
        bad_index.app_id = "badidx".into();
        bad_index.instances[0].tools = vec![7];
        std::fs::write(d.join("badidx.json"), serde_json::to_vec(&bad_index).unwrap()).unwrap();
        std::fs::write(d.join("huge.json"), vec![b' '; MAX_STORE_FILE_BYTES as usize + 1]).unwrap();
        // 非记录文件（临时文件、非法名称）被忽略
        std::fs::write(d.join("good.json.123.tmp"), b"x").unwrap();
        std::fs::write(d.join("Bad Name.json"), b"x").unwrap();

        let report = store.load(now, Duration::from_secs(3600));
        assert_eq!(report.apps.len(), 1);
        assert_eq!(report.apps[0].app_id, "good");
        let mut issues: Vec<(String, String)> = report.issues.into_iter().map(|i| (i.file, i.reason)).collect();
        issues.sort();
        let files: Vec<&str> = issues.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(files, vec!["badidx.json", "broken.json", "future.json", "huge.json", "mismatch.json"]);
        assert!(issues[0].1.contains("下标"), "{issues:?}");
        assert!(issues[1].1.contains("损坏"));
        assert!(issues[2].1.contains("版本 99"));
        assert!(issues[3].1.contains("上限"));
        assert!(d.join("broken.json").exists(), "问题文件不删除（便于排查，也不覆盖更新版本的数据）");

        let info = inspect(&dir, now, Duration::from_secs(3600)).unwrap();
        assert_eq!(info.len(), 6);
        assert!(info.iter().any(|f| f.file == "good.json" && f.instances == 1 && f.problem.is_none()));
        assert!(info.iter().any(|f| f.file == "future.json" && f.problem.is_some()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_rejects_oversized_and_invalid_app_ids() {
        let dir = temp_dir("big");
        let store = DormantStore::new(&dir);
        let now = now_ms();
        let small = snapshot("big", &[instance("i", &[tool("a", None)], now, 1)], &[], now).unwrap();
        store.save("big", Some(&small)).unwrap();
        let mut huge = small.clone();
        huge.tools[0].description = "x".repeat(MAX_STORE_FILE_BYTES as usize);
        let err = store.save("big", Some(&huge)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::FileTooLarge);
        assert!(!dir.join("dormant/big.json").exists(), "超出上限时删除旧文件，避免读回过时快照");
        assert_eq!(store.save("../x", Some(&small)).unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert!(inspect(&temp_dir("missing"), now, Duration::from_secs(1)).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
