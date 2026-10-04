//! 空闲条件。

use super::support::*;

// ---------------------------------------------------------------------------
// 空闲条件：每个重置条件
// ---------------------------------------------------------------------------

#[test]
fn running_and_queued_calls_block_idle_and_reset_timer() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.advance(IDLE - 1_000);
    h.invoke("c1", "a");
    h.invoke("c2", "a"); // 排队
    assert_eq!(h.c.queued_call_count(), 1);
    assert_eq!(h.wait_sleep(3 * IDLE), None, "调用进行中不休眠");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(h.wait_sleep(3 * IDLE), None, "排队中的调用开始执行，仍不休眠");
    h.c.complete_call("c2", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    let done = h.now;
    let (_, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, done + MERGE, "从最后一个调用完成时重新计时（处理过调用后为合并窗口）");
}

#[test]
fn resource_read_blocks_idle_and_completion_rechecks() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_resource(resource("r")).unwrap();
    h.connect();
    let ev = h.request("resources/read", json!({"name": "r"}));
    let read = ev
        .iter()
        .find_map(|e| match e {
            Event::ReadResource { read, .. } => Some(*read),
            _ => None,
        })
        .unwrap();
    assert_eq!(h.wait_sleep(3 * IDLE), None, "读取进行中不休眠");
    h.c.complete_read(read, Ok(json!(1))).unwrap();
    h.drain();
    assert_eq!(h.c.poll_timeout(), Some(h.now), "没有时间参数时请求立即重新判定");
    h.advance(0);
    let done = h.now;
    let (_, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, done + MERGE, "读取也算处理过调用：合并窗口");
}

#[test]
fn navigation_blocks_idle_and_completion_rechecks() {
    let mut cfg = config(LifecycleMode::Idle);
    cfg.navigation = true;
    let mut h = Harness::with(cfg);
    h.connect();
    let ev = h.request("app/navigate", json!({"page": "cart"}));
    let nav = ev
        .iter()
        .find_map(|e| match e {
            Event::Navigate { navigate, .. } => Some(*navigate),
            _ => None,
        })
        .unwrap();
    assert_eq!(h.wait_sleep(3 * IDLE), None, "导航进行中不休眠");
    h.c.complete_navigate(nav, Ok(())).unwrap();
    h.drain();
    assert_eq!(h.c.poll_timeout(), Some(h.now), "完成后立即重新判定");
    h.advance(0);
    let done = h.now;
    let (_, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, done + MERGE, "导航也算处理过请求：合并窗口");
}

#[test]
fn realtime_subscription_blocks_idle() {
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(realtime("r")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "r"}));
    assert!(h.c.is_subscribed(r));
    assert_eq!(h.wait_sleep(3 * IDLE), None, "有实时资源订阅时不休眠");
    h.request("resources/unsubscribe", json!({"name": "r"}));
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);

    // 注销资源导致订阅被清除时同样重新判定
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(realtime("r")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "r"}));
    h.advance(IDLE);
    h.c.unregister_resource(r).unwrap();
    h.drain();
    assert_eq!(h.c.poll_timeout(), Some(h.now));
    h.advance(0);
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);
}

#[test]
fn hold_blocks_idle_until_released() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    let hold = h.c.hold(h.now);
    let hold2 = h.c.hold(h.now);
    assert_ne!(hold, hold2);
    assert_eq!(h.c.hold_count(), 2);
    assert_eq!(h.wait_sleep(3 * IDLE), None);
    assert!(h.c.release_hold(hold, h.now));
    assert!(!h.c.release_hold(hold, h.now), "重复释放返回 false");
    assert_eq!(h.wait_sleep(3 * IDLE), None, "仍有一个持有");
    assert!(h.c.release_hold(hold2, h.now));
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);
}

#[test]
fn hold_for_call_outlives_the_call() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    assert_eq!(h.c.hold_for_call("nope", h.now), Err(CoreError::UnknownCall("nope".into())));
    h.invoke("c1", "a");
    let hold = h.c.hold_for_call("c1", h.now).unwrap();
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(h.wait_sleep(3 * IDLE), None, "调用完成后持有仍然有效");
    h.c.release_hold(hold, h.now);
    assert!(h.wait_sleep(3 * IDLE).is_some());
}

#[test]
fn visibility_change_restarts_timer_and_hidden_is_shorter() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.advance(50_000);
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    h.drain();
    let t = h.now;
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t + 15_000, "隐藏后使用 hiddenIdleTimeoutMs，并从变化时重新计时");
    assert_eq!(sleep["params"]["reason"], "idle", "空闲计时到期一律为 idle，隐藏只缩短计时");

    // 重新可见也重新计时
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    h.connect();
    h.advance(10_000);
    h.c.set_visibility(Visibility::Visible, true, h.now);
    h.drain();
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);
}

#[test]
fn lease_defers_sleep_and_zero_cancels() {
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.advance(10_000);
    h.notify("app/lease", json!({"ttlMs": 100_000}));
    let lease_end = h.now + 100_000;
    // 更短的新租约不缩短截止时刻
    h.advance(1_000);
    h.notify("app/lease", json!({"ttlMs": 5_000}));
    let (_, at) = h.wait_sleep(10 * IDLE).unwrap();
    assert_eq!(at, lease_end, "A1：租约与空闲计时并行，休眠时刻 = max(空闲起点 + 空闲时长, 租约到期)");

    // 租约比空闲时长短：按空闲计时
    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.notify("app/lease", json!({"ttlMs": 10_000}));
    let t = h.now;
    assert_eq!(h.wait_sleep(10 * IDLE).unwrap().1, t + IDLE);

    // 回退开关：租约到期后才开始计空闲（旧行为）
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.legacy_timers = true;
    let mut h = Harness::with(cfg);
    h.connect();
    h.notify("app/lease", json!({"ttlMs": 100_000}));
    let lease_end = h.now + 100_000;
    assert_eq!(h.wait_sleep(10 * IDLE).unwrap().1, lease_end + IDLE, "legacy_timers：串行");

    let mut h = Harness::new(LifecycleMode::Idle);
    h.connect();
    h.notify("app/lease", json!({"ttlMs": 1_000_000}));
    h.advance(IDLE * 2);
    assert!(!h.c.is_sleep_pending());
    h.notify("app/lease", json!({"ttlMs": 0}));
    let t = h.now;
    assert_eq!(h.wait_sleep(10 * IDLE).unwrap().1, t + IDLE, "取消租约后重新计时");

    // 参数无效只告警
    let ev = h.notify("app/lease", json!({"ttl": 1}));
    assert!(ev.iter().any(|e| matches!(e, Event::Warning(_))));
}
