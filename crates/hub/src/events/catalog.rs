//! 事件目录与去重窗口（纯状态，不做 I/O）：各实例经 `events/sync` 的运行时声明、实例断开后按 App 保留的最近一次声明，
//! 以及按 `(App 连接, eventId)` 的去重（连接断开即释放该连接的窗口）。清单 `events` 由调用方从注册表取出后合并
//! （清单只在注册表保存一份）。
//!
//! @why 去重按连接而不按实例：SDK 的 eventId 是进程内计数器，固定实例 ID 的 App 重启后会从头计数，按实例去重会误丢事件；
//! SDK 不缓存、不重发事件，跨连接去重没有意义。

use std::collections::{HashMap, HashSet, VecDeque};

use app_mcp_protocol::{EventInfo, is_valid_name};

/// 每个实例运行时声明的事件数上限（超出的部分丢弃并记日志，B-07）。
pub(crate) const MAX_DECLARED_EVENTS: usize = 256;

/// 每条连接的去重窗口：最近见过的 eventId 个数（超出时淘汰最旧的，B-07）。
///
/// @why 只为挡住同一连接上的重复消息；256 条远多于传输层可能重复的数量。
pub(crate) const DEDUP_WINDOW: usize = 256;

/// `eventId` 的最大长度（超出视为不合法）。
pub(crate) const MAX_EVENT_ID_LEN: usize = 128;

/// 一个 App 的运行时声明。
#[derive(Debug, Default)]
struct AppDecl {
    /// 已连接实例（按连接 ID）的最近一次声明。
    by_conn: HashMap<u64, Vec<EventInfo>>,
    /// 任一实例最近一次的声明（实例断开后保留，Hub 重启丢失）。
    last: Vec<EventInfo>,
}

/// 一条连接的去重窗口。
#[derive(Debug, Default)]
struct SeenWindow {
    order: VecDeque<String>,
    set: HashSet<String>,
}

/// 事件目录与去重窗口。
#[derive(Debug, Default)]
pub(crate) struct Catalog {
    apps: HashMap<String, AppDecl>,
    /// 连接 ID → 去重窗口（连接断开时移除）。
    seen: HashMap<u64, SeenWindow>,
}

/// 运行时声明的整理：去掉名称不合法与重名的条目，至多 [`MAX_DECLARED_EVENTS`] 个。返回 (保留的, 丢弃的个数)。
pub(crate) fn sanitize(events: Vec<EventInfo>) -> (Vec<EventInfo>, usize) {
    let total = events.len();
    let mut names = HashSet::new();
    let kept: Vec<EventInfo> = events
        .into_iter()
        .filter(|e| is_valid_name(&e.name) && names.insert(e.name.clone()))
        .take(MAX_DECLARED_EVENTS)
        .collect();
    let dropped = total - kept.len();
    (kept, dropped)
}

impl Catalog {
    /// 实例（连接 `conn_id`）的全量声明（`events/sync`）。
    pub fn sync(&mut self, app_id: &str, conn_id: u64, events: Vec<EventInfo>) {
        let app = self.apps.entry(app_id.to_owned()).or_default();
        app.last = events.clone();
        app.by_conn.insert(conn_id, events);
    }

    /// 连接断开：移除该连接的声明（App 的最近一次声明保留）与去重窗口。
    pub fn disconnected(&mut self, app_id: &str, conn_id: u64) {
        if let Some(app) = self.apps.get_mut(app_id) {
            app.by_conn.remove(&conn_id);
        }
        self.seen.remove(&conn_id);
    }

    /// 该连接是否在运行时声明了事件 `name`。
    pub fn conn_declares(&self, app_id: &str, conn_id: u64, name: &str) -> bool {
        self.apps
            .get(app_id)
            .and_then(|a| a.by_conn.get(&conn_id))
            .is_some_and(|evs| evs.iter().any(|e| e.name == name))
    }

    /// 是否有过该 App 的运行时声明（实例断开后仍算）。
    pub fn knows(&self, app_id: &str) -> bool {
        self.apps.contains_key(app_id)
    }

