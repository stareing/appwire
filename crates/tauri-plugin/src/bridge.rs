//! 页面桥接：WebView 中的页面经 Tauri IPC 把工具 / 资源登记到 Rust 侧的 [`NativeClient`]。
//!
//! 与 `@app-mcp/electron` 主进程（packages/electron/src/main.ts）是同一个 op/event 协议，消息类型的唯一定义在
//! `@app-mcp/web`（packages/web/src/electron-bridge.ts，`BRIDGE_VERSION = 1`）：
//!
//! - 页面 → Rust：`RendererOp`，返回 `OpReply`（`{ok: true, value?}` / `{ok: false, code?, message}`）；
//! - Rust → 页面：`MainEvent`（`call` / `cancel` / `read` / `state`），经 [`PageSink`] 送达。
//!
//! 每个 WebView（按 label）一个 [`Session`]，登记放在独立的 scope（`webview-<label>`）中；`hello`（页面重新加载）、
//! `reset`（页面卸载）、页面开始导航、窗口销毁或事件无法送达时整体注销，进行中的调用以 `APP_DISCONNECTED` 失败，
//! 页面持有的 hold 一并释放。
//!
//! 连接 ID（spec/protocol.md 10.3）随 `hello` 回复与 `state` 事件的可选字段 `connectionId` 送到页面（协议版本不变，
//! 见 electron-bridge.ts 的 `@compat`）。
//!
//! 导航（第 4c 项，spec/protocol.md 3.4，[`Bridge::enable_page_navigation`]）：页面 `navigation.set {enabled}` 声明本页处理导航
//! （最近一次开启的 WebView 为目标），Host 的 `app/navigate` 以事件 `navigate {navId, page, params?}` 送到该页，页面以
//! `navigate.result {navId, ok, kind?, message?, details?}` 回复（`kind` 为 `USER_ACTION_REQUIRED` 时 `details` 带
//! `{reason?, uri?}`）。转给页面之前先把该页所在窗口带到前台（[`PageSink::raise`]），因此桌面上 App 在后台时导航仍交给页面
//! （原生运行时的平台缺省 `navigate_in_background`：桌面 `true`、Android / iOS `false`）。消息类型定义在 `@app-mcp/web`（packages/web/src/electron-bridge.ts
//! `NavigationOp` / `NavigateEvent`），协议版本不变；页面侧由桥接客户端的 `setNavigationHandler` / `attachBridgeNavigation` 处理。
//!
//! 本模块与 Tauri 无关（只依赖 [`PageSink`]），便于不启动 WebView 测试。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use app_mcp_native::{
    Activation, CallHandle, CallResult, CancelListener, CancelReason, ContentAnnotations,
    ErrorKind, HoldHandle, NativeClient, NativeError, NavigateHandle, NavigationHandler, ReadHandle, ResourceHandle, ResourceOptions,
    ResourceReader, ResourceSpec, ResultStatus, Risk, ScopeHandle, StateInfo, StateStatus,
    ToolAnnotations, ToolHandle, ToolHandler, ToolOptions, ToolSpec, ToolSurface,
};
use serde::Deserialize;
use serde_json::{Value, json};

/// 桥接协议版本，须与 `@app-mcp/web` 的 `BRIDGE_VERSION` 及 `js/bridge.js` 一致。
pub const BRIDGE_VERSION: u32 = 1;

/// 把事件交给页面。返回 `false` 表示页面已不可达（会话随之注销）。
pub(crate) trait PageSink: Send + Sync + 'static {
    fn deliver(&self, event: &Value) -> bool;
    /// 把页面所在窗口带到前台（最小化则还原、隐藏则显示，并聚焦；已可见时不动）：Host 的导航请求送到页面之前调用（spec/protocol.md 3.4）。
    /// 缺省什么也不做（移动端、测试）。
    fn raise(&self) {}
}

