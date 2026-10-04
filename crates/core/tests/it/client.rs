//! Client 状态机的行为测试（spec/protocol.md 第 5 节），全部用确定性时间驱动。

mod support;
mod calls;
mod dedup;
mod deprecation;
mod diagnostics;
mod events;
mod handshake;
mod heartbeat;
mod navigate;
mod registration;
mod resources;
mod undo;
