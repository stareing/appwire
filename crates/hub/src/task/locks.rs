//! Agent 任务名下的对象锁（第 16 项 N6；spec/hub-api.md 3.6「对象锁」）：锁随任务存放，任务移除即全部释放（健壮锁）。
//!
//! 纯状态（不读时钟，调用方传入 `now`）：到期在取用时判定，不设定时器。Hub 侧的加锁 / 解锁工具与调用前检查在
//! [`crate::object_lock`]。

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use super::{CallerKey, TaskTable};

/// 锁的对象：`key` 为 `None` 是 App 锁（拦截其他调用方的写调用），否则为 App 内的命名锁（只与同名加锁冲突）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct LockTarget {
    pub app_id: String,
    pub key: Option<String>,
}

/// 一个任务持有的锁：对象 → 到期时刻。
#[derive(Debug, Default)]
pub(crate) struct TaskLocks {
    held: HashMap<LockTarget, Instant>,
}

impl TaskLocks {
    fn expires(&self, target: &LockTarget, now: Instant) -> Option<Instant> {
        self.held.get(target).copied().filter(|e| *e > now)
    }

    fn prune(&mut self, now: Instant) {
        self.held.retain(|_, e| *e > now);
    }
}

/// 加锁被拒的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LockRefusal {
    /// 其他持有者持有且未到期：持有者与剩余有效期。
    Held { holder: CallerKey, remaining: Duration },
    /// 本任务持有的锁已达上限。
    Limit(usize),
}

impl TaskTable {
    /// 除 `except` 外持有 `target` 且未到期的任务：(持有者, 剩余有效期)。
    pub(crate) fn lock_holder(&self, target: &LockTarget, except: &CallerKey, now: Instant) -> Option<(CallerKey, Duration)> {
        self.iter()
            .filter(|(k, _)| *k != except)
            .find_map(|(k, t)| t.locks.expires(target, now).map(|e| (k.clone(), e - now)))
    }

    /// 为 `caller` 加锁或续期（有效期从 `now` 重算）；返回是否为续期。
    ///
    /// @error 他人持有 → [`LockRefusal::Held`]；新锁使本任务持有的未到期锁超过 `max` → [`LockRefusal::Limit`]。
    pub(crate) fn acquire_lock(
        &mut self,
        caller: &CallerKey,
        target: LockTarget,
        ttl: Duration,
        max: usize,
        now: Instant,
    ) -> Result<bool, LockRefusal> {
        if let Some((holder, remaining)) = self.lock_holder(&target, caller, now) {
            return Err(LockRefusal::Held { holder, remaining });
        }
        let locks = &mut self.entry(caller).locks;
        locks.prune(now);
        let renewed = locks.held.contains_key(&target);
        if !renewed && locks.held.len() >= max {
            return Err(LockRefusal::Limit(max));
        }
        locks.held.insert(target, now + ttl);
        Ok(renewed)
    }

    /// `caller` 释放自己持有的锁；未持有或已到期时返回 `false`。
    pub(crate) fn release_lock(&mut self, caller: &CallerKey, target: &LockTarget, now: Instant) -> bool {
        let Some(task) = self.get_mut(caller) else { return false };
        let live = task.locks.expires(target, now).is_some();
        task.locks.held.remove(target);
        live
    }