/// 判断某个 WebView（按 label）是否允许登记。
pub(crate) type AcceptFn = dyn Fn(&str) -> bool + Send + Sync;
mod messages;
mod session;
mod sessions;

pub(crate) use messages::state_json;
use messages::{lock, op_error, present, reply_err, reply_ok};

// ---------------------------------------------------------------------------
// 消息
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ToolSpecMessage {
    description: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "inputSchema")]
    input_schema: Option<Value>,
    #[serde(default)]
    risk: Option<Risk>,
    #[serde(default)]
    activation: Option<Activation>,
    #[serde(default)]
    enabled: Option<bool>,
    /// 标准 MCP 工具注解；`tool.update` 时缺省表示清除（页面每次发送完整定义）。
    #[serde(default)]
    annotations: Option<ToolAnnotations>,
    /// 结果的 JSON Schema；`tool.update` 时缺省表示清除。
    #[serde(default, rename = "outputSchema")]
    output_schema: Option<Value>,
    /// 对界面的依赖（spec/protocol.md 3.4）；缺省 `app`。
    #[serde(default)]
    surface: ToolSurface,
    /// 所在页面名；`tool.update` 时缺省表示清除。
    #[serde(default)]
    page: Option<String>,
    /// 后台替代：同一 App 中一个 `app` 工具的局部名（spec/protocol.md 3.4）；`tool.update` 时缺省表示清除。
    #[serde(default, rename = "backgroundTool")]
    background_tool: Option<String>,
    /// 本工具同时执行的调用上限（spec/protocol.md 5.3）；缺省 0 = 不单独限制。
    #[serde(default)]
    concurrency: u32,
    /// 互斥组（spec/protocol.md 5.3）；`tool.update` 时缺省表示清除。
    #[serde(default)]
    exclusive: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op")]
enum PageOp {
    #[serde(rename = "hello")]
    Hello,
    #[serde(rename = "reset")]
    Reset,
    #[serde(rename = "tool.register")]
    ToolRegister {
        id: u64,
        #[serde(default, rename = "scopeId")]
        scope_id: Option<u64>,
        name: String,
        spec: ToolSpecMessage,
    },
    #[serde(rename = "tool.update")]
    ToolUpdate { id: u64, spec: ToolSpecMessage },
    #[serde(rename = "tool.dispose")]
    ToolDispose { id: u64 },
    #[serde(rename = "resource.register")]
    ResourceRegister {
        id: u64,
        #[serde(default, rename = "scopeId")]
        scope_id: Option<u64>,
        name: String,
        description: String,
        #[serde(default, rename = "mimeType")]
        mime_type: Option<String>,
        /// @compat 旧页面 SDK 不发送该字段，缺省 `false`（spec/lifecycle.md 第 13 节 B3）。
        #[serde(default)]
        realtime: bool,
        /// 资源内容的标注（MCP 内容注解）。@compat 旧页面 SDK 不发送，缺省未声明；取值不合法时整条登记被拒绝。
        #[serde(default)]
        annotations: Option<ContentAnnotations>,
    },
    #[serde(rename = "resource.notify")]
    ResourceNotify { id: u64 },
    #[serde(rename = "resource.dispose")]
    ResourceDispose { id: u64 },
    #[serde(rename = "scope.create")]
    ScopeCreate {
        id: u64,
        #[serde(default, rename = "scopeId")]
        scope_id: Option<u64>,
        name: String,
    },
    #[serde(rename = "scope.dispose")]
    ScopeDispose { id: u64 },
    #[serde(rename = "call.result")]
    CallResult {
        #[serde(rename = "callId")]
        call_id: String,
        #[serde(flatten)]
        outcome: Outcome,
    },
    /// 页面 handler 的 `context.progress()`（spec/protocol.md 3.3）。
    #[serde(rename = "call.progress")]
    CallProgress {
        #[serde(rename = "callId")]
        call_id: String,
        progress: f64,
        #[serde(default)]
        total: Option<f64>,
        #[serde(default)]
        message: Option<String>,
    },
    #[serde(rename = "read.result")]
    ReadResult {
        #[serde(rename = "readId")]
        read_id: u64,
        #[serde(flatten)]
        outcome: Outcome,
    },
    #[serde(rename = "lifecycle.wake")]
    Wake,
    #[serde(rename = "lifecycle.sleep")]
    Sleep,
    #[serde(rename = "lifecycle.connectNow")]
    ConnectNow,
    #[serde(rename = "lifecycle.hold")]
    Hold {
        #[serde(rename = "holdId")]
        hold_id: u64,
    },
    #[serde(rename = "lifecycle.release")]
    Release {
        #[serde(rename = "holdId")]
        hold_id: u64,
    },
    /// 本页开启 / 关闭导航处理（[`Bridge::enable_page_navigation`]）。
    #[serde(rename = "navigation.set")]
    NavigationSet { enabled: bool },
    /// 页面对一次 `navigate` 事件的回复：`ok: false` 时 `kind` 为 `NAVIGATION_DENIED` 则拒绝、`USER_ACTION_REQUIRED`
    /// 则需要用户操作（`details` 的 `reason` / `uri`），其他按失败。
    #[serde(rename = "navigate.result")]
    NavigateResult {
        #[serde(rename = "navId")]
        nav_id: u64,
        ok: bool,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        details: Option<Value>,
    },
}

