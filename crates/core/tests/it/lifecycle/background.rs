//! 合并窗口、订阅与后台休眠。

use super::support::*;

// ---------------------------------------------------------------------------
// 4e 第二部分（spec/lifecycle.md 第 13 节）：B1 合并窗口、B3 订阅、B4 后台立即休眠
// ---------------------------------------------------------------------------

#[test]
fn merge_window_applies_only_after_a_call_and_respects_lease() {
    // 连上后没有调用：仍按 idleTimeoutMs
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    let t0 = h.now;
    h.connect();
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t0 + IDLE);

    // 调用后有租约：在线到租约到期（合并窗口不缩短租约）
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.notify("app/lease", json!({"ttlMs": 30_000}));
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + 30_000);

    // 合并窗口不小于空闲时长：等同旧行为
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.merge_window_ms = 10 * IDLE;
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);

    // "处理过调用"随连接清除：回连后没有调用又按 idleTimeoutMs
    let mut h = Harness::new(LifecycleMode::Idle);
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    h.go_dormant();
    assert!(h.c.wake(h.now));
    h.drain();
    let hello = h.link();
    h.paired(&hello, false);
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);
}

#[test]
fn plain_subscription_does_not_block_idle_and_changes_are_replayed_on_resubscribe() {
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(resource("cart")).unwrap();
    let other = h.c.register_resource(resource("other")).unwrap();
    let t0 = h.now;
    h.connect();
    h.request("resources/subscribe", json!({"name": "cart"}));
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t0 + IDLE, "普通资源的订阅不阻止休眠");
    h.accept_sleep(&sleep, "resume-1");

    // 休眠期间变化：不回连，只记下
    h.c.notify_resource_changed(r, h.now).unwrap();
    h.c.notify_resource_changed(other, h.now).unwrap();
    assert!(h.drain().is_empty(), "普通资源变化不回连");
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    assert_eq!(h.c.poll_timeout(), None);

    // 回连后 Host 重新订阅：先回复 {}，再补发 resources/updated；未订阅过的资源不补发
    assert!(h.c.wake(h.now));
    h.drain();
    let hello = h.link();
    h.paired(&hello, true);
    let ev = h.request("resources/subscribe", json!({"name": "cart"}));
    let msgs = sends(&ev);
    assert_eq!(methods(&msgs), vec!["<response>", "resources/updated"]);
    assert_eq!(msgs[1]["params"]["name"], "cart");
    // 只补发一次
    h.request("resources/unsubscribe", json!({"name": "cart"}));
    let ev = h.request("resources/subscribe", json!({"name": "cart"}));
    assert_eq!(methods(&sends(&ev)), vec!["<response>"]);
}

#[test]
fn change_before_resubscribe_is_replayed_and_unclaimed_subscriptions_are_dropped() {
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(resource("cart")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "cart"}));
    // 断线（不是休眠）后也补发
    h.c.handle_disconnected(h.now);
    h.drain();
    h.c.notify_resource_changed(r, h.now).unwrap();
    h.advance(1_000);
    let hello = h.link();
    h.paired(&hello, false);
    // 已连接但 Host 尚未重新订阅时的变化同样记下
    h.c.notify_resource_changed(r, h.now).unwrap();
    assert!(sends(&h.drain()).is_empty());
    let ev = h.request("resources/subscribe", json!({"name": "cart"}));
    assert_eq!(methods(&sends(&ev)), vec!["<response>", "resources/updated"]);

    // Host 这次连接没有重新订阅：下次断开后不再跟踪
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(resource("cart")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "cart"}));
    h.c.handle_disconnected(h.now);
    h.drain();
    h.advance(1_000);
    let hello = h.link();
    h.paired(&hello, false);
    h.c.handle_disconnected(h.now);
    h.drain();
    h.c.notify_resource_changed(r, h.now).unwrap();
    h.advance(1_000);
    let hello = h.link();
    h.paired(&hello, false);
    let ev = h.request("resources/subscribe", json!({"name": "cart"}));
    assert_eq!(methods(&sends(&ev)), vec!["<response>"]);
}

#[test]
fn realtime_change_while_dormant_wakes_to_push() {
    let mut h = Harness::new(LifecycleMode::Idle);
    let r = h.c.register_resource(realtime("order")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "order"}));
    // 实时订阅阻止自动休眠；App 显式休眠
    assert!(h.c.sleep(h.now));
    let sleep = find(&sends(&h.drain()), "app/sleep").cloned().unwrap();
    h.accept_sleep(&sleep, "resume-1");
    assert_eq!(h.c.state(), &ConnectionState::Dormant);

    h.c.notify_resource_changed(r, h.now).unwrap();
    assert_eq!(h.drain(), vec![Event::Connect, Event::StateChanged(ConnectionState::Waking)]);
    let hello = h.link();
    assert_eq!(hello["params"]["wakeReason"], "app");
    h.paired(&hello, true);
    let ev = h.request("resources/subscribe", json!({"name": "order"}));
    assert_eq!(methods(&sends(&ev)), vec!["<response>", "resources/updated"]);
}

