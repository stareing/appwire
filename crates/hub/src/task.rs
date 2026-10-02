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
    fn as_str(self) -> &'static str {
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

/// 调用方键（单一定义；见模块文档的表）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CallerKey {
    key: String,
    lifetime: TaskLifetime,
}

impl CallerKey {
    /// legacy MCP 会话。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn mcp_session(id: u64) -> Self {
        Self { key: format!("mcp:{id}"), lifetime: TaskLifetime::UntilEnd }
    }

    /// 无会话的 MCP 请求：按主体。
    #[cfg(feature = "mcp-server")]
    pub(crate) fn principal(p: Principal) -> Self {
        Self { key: format!("principal:{}", p.as_str()), lifetime: TaskLifetime::UntilIdle }
    }

    /// Hub API 的会话（`None` = 默认会话）。
    pub(crate) fn api(session: Option<&str>) -> Self {
        let key = match session {
            Some(s) => format!("api:{s}"),
            None => "api".to_owned(),
        };
        Self { key, lifetime: TaskLifetime::UntilEnd }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.key
    }

    pub(crate) fn lifetime(&self) -> TaskLifetime {
        self.lifetime
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
    /// `apps.select`：appId → instanceId。
    pub selected: HashMap<String, String>,
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

    /// 去掉已到期的租约记录（SDK 侧同样已到期，收回时不必再发 `ttlMs: 0`）。
    pub(crate) fn prune_expired_leases(&mut self, now: Instant) {
        self.leases.retain(|_, l| l.expires().is_some_and(|e| e > now));
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
            (CallerKey::mcp_session(3), "mcp:3", TaskLifetime::UntilEnd),
            (CallerKey::principal(Principal::Local), "principal:local", TaskLifetime::UntilIdle),
            (CallerKey::api(None), "api", TaskLifetime::UntilEnd),
            (CallerKey::api(Some("x")), "api:x", TaskLifetime::UntilEnd),
        ];
        for (k, s, life) in &keys {
            assert_eq!((k.as_str(), k.lifetime()), (*s, *life));
            assert_eq!(k.to_string(), *s);
        }
        let set: HashSet<&CallerKey> = keys.iter().map(|(k, _, _)| k).collect();
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
        t.entry(&p).selected.insert("shop".into(), "a".into());
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
}