/// `Outcome`：`{ok: true, data, stateHints?, status?, stateResource?, summary?, annotations?}` /
/// `{ok: false, kind, message, details?}`（`details` 为对象时随错误的 `data` 发给 Host，如 `USER_ACTION_REQUIRED` 的
/// `reason` / `uri`）。
///
/// @compat 信封字段先按原始 JSON 接收（出现的 `null` 也保留），在 [`Outcome::call_result`] 中按 @app-mcp/web 的规则判断：
/// 取值不合法时整个结果作为 `data`（与 web / node 一致），而不是整条消息解析失败（那样调用会一直挂到超时）。
#[derive(Debug, Deserialize)]
struct Outcome {
    ok: bool,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default, rename = "stateHints", deserialize_with = "present")]
    state_hints: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    status: Option<Value>,
    #[serde(default, rename = "stateResource", deserialize_with = "present")]
    state_resource: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    summary: Option<Value>,
    #[serde(default, deserialize_with = "present")]
    annotations: Option<Value>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    message: Option<String>,
    /// @compat 可选字段（旧页面不发送）；只接受对象，其他值忽略。
    #[serde(default)]
    details: Option<Value>,
}

#[derive(Debug)]
struct OpError {
    code: &'static str,
    message: String,
}

// ---------------------------------------------------------------------------
// 桥接
// ---------------------------------------------------------------------------

/// 一个 App 的页面桥接：持有原生客户端与全部 WebView 会话。
pub(crate) struct Bridge {
    client: NativeClient,
    sessions: Arc<Sessions>,
    accept: Option<Arc<AcceptFn>>,
}

impl Bridge {
    pub(crate) fn new(
        client: NativeClient,
        sessions: Arc<Sessions>,
        accept: Option<Arc<AcceptFn>>,
    ) -> Self {
        sessions.attach_client(client.clone());
        Self {
            client,
            sessions,
            accept,
        }
    }

    pub(crate) fn client(&self) -> &NativeClient {
        &self.client
    }

    pub(crate) fn sessions(&self) -> &Arc<Sessions> {
        &self.sessions
    }

    /// 把 Host 的导航请求转给页面：在客户端上设置导航回调（握手声明 `capabilities.navigate`，应在 `start` 之前调用），
    /// 之后页面才能 `navigation.set`。没有页面开启导航时，导航以失败回复。
    pub(crate) fn enable_page_navigation(&self) {
        self.sessions.navigation_enabled.store(true, std::sync::atomic::Ordering::SeqCst);
        let handler = PageNavigation { sessions: Arc::downgrade(&self.sessions) };
        self.client.set_navigation_handler(Some(Arc::new(handler)));
    }

