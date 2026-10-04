//! app-mcp-native 集成测试：合并为一个测试程序（各文件为模块），避免每个文件各自链接一次。
//!
//! 只跑某个模块：`cargo test -p app-mcp-native --test it -- lifecycle::`。
//! @why `env.rs` 修改进程环境变量（`APP_MCP_LAUNCH_TOKEN`），依赖"本程序只有这一个测试"，仍为独立程序。

#[path = "../common/mod.rs"]
mod common;
#[path = "../../src/test_support.rs"]
mod test_support;
#[cfg(all(target_os = "linux", not(target_env = "ohos"), feature = "dbus"))]
#[path = "../../src/test_bus.rs"]
#[allow(dead_code)]
mod test_bus;

mod cache;
mod channel;
mod conformance;
mod deprecation;
mod events;
mod fake_host;
mod lifecycle;
mod names;
mod runtime;
