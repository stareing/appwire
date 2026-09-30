//! app-mcp-hostw：与 app-mcp-host 相同，但在 Windows 上是 GUI 子系统程序——启动时不创建控制台窗口。
//! 供登录启动项（`service install`）与 `service start` 运行 `serve` 使用；日志写 `<home>/logs/`。
//! 其他平台上与 app-mcp-host 完全相同（服务文件直接使用 app-mcp-host）。
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    app_mcp_host::main_entry()
}
