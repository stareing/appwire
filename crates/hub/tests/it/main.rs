//! app-mcp-hub 集成测试：合并为一个测试程序（各文件为模块），避免每个文件各自链接整个 hub。
//!
//! 只跑某个模块：`cargo test -p app-mcp-hub --test it -- lifecycle::`。
//! @why `naming.rs`（统计本进程 fd / 线程数）与 `naming_pipe.rs`（修改进程环境变量）依赖"本程序只有这些测试"，仍为独立程序。

mod support {
    pub mod fake_app;
    #[cfg(feature = "mcp-server")]
    pub mod mcp_http;
}

mod agent_control;
#[cfg(feature = "mcp-server")]
mod agents;
mod background;
mod call_meta;
mod call_objects;
mod diagnostics;
mod events;
mod hosted;
mod hub_api;
mod intents;
mod ipc;
mod lifecycle;
mod listen;
mod locks;
#[cfg(feature = "mcp-server")]
mod mcp_ipc;
mod mux;
mod navigation;
mod policy;
mod power;
mod progress;
mod result_cache;
mod safety;
mod schema_evolution;
mod search;
mod transport;
mod undo;
