//! 撤销记录（纯状态）与上限配置的单元测试；端到端见 `tests/it/undo.rs`。

use std::time::Duration;

use serde_json::json;
use tokio::time::Instant;

use super::{DEFAULT_UNDO_MAX_PER_TASK, DEFAULT_UNDO_TTL, TaskUndo, UndoLimits, UndoRecord};
use crate::call::{BuiltinSet, builtin_hub_tools};
use crate::names::TOOL_APPS_UNDO;

const TTL: Duration = Duration::from_secs(60);

fn rec(call_id: &str, at: Instant) -> UndoRecord {
    UndoRecord {
        call_id: call_id.to_owned(),
        app_id: "todo".into(),
        instance_id: Some("i1".into()),
        tool: "remove".into(),
        arguments: json!({ "id": call_id }),
        label: None,
        at,
    }
}

fn ids(u: &mut TaskUndo, now: Instant) -> Vec<String> {
    let mut out = Vec::new();
    // @why 有界循环：取出若不移除（实现缺陷）时测试失败而不是无限分配内存。
    for _ in 0..16 {
        let Some(r) = u.take(None, TTL, now) else { break };
        out.push(r.call_id);
    }
    out.reverse();
    out
}

/// 缺省取最近一条；指定 callId 取该条；取出即移除（只能撤销一次）；不存在 → `None`。
#[test]
fn take_latest_or_by_id_once() {
    let t0 = Instant::now();
    let mut u = TaskUndo::default();
    for id in ["a", "b", "c"] {
        u.push(rec(id, t0), TTL, 8);
    }
    assert_eq!(u.take(None, TTL, t0).map(|r| r.call_id).as_deref(), Some("c"), "缺省：最近一条");
    assert_eq!(u.take(Some("a"), TTL, t0).map(|r| r.call_id).as_deref(), Some("a"), "指定 callId");
    assert_eq!(u.take(Some("a"), TTL, t0), None, "同一 callId 只能取出一次");
    assert_eq!(u.take(Some("zzz"), TTL, t0), None, "不存在");
    assert_eq!(u.take(None, TTL, t0).map(|r| r.call_id).as_deref(), Some("b"));
    assert_eq!(u.take(None, TTL, t0), None, "空表");
}

/// 过期记录在取用时丢弃：恰好满 TTL 即过期；缺省取用跳过过期的、取未过期中最近的一条。
#[test]
fn ttl_is_lazy_and_exact() {
    let t0 = Instant::now();
    let mut u = TaskUndo::default();
    u.push(rec("old", t0), TTL, 8);
    u.push(rec("new", t0 + Duration::from_secs(30)), TTL, 8);
    assert_eq!(u.len(), 2, "不设定时器：到期前后都留在表中直到取用");
    let at_edge = t0 + TTL - Duration::from_millis(1);
    assert_eq!(u.take(Some("old"), TTL, at_edge).map(|r| r.call_id).as_deref(), Some("old"), "差 1 ms 未过期");
    u.push(rec("old", t0), TTL, 8);
    assert_eq!(u.take(Some("old"), TTL, t0 + TTL), None, "满 TTL 即过期");
    assert_eq!(u.len(), 1, "取用时丢弃了过期记录");
    assert_eq!(ids(&mut u, t0 + TTL), vec!["new"]);
    u.push(rec("x", t0), TTL, 8);
    assert_eq!(u.take(None, TTL, t0 + TTL), None, "缺省取用同样不返回过期记录");
}

/// 超过条数上限时丢弃最早的；登记时也丢弃已过期的；同一 callId 再登记替换旧记录；上限 0 不登记。
#[test]
fn cap_drops_oldest() {
    let t0 = Instant::now();
    let mut u = TaskUndo::default();
    for id in ["a", "b", "c", "d"] {
        u.push(rec(id, t0), TTL, 3);
    }
    assert_eq!(u.len(), 3);
    assert_eq!(u.take(Some("a"), TTL, t0), None, "最早的一条被丢弃");
    assert_eq!(ids(&mut u, t0), vec!["b", "c", "d"]);

    u.push(rec("stale", t0), TTL, 2);
    u.push(rec("fresh", t0 + TTL), TTL, 2);
    assert_eq!(u.len(), 1, "登记时丢弃过期记录（不占条数）");
    u.push(rec("fresh", t0 + TTL), TTL, 2);
    assert_eq!(u.len(), 1, "同一 callId 只保留一条");
    u.push(rec("z", t0 + TTL), TTL, 0);
    assert_eq!(ids(&mut u, t0 + TTL), vec!["fresh"], "上限 0 不登记");
}

#[test]
fn limits_defaults_overrides_and_validation() {
    let d = UndoLimits::default();
    assert_eq!((d.ttl, d.max_per_task), (DEFAULT_UNDO_TTL, DEFAULT_UNDO_MAX_PER_TASK));
    assert_eq!((DEFAULT_UNDO_TTL, DEFAULT_UNDO_MAX_PER_TASK), (Duration::from_secs(1800), 32), "spec/hub-api.md 3.23 的默认值");
    assert!(d.enabled() && d.validate().is_ok());
    assert_eq!(UndoLimits::with_overrides(None, None), d);
    let l = UndoLimits::with_overrides(Some(1500), Some(4));
    assert_eq!((l.ttl, l.max_per_task), (Duration::from_millis(1500), 4));
    let off = UndoLimits::with_overrides(Some(0), Some(0));
    assert!(!off.enabled() && off.validate().is_ok(), "关闭时不校验 TTL");
    let err = UndoLimits::with_overrides(Some(0), None).validate().expect_err("开启时 TTL 为 0");
    assert!(err.contains("undo.ttlMs"), "{err}");
}

/// `apps.undo` 只在撤销开启时列出；注解按 spec/hub-api.md 3.23；接受 `taskId`。
#[test]
fn apps_undo_listed_only_when_enabled() {
    let listed = |undo| builtin_hub_tools(BuiltinSet { undo, tasks: true, ..BuiltinSet::default() });
    assert!(!listed(false).iter().any(|t| t.name == TOOL_APPS_UNDO), "关闭时不列出");
    let tools = listed(true);
    let t = tools.iter().find(|t| t.name == TOOL_APPS_UNDO).expect("开启时列出");
    let a = &t.annotations;
    assert_eq!(
        (a.read_only_hint, a.destructive_hint, a.idempotent_hint, a.open_world_hint),
        (Some(false), Some(true), Some(false), Some(false))
    );
    assert!(t.input_schema["properties"]["callId"].is_object() && t.input_schema["properties"]["taskId"].is_object(), "{}", t.input_schema);
    assert!(t.input_schema.get("required").is_none(), "callId 可省略");
}
