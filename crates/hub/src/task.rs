//! 调用方键与 Agent 任务对象（docs/plans/12-mcp-stateless.md 3.1 / S4；docs/plans/16-agent-os.md P1）。
//!
//! 调用方的跨请求状态（`apps.select` 的选择、已附带的总览、租约、渐进暴露已列出的 App）挂在**任务对象**上，
//! 任务按**调用方键**寻址，不挂在传输会话上：
//!
//! | 调用方 | 键 | 寿命（[`TaskLifetime`]） |
//! |---|---|---|
//! | legacy MCP 会话（`initialize` 握手过的连接 / `Mcp-Session-Id`） | `mcp:<n>` | 会话结束（[`TaskLifetime::UntilEnd`]） |
//! | 无会话（modern）MCP 请求：HTTP、IPC、stdio 一律如此 | `principal:<主体>` | 请求流空闲 `HubConfig::task_idle_ttl`（[`TaskLifetime::UntilIdle`]） |
//! | Hub API（`CallRequest.session` 等） | `api` / `api:<session>` | `Hub::reset_session`（[`TaskLifetime::UntilEnd`]） |
//!
//! 本模块是纯状态（不做 I/O、不读时钟）。租约的发出与收回在 [`crate::lifecycle`]，空闲判定复用租约的请求流活动
//! （[`crate::lease::LeaseBook`]，16 U9）。
//!
//! @invariant 每个调用方键至多一个任务；键的字符串形式在 Hub 内唯一定义于此（D-07），租约统计、日志、
//! `LeasePairStatus.session` 都用 [`CallerKey::as_str`]。

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::lifecycle::LeaseEntry;

/// 调用方主体：由**传输层凭据**决定，不取自 `clientInfo`（MCP 规范：自报信息不可信，S-F6）。
///
/// @why 现在只有 [`Principal::Local`]：HTTP 只有一个本机令牌（未带令牌的回环请求同样是本机用户）、IPC 由操作系统核对为
/// 同一用户、stdio 的对端是父进程——三者都是"本机用户"。第 16 项 N5 按 Agent 发令牌后在此增加变体，主体自然细分，
/// 调用方键与任务表无需再改（docs/plans/12-mcp-stateless.md 3.1）。
#[cfg(feature = "mcp-server")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Principal {
    /// 本机用户（本机令牌 / IPC 同用户 / stdio 父进程）。
    Local,
}

#[cfg(feature = "mcp-server")]
impl Principal {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Principal::Local => "local",
        }
    }
}

/// 任务的寿命。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TaskLifetime {
    /// 有明确的结束信号（MCP 会话结束、`Hub::reset_session`），之前一直保留。
    UntilEnd,
    /// 没有结束信号（无会话的 MCP 请求）：请求流空闲达 `HubConfig::task_idle_ttl` 后由 Hub 回收。
    UntilIdle,
}

/// 调用方的种类（`/status` 的 `tasks[].kind`；见模块文档的表）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallerKind {
    /// legacy MCP 会话（`mcp:<n>`）。
    McpSession,
    /// 无会话（modern）MCP 请求的主体（`principal:<主体>`）。
    Principal,
    /// Hub API 会话（`api` / `api:<session>`）。
    Api,
}

/// 调用方键（单一定义；见模块文档的表）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CallerKey {
    key: String,
    kind: CallerKind,
}

impl CallerKey {
    /// legacy MCP 会话。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn mcp_session(id: u64) -> Self {
        Self { key: format!("mcp:{id}"), kind: CallerKind::McpSession }
    }

    /// 无会话的 MCP 请求：按主体。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn principal(p: Principal) -> Self {
        Self { key: format!("principal:{}", p.as_str()), kind: CallerKind::Principal }
    }

    /// Hub API 的会话（`None` = 默认会话）。
    pub(crate) fn api(session: Option<&str>) -> Self {
        let key = match session {
            Some(s) => format!("api:{s}"),
            None => "api".to_owned(),
        };
        Self { key, kind: CallerKind::Api }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.key
    }

    pub(crate) fn kind(&self) -> CallerKind {
        self.kind
    }

    /// 无会话（modern）调用方：列表只随服务器状态与主体变化（S5），`apps.select` 带空闲有效期（S6）。
    pub(crate) fn is_stateless(&self) -> bool {
        self.kind == CallerKind::Principal
    }

    pub(crate) fn lifetime(&self) -> TaskLifetime {
        match self.kind {
            CallerKind::Principal => TaskLifetime::UntilIdle,
            CallerKind::McpSession | CallerKind::Api => TaskLifetime::UntilEnd,
        }
    }
}

impl fmt::Display for CallerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key)
    }
}

