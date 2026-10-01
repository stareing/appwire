//! 按键有序的小映射 / 集合：底层为按键排序的 `Vec`，二分查找。
//!
//! @why 核心里的映射都很小（工具、资源、scope、进行中的请求 / 读取 / 订阅 / 持有，通常几十项以内），
//!      `BTreeMap` / `HashMap` 每种键值类型各生成一整套节点与哈希表代码，是 WASM 体积的最大来源；
//!      排序 `Vec` 的单态化代码很小，查找 O(log n)，插入 / 删除为一次内存移动。
//! @invariant `entries` 按键严格递增（无重复键）；迭代顺序即键顺序，与 `BTreeMap` 相同。
//! @compat 句柄（`ToolId` 等）单调递增分配，按句柄插入总是追加到末尾（O(1)）。

use std::borrow::Borrow;

/// 按键有序的映射，接口取 `BTreeMap` 中核心用到的子集。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VecMap<K, V> {
    entries: Vec<(K, V)>,
}

impl<K, V> Default for VecMap<K, V> {
    fn default() -> Self {
        VecMap {
            entries: Vec::new(),
        }
    }
}

impl<K: Ord, V> VecMap<K, V> {
    /// 键所在位置（`Ok`）或应插入的位置（`Err`）。
    fn position<Q>(&self, key: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        // 单调递增句柄的常见情形：新键大于末尾键，免二分
        match self.entries.last() {
            None => Err(0),
            Some((last, _)) if last.borrow() < key => Err(self.entries.len()),
            _ => self.entries.binary_search_by(|(k, _)| k.borrow().cmp(key)),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.position(key).is_ok()
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.position(key).ok().map(|i| &self.entries[i].1)
    }

    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.position(key).ok().map(|i| &mut self.entries[i].1)
    }

    /// 插入或替换，返回旧值。
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        match self.position(&key) {
            Ok(i) => Some(std::mem::replace(&mut self.entries[i].1, value)),
            Err(i) => {
                self.entries.insert(i, (key, value));
                None
            }
        }
    }

    /// 取键对应的值，不存在时插入默认值（`entry(key).or_default()`）。
    pub fn get_or_insert_default(&mut self, key: K) -> &mut V
    where
        V: Default,
    {
        let i = match self.position(&key) {
            Ok(i) => i,
            Err(i) => {
                self.entries.insert(i, (key, V::default()));
                i
            }
        };
        &mut self.entries[i].1
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.position(key).ok().map(|i| self.entries.remove(i).1)
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&K, &mut V) -> bool) {
        self.entries.retain_mut(|(k, v)| keep(k, v));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, v)| v)
    }
}

impl<K: Ord, V> FromIterator<(K, V)> for VecMap<K, V> {
    /// 重复键保留最后一个值（与 `BTreeMap` 的 `collect` 一致）。
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = VecMap::default();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

/// 按键有序的集合，接口取 `BTreeSet` 中核心用到的子集。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VecSet<K> {
    items: Vec<K>,
}

impl<K> Default for VecSet<K> {
    fn default() -> Self {
        VecSet { items: Vec::new() }
    }
}

impl<K: Ord> VecSet<K> {
    /// 插入，返回之前是否不存在。
    pub fn insert(&mut self, key: K) -> bool {
        match self.items.binary_search(&key) {
            Ok(_) => false,
            Err(i) => {
                self.items.insert(i, key);
                true
            }
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.items.binary_search(key).is_ok()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

impl<K> IntoIterator for VecSet<K> {
    type Item = K;
    type IntoIter = std::vec::IntoIter<K>;

    /// 按键升序。
    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_keeps_key_order_and_replaces() {
        let mut m = VecMap::default();
        assert!(m.is_empty());
        assert_eq!(m.insert(3, "c"), None);
        assert_eq!(m.insert(1, "a"), None);
        assert_eq!(m.insert(5, "e"), None);
        assert_eq!(m.insert(2, "b"), None);
        assert_eq!(m.insert(3, "C"), Some("c"));
        assert_eq!(m.len(), 4);
        assert_eq!(m.iter().map(|(k, _)| *k).collect::<Vec<_>>(), [1, 2, 3, 5]);
        assert_eq!(
            m.values().copied().collect::<Vec<_>>(),
            ["a", "b", "C", "e"]
        );
        assert_eq!(m.get(&3), Some(&"C"));
        assert_eq!(m.get(&4), None);
        assert!(m.contains_key(&5));
        assert!(!m.contains_key(&0));
        *m.get_mut(&1).unwrap() = "A";
        assert_eq!(m.get(&1), Some(&"A"));
        assert_eq!(m.get_mut(&9), None);
    }

    #[test]
    fn map_remove_retain_clear() {
        let mut m: VecMap<u64, u64> = (1..=6).map(|k| (k, k * 10)).collect();
        assert_eq!(m.remove(&3), Some(30));
        assert_eq!(m.remove(&3), None);
        assert_eq!(m.remove(&99), None);
        m.retain(|k, v| {
            *v += 1;
            k % 2 == 0
        });
        assert_eq!(
            m.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(),
            [(2, 21), (4, 41), (6, 61)]
        );
        m.clear();
        assert!(m.is_empty());
        assert_eq!(m.get(&2), None);
    }

    #[test]
    fn map_borrowed_string_keys() {
        let mut m: VecMap<String, u32> = VecMap::default();
        m.insert("b".into(), 2);
        m.insert("a".into(), 1);
        assert_eq!(m.get("a"), Some(&1));
        assert!(m.contains_key("b"));
        assert_eq!(m.remove("b"), Some(2));
        *m.get_or_insert_default("c".into()) += 5;
        *m.get_or_insert_default("c".into()) += 1;
        assert_eq!(m.get("c"), Some(&6));
        assert_eq!(
            m.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["a", "c"]
        );
    }

    #[test]
    fn map_collect_keeps_last_duplicate() {
        let m: VecMap<&str, u32> = [("x", 1), ("y", 2), ("x", 3)].into_iter().collect();
        assert_eq!(
            m.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(),
            [("x", 3), ("y", 2)]
        );
    }

    #[test]
    fn set_dedups_and_iterates_sorted() {
        let mut s = VecSet::default();
        assert!(s.insert("b".to_owned()));
        assert!(s.insert("a".to_owned()));
        assert!(!s.insert("b".to_owned()));
        assert!(s.contains(&"a".to_owned()));
        assert!(!s.contains(&"z".to_owned()));
        assert_eq!(
            std::mem::take(&mut s).into_iter().collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(!s.contains(&"a".to_owned()));
        s.insert("c".to_owned());
        s.clear();
        assert!(!s.contains(&"c".to_owned()));
    }
}
