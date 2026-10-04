//! 缓存存储：纯状态（不读时钟、不做 I/O，时刻由调用方传入）。
//!
//! @invariant 条目数 ≤ `max_entries`、字节数 ≤ `max_bytes`（插入时按最久未用淘汰）；单条 ≤ `max_entry_bytes`。
//! 过期条目在被访问时移除（不设定时器），未被访问的过期条目照常占位，最终被 LRU 淘汰。

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use tokio::time::Instant;

use super::key::CacheKey;
use super::{CacheLimits, CacheStatus};

/// 一个条目。
#[derive(Debug)]
struct Entry<V> {
    value: V,
    bytes: usize,
    produced_at: Instant,
    expires_at: Instant,
    /// 最近使用序号（[`ResultCache::order`] 的键）。
    used: u64,
}

/// 命中的条目。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hit<V> {
    pub value: V,
    /// 距产出的时长。
    pub age: Duration,
    /// 剩余有效期。
    pub remaining: Duration,
}

/// 结果缓存（[`crate::hub::HubShared::result_cache`] 的状态）。
#[derive(Debug)]
pub(crate) struct ResultCache<V> {
    limits: CacheLimits,
    entries: HashMap<CacheKey, Entry<V>>,
    /// 最近使用序号 → 键：第一项即最久未用。
    order: BTreeMap<u64, CacheKey>,
    next_use: u64,
    bytes: usize,
    /// 数据失效的代数（写调用、`resources/updated`、`stateHints` 时加一）：调用开始后发生过数据失效的结果不存，
    /// 免得把写之前读到的旧值存到写之后。
    epoch: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl<V: Clone> ResultCache<V> {
    pub(crate) fn new(limits: CacheLimits) -> Self {
        Self {
            limits,
            entries: HashMap::new(),
            order: BTreeMap::new(),
            next_use: 0,
            bytes: 0,
            epoch: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.limits.enabled()
    }

    /// 查询并计数：命中（未过期且 `accept` 为真）→ 刷新最近使用；过期 → 移除并计未命中。
    pub(crate) fn get(&mut self, key: &CacheKey, now: Instant, accept: impl Fn(&V) -> bool) -> Option<Hit<V>> {
        let Some(entry) = self.entries.get(key) else {
            self.misses += 1;
            return None;
        };
        if now >= entry.expires_at {
            self.remove(key);
            self.misses += 1;
            return None;
        }
        if !accept(&entry.value) {
            self.misses += 1;
            return None;
        }
        let hit = Hit {
            value: entry.value.clone(),
            age: now.saturating_duration_since(entry.produced_at),
            remaining: entry.expires_at.saturating_duration_since(now),
        };
        self.touch(key);
        self.hits += 1;
        Some(hit)
    }

    /// 存入（覆盖同键旧条目）。条目（键 + `payload_bytes`）超过单条上限或总上限时不存，并移除同键旧条目（以新结果为准）。
    /// 之后按最久未用淘汰到条数与字节上限之内。返回是否存入。
    pub(crate) fn insert(&mut self, key: CacheKey, value: V, payload_bytes: usize, ttl: Duration, now: Instant) -> bool {
        self.remove(&key);
        let bytes = key.bytes().saturating_add(payload_bytes);
        let Some(expires_at) = now.checked_add(ttl) else { return false };
        if !self.enabled() || bytes > self.limits.max_entry_bytes || bytes > self.limits.max_bytes {
            return false;
        }
        let used = self.bump_use();
        self.order.insert(used, key.clone());
        self.entries.insert(key, Entry { value, bytes, produced_at: now, expires_at, used });
        self.bytes += bytes;
        while self.entries.len() > self.limits.max_entries || self.bytes > self.limits.max_bytes {
            let Some((_, oldest)) = self.order.pop_first() else { break };
            if let Some(e) = self.entries.remove(&oldest) {
                self.bytes -= e.bytes;
                self.evictions += 1;
            }
        }
        true
    }

    /// 当前的数据失效代数。
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// 记一次数据失效（见 [`Self::epoch`]）。
    pub(crate) fn bump_epoch(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
    }

    /// 清空 App 的全部条目，返回清除数。
    pub(crate) fn clear_app(&mut self, app_id: &str) -> usize {
        self.remove_where(|k| k.app_id == app_id)
    }

    /// 清除 App 的资源 `name` 的条目（各范围），返回清除数。
    pub(crate) fn clear_resource(&mut self, app_id: &str, name: &str) -> usize {
        self.remove_where(|k| k.app_id == app_id && k.is_resource(name))
    }

    pub(crate) fn status(&self) -> CacheStatus {
        CacheStatus {
            entries: self.entries.len(),
            bytes: self.bytes as u64,
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
        }
    }

    fn bump_use(&mut self) -> u64 {
        self.next_use += 1;
        self.next_use
    }

    fn touch(&mut self, key: &CacheKey) {
        let used = self.bump_use();
        if let Some(e) = self.entries.get_mut(key) {
            self.order.remove(&e.used);
            e.used = used;
            self.order.insert(used, key.clone());
        }
    }

    fn remove(&mut self, key: &CacheKey) {
        if let Some(e) = self.entries.remove(key) {
            self.order.remove(&e.used);
            self.bytes -= e.bytes;
        }
    }

    fn remove_where(&mut self, pred: impl Fn(&CacheKey) -> bool) -> usize {
        let doomed: Vec<CacheKey> = self.entries.keys().filter(|k| pred(k)).cloned().collect();
        for k in &doomed {
            self.remove(k);
        }
        doomed.len()
    }
}
