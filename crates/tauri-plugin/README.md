# tauri-plugin-app-mcp

app-mcp 的 Tauri v2 插件。Rust 侧与 WebView 页面的工具合并为同一个 App：

```
WebView 页面（@app-mcp/web，宿主 IPC 桥接）
   │  invoke('plugin:app-mcp|op') ↓      ↑ Webview::eval → window.__APP_MCP_TAURI_DISPATCH__
Tauri 核心进程：插件 + app-mcp-native（每个 App 一个 NativeClient）
   │  命名管道 / Unix 域套接字（默认端点解析）
Host
```

- **Rust 侧**：插件在 setup 时创建 `NativeClient`（`client_kind = hybrid`），默认端点同其他原生 SDK
  （`APP_MCP_ENDPOINT` → 登记文件 `~/.app-mcp/run/endpoints.json` → 平台默认 IPC）。托盘、文件、窗口等原生能力
  直接在 `app.app_mcp()?.client()` 上注册。
- **页面侧**：插件给每个 WebView 主框架注入 `js/bridge.js`，暴露与 Electron preload 同形的 `window.appMcpBridge`
  （协议即 `@app-mcp/electron` 的 op/event，类型定义在 `packages/web/src/electron-bridge.ts`）。页面**照常用**
  `@app-mcp/web` 的 `createAppMcp` / `@app-mcp/react`，SDK 自动改走桥接，不加载 WASM、不连接 Host。
  需要插件缺失时直接报错的，用 `@app-mcp/tauri` 的 `createTauriAppMcp`。
- **作用域**：每个 WebView（按 label）一个 scope `webview-<label>`；`hello`（页面加载）、`reset`、页面开始导航 / 刷新、
  窗口销毁、事件无法送达时整体注销，进行中的调用以 `APP_DISCONNECTED` 失败，页面持有的 `hold()` 一并释放。

## 接入

```toml
# src-tauri/Cargo.toml
[dependencies]
tauri-plugin-app-mcp = { path = "…/crates/tauri-plugin" }
```

```rust
use std::sync::Arc;
use tauri_plugin_app_mcp::{AppMcpExt, CallHandle, NativeConfig, Risk, ToolHandler, ToolSpec};

struct Quit(tauri::AppHandle);
impl ToolHandler for Quit {
    fn invoke(&self, call: CallHandle) {
        let _ = call.complete(None, vec![]);
        self.0.exit(0);
    }
}

tauri::Builder::default()
    .plugin(tauri_plugin_app_mcp::init(NativeConfig::new("shop", "示例商城")))
    .setup(|app| {
        if let Some(app_mcp) = app.app_mcp() {
            app_mcp.client().register_tool(ToolSpec::new("app.quit", "退出应用"), Arc::new(Quit(app.handle().clone())))?;
        }
        Ok(())
    })
    .run(tauri::generate_context!())?;
```

capability（`src-tauri/capabilities/*.json`）加上 `"app-mcp:default"`（允许页面调用插件命令 `op`）：

```json
{ "identifier": "default", "windows": ["main"], "permissions": ["core:default", "app-mcp:default"] }
```

页面：

```ts
import { createAppMcp } from '@app-mcp/web'
const appMcp = createAppMcp({ appId: 'shop', appName: '示例商城' }) // 身份由 Rust 侧决定，这里的选项被忽略
appMcp.tool('cart.clear', { description: '清空购物车', handler: () => cart.clear() })
```

完整示例见 `examples/tauri`（页面工具 `counter.*` + Rust 工具 `window.title`）。

### Builder 选项

| 方法 | 默认 | 说明 |
|---|---|---|
| `listener(Arc<dyn ClientListener>)` | 无 | App 自己的状态 / 配对 token / 日志 / idle-exit 监听（插件先把状态转发给页面） |
| `accept_webview(Fn(&str) -> bool)` | 全部 | 只接受这些 WebView label 的登记，其余返回 `FORBIDDEN` |
| `inject_bridge(bool)` | `true` | 注入 `BRIDGE_SCRIPT`；关闭后可自行注入（如 iframe） |
| `track_visibility(bool)` | `true` | 窗口状态 → `set_visibility` |
| `quit_on_idle_exit(bool)` | `true` | 休眠且 `residency` 允许退出时 `AppHandle::exit(0)` |
| `wake_from_args(bool)` | `true` | 启动参数交给 `handle_wake`（冷启动唤醒） |
| `auto_start(bool)` | `true` | 初始化后 `start()` |

