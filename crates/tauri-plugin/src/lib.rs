//! tauri-plugin-app-mcp：Tauri v2 插件。
//!
//! - **Rust 侧**：每个 App 一个 [`NativeClient`]（`app-mcp-native`，在 Tauri 核心进程中），默认经本地 IPC
//!   （Unix 域套接字 / Windows 命名管道，端点解析见 [`NativeConfig::host_url`]）连接 Host；托盘、文件、
//!   窗口等原生能力直接在 [`AppMcp::client`] 上注册为工具。
//! - **页面侧**：插件给每个 WebView 注入初始化脚本（`js/bridge.js`），暴露与 Electron preload 同形的宿主 IPC
//!   桥接对象 `window.appMcpBridge`。页面照常用 `@app-mcp/web` 的 `createAppMcp`（或 `@app-mcp/react`），
//!   SDK 检测到桥接后经 Tauri `invoke` 把工具登记到 Rust 侧客户端，不加载 WASM、不连接 Host。
//!   页面工具与 Rust 工具合并为同一个 App 的工具集；每个 WebView 一个 scope，页面刷新 / 导航、窗口关闭时整体注销。
//! - **生命周期**（spec/lifecycle.md）：窗口显示 / 最小化 / 焦点 → `set_visibility`；冷启动参数、macOS / iOS /
//!   Android 的 `RunEvent::Opened` → `handle_wake`；休眠且驻留策略允许退出时 `AppHandle::exit(0)`；
//!   `RunEvent::Exit` 时停止客户端（先冲刷结果）。单实例 / deep link 见 [`AppMcp::handle_wake_args`]。
//!
//! ```ignore
//! use std::sync::Arc;
//! use tauri_plugin_app_mcp::{AppMcpExt, CallHandle, NativeConfig, ToolHandler, ToolSpec};
//!
//! struct Quit(tauri::AppHandle);
//! impl ToolHandler for Quit {
//!     fn invoke(&self, call: CallHandle) {
//!         let _ = call.complete(None, vec![]);
//!         self.0.exit(0);
//!     }
//! }
//!
//! tauri::Builder::default()
//!     .plugin(tauri_plugin_app_mcp::init(NativeConfig::new("shop", "示例商城")))
//!     .setup(|app| {
//!         if let Some(app_mcp) = app.app_mcp() {
//!             app_mcp.client().register_tool(ToolSpec::new("app.quit", "退出应用"), Arc::new(Quit(app.handle().clone())))?;
//!         }
//!         Ok(())
//!     })
//!     .run(tauri::generate_context!())
//!     .expect("启动失败");
//! ```
//!
//! App 的 capability 需要包含 `"app-mcp:default"`（允许页面调用插件命令 `op`）。

mod bridge;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tauri::plugin::{PluginApi, TauriPlugin};
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, RunEvent, Runtime, Webview, WindowEvent};

pub use app_mcp_native::{
    Activation, AppOverview, Audience, CallHandle, CallResult, CancelListener, CancelReason,
    ClientKind, ClientListener, ContentAnnotations, ErrorKind, HeartbeatMode, HoldHandle,
    LifecycleMode, LifecyclePolicy, LogLevel, NativeClient, NativeConfig, NativeError, ReadHandle,
    Residency, ResourceHandle, ResourceOptions, ResourceReader, ResourceSpec, ResultStatus, Risk,
    ScopeHandle, SleepReason, StateInfo, StateStatus, ToolAnnotations, ToolHandle, ToolHandler,
    ToolOptions, ToolSpec, Visibility, WakeDescriptor, WakeKind, WakeReason,
};
pub use bridge::BRIDGE_VERSION;

use bridge::{AcceptFn, Bridge, PageSink, Sessions};

/// 插件名（命令为 `plugin:app-mcp|op`，权限为 `app-mcp:default`）。
pub const PLUGIN_NAME: &str = "app-mcp";