    /// 处理页面发来的一条操作。`sink` 只在需要为该 WebView 新建会话时调用。
    pub(crate) fn handle(
        &self,
        label: &str,
        window: &str,
        sink: impl FnOnce() -> Arc<dyn PageSink>,
        raw: Value,
    ) -> Value {
        if let Some(accept) = &self.accept
            && !accept(label)
        {
            return reply_err("FORBIDDEN", "该页面不允许登记 app-mcp 工具");
        }
        let op: PageOp = match serde_json::from_value(raw) {
            Ok(op) => op,
            Err(e) => return reply_err("INVALID_OP", format!("非法的消息：{e}")),
        };
        match op {
            PageOp::Hello => {
                // @why 不清除导航目标：页面 SDK 的 hello 经异步队列发送，可能晚于本页的 navigation.set；
                //   页面卸载由 reset / 页面开始加载 / 窗口销毁处理（Sessions::end_page）。
                self.sessions.end(label);
                let mut hello = json!({
                    "instanceId": self.client.instance_id(),
                    "state": state_json(&self.client.state()),
                });
                if let Some(cid) = self.client.connection_id() {
                    hello["connectionId"] = Value::String(cid);
                }
                reply_ok(Some(hello))
            }
            PageOp::Reset => {
                self.sessions.end_page(label);
                reply_ok(None)
            }
            PageOp::NavigateResult { nav_id, ok, kind, message, details } => {
                let failure = NavigationFailure { kind: kind.as_deref(), message: message.as_deref(), details: details.as_ref() };
                self.sessions.finish_navigation(nav_id, ok, failure);
                reply_ok(None)
            }
            PageOp::NavigationSet { enabled } => match self.sessions.set_navigation_target(label, window, enabled, sink) {
                Ok(()) => reply_ok(None),
                Err(e) => reply_err(e.code, e.message),
            },
            op => {
                let session = match self.sessions.get_or_create(label, || {
                    Session::new(
                        label,
                        window,
                        &self.client,
                        sink(),
                        Arc::downgrade(&self.sessions),
                    )
                }) {
                    Ok(session) => session,
                    Err(e) => return reply_err(e.code, e.message),
                };
                match session.handle(&self.client, op) {
                    Ok(value) => reply_ok(value),
                    Err(e) => reply_err(e.code, e.message),
                }
            }
        }
    }
}

impl Drop for Bridge {
    /// @invariant 断开 `Sessions` 持有的客户端克隆（见 [`Sessions::attach_client`]），
    /// 使 `self.client` 成为最后一个所有者，客户端随插件状态一起停止。
    fn drop(&mut self) {
        self.sessions.detach_client();
    }
}

/// 页面 `navigate.result`（`ok: false`）的失败字段，均未经校验。
struct NavigationFailure<'a> {
    kind: Option<&'a str>,
    message: Option<&'a str>,
    details: Option<&'a Value>,
}

/// 处理导航的页面（[`Sessions::set_navigation_target`]）与等待其回复的导航。
#[derive(Default)]
struct NavigationState {
    /// 最近一次开启导航的 WebView。
    /// @invariant 跨 `hello` 保留，页面卸载（[`Sessions::end_page`]）、窗口销毁或事件无法送达时清除。
    target: Option<NavTarget>,
    /// navId → (送往的 WebView, 导航请求)。
    pending: HashMap<u64, (PageRef, NavigateHandle)>,
    next_id: u64,
}

/// WebView 的 label 与所在窗口的 label。
#[derive(Clone)]
struct PageRef {
    label: String,
    window: String,
}

#[derive(Clone)]
struct NavTarget {
    page: PageRef,
    sink: Arc<dyn PageSink>,
}

