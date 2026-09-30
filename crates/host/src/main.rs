//! app-mcp-host 可执行程序。实现见 `lib.rs`（与 Windows 无窗口版 `app-mcp-hostw` 共用）。
//!
//! stdio 模式下 stdout 专用于 MCP 协议，所有日志写 stderr。

fn main() -> std::process::ExitCode {
    app_mcp_host::main_entry()
}