    /// 全部未到期的锁：(持有者, 对象, 到期时刻)，按对象排序。
    pub(crate) fn live_locks(&self, now: Instant) -> Vec<(CallerKey, LockTarget, Instant)> {
        let mut out: Vec<_> = self
            .iter()
            .flat_map(|(k, t)| {
                t.locks.held.iter().filter(move |(_, e)| **e > now).map(move |(target, e)| (k.clone(), target.clone(), *e))
            })
            .collect();
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: Duration = Duration::from_secs(60);

    fn app(id: &str) -> LockTarget {
        LockTarget { app_id: id.into(), key: None }
    }

    fn named(id: &str, key: &str) -> LockTarget {
        LockTarget { app_id: id.into(), key: Some(key.into()) }
    }

    #[test]
    fn acquire_conflict_renew_release() {
        let (a, b) = (CallerKey::api(Some("a")), CallerKey::api(Some("b")));
        let mut t = TaskTable::default();
        let now = Instant::now();
        assert_eq!(t.acquire_lock(&a, app("shop"), TTL, 16, now), Ok(false));
        assert_eq!(t.acquire_lock(&a, app("shop"), TTL, 16, now + Duration::from_secs(10)), Ok(true), "同一持有者再加 = 续期");
        let at = now + Duration::from_secs(20);
        assert_eq!(
            t.acquire_lock(&b, app("shop"), TTL, 16, at),
            Err(LockRefusal::Held { holder: a.clone(), remaining: Duration::from_secs(50) }),
            "续期后有效期从续期时刻重算"
        );
        // App 锁与命名锁、不同命名锁互不冲突
        assert_eq!(t.acquire_lock(&b, named("shop", "doc-1"), TTL, 16, at), Ok(false));
        assert_eq!(t.acquire_lock(&a, named("shop", "doc-2"), TTL, 16, at), Ok(false));
        assert!(matches!(t.acquire_lock(&a, named("shop", "doc-1"), TTL, 16, at), Err(LockRefusal::Held { .. })));
        assert_eq!(t.lock_holder(&app("shop"), &b, at).map(|(k, _)| k), Some(a.clone()));
        assert_eq!(t.lock_holder(&app("shop"), &a, at), None, "持有者自己不算冲突");

        assert!(!t.release_lock(&b, &app("shop"), at), "不能释放他人的锁");
        assert!(t.release_lock(&a, &app("shop"), at));
        assert!(!t.release_lock(&a, &app("shop"), at), "幂等");
        assert_eq!(t.acquire_lock(&b, app("shop"), TTL, 16, at), Ok(false));
        let live: Vec<_> = t.live_locks(at).into_iter().map(|(k, target, _)| (k.as_str().to_owned(), target)).collect();
        assert_eq!(
            live,
            [("api:b".into(), app("shop")), ("api:b".into(), named("shop", "doc-1")), ("api:a".into(), named("shop", "doc-2"))]
        );
    }

    /// 到期即失效（取用时判定）；任务移除即释放其全部锁。
    #[test]
    fn expiry_and_task_removal_release() {
        let (a, b) = (CallerKey::api(Some("a")), CallerKey::api(None));
        let mut t = TaskTable::default();
        let now = Instant::now();
        t.acquire_lock(&a, app("shop"), TTL, 16, now).unwrap();
        let later = now + TTL;
        assert_eq!(t.lock_holder(&app("shop"), &b, later), None, "到期时刻起失效");
        assert!(t.live_locks(later).is_empty());
        assert!(!t.release_lock(&a, &app("shop"), later), "已到期的不算释放");

        t.acquire_lock(&a, app("shop"), TTL, 16, later).unwrap();
        t.remove(&a);
        assert_eq!(t.acquire_lock(&b, app("shop"), TTL, 16, later), Ok(false), "持有者的任务结束即释放");
    }

    /// B-07：每个任务的未到期锁数有上限；续期不计新锁，到期的锁不占名额。
    #[test]
    fn per_task_limit() {
        let a = CallerKey::api(None);
        let mut t = TaskTable::default();
        let now = Instant::now();
        t.acquire_lock(&a, named("x", "1"), TTL, 2, now).unwrap();
        t.acquire_lock(&a, named("x", "2"), Duration::from_secs(1), 2, now).unwrap();
        assert_eq!(t.acquire_lock(&a, named("x", "3"), TTL, 2, now), Err(LockRefusal::Limit(2)));
        assert_eq!(t.acquire_lock(&a, named("x", "1"), TTL, 2, now), Ok(true), "续期不受上限影响");
        let later = now + Duration::from_secs(1);
        assert_eq!(t.acquire_lock(&a, named("x", "3"), TTL, 2, later), Ok(false), "到期的锁不占名额");
    }
}
