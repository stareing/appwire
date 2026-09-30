//! 实例路由（`app-mcp-plan.md` 12.2.2）。
//!
//! 目标实例的优先级：
//! 1. 会话中通过 `apps.select` 选定的实例（仍连接时）；
//! 2. 已聚焦的实例；
//! 3. 最近活跃的实例（最近一次可见性变为 visible / 获得焦点，或最近一次完成调用）；
//! 4. 最早连接的实例。
//!
//! 调用方只传入“注册了该工具 / 资源”的实例，本模块只负责排序。

use std::cmp::Ordering;

/// 参与路由的实例摘要。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate<'a> {
    pub instance_id: &'a str,
    pub focused: bool,
    /// 最近活跃序号（越大越近）；从未活跃为 `None`。
    pub last_active: Option<u64>,
    /// 连接序号（越小越早）。
    pub connected: u64,
}

fn compare(a: &Candidate<'_>, b: &Candidate<'_>, selected: Option<&str>) -> Ordering {
    let sel = |c: &Candidate<'_>| selected == Some(c.instance_id);
    sel(b)
        .cmp(&sel(a))
        .then_with(|| b.focused.cmp(&a.focused))
        // None < Some(_)，所以 b 与 a 比较得到“最近活跃在前”。
        .then_with(|| b.last_active.cmp(&a.last_active))
        .then_with(|| a.connected.cmp(&b.connected))
}

/// 按路由优先级排序（第一个即目标实例）。
pub fn order<'a>(candidates: &[Candidate<'a>], selected: Option<&str>) -> Vec<&'a str> {
    let mut sorted: Vec<&Candidate<'a>> = candidates.iter().collect();
    sorted.sort_by(|a, b| compare(a, b, selected));
    sorted.into_iter().map(|c| c.instance_id).collect()
}

/// 选出目标实例；没有候选时返回 `None`。
pub fn pick<'a>(candidates: &[Candidate<'a>], selected: Option<&str>) -> Option<&'a str> {
    candidates
        .iter()
        .min_by(|a, b| compare(a, b, selected))
        .map(|c| c.instance_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, focused: bool, last_active: Option<u64>, connected: u64) -> Candidate<'_> {
        Candidate {
            instance_id: id,
            focused,
            last_active,
            connected,
        }
    }

    #[test]
    fn empty() {
        assert_eq!(pick(&[], None), None);
        assert_eq!(pick(&[], Some("a")), None);
    }

    #[test]
    fn earliest_connected_by_default() {
        let cs = [c("b", false, None, 2), c("a", false, None, 1)];
        assert_eq!(pick(&cs, None), Some("a"));
    }

    #[test]
    fn most_recently_active_beats_earliest() {
        let cs = [
            c("a", false, Some(3), 1),
            c("b", false, Some(7), 2),
            c("c", false, None, 0),
        ];
        assert_eq!(pick(&cs, None), Some("b"));
        assert_eq!(order(&cs, None), vec!["b", "a", "c"]);
    }

    #[test]
    fn focused_beats_recent() {
        let cs = [c("a", false, Some(9), 1), c("b", true, Some(2), 2)];
        assert_eq!(pick(&cs, None), Some("b"));
    }

    #[test]
    fn selected_beats_everything() {
        let cs = [c("a", true, Some(9), 1), c("b", false, None, 2)];
        assert_eq!(pick(&cs, Some("b")), Some("b"));
        assert_eq!(order(&cs, Some("b")), vec!["b", "a"]);
    }

    #[test]
    fn selected_not_among_candidates_is_ignored() {
        let cs = [c("a", false, None, 1), c("b", true, None, 2)];
        assert_eq!(pick(&cs, Some("gone")), Some("b"));
    }

    #[test]
    fn multiple_focused_uses_recency_then_connect_order() {
        let cs = [
            c("a", true, Some(1), 1),
            c("b", true, Some(5), 2),
            c("c", true, Some(5), 0),
        ];
        assert_eq!(pick(&cs, None), Some("c"));
    }
}
