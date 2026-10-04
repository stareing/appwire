//! app-mcp-host 集成测试：合并为一个测试程序（各文件为模块），避免每个文件各自链接 hub / native / host。
//!
//! 只跑某个模块：`cargo test -p app-mcp-host --test it -- serve::`。
//! @why 仍为独立程序：`lifecycle_bench.rs`（`#[global_allocator]` 计数、统计本进程线程 / 内存）、
//! `service_systemd*.rs`（真实安装同名 systemd 用户单元，同一程序内并行会互相覆盖）。

mod support {
    pub mod mcp_http;
}

mod doctor;
mod doctor_naming;
mod doctor_naming_pipes;
mod host_e2e;
mod on_demand;
mod serve;
mod setup;
mod upstream_http;
mod validate;