/// 一个 Agent 任务：调用方的跨请求状态。
///
/// 后续项挂在这里，不另立对象：N5 Agent 身份、P2 按 Agent 匹配、P3 按 Agent 记账、N6 对象锁、第 17 项句柄、
/// 第 12 项 S8 任务句柄（[`AgentTask::id`] 即句柄，经工具参数 / 结果传递）。
#[derive(Debug)]
pub(crate) struct AgentTask {
    /// Hub 签发的任务 ID：`task-` + 128 位随机数的十六进制（S8 作为显式句柄时满足 SEP-2567 的不可猜测要求）。
    pub id: String,
    /// `apps.select`：appId → 选择。
    pub selected: HashMap<String, Selection>,
    /// 已附带的总览：appId → 版本。
    pub delivered: HashMap<String, String>,
    /// 本任务发出的租约：连接 ID → 租约。
    pub leases: HashMap<u64, LeaseEntry>,
    /// 渐进暴露：本任务展开过（`apps.tools`）或调用过的 App（含上游）。
    pub exposed: HashSet<String>,
}

impl AgentTask {
    fn new() -> Self {
        Self {
            id: format!("task-{:032x}", rand::random::<u128>()),
            selected: HashMap::new(),
            delivered: HashMap::new(),
            leases: HashMap::new(),
            exposed: HashSet::new(),
        }
    }

    /// 记下 `apps.select` 的选择（覆盖同一 App 之前的选择，有效期从现在算起）。
    pub(crate) fn select(&mut self, app_id: &str, instance_id: &str, now: Instant) {
        self.selected.insert(app_id.to_owned(), Selection { instance_id: instance_id.to_owned(), used_at: now });
    }

    /// 路由时取用某 App 的选择：已过期的移除并返回 `None`，未过期的续期（`used_at = now`）。
    ///
    /// @input ttl 选择的空闲有效期；`None` = 不过期（legacy 会话、Hub API）。
    pub(crate) fn use_selection(&mut self, app_id: &str, ttl: Option<Duration>, now: Instant) -> Option<String> {
        let sel = self.selected.get_mut(app_id)?;
        if sel.expired(ttl, now) {
            self.selected.remove(app_id);
            return None;
        }
        sel.used_at = now;
        Some(sel.instance_id.clone())
    }

    /// 未过期的选择（不续期，供列出）：(appId, instanceId, 到期时刻；`None` = 不过期)。
    pub(crate) fn live_selections(&self, ttl: Option<Duration>, now: Instant) -> impl Iterator<Item = (&str, &str, Option<Instant>)> {
        self.selected
            .iter()
            .filter(move |(_, s)| !s.expired(ttl, now))
            .map(move |(app, s)| (app.as_str(), s.instance_id.as_str(), ttl.map(|t| s.used_at + t)))
    }

    /// 去掉已到期的租约记录（SDK 侧同样已到期，收回时不必再发 `ttlMs: 0`）。
    pub(crate) fn prune_expired_leases(&mut self, now: Instant) {
        self.leases.retain(|_, l| l.expires().is_some_and(|e| e > now));
    }
}

/// `apps.select` 的一个选择。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub instance_id: String,
    /// 选定或最近一次用于路由的时刻（空闲有效期从这里算起）。
    pub used_at: Instant,
}

impl Selection {
    fn expired(&self, ttl: Option<Duration>, now: Instant) -> bool {
        ttl.is_some_and(|t| now.saturating_duration_since(self.used_at) >= t)
    }
}

/// 任务表：调用方键 → 任务（`HubShared` 持有，一把锁）。
#[derive(Debug, Default)]
pub(crate) struct TaskTable {
    tasks: HashMap<CallerKey, AgentTask>,
}

impl TaskTable {
    pub(crate) fn get(&self, key: &CallerKey) -> Option<&AgentTask> {
        self.tasks.get(key)
    }

    pub(crate) fn get_mut(&mut self, key: &CallerKey) -> Option<&mut AgentTask> {
        self.tasks.get_mut(key)
    }

    /// 调用方的任务；没有时创建（第一次写入状态时）。
    pub(crate) fn entry(&mut self, key: &CallerKey) -> &mut AgentTask {
        self.tasks.entry(key.clone()).or_insert_with(|| {
            let task = AgentTask::new();
            tracing::debug!(caller = %key, task = %task.id, "创建 Agent 任务");
            task
        })
    }