/// 全部 WebView 会话（按 label）。
#[derive(Default)]
pub(crate) struct Sessions {
    map: Mutex<HashMap<String, Arc<Session>>>,
    navigation: Mutex<NavigationState>,
    /// [`Bridge::enable_page_navigation`] 已调用。
    navigation_enabled: std::sync::atomic::AtomicBool,
    /// 广播状态时读取连接 ID 用。
    /// @why 状态回调（`PluginListener`）在客户端创建之前就要构造，`StateInfo` 不带连接 ID，只能回头问客户端；
    /// 客户端持有监听器、监听器持有本结构，因此这里是一个引用环，由 [`Bridge`] 的 `Drop` 断开。
    /// @invariant 只在持锁期间使用，不把克隆带出锁外：否则最后一个克隆可能在分发线程上释放，
    /// 而 `NativeClient` 的析构要等待分发线程结束。
    client: Mutex<Option<NativeClient>>,
}

#[derive(Default)]
struct SessionState {
    disposed: bool,
    tools: HashMap<u64, ToolHandle>,
    resources: HashMap<u64, ResourceHandle>,
    scopes: HashMap<u64, ScopeHandle>,
    calls: HashMap<String, CallHandle>,
    reads: HashMap<u64, ReadHandle>,
    holds: HashMap<u64, HoldHandle>,
    next_read_id: u64,
}

/// 一个 WebView 的登记。
pub(crate) struct Session {
    label: String,
    window: String,
    scope: ScopeHandle,
    sink: Arc<dyn PageSink>,
    owner: Weak<Sessions>,
    state: Mutex<SessionState>,
}

struct PageTool {
    session: Weak<Session>,
    tool_id: u64,
}

impl ToolHandler for PageTool {
    fn invoke(&self, call: CallHandle) {
        match self.session.upgrade() {
            Some(session) => session.forward_call(self.tool_id, call),
            None => {
                let _ = call.fail(ErrorKind::AppDisconnected, "页面已关闭");
            }
        }
    }
}

struct PageResource {
    session: Weak<Session>,
    resource_id: u64,
}

impl ResourceReader for PageResource {
    fn read(&self, read: ReadHandle) {
        match self.session.upgrade() {
            Some(session) => session.forward_read(self.resource_id, read),
            None => {
                let _ = read.fail(ErrorKind::AppDisconnected, "页面已关闭");
            }
        }
    }
}

/// Host 的导航请求 → [`Sessions::forward_navigation`]。
struct PageNavigation {
    sessions: Weak<Sessions>,
}

impl NavigationHandler for PageNavigation {
    fn navigate(&self, request: NavigateHandle) {
        match self.sessions.upgrade() {
            Some(sessions) => sessions.forward_navigation(request),
            None => {
                let _ = request.fail("App 正在退出");
            }
        }
    }
}

struct PageCancel {
    session: Weak<Session>,
    call_id: String,
}

impl CancelListener for PageCancel {
    fn on_cancel(&self, reason: CancelReason) {
        if let Some(session) = self.session.upgrade() {
            session.on_cancel(&self.call_id, reason);
        }
    }
}

#[cfg(test)]
mod spec_tests {
    use super::*;

    /// 页面工具定义的 `backgroundTool`（spec/protocol.md 3.4）进入原生选项；缺省（`tool.update` 时即清除）为 `None`。
    #[test]
    fn tool_spec_background_tool() {
        let parse = |v: Value| serde_json::from_value::<ToolSpecMessage>(v).map(|m| m.into_parts("cart.checkout".into()).1);
        let options = parse(json!({ "description": "结算", "surface": "view", "backgroundTool": "cart.checkoutBg" }));
        assert_eq!(options.ok().and_then(|o| o.background_tool).as_deref(), Some("cart.checkoutBg"));
        let options = parse(json!({ "description": "结算" }));
        assert_eq!(options.ok().map(|o| o.background_tool), Some(None));
        assert!(parse(json!({ "description": "结算", "backgroundTool": 1 })).is_err());
    }
}