    /// App 的事件目录：`manifest` ∪ 最近一次声明 ∪ 各已连接实例的声明，按名去重（先出现的优先），按名排序。
    pub fn declared(&self, app_id: &str, manifest: &[EventInfo]) -> Vec<EventInfo> {
        let mut out: Vec<EventInfo> = Vec::new();
        let mut names = HashSet::new();
        let runtime = self.apps.get(app_id).into_iter().flat_map(|a| a.last.iter().chain(a.by_conn.values().flatten()));
        for e in manifest.iter().chain(runtime) {
            if names.insert(e.name.as_str()) {
                out.push(e.clone());
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// 记下连接 `conn_id` 上的 `eventId`；该连接上之前见过时返回 `false`（重复）。窗口满时淘汰最旧的。
    pub fn first_seen(&mut self, conn_id: u64, event_id: &str) -> bool {
        let w = self.seen.entry(conn_id).or_default();
        if w.set.contains(event_id) {
            return false;
        }
        if w.order.len() >= DEDUP_WINDOW
            && let Some(old) = w.order.pop_front()
        {
            w.set.remove(&old);
        }
        w.set.insert(event_id.to_owned());
        w.order.push_back(event_id.to_owned());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str) -> EventInfo {
        EventInfo { name: name.into(), description: "d".into(), payload_schema: None }
    }

    #[test]
    fn sanitize_drops_invalid_and_duplicates_with_cap() {
        let (kept, dropped) = sanitize(vec![info("a"), info("bad name"), info("a"), info("b")]);
        assert_eq!(kept.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(dropped, 2);
        let many: Vec<EventInfo> = (0..MAX_DECLARED_EVENTS + 5).map(|i| info(&format!("e{i}"))).collect();
        let (kept, dropped) = sanitize(many);
        assert_eq!((kept.len(), dropped), (MAX_DECLARED_EVENTS, 5));
    }

    #[test]
    fn declarations_by_connection_and_retained_after_disconnect() {
        let mut c = Catalog::default();
        assert!(!c.knows("shop"));
        c.sync("shop", 1, vec![info("order.shipped")]);
        c.sync("shop", 2, vec![info("cart.changed")]);
        assert!(c.conn_declares("shop", 1, "order.shipped"));
        assert!(!c.conn_declares("shop", 1, "cart.changed"), "按实例校验：别的实例的声明不算");
        let names = |v: Vec<EventInfo>| v.into_iter().map(|e| e.name).collect::<Vec<_>>();
        assert_eq!(names(c.declared("shop", &[info("static.one")])), ["cart.changed", "order.shipped", "static.one"]);
        c.disconnected("shop", 1);
        c.disconnected("shop", 2);
        assert!(!c.conn_declares("shop", 2, "cart.changed"));
        // 断开后按 App 保留最近一次声明（实例 2 的）。
        assert_eq!(names(c.declared("shop", &[])), ["cart.changed"]);
        assert!(c.knows("shop"));
    }

    #[test]
    fn dedup_by_connection_and_event_id_with_bounded_window() {
        let mut c = Catalog::default();
        assert!(c.first_seen(1, "e1"));
        assert!(!c.first_seen(1, "e1"), "同一连接同一 eventId 重复");
        assert!(c.first_seen(2, "e1"), "不同连接互不影响（App 重启后计数器从头开始）");
        for i in 0..DEDUP_WINDOW {
            assert!(c.first_seen(1, &format!("w{i}")));
        }
        let w = &c.seen[&1];
        assert!(w.order.len() <= DEDUP_WINDOW && w.set.len() == w.order.len());
        assert!(c.first_seen(1, "e1"), "超出窗口后最旧的被淘汰");
        c.sync("shop", 2, vec![]);
        c.disconnected("shop", 2);
        assert!(!c.seen.contains_key(&2), "断开释放该连接的窗口");
        assert!(c.first_seen(2, "e1"));
    }
}
