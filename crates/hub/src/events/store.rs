//! 信箱持久化（第 16 项 P4，spec/hub-api.md 3.17「持久化」）：配置了 [`crate::HubConfig::state_dir`] 时，已登记 Agent
//! （订阅方 `agent:<名>`）的订阅与信箱写到 `<state_dir>/inbox/<名的安全文件名>.json`，Hub 启动时读回。
//!
//! - 同休眠记录存储（[`crate::dormant_store`]）：原子写（临时文件 + 改名，Unix 0600）、带版本号、单个文件与读回文件数有上限；
//!   读回时丢弃过期事件，文件损坏、版本未知、超出上限 → 跳过并记 warn 日志，不删除、不中断启动。
//! - 订阅的频率窗口不持久化（重启后重新计）。
//!
//! 转换函数是纯函数；文件 I/O 只在 [`InboxStore`] 的方法中。

use std::io;
use std::path::{Path, PathBuf};

use app_mcp_protocol::{MAX_EVENT_PAYLOAD_BYTES, is_valid_app_id, is_valid_name};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::inbox::{Inbox, Subscription};
use super::types::{AppEvent, EventLimits};

/// `<state_dir>` 下存放信箱的子目录。
pub const INBOX_DIR: &str = "inbox";

/// 文件格式版本；读到其他版本时跳过。
pub const INBOX_STORE_VERSION: u32 = 1;

/// 单个文件的大小上限。
///
/// @why 信箱最多 `max_inbox_events`（默认 100）条、每条载荷 ≤ 8 KiB，约 1 MiB；2 MiB 留出订阅与字段名的余量，
/// 超出时（调大了上限）不写，防止失控的信箱撑大磁盘与启动读取。
pub const MAX_INBOX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// 启动时最多读回的文件数（与 Agent 登记数上限同量级）。
pub const MAX_INBOX_FILES: usize = 1024;

/// 订阅 ID 的最大长度（读回时校验）。
const MAX_SUBSCRIPTION_ID_LEN: usize = 64;

/// 一个 Agent 的信箱文件。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredInbox {
    version: u32,
    agent: String,
    saved_at_ms: u64,
    subscriptions: Vec<StoredSubscription>,
    events: Vec<AppEvent>,
    #[serde(default)]
    dropped: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSubscription {
    subscription_id: String,
    app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    filter: Option<Map<String, Value>>,
    #[serde(default)]
    delivered: u64,
    #[serde(default)]
    dropped: u64,
}

/// Agent 名 → 文件名主干：小写字母、数字、`-`、`.` 原样，其余字节（大写字母、`_`）写成 `_` + 两位十六进制。
///
/// @why 大小写不敏感的文件系统（Windows / macOS）上 `Bot` 与 `bot` 不能落到同一个文件；Agent 名已限制为 ASCII
/// （[`crate::agents::is_valid_agent_name`]），转义后仍只含安全字符、不以 `.` 开头。
pub(crate) fn safe_file_stem(agent: &str) -> String {
    let mut out = String::with_capacity(agent.len());
    for b in agent.bytes() {
        if b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.' {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("_{b:02x}"));
        }
    }
    out
}

fn to_stored(agent: &str, inbox: &Inbox, now_ms: u64) -> StoredInbox {
    StoredInbox {
        version: INBOX_STORE_VERSION,
        agent: agent.to_owned(),
        saved_at_ms: now_ms,
        subscriptions: inbox
            .subscriptions
            .iter()
            .map(|s| StoredSubscription {
                subscription_id: s.id.clone(),
                app_id: s.app_id.clone(),
                event: s.event.clone(),
                filter: s.filter.clone(),
                delivered: s.delivered,
                dropped: s.dropped,
            })
            .collect(),
        events: inbox.events.iter().cloned().collect(),
        dropped: inbox.dropped,
    }
}

/// 事件是否合法（读回时按不可信输入校验：与 Hub 收到 `events/emit` 时同一套规则）。
pub(crate) fn payload_ok(payload: Option<&Value>) -> bool {
    match payload {
        None => true,
        Some(v @ Value::Object(_)) => serde_json::to_vec(v).is_ok_and(|b| b.len() <= MAX_EVENT_PAYLOAD_BYTES),
        Some(_) => false,
    }
}