#[test]
fn legacy_timers_restore_subscription_and_merge_rules() {
    let mut cfg = config(LifecycleMode::Idle);
    cfg.lifecycle.legacy_timers = true;
    let mut h = Harness::with(cfg);
    let r = h.c.register_resource(resource("cart")).unwrap();
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.request("resources/subscribe", json!({"name": "cart"}));
    assert_eq!(h.wait_sleep(3 * IDLE), None, "旧行为：任何订阅都阻止休眠");
    h.request("resources/unsubscribe", json!({"name": "cart"}));
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    let t = h.now;
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t + IDLE, "旧行为：调用后仍按空闲时长");
    h.accept_sleep(&sleep, "resume-1");
    h.c.notify_resource_changed(r, h.now).unwrap();
    assert!(h.drain().is_empty());
}

fn background_cfg(mode: LifecycleMode) -> ClientConfig {
    let mut cfg = config(mode);
    cfg.lifecycle.sleep_on_background = true;
    cfg
}

#[test]
fn entering_background_sleeps_at_once_ignoring_lease() {
    let mut h = Harness::with(background_cfg(LifecycleMode::Idle));
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.notify("app/lease", json!({"ttlMs": 60_000}));
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    let msgs = sends(&h.drain());
    assert_eq!(methods(&msgs), vec!["app/visibility", "app/sleep"]);
    assert_eq!(msgs[1]["params"]["reason"], "background");

    // Host 拒绝：回到普通规则（租约 + 隐藏空闲时长）
    let t = h.now;
    h.respond(&msgs[1], json!({"accepted": false}));
    let (sleep, at) = h.wait_sleep(3 * IDLE).unwrap();
    assert_eq!(at, t + 60_000);
    assert_eq!(sleep["params"]["reason"], "idle");
}

#[test]
fn background_sleep_waits_for_running_call_then_sleeps_at_once() {
    let mut h = Harness::with(background_cfg(LifecycleMode::OnDemand));
    h.c.register_tool(tool("a")).unwrap();
    h.c.start(h.now);
    h.drain();
    assert!(h.c.connect_now(h.now));
    h.drain();
    let hello = h.link();
    h.paired(&hello, false);
    h.invoke("c1", "a");
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    assert_eq!(methods(&sends(&h.drain())), vec!["app/visibility"], "调用进行中不休眠");
    h.advance(5_000);
    h.notify("app/lease", json!({"ttlMs": 60_000}));
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    assert_eq!(h.c.poll_timeout(), Some(h.now), "调用结束即到期，不等租约");
    let sleep = find(&sends(&h.advance(0)), "app/sleep").cloned().expect("应立即休眠");
    assert_eq!(sleep["params"]["reason"], "background");

    // 回到可见清除标记：普通规则
    let mut h = Harness::with(background_cfg(LifecycleMode::Idle));
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    let hold = h.c.hold(h.now);
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    h.c.set_visibility(Visibility::Visible, true, h.now);
    h.drain();
    h.c.release_hold(hold, h.now);
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + IDLE);
}

#[test]
fn background_in_backoff_goes_dormant_and_switch_is_scoped() {
    let mut h = Harness::with(background_cfg(LifecycleMode::Idle));
    h.c.start(h.now);
    h.drain();
    h.c.handle_disconnected(h.now);
    h.drain();
    assert!(matches!(h.c.state(), ConnectionState::Backoff { .. }));
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    assert_eq!(h.c.state(), &ConnectionState::Dormant);
    assert_eq!(h.c.poll_timeout(), None);

    // 关闭（默认）、persistent、legacy_timers：进入后台不立即休眠
    for (mode, flag, legacy) in
        [(LifecycleMode::Idle, false, false), (LifecycleMode::Persistent, true, false), (LifecycleMode::Idle, true, true)]
    {
        let mut cfg = config(mode);
        cfg.lifecycle.sleep_on_background = flag;
        cfg.lifecycle.legacy_timers = legacy;
        let mut h = Harness::with(cfg);
        h.connect();
        h.c.set_visibility(Visibility::Hidden, false, h.now);
        assert_eq!(methods(&sends(&h.drain())), vec!["app/visibility"], "{mode:?} {flag} {legacy}");
    }

    // 隐藏 → 冻结不是"进入后台"；已隐藏时连上的不触发
    let mut h = Harness::with(background_cfg(LifecycleMode::Idle));
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    h.connect();
    h.c.set_visibility(Visibility::Frozen, false, h.now);
    assert_eq!(methods(&sends(&h.drain())), vec!["app/visibility"]);
}

