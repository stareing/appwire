//! Client 状态机的行为测试（spec/protocol.md 第 5 节），全部用确定性时间驱动。

#[path = "client/support.rs"]
mod support;
#[path = "client/calls.rs"]
mod calls;
#[path = "client/dedup.rs"]
mod dedup;
#[path = "client/diagnostics.rs"]
mod diagnostics;
#[path = "client/events.rs"]
mod events;
#[path = "client/handshake.rs"]
mod handshake;
#[path = "client/heartbeat.rs"]
mod heartbeat;
#[path = "client/navigate.rs"]
mod navigate;
#[path = "client/registration.rs"]
mod registration;
#[path = "client/resources.rs"]
mod resources;