/// 文件内容 → 信箱（校验后）：过期事件丢弃；订阅与事件按上限截断（事件保留最新的）。
///
/// @security 文件来自磁盘，按不可信输入校验：版本、Agent 名、appId、事件名、载荷形状与大小。
fn restore(stored: StoredInbox, now_ms: u64, limits: &EventLimits) -> Result<Inbox, String> {
    if stored.version != INBOX_STORE_VERSION {
        return Err(format!("版本 {} 不受支持（本 Host 读取版本 {INBOX_STORE_VERSION}）", stored.version));
    }
    if !crate::agents::is_valid_agent_name(&stored.agent) {
        return Err(format!("Agent 名「{}」不合法", stored.agent));
    }
    let mut subscriptions = Vec::new();
    for s in stored.subscriptions.into_iter().take(limits.max_subscriptions) {
        let id_ok = !s.subscription_id.is_empty() && s.subscription_id.len() <= MAX_SUBSCRIPTION_ID_LEN;
        if !id_ok || !is_valid_app_id(&s.app_id) || s.event.as_deref().is_some_and(|e| !is_valid_name(e)) {
            return Err(format!("订阅「{}」不合法", s.subscription_id));
        }
        let mut sub = Subscription::new(s.subscription_id, s.app_id, s.event, s.filter);
        sub.delivered = s.delivered;
        sub.dropped = s.dropped;
        subscriptions.push(sub);
    }
    let mut events: Vec<AppEvent> = Vec::new();
    for e in stored.events {
        if !is_valid_app_id(&e.app_id) || !is_valid_name(&e.name) || !payload_ok(e.payload.as_ref()) {
            return Err(format!("事件「{}」不合法", e.id));
        }
        events.push(e);
    }
    let max = limits.max_inbox_events.max(1);
    let skip = events.len().saturating_sub(max);
    let mut inbox = Inbox { subscriptions, events: events.into_iter().skip(skip).collect(), dropped: stored.dropped };
    inbox.expire(now_ms, limits);
    Ok(inbox)
}

/// 信箱目录（`<state_dir>/inbox`）的读写。
#[derive(Clone, Debug)]
pub(crate) struct InboxStore {
    dir: PathBuf,
}

impl InboxStore {
    pub fn new(state_dir: &Path) -> Self {
        Self { dir: state_dir.join(INBOX_DIR) }
    }

    fn path(&self, agent: &str) -> PathBuf {
        self.dir.join(format!("{}.json", safe_file_stem(agent)))
    }