// ---------------------------------------------------------------------------
// 后台连接只认自适应租约（spec/lifecycle.md 第 13 节 B4「后台连接」）
// ---------------------------------------------------------------------------

/// 隐藏中被 OS 激活唤醒（`on-demand` + `sleepOnBackground`），握手完成，处理一次调用，返回调用完成时刻。
fn background_woken(cfg: ClientConfig) -> (Harness, Millis) {
    let mut h = Harness::with(cfg);
    h.c.register_tool(tool("a")).unwrap();
    h.c.set_visibility(Visibility::Hidden, false, h.now);
    h.c.start(h.now);
    h.drain();
    assert!(h.c.handle_wake("app-mcp-wake:tok-bg", h.now));
    assert!(h.drain().contains(&Event::Connect));
    let hello = h.link();
    assert_eq!(hello["params"]["wakeReason"], "os-activation");
    h.paired(&hello, false);
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.drain();
    let t = h.now;
    (h, t)
}

#[test]
fn background_connection_ignores_default_lease_and_sleeps_after_merge_window() {
    // 设备实测（Android 后台唤醒）：默认值租约 60 s 让 App 在线到租约收回（约 32 s）。现在只留合并窗口。
    let (mut h, t) = background_woken(background_cfg(LifecycleMode::OnDemand));
    h.notify("app/lease", json!({"ttlMs": 60_000}));
    let (sleep, at) = h.wait_sleep(3 * IDLE).expect("应休眠");
    assert_eq!(at, t + MERGE, "默认值租约不延长后台连接");
    assert_eq!(sleep["params"]["reason"], "grace", "不是进入后台（B4 立即休眠），原因按模式");
}

#[test]
fn background_connection_honours_adaptive_lease() {
    let (mut h, t) = background_woken(background_cfg(LifecycleMode::OnDemand));
    h.notify("app/lease", json!({"ttlMs": 60_000}));
    h.notify("app/lease", json!({"ttlMs": 8_000, "adaptive": true}));
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + 8_000, "自适应租约是 Hub 的预测，照常生效");

    // ttlMs: 0 两种都取消，随后补发的自适应剩余照常生效
    let (mut h, t) = background_woken(background_cfg(LifecycleMode::OnDemand));
    h.notify("app/lease", json!({"ttlMs": 20_000, "adaptive": true}));
    h.notify("app/lease", json!({"ttlMs": 0}));
    h.notify("app/lease", json!({"ttlMs": 5_000, "adaptive": true}));
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + 5_000);
}

#[test]
fn background_connection_becoming_visible_honours_earlier_default_lease() {
    let (mut h, t) = background_woken(background_cfg(LifecycleMode::OnDemand));
    h.notify("app/lease", json!({"ttlMs": 60_000}));
    h.advance(1_000);
    h.c.set_visibility(Visibility::Visible, true, h.now);
    h.drain();
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + 60_000, "回到可见：普通规则，之前记下的默认值租约生效");
}

#[test]
fn background_connection_switch_is_scoped() {
    // 未开启 sleepOnBackground、legacy_timers、握手时可见：默认值租约照常生效
    let mut off = config(LifecycleMode::OnDemand);
    off.lifecycle.sleep_on_background = false;
    let mut legacy = background_cfg(LifecycleMode::OnDemand);
    legacy.lifecycle.legacy_timers = true;
    // legacy：租约到期后再计 grace（10 s）
    for (name, cfg, after_lease) in [("off", off, 0), ("legacy", legacy, 10_000)] {
        let (mut h, t) = background_woken(cfg);
        h.notify("app/lease", json!({"ttlMs": 60_000}));
        assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + 60_000 + after_lease, "{name}");
    }
    let mut h = Harness::with(background_cfg(LifecycleMode::Idle));
    h.c.register_tool(tool("a")).unwrap();
    h.connect();
    h.invoke("c1", "a");
    h.c.complete_call("c1", Ok(CallOutput::default()), h.now).unwrap();
    h.notify("app/lease", json!({"ttlMs": 60_000}));
    let t = h.now;
    assert_eq!(h.wait_sleep(3 * IDLE).unwrap().1, t + 60_000, "握手时可见");
}
