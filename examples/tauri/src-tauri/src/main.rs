//! app-mcp Tauri 示例：页面工具（counter.*，见 ../src/main.ts）与 Rust 工具（window.title）合并为同一个 App。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tauri_plugin_app_mcp::{
    AppMcpExt, CallHandle, ErrorKind, NativeConfig, Risk, ToolHandler, ToolSpec,
};

/// Rust 侧工具：读取主窗口标题。
struct WindowTitle(AppHandle);

impl ToolHandler for WindowTitle {
    fn invoke(&self, call: CallHandle) {
        match self.0.get_webview_window("main").map(|w| w.title()) {
            Some(Ok(title)) => {
                let _ = call.complete(
                    Some(&serde_json::json!({ "title": title }).to_string()),
                    vec![],
                );
            }
            Some(Err(e)) => {
                let _ = call.fail(ErrorKind::HandlerError, &format!("读取标题失败：{e}"));
            }
            None => {
                let _ = call.fail(ErrorKind::HandlerError, "主窗口不存在");
            }
        }
    }
}

fn main() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_app_mcp::init(NativeConfig::new(
            "tauri-counter",
            "Tauri 计数器",
        )))
        .setup(|app| {
            if let Some(app_mcp) = app.app_mcp() {
                let mut spec = ToolSpec::new("window.title", "读取主窗口标题");
                spec.risk = Risk::Read;
                app_mcp
                    .client()
                    .register_tool(spec, Arc::new(WindowTitle(app.handle().clone())))?;
            }
            Ok(())
        })
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("启动失败：{e}");
        std::process::exit(1);
    }
}
