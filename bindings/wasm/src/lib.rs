//! app-mcp-core 的浏览器 WASM 绑定。
//!
//! 只包装 sans-IO 核心：WebSocket、定时器与 handler 调用都由 JS 驱动层（`@app-mcp/web`）实现。
//!
//! 约定：
//! - 时间参数为 `f64` 毫秒（JS 侧用 `performance.now()`），内部截断为 [`Millis`]。
//! - 工具、资源、scope、读取句柄在 JS 中是普通数字。
//! - 配置、定义、结果用普通 JS 对象传入（serde-wasm-bindgen 转换），字段名为 camelCase。
//! - 事件以带 `type` 字段的对象返回（见 [`JsEvent`]），没有事件时返回 `undefined`。
//! - [`CoreError`] 与参数转换失败都以 JS `Error` 抛出。

mod convert;

pub use convert::{
    JsCallOutcome, JsConfig, JsEvent, JsLifecycle, JsResourceDef, JsState, JsToolDef, JsToolUpdate,
};

use app_mcp_core::{Client, ConnectionErrorCode, ConnectionIssue, HoldId, Millis, ReadId, ResourceId, ScopeId, ToolId, Visibility};
use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::prelude::*;

/// 把 JS 传入的时间转换为核心的单调毫秒数（负数与 NaN 视为 0）。
fn millis(now: f64) -> Millis {
    if now.is_finite() && now > 0.0 { now as Millis } else { 0 }
}

/// JS 数字句柄 → u64。非整数或负数视为无效句柄。
fn handle(id: f64) -> Result<u64, JsError> {
    if id.is_finite() && id >= 0.0 && id.fract() == 0.0 && id <= convert::MAX_SAFE_INTEGER as f64 {
        Ok(id as u64)
    } else {
        Err(JsError::new(&format!("无效的句柄：{id}")))
    }
}

fn from_js<T: DeserializeOwned>(value: JsValue, what: &str) -> Result<T, JsError> {
    serde_wasm_bindgen::from_value(value).map_err(|e| JsError::new(&format!("{what} 格式错误：{e}")))
}

fn to_js<T: Serialize + ?Sized>(value: &T) -> Result<JsValue, JsError> {
    value
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|e| JsError::new(&format!("无法转换为 JS 值：{e}")))
}

fn core_err(e: app_mcp_core::CoreError) -> JsError {
    JsError::new(&e.to_string())
}

/// 浏览器侧的 sans-IO 客户端。方法与 [`app_mcp_core::Client`] 一一对应。
#[wasm_bindgen]
pub struct WasmClient {
    inner: Client,
}

#[wasm_bindgen]
impl WasmClient {
    /// 用配置对象创建客户端，字段见 [`JsConfig`]。
    #[wasm_bindgen(constructor)]
    pub fn new(config: JsValue) -> Result<WasmClient, JsError> {
        #[cfg(feature = "debug")]
        console_error_panic_hook::set_once();
        let config: JsConfig = from_js(config, "配置")?;
        Ok(WasmClient { inner: Client::new(config.into_core()) })
    }

    /// 当前连接状态，形如 `{ status: 'backoff', retryAt }`（`retryAt` 为核心时钟毫秒）。
    pub fn state(&self) -> Result<JsValue, JsError> {
        to_js(&JsState::from_core(self.inner.state()))
    }

    /// 当前已配对的 token。
    pub fn token(&self) -> Option<String> {
        self.inner.token().map(str::to_owned)
    }

    pub fn start(&mut self, now: f64) {
        self.inner.start(millis(now));
    }

    pub fn stop(&mut self, now: f64) {
        self.inner.stop(millis(now));
    }

    // ---- scope ----------------------------------------------------------

    #[wasm_bindgen(js_name = createScope)]
    pub fn create_scope(&mut self, name: &str, parent: Option<f64>) -> Result<f64, JsError> {
        let parent = parent.map(handle).transpose()?.map(ScopeId);
        let id = self.inner.create_scope(name, parent).map_err(core_err)?;
        Ok(id.0 as f64)
    }