    /// 写入 Agent 的信箱；信箱为空（没有订阅也没有事件）时删除文件。
    ///
    /// @side-effect 首次写入时创建目录（Unix 0700，须属于当前用户）。
    /// @error 超出 [`MAX_INBOX_FILE_BYTES`] 时不写（删除旧文件）并返回错误。
    pub fn save(&self, agent: &str, inbox: &Inbox, now_ms: u64) -> io::Result<()> {
        let path = self.path(agent);
        if inbox.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let text = serde_json::to_vec(&to_stored(agent, inbox, now_ms)).map_err(io::Error::other)?;
        if text.len() as u64 > MAX_INBOX_FILE_BYTES {
            let _ = std::fs::remove_file(&path);
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!("Agent「{agent}」的信箱 {} 字节超出上限 {MAX_INBOX_FILE_BYTES}，不持久化", text.len()),
            ));
        }
        crate::instance::prepare_dir(&self.dir)?;
        crate::instance::write_atomic(&path, &text)
    }

    /// 读回全部信箱：(Agent 名, 信箱)。问题文件跳过并记 warn 日志；目录不存在时为空。
    pub fn load(&self, now_ms: u64, limits: &EventLimits) -> Vec<(String, Inbox)> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                tracing::warn!(dir = %self.dir.display(), "信箱目录无法读取：{e}");
                return Vec::new();
            }
        };
        let mut files: Vec<(String, u64)> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let meta = entry.metadata().ok()?;
                (name.ends_with(".json") && meta.is_file()).then_some((name, meta.len()))
            })
            .collect();
        files.sort();
        let mut out = Vec::new();
        for (i, (name, len)) in files.into_iter().enumerate() {
            let path = self.dir.join(&name);
            if i >= MAX_INBOX_FILES {
                tracing::warn!(file = %path.display(), "超出读取上限 {MAX_INBOX_FILES} 个信箱文件，已跳过");
                continue;
            }
            match self.read_one(&name, len, now_ms, limits) {
                Ok(loaded) => out.push(loaded),
                Err(reason) => tracing::warn!(file = %path.display(), %reason, "跳过信箱文件"),
            }
        }
        out
    }

    fn read_one(&self, name: &str, len: u64, now_ms: u64, limits: &EventLimits) -> Result<(String, Inbox), String> {
        if len > MAX_INBOX_FILE_BYTES {
            return Err(format!("文件 {len} 字节超出上限 {MAX_INBOX_FILE_BYTES}"));
        }
        let bytes = std::fs::read(self.dir.join(name)).map_err(|e| format!("无法读取：{e}"))?;
        #[derive(Deserialize)]
        struct Header {
            version: u32,
        }
        let header: Header = serde_json::from_slice(&bytes).map_err(|e| format!("内容损坏：{e}"))?;
        if header.version != INBOX_STORE_VERSION {
            return Err(format!("版本 {} 不受支持（本 Host 读取版本 {INBOX_STORE_VERSION}）", header.version));
        }
        let stored: StoredInbox = serde_json::from_slice(&bytes).map_err(|e| format!("内容损坏：{e}"))?;
        if name.strip_suffix(".json") != Some(safe_file_stem(&stored.agent).as_str()) {
            return Err(format!("文件名与内容中的 Agent「{}」不一致", stored.agent));
        }
        let agent = stored.agent.clone();
        restore(stored, now_ms, limits).map(|inbox| (agent, inbox))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let n: u64 = rand::random();
        std::env::temp_dir().join(format!("app-mcp-inbox-{tag}-{}-{n:x}", std::process::id()))
    }

    fn ev(id: &str, at: u64) -> AppEvent {
        AppEvent { id: id.into(), app_id: "shop".into(), instance_id: "i1".into(), name: "order.shipped".into(), payload: Some(json!({"n": 1})), at }
    }

    fn inbox() -> Inbox {
        let mut sub = Subscription::new("sub-1".into(), "shop".into(), Some("order.shipped".into()), json!({"n": 1}).as_object().cloned());
        sub.delivered = 2;
        Inbox { subscriptions: vec![sub], events: [ev("ev-1", 1_000), ev("ev-2", 2_000)].into(), dropped: 3 }
    }

    #[test]
    fn safe_stem_is_case_distinct_and_plain() {
        assert_eq!(safe_file_stem("claude-code.v2"), "claude-code.v2");
        assert_eq!(safe_file_stem("Bot_1"), "_42ot_5f1");
        assert_ne!(safe_file_stem("Bot"), safe_file_stem("bot"));
    }

    #[test]
    fn roundtrip_and_empty_deletes() {
        let dir = temp_dir("rt");
        let store = InboxStore::new(&dir);
        let limits = EventLimits::default();
        store.save("Claude", &inbox(), 5_000).unwrap();
        let loaded = store.load(5_000, &limits);
        assert_eq!(loaded.len(), 1);
        let (agent, got) = &loaded[0];
        assert_eq!(agent, "Claude");
        let want = inbox();
        assert_eq!(got.events, want.events);
        assert_eq!(got.dropped, 3);
        assert_eq!(got.subscriptions[0].id, "sub-1");
        assert_eq!((got.subscriptions[0].delivered, got.subscriptions[0].filter.clone()), (2, want.subscriptions[0].filter.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(INBOX_DIR).join("_43laude.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        store.save("Claude", &Inbox::default(), 6_000).unwrap();
        assert!(store.load(6_000, &limits).is_empty(), "空信箱删除文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_drops_expired_and_truncates() {
        let dir = temp_dir("ttl");
        let store = InboxStore::new(&dir);
        store.save("a", &inbox(), 0).unwrap();
        let limits = EventLimits { inbox_ttl: Duration::from_millis(1_500), ..EventLimits::default() };
        let got = &store.load(3_000, &limits)[0].1;
        assert_eq!(got.events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["ev-2"], "ev-1 已过期");
        let limits = EventLimits { max_inbox_events: 1, max_subscriptions: 0, ..EventLimits::default() };
        let got = &store.load(3_000, &limits)[0].1;
        assert_eq!(got.events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["ev-2"], "保留最新的");
        assert!(got.subscriptions.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_unknown_version_mismatched_and_invalid_files_are_skipped() {
        let dir = temp_dir("bad");
        let store = InboxStore::new(&dir);
        store.save("good", &inbox(), 0).unwrap();
        let d = dir.join(INBOX_DIR);
        std::fs::write(d.join("broken.json"), b"{not json").unwrap();
        std::fs::write(d.join("future.json"), br#"{"version":99}"#).unwrap();
        let mut stored = to_stored("other", &inbox(), 0);
        std::fs::write(d.join("mismatch.json"), serde_json::to_vec(&stored).unwrap()).unwrap();
        stored.agent = "evil".into();
        stored.events[0].payload = Some(json!([1]));
        std::fs::write(d.join("evil.json"), serde_json::to_vec(&stored).unwrap()).unwrap();
        stored = to_stored("bad name!", &inbox(), 0);
        std::fs::write(d.join(format!("{}.json", safe_file_stem("bad name!"))), serde_json::to_vec(&stored).unwrap()).unwrap();
        std::fs::write(d.join("notes.txt"), b"ignored").unwrap();
        let loaded = store.load(0, &EventLimits::default());
        assert_eq!(loaded.iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(), ["good"]);
        assert!(d.join("broken.json").exists(), "损坏文件不删除");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn payload_rules() {
        assert!(payload_ok(None));
        assert!(payload_ok(Some(&json!({"a": 1}))));
        assert!(!payload_ok(Some(&json!("x"))));
        let big = "x".repeat(MAX_EVENT_PAYLOAD_BYTES);
        assert!(!payload_ok(Some(&json!({ "a": big }))));
    }
}
