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
//! | 无会话请求带任务句柄（工具参数 `taskId` / `_meta` `dev.appwire/taskId`，第 12 项 S8） | `principal:<主体>/<任务 ID>` | 同主体（`apps.task.end` 可提前结束） |
//!
//! 任务句柄即 [`AgentTask::id`]：由 `apps.task.begin` 签发（[`TaskTable::begin_handle`]），归签发时的主体所有，
//! 其他主体出示同一 ID 时与不存在相同。
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

use crate::agents::AgentName;
use crate::lifecycle::LeaseEntry;

mod locks;

pub(crate) use locks::{LockRefusal, LockTarget};

/// 调用方主体：由**传输层凭据**决定，不取自 `clientInfo`（MCP 规范：自报信息不可信，S-F6）。
///
/// 本机令牌（及允许不带令牌的回环请求）、IPC 同一用户、stdio 父进程都是"本机用户" [`Principal::Local`]；
/// 出示已登记 Agent 令牌的 `/mcp` 请求为 [`Principal::Agent`]（第 16 项 N5，[`crate::agents`]）。
#[cfg(feature = "mcp-server")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Principal {
    /// 本机用户（本机令牌 / IPC 同用户 / stdio 父进程）。
    Local,
    /// 已登记的 Agent（出示了其令牌）。
    Agent(AgentName),
}

#[cfg(feature = "mcp-server")]
impl Principal {
    /// 主体的字符串形式：`local` / `agent:<名>`（调用方键 `principal:<主体>`、`ApprovalRequest::principal`）。
    pub(crate) fn label(&self) -> String {
        match self {
            Principal::Local => "local".to_owned(),
            Principal::Agent(name) => format!("agent:{name}"),
        }
    }