/// 注入每个 WebView 主框架的初始化脚本：暴露 `window.appMcpBridge`。
/// [`Builder::inject_bridge`] 为 `false` 时，可以自行把它放进页面（如 iframe 或自定义注入时机）。
pub const BRIDGE_SCRIPT: &str = include_str!("../js/bridge.js");

/// Rust → 页面的事件经该全局函数送达（由 [`BRIDGE_SCRIPT`] 定义）。
const DISPATCH_FN: &str = "window.__APP_MCP_TAURI_DISPATCH__";

/// 以默认选项创建插件。
pub fn init<R: Runtime>(config: NativeConfig) -> TauriPlugin<R> {
    Builder::new(config).build()
}

/// 插件构建器。
pub struct Builder {
    config: NativeConfig,
    listener: Option<Arc<dyn ClientListener>>,
    accept: Option<Arc<AcceptFn>>,
    inject_bridge: bool,
    track_visibility: bool,
    quit_on_idle_exit: bool,
    wake_from_args: bool,
    auto_start: bool,
}

impl Builder {
    /// `config.client_kind` 固定为 [`ClientKind::Hybrid`]。
    pub fn new(config: NativeConfig) -> Self {
        Self {
            config,
            listener: None,
            accept: None,
            inject_bridge: true,
            track_visibility: true,
            quit_on_idle_exit: true,
            wake_from_args: true,
            auto_start: true,
        }
    }

    /// App 自己的客户端事件监听（状态、配对 token 持久化、日志、idle-exit）。插件把状态转发给页面后再调用它。
    pub fn listener(mut self, listener: Arc<dyn ClientListener>) -> Self {
        self.listener = Some(listener);
        self
    }

    /// 只接受这些 WebView（按 label 判断）的登记。缺省接受全部。
    pub fn accept_webview(mut self, accept: impl Fn(&str) -> bool + Send + Sync + 'static) -> Self {
        self.accept = Some(Arc::new(accept));
        self
    }

    /// 是否给每个 WebView 注入 [`BRIDGE_SCRIPT`]，默认 `true`。
    pub fn inject_bridge(mut self, inject: bool) -> Self {
        self.inject_bridge = inject;
        self
    }

    /// 是否按窗口显示 / 最小化 / 焦点上报可见性（`set_visibility`），默认 `true`。
    pub fn track_visibility(mut self, track: bool) -> Self {
        self.track_visibility = track;
        self
    }

    /// 休眠且 `residency` 允许退出时调用 `AppHandle::exit(0)`，默认 `true`。为 `false` 时由 App 在
    /// [`ClientListener::on_idle_exit`] 中自行处理。
    pub fn quit_on_idle_exit(mut self, quit: bool) -> Self {
        self.quit_on_idle_exit = quit;
        self
    }

    /// 启动时把进程参数交给 `handle_wake`（Host 冷启动唤醒：`app-mcp-wake:<token>` / `<scheme>://app-mcp/wake?token=`），默认 `true`。
    pub fn wake_from_args(mut self, wake: bool) -> Self {
        self.wake_from_args = wake;
        self
    }

    /// 插件初始化后立即 `start()`（`on-demand` 模式下不会立刻连接），默认 `true`。
    pub fn auto_start(mut self, start: bool) -> Self {
        self.auto_start = start;
        self
    }

    pub fn build<R: Runtime>(self) -> TauriPlugin<R> {
        let inject = self.inject_bridge;
        let mut builder = tauri::plugin::Builder::<R>::new(PLUGIN_NAME)
            .invoke_handler(tauri::generate_handler![op]);
        if inject {
            builder = builder.js_init_script(BRIDGE_SCRIPT);
        }
        builder
            .setup(move |app, api| self.setup(app, api))
            .on_page_load(|webview, payload| {
                // 页面开始导航（含刷新）：旧页面的登记作废。
                if payload.event() == PageLoadEvent::Started
                    && let Some(app_mcp) = webview.app_mcp()
                {
                    app_mcp.bridge.sessions().end(webview.label());
                }
            })
            .on_window_ready(|window| {
                if let Some(app_mcp) = window.app_mcp() {
                    app_mcp.refresh_visibility();
                }
            })
            .on_event(|app, event| {
                if let Some(app_mcp) = app.app_mcp() {
                    app_mcp.on_run_event(event);
                }
            })
            .build()
    }