    /// 结束任务（返回被移除的任务）。
    pub(crate) fn remove(&mut self, key: &CallerKey) -> Option<AgentTask> {
        let task = self.tasks.remove(key);
        if let Some(t) = &task {
            tracing::debug!(caller = %key, task = %t.id, "结束 Agent 任务");
        }
        task
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&CallerKey, &AgentTask)> {
        self.tasks.iter()
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &AgentTask> {
        self.tasks.values()
    }

    /// 字符串形式为 `key` 的任务的调用方键（租约统计按字符串记，空闲收回时换回调用方键）。
    pub(crate) fn caller_key(&self, key: &str) -> Option<CallerKey> {
        self.tasks.keys().find(|k| k.as_str() == key).cloned()
    }

    /// 按空闲回收的任务的调用方键。
    pub(crate) fn idle_lifetime_keys(&self) -> Vec<CallerKey> {
        self.tasks.keys().filter(|k| k.lifetime() == TaskLifetime::UntilIdle).cloned().collect()
    }

    #[cfg(all(test, feature = "mcp-server"))]
    pub(crate) fn len(&self) -> usize {
        self.tasks.len()
    }
}

#[cfg(all(test, feature = "mcp-server"))]
mod tests {
    use super::*;

    #[test]
    fn caller_keys_are_distinct_and_carry_lifetime() {
        let keys = [
            (CallerKey::mcp_session(3), "mcp:3", TaskLifetime::UntilEnd, CallerKind::McpSession),
            (CallerKey::principal(Principal::Local), "principal:local", TaskLifetime::UntilIdle, CallerKind::Principal),
            (CallerKey::api(None), "api", TaskLifetime::UntilEnd, CallerKind::Api),
            (CallerKey::api(Some("x")), "api:x", TaskLifetime::UntilEnd, CallerKind::Api),
        ];
        for (k, s, life, kind) in &keys {
            assert_eq!((k.as_str(), k.lifetime(), k.kind()), (*s, *life, *kind));
            assert_eq!(k.to_string(), *s);
            assert_eq!(k.is_stateless(), *kind == CallerKind::Principal);
        }
        let set: HashSet<&CallerKey> = keys.iter().map(|(k, _, _, _)| k).collect();
        assert_eq!(set.len(), keys.len());
    }

    #[test]
    fn one_task_per_caller_with_stable_unguessable_id() {
        let mut t = TaskTable::default();
        let p = CallerKey::principal(Principal::Local);
        let s = CallerKey::mcp_session(1);
        let id = t.entry(&p).id.clone();
        assert!(id.starts_with("task-") && id.len() == "task-".len() + 32, "{id}");
        // 再次写入同一调用方：同一个任务
        t.entry(&p).select("shop", "a", Instant::now());
        assert_eq!(t.entry(&p).id, id);
        assert_eq!(t.len(), 1);
        // 另一个调用方：另一个任务，ID 不同
        let other = t.entry(&s).id.clone();
        assert_ne!(other, id);
        assert_eq!(t.idle_lifetime_keys(), vec![p.clone()]);
        // 结束后重建是新任务（新 ID），旧状态不在
        assert!(t.remove(&p).is_some());
        assert!(t.get(&p).is_none());
        assert_ne!(t.entry(&p).id, id);
        assert!(t.get(&p).is_some_and(|x| x.selected.is_empty()));
    }

    /// 选择的空闲有效期：未用满有效期时可取用并续期；用满即过期（取用时移除）；`None` 不过期。
    #[test]
    fn selection_idle_ttl_renews_on_use_and_expires() {
        let mut t = TaskTable::default();
        let p = CallerKey::principal(Principal::Local);
        let t0 = Instant::now();
        let ttl = Some(Duration::from_secs(60));
        let task = t.entry(&p);
        task.select("shop", "a", t0);
        task.select("mail", "m", t0);
        // 59 秒时取用：仍有效，并续期到 119 秒
        assert_eq!(task.use_selection("shop", ttl, t0 + Duration::from_secs(59)).as_deref(), Some("a"));
        let live: Vec<_> = task.live_selections(ttl, t0 + Duration::from_secs(60)).collect();
        assert_eq!(live, vec![("shop", "a", Some(t0 + Duration::from_secs(119)))], "mail 未续期，60 秒时已过期");
        // 列出不续期、不移除；取用已过期的选择时移除
        assert_eq!(task.use_selection("mail", ttl, t0 + Duration::from_secs(60)), None);
        assert!(!task.selected.contains_key("mail"));
        assert_eq!(task.use_selection("shop", ttl, t0 + Duration::from_secs(119)), None);
        assert!(task.selected.is_empty());
        // 不过期
        task.select("shop", "b", t0);
        assert_eq!(task.use_selection("shop", None, t0 + Duration::from_secs(86_400)).as_deref(), Some("b"));
        assert_eq!(task.live_selections(None, t0 + Duration::from_secs(86_400)).count(), 1);
        // 未知 App
        assert_eq!(task.use_selection("x", ttl, t0), None);
    }
}