    #[wasm_bindgen(js_name = disposeScope)]
    pub fn dispose_scope(&mut self, scope: f64) -> Result<(), JsError> {
        self.inner.dispose_scope(ScopeId(handle(scope)?)).map_err(core_err)
    }

    // ---- 工具 -----------------------------------------------------------

    /// 注册工具，字段见 [`JsToolDef`]。返回工具句柄。
    #[wasm_bindgen(js_name = registerTool)]
    pub fn register_tool(&mut self, def: JsValue) -> Result<f64, JsError> {
        let def: JsToolDef = from_js(def, "工具定义")?;
        let def = def.into_core().map_err(|e| JsError::new(&e))?;
        let id = self.inner.register_tool(def).map_err(core_err)?;
        Ok(id.0 as f64)
    }

    /// 部分更新工具，字段见 [`JsToolUpdate`]；`activation` / `title` 传 `null` 表示清除。
    #[wasm_bindgen(js_name = updateTool)]
    pub fn update_tool(&mut self, tool: f64, update: JsValue) -> Result<(), JsError> {
        let update: JsToolUpdate = from_js(update, "工具更新")?;
        self.inner.update_tool(ToolId(handle(tool)?), update.into_core()).map_err(core_err)
    }

    #[wasm_bindgen(js_name = unregisterTool)]
    pub fn unregister_tool(&mut self, tool: f64) -> Result<(), JsError> {
        self.inner.unregister_tool(ToolId(handle(tool)?)).map_err(core_err)
    }

    // ---- 资源 -----------------------------------------------------------

    /// 注册资源，字段见 [`JsResourceDef`]。返回资源句柄。
    #[wasm_bindgen(js_name = registerResource)]
    pub fn register_resource(&mut self, def: JsValue) -> Result<f64, JsError> {
        let def: JsResourceDef = from_js(def, "资源定义")?;
        let def = def.into_core().map_err(|e| JsError::new(&e))?;
        let id = self.inner.register_resource(def).map_err(core_err)?;
        Ok(id.0 as f64)
    }

    #[wasm_bindgen(js_name = notifyResourceChanged)]
    pub fn notify_resource_changed(&mut self, resource: f64, now: f64) -> Result<(), JsError> {
        self.inner.notify_resource_changed(ResourceId(handle(resource)?), millis(now)).map_err(core_err)
    }

    #[wasm_bindgen(js_name = unregisterResource)]
    pub fn unregister_resource(&mut self, resource: f64) -> Result<(), JsError> {
        self.inner.unregister_resource(ResourceId(handle(resource)?)).map_err(core_err)
    }

    // ---- 可见性 ---------------------------------------------------------

    /// `visibility` 为 `'visible' | 'hidden' | 'frozen'`。
    #[wasm_bindgen(js_name = setVisibility)]
    pub fn set_visibility(&mut self, visibility: &str, focused: bool, now: f64) -> Result<(), JsError> {
        let visibility: Visibility = convert::parse_visibility(visibility).map_err(|e| JsError::new(&e))?;
        self.inner.set_visibility(visibility, focused, millis(now));
        Ok(())
    }

    // ---- 驱动层输入 -----------------------------------------------------

    #[wasm_bindgen(js_name = handleConnected)]
    pub fn handle_connected(&mut self, now: f64) {
        self.inner.handle_connected(millis(now));
    }

    #[wasm_bindgen(js_name = handleDisconnected)]
    pub fn handle_disconnected(&mut self, now: f64) {
        self.inner.handle_disconnected(millis(now));
    }

    #[wasm_bindgen(js_name = handleMessage)]
    pub fn handle_message(&mut self, text: &str, now: f64) {
        self.inner.handle_message(text, millis(now));
    }

