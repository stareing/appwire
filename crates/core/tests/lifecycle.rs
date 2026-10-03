//! 生命周期行为测试（spec/lifecycle.md 第 2、3、6、7 节），全部用确定性时间驱动。

#[path = "lifecycle/support.rs"]
mod support;
#[path = "lifecycle/background.rs"]
mod background;
#[path = "lifecycle/events.rs"]
mod events;
#[path = "lifecycle/idle.rs"]
mod idle;
#[path = "lifecycle/modes.rs"]
mod modes;
#[path = "lifecycle/sleep.rs"]
mod sleep;
#[path = "lifecycle/wake.rs"]
mod wake;