    pub(crate) fn agent(&self) -> Option<&AgentName> {
        match self {
            Principal::Local => None,
            Principal::Agent(name) => Some(name),
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
///
/// @invariant 相等与哈希只看 `key`：字符串已唯一确定种类、是否句柄与发起方 Agent（legacy 会话的 Agent 在会话内不变）。
#[derive(Clone, Debug)]
pub(crate) struct CallerKey {
    key: String,
    kind: CallerKind,
    /// 任务句柄的调用方（`principal:<主体>/<任务 ID>`）：种类仍为 [`CallerKind::Principal`]（无会话请求），任务 ID 来自句柄。
    handle: bool,
    /// 发起方 Agent（第 16 项 N5）：Agent 主体及其任务句柄、以 Agent 令牌建立的 legacy 会话；本机用户与 Hub API 为 `None`。
    agent: Option<AgentName>,
}

/// 任务 ID 的前缀（[`AgentTask::id`]）。
const TASK_ID_PREFIX: &str = "task-";
/// 任务 ID 中随机数的十六进制位数（128 位）。
const TASK_ID_HEX_LEN: usize = 32;

/// 签发一个任务 ID：`task-` + 128 位随机数的十六进制（SEP-2567：无鉴权时句柄 ≥128 bit 随机，不可猜测）。
fn new_task_id() -> String {
    format!("{TASK_ID_PREFIX}{:0width$x}", rand::random::<u128>(), width = TASK_ID_HEX_LEN)
}

/// `id` 是否具有 Hub 签发的任务 ID 的格式（不代表该任务存在）。
pub(crate) fn is_task_id(id: &str) -> bool {
    id.strip_prefix(TASK_ID_PREFIX)
        .is_some_and(|hex| hex.len() == TASK_ID_HEX_LEN && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

impl CallerKey {
    /// legacy MCP 会话。
    #[cfg(feature = "mcp-server")]
    ///
    /// @input agent 会话建立（`initialize`）时出示的 Agent 身份。
    pub(crate) fn mcp_session(id: u64, agent: Option<AgentName>) -> Self {
        Self { key: format!("mcp:{id}"), kind: CallerKind::McpSession, handle: false, agent }
    }

    /// 无会话的 MCP 请求：按主体。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn principal(p: &Principal) -> Self {
        Self { key: format!("principal:{}", p.label()), kind: CallerKind::Principal, handle: false, agent: p.agent().cloned() }
    }

    /// Hub API 的会话（`None` = 默认会话）。
    pub(crate) fn api(session: Option<&str>) -> Self {
        let key = match session {
            Some(s) => format!("api:{s}"),
            None => "api".to_owned(),
        };
        Self { key, kind: CallerKind::Api, handle: false, agent: None }
    }

    /// 主体名下的任务句柄 `task_id`（`owner` 为无会话主体的调用方键）。
    ///
    /// @input task_id 已通过 [`is_task_id`]（键中不会出现 `/` 等分隔字符）。
    pub(crate) fn task_handle(owner: &CallerKey, task_id: &str) -> Self {
        debug_assert!(owner.can_own_handles() && is_task_id(task_id));
        Self { key: format!("{}/{task_id}", owner.key), kind: owner.kind, handle: true, agent: owner.agent.clone() }
    }

    /// 可以签发 / 使用任务句柄的调用方：无会话主体本身（legacy 会话与 Hub API 已各有自己的任务，句柄不嵌套）。
    pub(crate) fn can_own_handles(&self) -> bool {
        self.kind == CallerKind::Principal && !self.handle
    }

    /// 任务句柄的调用方。
    pub(crate) fn is_task_handle(&self) -> bool {
        self.handle
    }

    /// 任务句柄的任务 ID；不是句柄时为 `None`。
    pub(crate) fn handle_task_id(&self) -> Option<&str> {
        self.handle.then(|| self.key.rsplit_once('/').map_or("", |(_, id)| id))
    }

    /// 本调用方键是否为 `owner` 名下的任务句柄。
    fn is_handle_of(&self, owner: &CallerKey) -> bool {
        self.handle && self.key.strip_prefix(owner.key.as_str()).is_some_and(|rest| rest.starts_with('/'))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.key
    }

    pub(crate) fn kind(&self) -> CallerKind {
        self.kind
    }

    /// 发起方 Agent 名（第 16 项 N5）；本机用户与 Hub API 为 `None`。
    pub(crate) fn agent(&self) -> Option<&AgentName> {
        self.agent.as_ref()
    }

    /// 记账主体（第 16 项 P3，[`crate::usage`]）：已登记 Agent → `agent:<名>`，Hub API → `api`，其余 MCP 调用方 → `local`。
    pub(crate) fn usage_subject(&self) -> String {
        match (&self.agent, self.kind) {
            (Some(a), _) => format!("agent:{a}"),
            (None, CallerKind::Api) => "api".to_owned(),
            (None, _) => "local".to_owned(),
        }
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

impl PartialEq for CallerKey {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for CallerKey {}

impl std::hash::Hash for CallerKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key.hash(state);
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
/// 第 12 项 S8 任务句柄（[`AgentTask::id`] 即句柄，经工具参数 / 结果传递，[`CallerKey::task_handle`]）。
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
    /// 对象锁（第 16 项 N6）：随任务移除一并释放。
    pub locks: locks::TaskLocks,
    /// 撤销记录（第 15 项 X2，[`crate::undo`]）：随任务移除一并清除。
    pub undo: crate::undo::TaskUndo,
}

impl AgentTask {
    fn new(id: String) -> Self {
        Self {
            id,
            selected: HashMap::new(),
            delivered: HashMap::new(),
            leases: HashMap::new(),
            exposed: HashSet::new(),
            locks: locks::TaskLocks::default(),
            undo: crate::undo::TaskUndo::default(),
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
    ///
    /// @why 句柄调用方的任务 ID 取自句柄：句柄在一次调用进行中被 `apps.task.end` 结束时，该调用随后写入的状态（如租约）
    /// 重建的任务仍是同一 ID，且没有请求活动记录，下一轮空闲回收即收回（不会以另一个 ID 泄漏）。
    pub(crate) fn entry(&mut self, key: &CallerKey) -> &mut AgentTask {
        self.tasks.entry(key.clone()).or_insert_with(|| {
            let task = AgentTask::new(key.handle_task_id().map_or_else(new_task_id, str::to_owned));
            tracing::debug!(caller = %key, task = %task.id, "创建 Agent 任务");
            task
        })
    }

    /// 为 `owner` 签发一个任务句柄并创建其任务。
    ///
    /// @error `owner` 名下已有 `max` 个句柄（B-07）→ `Err(max)`。
    pub(crate) fn begin_handle(&mut self, owner: &CallerKey, max: usize) -> Result<CallerKey, usize> {
        if self.handle_count(owner) >= max {
            return Err(max);
        }
        let key = CallerKey::task_handle(owner, &new_task_id());
        tracing::debug!(caller = %key, "签发 Agent 任务句柄");
        self.entry(&key);
        Ok(key)
    }

    /// `owner` 名下现有的任务句柄数。
    pub(crate) fn handle_count(&self, owner: &CallerKey) -> usize {
        self.tasks.keys().filter(|k| k.is_handle_of(owner)).count()
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
            (CallerKey::mcp_session(3, None), "mcp:3", TaskLifetime::UntilEnd, CallerKind::McpSession),
            (CallerKey::principal(&Principal::Local), "principal:local", TaskLifetime::UntilIdle, CallerKind::Principal),
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
        assert!(keys.iter().all(|(k, _, _, _)| !k.is_task_handle()));
        assert_eq!(keys.iter().filter(|(k, _, _, _)| k.can_own_handles()).count(), 1, "只有无会话主体可持有句柄");
    }

    /// N5：每个 Agent 一个主体（各自的任务与句柄）；句柄与 legacy 会话带着发起方 Agent；本机用户与 Hub API 没有。
    #[test]
    fn agent_principals_are_separate_callers() {
        let claude = Principal::Agent(AgentName::for_test("claude"));
        let cursor = Principal::Agent(AgentName::for_test("cursor"));
        let (pc, pr, pl) = (CallerKey::principal(&claude), CallerKey::principal(&cursor), CallerKey::principal(&Principal::Local));
        assert_eq!((pc.as_str(), pr.as_str()), ("principal:agent:claude", "principal:agent:cursor"));
        assert_eq!(pc.agent().map(AgentName::as_str), Some("claude"));
        assert_eq!((pl.agent(), CallerKey::api(None).agent()), (None, None));
        assert!(pc.can_own_handles() && pc.is_stateless());

        let mut t = TaskTable::default();
        let h = t.begin_handle(&pc, 1).expect("claude 的句柄");
        assert_eq!(h.agent().map(AgentName::as_str), Some("claude"));
        assert!(h.as_str().starts_with("principal:agent:claude/task-"), "{h}");
        assert!(t.begin_handle(&pr, 1).is_ok(), "句柄上限按 Agent 分别计算");
        assert_eq!((t.handle_count(&pc), t.handle_count(&pr), t.handle_count(&pl)), (1, 1, 0));
        t.entry(&pc).select("shop", "a", Instant::now());
        assert!(t.get(&pr).is_none() && t.get(&pl).is_none(), "选择不跨 Agent");

        let s = CallerKey::mcp_session(7, claude.agent().cloned());
        assert_eq!((s.as_str(), s.agent().map(AgentName::as_str)), ("mcp:7", Some("claude")));
    }

    #[test]
    fn task_id_format() {
        let id = new_task_id();
        assert!(is_task_id(&id), "{id}");
        for bad in ["", "task-", "task-0123", "TASK-0123456789abcdef0123456789abcdef", "task-0123456789ABCDEF0123456789abcdef",
            "task-0123456789abcdef0123456789abcdeg", "task-0123456789abcdef0123456789abcdef0", "principal:local/task-0"] {
            assert!(!is_task_id(bad), "{bad}");
        }
    }

    /// 句柄：每个句柄一个独立任务（任务 ID = 句柄），归签发主体；数量上限；结束后同一句柄不复存在。
    #[test]
    fn task_handles_are_separate_tasks_with_cap() {
        let mut t = TaskTable::default();
        let p = CallerKey::principal(&Principal::Local);
        let a = t.begin_handle(&p, 2).expect("a");
        let b = t.begin_handle(&p, 2).expect("b");
        assert_eq!(t.begin_handle(&p, 2), Err(2), "超过上限");
        assert_eq!(t.handle_count(&p), 2);
        for k in [&a, &b] {
            let id = k.handle_task_id().expect("id").to_owned();
            assert!(is_task_id(&id) && k.as_str() == format!("principal:local/{id}"), "{k}");
            assert_eq!(t.get(k).map(|x| x.id.clone()), Some(id));
            assert!(k.is_task_handle() && k.is_stateless() && !k.can_own_handles());
            assert_eq!((k.kind(), k.lifetime()), (CallerKind::Principal, TaskLifetime::UntilIdle));
        }
        assert_ne!(a, b);
        // 主体自己的任务不是句柄，不计数；句柄的选择互不影响
        t.entry(&p).select("shop", "p", Instant::now());
        t.entry(&a).select("shop", "x", Instant::now());
        assert_eq!(t.handle_count(&p), 2);
        assert!(t.get(&b).is_some_and(|x| x.selected.is_empty()));
        // 其他调用方名下没有句柄；与主体同前缀但不是句柄的键不计
        assert_eq!(t.handle_count(&CallerKey::mcp_session(1, None)), 0);
        let mut idle = t.idle_lifetime_keys();
        idle.sort_by(|x, y| x.as_str().cmp(y.as_str()));
        assert_eq!(idle.len(), 3, "句柄任务与主体任务都按空闲回收");
        assert!(t.remove(&a).is_some());
        assert!(t.get(&a).is_none());
        assert_eq!(t.handle_count(&p), 1);
        assert!(t.begin_handle(&p, 2).is_ok(), "结束一个后可再签发");
        // 句柄键重建任务时沿用句柄的 ID
        let id_b = t.get(&b).map(|x| x.id.clone());
        t.remove(&b);
        assert_eq!(Some(t.entry(&b).id.clone()), id_b);
    }

    #[test]
    fn one_task_per_caller_with_stable_unguessable_id() {
        let mut t = TaskTable::default();
        let p = CallerKey::principal(&Principal::Local);
        let s = CallerKey::mcp_session(1, None);
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
        let p = CallerKey::principal(&Principal::Local);
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