    #[wasm_bindgen(js_name = handleTimeout)]
    pub fn handle_timeout(&mut self, now: f64) {
        self.inner.handle_timeout(millis(now));
    }

    /// handler 完成。`outcome` 为 `{ data, stateHints? }` 或 `{ error: { kind, message, details? } }`。
    #[wasm_bindgen(js_name = completeCall)]
    pub fn complete_call(&mut self, call_id: &str, outcome: JsValue, now: f64) -> Result<(), JsError> {
        let outcome: JsCallOutcome = from_js(outcome, "调用结果")?;
        self.inner.complete_call(call_id, outcome.into_call(), millis(now)).map_err(core_err)
    }

    /// 资源读取完成。`outcome` 为 `{ data }` 或 `{ error: { kind, message, details? } }`。
    #[wasm_bindgen(js_name = completeRead)]
    pub fn complete_read(&mut self, read: f64, outcome: JsValue) -> Result<(), JsError> {
        let outcome: JsCallOutcome = from_js(outcome, "读取结果")?;
        self.inner.complete_read(ReadId(handle(read)?), outcome.into_read()).map_err(core_err)
    }

    // ---- 驱动层输出 -----------------------------------------------------

    /// 取出下一个事件（带 `type` 字段的对象）；没有事件时返回 `undefined`。
    #[wasm_bindgen(js_name = pollEvent)]
    pub fn poll_event(&mut self) -> Result<JsValue, JsError> {
        match self.inner.poll_event() {
            Some(ev) => to_js(&JsEvent::from_core(ev)),
            None => Ok(JsValue::UNDEFINED),
        }
    }

    /// 下一次需要调用 `handleTimeout` 的时刻（核心时钟毫秒）；没有待处理定时器时为 `undefined`。
    /// 休眠（`dormant`）时始终为 `undefined`。
    #[wasm_bindgen(js_name = pollTimeout)]
    pub fn poll_timeout(&self) -> Option<f64> {
        self.inner.poll_timeout().map(|t| t as f64)
    }

    // ---- 生命周期（spec/lifecycle.md）------------------------------------

    /// 处理唤醒参数（URL 或 `#app-mcp-wake=<token>` 片段等）。不是本 SDK 的唤醒返回 `false`。
    #[wasm_bindgen(js_name = handleWake)]
    pub fn handle_wake(&mut self, args: &str, now: f64) -> bool {
        self.inner.handle_wake(args, millis(now))
    }

    /// App 主动回连（原因 `app`）。返回是否发起了回连。
    pub fn wake(&mut self, now: f64) -> bool {
        self.inner.wake(millis(now))
    }

    /// 以指定原因回连：`'os-activation' | 'app' | 'visible' | 'cold-start'`。
    #[wasm_bindgen(js_name = wakeWithReason)]
    pub fn wake_with_reason(&mut self, reason: &str, now: f64) -> Result<bool, JsError> {
        let reason = convert::parse_wake_reason(reason).map_err(|e| JsError::new(&e))?;
        Ok(self.inner.wake_with_reason(reason, millis(now)))
    }

    /// `on-demand` 模式下主动连接；尚未 `start` 时等同于 `start`。
    #[wasm_bindgen(js_name = connectNow)]
    pub fn connect_now(&mut self, now: f64) -> bool {
        self.inner.connect_now(millis(now))
    }

    /// App 主动请求休眠（原因 `app`）。返回是否有效果。
    pub fn sleep(&mut self, now: f64) -> bool {
        self.inner.sleep(millis(now))
    }

    /// 以指定原因请求休眠：`'idle' | 'grace' | 'background' | 'app'`（bfcache 前用 `background`）。
    #[wasm_bindgen(js_name = sleepWithReason)]
    pub fn sleep_with_reason(&mut self, reason: &str, now: f64) -> Result<bool, JsError> {
        let reason = convert::parse_sleep_reason(reason).map_err(|e| JsError::new(&e))?;
        Ok(self.inner.sleep_with_reason(reason, millis(now)))
    }