    fn setup<R: Runtime>(
        self,
        app: &AppHandle<R>,
        _api: PluginApi<R, ()>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut config = self.config;
        config.client_kind = ClientKind::Hybrid;
        let sessions = Arc::new(Sessions::new());
        let idle_exit: Option<Box<dyn Fn() + Send + Sync>> = if self.quit_on_idle_exit {
            let handle = app.clone();
            Some(Box::new(move || handle.exit(0)))
        } else {
            None
        };
        let listener = PluginListener {
            sessions: sessions.clone(),
            user: self.listener,
            idle_exit,
        };
        let client = NativeClient::new(config, Some(Arc::new(listener)))?;
        let app_mcp = AppMcp {
            bridge: Bridge::new(client, sessions, self.accept),
            app: app.clone(),
            track_visibility: self.track_visibility,
            last_visibility: Mutex::new(None),
        };
        if self.wake_from_args {
            app_mcp.handle_wake_args(std::env::args().skip(1));
        }
        if self.auto_start {
            app_mcp.client().start();
        }
        app.manage(app_mcp);
        Ok(())
    }
}

/// 插件状态（`app.app_mcp()`）。
pub struct AppMcp<R: Runtime> {
    bridge: Bridge,
    app: AppHandle<R>,
    track_visibility: bool,
    last_visibility: Mutex<Option<(Visibility, bool)>>,
}

impl<R: Runtime> AppMcp<R> {
    /// 原生客户端：注册 Rust 侧工具 / 资源、生命周期操作（`wake` / `sleep` / `hold` / `connect_now`）。
    pub fn client(&self) -> &NativeClient {
        self.bridge.client()
    }

    /// 当前有登记的 WebView 数量。
    pub fn page_count(&self) -> usize {
        self.bridge.sessions().count()
    }

    /// 把激活参数交给 `handle_wake`，逐个识别，识别到唤醒即返回 `true`。
    ///
    /// 用于 `tauri-plugin-single-instance`（第二个实例的 argv）与 `tauri-plugin-deep-link`（`on_open_url` 的 URL）：
    ///
    /// ```ignore
    /// .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
    ///     if let Some(app_mcp) = app.app_mcp() { app_mcp.handle_wake_args(argv); }
    /// }))
    /// ```
    pub fn handle_wake_args<I, S>(&self, args: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        args.into_iter()
            .any(|arg| self.client().handle_wake(arg.as_ref()))
    }

    /// 按全部窗口的当前状态重新上报可见性（任一窗口显示且未最小化为可见；其中有焦点窗口为聚焦）。
    ///
    /// 插件在窗口创建、焦点变化、尺寸变化（含最小化 / 还原）、销毁时自动调用；Tauri 没有显示 / 隐藏事件，
    /// App 调用 `window.hide()` / `window.show()`（如托盘应用）之后应调用一次。
    pub fn refresh_visibility(&self) {
        if !self.track_visibility {
            return;
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            let mut visible = false;
            let mut focused = false;
            for window in self.app.webview_windows().values() {
                let shown =
                    window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false);
                visible |= shown;
                focused |= shown && window.is_focused().unwrap_or(false);
            }
            let visibility = if visible {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
            self.report_visibility(visibility, focused);
        }
    }

    fn report_visibility(&self, visibility: Visibility, focused: bool) {
        let mut last = self
            .last_visibility
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if *last == Some((visibility, focused)) {
            return;
        }
        *last = Some((visibility, focused));
        drop(last);
        self.client().set_visibility(visibility, focused);
    }