## 生命周期（spec/lifecycle.md）

| Tauri 事件 | SDK |
|---|---|
| 窗口创建、`Focused`、`Resized`（含最小化 / 还原）、`Destroyed` | 任一窗口可见且未最小化 → `visible`（其中有焦点 → focused），否则 `hidden`；去重后 `set_visibility` |
| 移动端 `WindowEvent::Suspended` / `Resumed` | `hidden` / `visible + focused` |
| 启动参数 | `handle_wake`（`app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=`） |
| `RunEvent::Opened { urls }`（macOS / iOS / Android 的 URL 打开） | `handle_wake` |
| `ClientListener::on_idle_exit` | `AppHandle::exit(0)`（可关闭） |
| `RunEvent::Exit` | 注销全部页面登记，`stop()`（停止前冲刷结果） |
| 页面 `wake()` / `sleep()` / `connectNow()` / `hold()` | 转给 `NativeClient`；hold 按 WebView 记录 |

Tauri 没有窗口显示 / 隐藏事件：托盘类 App 调用 `window.hide()` / `show()` 后调用一次 `app_mcp.refresh_visibility()`。

**单实例 / deep link**（Windows、Linux 上 URL 与唤醒参数由新进程的 argv 带入）：

```rust
.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
    if let Some(app_mcp) = app.app_mcp() { app_mcp.handle_wake_args(argv); }
}))
// tauri-plugin-deep-link（macOS / 移动端已由 RunEvent::Opened 处理，这里可选），在 setup 中：
let handle = app.handle().clone();
app.deep_link().on_open_url(move |event| {
    if let Some(app_mcp) = handle.app_mcp() { app_mcp.handle_wake_args(event.urls().iter().map(|u| u.as_str())); }
});
```

唤醒描述用 `uri`（`config.lifecycle.wake = Some(WakeDescriptor { kind: WakeKind::Uri, target: Some("shop://".into()), background: false })`），
scheme 由 `tauri-plugin-deep-link` 注册。

**策略与 4e 开关**：插件原样使用传入的 `NativeConfig`，不按平台改默认值——生命周期缺省 `persistent`（核心默认）。
`idle` / `on-demand` 需要唤醒途径（上面的单实例 + deep link + 唤醒描述），否则休眠后 Host 只能按清单 `launch` 冷启动新实例；
确认接好后自行设置。其余开关都在 `NativeConfig` 上：`heartbeat`（`HeartbeatMode::Auto` 缺省 / `Always` / `Off`）、
`lifecycle.host_absent_retries`（3，0 = 一直重连）、`merge_window_ms`（2000）、`sleep_on_background`（`false`；移动端建议 `true`）、
`legacy_timers`（`false`），见 spec/lifecycle.md 第 3、11、13 节。

**实时资源**：页面声明 `realtime: true` 的资源（`@app-mcp/web` 的 `resource(name, { realtime: true, ... })`）经桥接以
`ResourceOptions { realtime: true }` 登记；Rust 侧用 `client().register_resource_with(spec, ResourceOptions { realtime: true }, reader)`。
缺省 `false`：订阅不阻止休眠（第 13 节 B3）。

## 构建与测试

本 crate 是**独立 workspace**（不在根 `Cargo.toml` 中）：Tauri 在 Linux 上依赖 webkit2gtk-4.1，放进根 workspace 会让
没有该系统库的环境 `cargo test --workspace` 失败。编译产物仍放仓库根 `target/`（`.cargo/config.toml`）。

```bash
cd crates/tauri-plugin
cargo test          # Linux 需要 webkit2gtk-4.1 / libsoup-3.0 开发包（pkg-config 可找到）
cargo clippy --all-targets
```

测试：桥接逻辑经真实 `NativeClient` + 嵌入式 Hub（本地 IPC）验证往返调用、错误类别、取消、scope、资源读取、按窗口注销、
送达失败注销；插件经 Tauri `MockRuntime` 的真实 IPC 命令（含 ACL）验证登记、可见性上报与退出。
`packages/tauri` 的 vitest 在隔离的 `window` 上执行同一份 `js/bridge.js`，与 `@app-mcp/web` 对接。