    /// 临时阻止自动休眠，返回持有句柄（数字），用 `releaseHold` 释放。
    pub fn hold(&mut self, now: f64) -> f64 {
        self.inner.hold(millis(now)).0 as f64
    }

    /// 为进行中的调用延长持有（handler 的 `context.hold()`）。调用不存在时抛错。
    #[wasm_bindgen(js_name = holdForCall)]
    pub fn hold_for_call(&mut self, call_id: &str, now: f64) -> Result<f64, JsError> {
        let id = self.inner.hold_for_call(call_id, millis(now)).map_err(core_err)?;
        Ok(id.0 as f64)
    }

    /// 释放持有。已释放或未知的句柄返回 `false`。
    #[wasm_bindgen(js_name = releaseHold)]
    pub fn release_hold(&mut self, hold: f64, now: f64) -> Result<bool, JsError> {
        Ok(self.inner.release_hold(HoldId(handle(hold)?), millis(now)))
    }

    /// 当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）。
    #[wasm_bindgen(js_name = toolsHash)]
    pub fn tools_hash(&self) -> String {
        self.inner.tools_hash()
    }

    /// 上次休眠时 Host 返回、尚未使用的恢复令牌。
    #[wasm_bindgen(js_name = resumeToken)]
    pub fn resume_token(&self) -> Option<String> {
        self.inner.resume_token().map(str::to_owned)
    }

    /// 建立连接失败（带错误码，spec/protocol.md 10.1）：同 `handleDisconnected`，`backoff` 状态带 `reason` / `code`。
    /// 不认识的错误码抛错。
    #[wasm_bindgen(js_name = handleConnectFailed)]
    pub fn handle_connect_failed(&mut self, code: &str, message: &str, now: f64) -> Result<(), JsError> {
        self.inner.handle_connect_failed(connection_issue(code, message)?, millis(now));
        Ok(())
    }

    /// 已建立的连接断开（带错误码 `CONNECTION_CLOSED` / `CONNECTION_LOST`，spec/protocol.md 10.1）：
    /// 同 `handleDisconnected`，`backoff` 状态带 `reason` / `code`。不认识的错误码抛错。
    #[wasm_bindgen(js_name = handleDisconnectedWith)]
    pub fn handle_disconnected_with(&mut self, code: &str, message: &str, now: f64) -> Result<(), JsError> {
        self.inner.handle_disconnected_with(connection_issue(code, message)?, millis(now));
        Ok(())
    }

    /// Host 为当前连接分配的连接 ID（spec/protocol.md 10.3）。
    #[wasm_bindgen(js_name = connectionId)]
    pub fn connection_id(&self) -> Option<String> {
        self.inner.connection_id().map(str::to_owned)
    }

    /// 记录一次连接问题，下次握手成功后以 `app/diagnostic` 上报（spec/protocol.md 10.2）。
    #[wasm_bindgen(js_name = reportIssue)]
    pub fn report_issue(&mut self, code: &str, message: &str) {
        self.inner.report_issue(code, message);
    }
}

/// JS 传入的错误码 + 说明 → [`ConnectionIssue`]。
///
/// @error 不认识的错误码返回 `未知错误码：<code>`。
fn connection_issue(code: &str, message: &str) -> Result<ConnectionIssue, JsError> {
    let code = ConnectionErrorCode::parse(code).ok_or_else(|| JsError::new(&format!("未知错误码：{code}")))?;
    Ok(ConnectionIssue::new(code, message))
}

/// 从 URL / 激活参数中提取唤醒令牌（不改变任何客户端状态），供 JS 侧在读取后从地址栏移除片段。
#[wasm_bindgen(js_name = parseWakeToken)]
pub fn parse_wake_token(args: &str) -> Option<String> {
    app_mcp_core::parse_wake_token(args)
}