    fn on_run_event(&self, event: &RunEvent) {
        match event {
            RunEvent::WindowEvent { label, event, .. } => match event {
                WindowEvent::Destroyed => {
                    self.bridge.sessions().end_window(label);
                    self.refresh_visibility();
                }
                WindowEvent::Focused(_) | WindowEvent::Resized(_) => self.refresh_visibility(),
                #[cfg(any(target_os = "android", target_os = "ios"))]
                WindowEvent::Suspended => {
                    if self.track_visibility {
                        self.report_visibility(Visibility::Hidden, false);
                    }
                }
                #[cfg(any(target_os = "android", target_os = "ios"))]
                WindowEvent::Resumed => {
                    if self.track_visibility {
                        self.report_visibility(Visibility::Visible, true);
                    }
                }
                _ => {}
            },
            #[cfg(any(target_os = "macos", target_os = "ios", target_os = "android"))]
            RunEvent::Opened { urls, .. } => {
                self.handle_wake_args(urls.iter().map(|url| url.as_str()));
            }
            RunEvent::Exit => {
                // 先让页面工具的进行中调用失败，再停止（停止前冲刷结果）。
                self.bridge.sessions().end_all();
                self.client().stop();
            }
            _ => {}
        }
    }
}

/// 从 `App` / `AppHandle` / `Window` / `Webview` 取得插件状态；插件未注册时为 `None`。
pub trait AppMcpExt<R: Runtime> {
    fn app_mcp(&self) -> Option<&AppMcp<R>>;
}

impl<R: Runtime, T: Manager<R>> AppMcpExt<R> for T {
    fn app_mcp(&self) -> Option<&AppMcp<R>> {
        self.try_state::<AppMcp<R>>().map(|state| state.inner())
    }
}

/// 页面 → Rust：一条桥接操作（`RendererOp`），返回 `OpReply`。
#[tauri::command]
async fn op<R: Runtime>(webview: Webview<R>, op: Value) -> Value {
    let Some(app_mcp) = webview.app_mcp() else {
        return serde_json::json!({ "ok": false, "code": "DISPOSED", "message": "app-mcp 插件未初始化" });
    };
    let label = webview.label().to_owned();
    let window = webview.window().label().to_owned();
    app_mcp.bridge.handle(
        &label,
        &window,
        || Arc::new(WebviewSink(webview.clone())),
        op,
    )
}

/// 经 `Webview::eval` 调用页面上的 [`DISPATCH_FN`]：只送达这一个 WebView，按发送顺序执行。
struct WebviewSink<R: Runtime>(Webview<R>);

impl<R: Runtime> PageSink for WebviewSink<R> {
    fn deliver(&self, event: &Value) -> bool {
        self.0
            .eval(format!("{DISPATCH_FN}&&{DISPATCH_FN}({event})"))
            .is_ok()
    }
}

struct PluginListener {
    sessions: Arc<Sessions>,
    user: Option<Arc<dyn ClientListener>>,
    idle_exit: Option<Box<dyn Fn() + Send + Sync>>,
}

impl ClientListener for PluginListener {
    fn on_state_changed(&self, state: StateInfo) {
        self.sessions.broadcast_state(&state);
        if let Some(user) = &self.user {
            user.on_state_changed(state);
        }
    }

    fn on_paired(&self, token: String) {
        if let Some(user) = &self.user {
            user.on_paired(token);
        }
    }

    fn on_log(&self, level: LogLevel, message: String) {
        if let Some(user) = &self.user {
            user.on_log(level, message);
        }
    }

    fn on_idle_exit(&self) {
        if let Some(user) = &self.user {
            user.on_idle_exit();
        }
        if let Some(exit) = &self.idle_exit {
            exit();
        }
    }
}
