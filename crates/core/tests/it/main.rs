//! app-mcp-core 集成测试：合并为一个测试程序（各文件为模块）。全部为确定性时间驱动的纯状态机测试，没有进程级共享状态。
//!
//! 只跑某个模块：`cargo test -p app-mcp-core --test it -- power::`。

mod client;
mod hot_path;
mod inbound;
mod lifecycle;
mod power;
