//! 原生客户端类 `NativeClient`：配置转换、启停、生命周期控制与顶层注册。

use std::sync::{Arc, Mutex};

use app_mcp_native as native;
use napi_derive::napi;

use super::WeakTsfn;
use super::callbacks::{JsClientListener, JsNavigationHandler, JsResourceReader, JsToolHandler};
use super::convert::{
    parse_busy_policy, parse_client_kind, parse_heartbeat, parse_sleep_reason, parse_visibility, parse_wake_reason, to_js_error,
};
use super::handles::{Call, Hold, Navigate, Read, Resource, Scope, Tool};
use super::objects::{ClientConfig, ClientEvent, JsStateInfo, ResourceSpecInit, ToolSpecInit};

/// 本绑定抛出的错误：`status` 字符串成为 JS 错误的 `code`（napi-derive 按名称 `Result` 识别返回类型）。
use napi::Result;

/// 原生客户端。构造时启动后台运行时线程（不连接），`start()` 后开始连接 Host。
#[napi(js_name = "NativeClient")]
pub struct JsNativeClient {
    inner: native::NativeClient,
    listener: Option<Arc<JsClientListener>>,
}

#[napi]
impl JsNativeClient {
    /// `listener(event)` 在 Node 事件循环上接收状态、配对与日志事件。
    #[napi(constructor)]
    pub fn new(config: ClientConfig, listener: Option<WeakTsfn<ClientEvent>>) -> Result<Self, String> {
        let mut cfg = native::NativeConfig::new(config.app_id, config.app_name);
        cfg.instance_id = config.instance_id;
        if let Some(kind) = config.client_kind.as_deref() {
            cfg.client_kind = parse_client_kind(kind)?;
        }
        if let Some(url) = config.host_url {
            cfg.host_url = url;
        }
        cfg.app_version = config.app_version;
        cfg.instance_title = config.instance_title;
        cfg.token = config.token;
        cfg.launch_token = config.launch_token;
        if let Some(n) = config.max_concurrent_calls {
            cfg.max_concurrent_calls = n;
        }
        if let Some(n) = config.max_queued_calls {
            cfg.max_queued_calls = n;
        }
        if let Some(p) = config.busy_policy.as_deref() {
            cfg.busy_policy = parse_busy_policy(p)?;
        }
        cfg.overview = config.overview.map(Into::into);
        if let Some(lifecycle) = config.lifecycle {
            cfg.lifecycle = lifecycle.into_policy()?;
        }
        if let Some(ms) = config.connect_timeout_ms {
            cfg.connect_timeout_ms = ms;
        }
        if let Some(h) = config.heartbeat.as_deref() {
            cfg.heartbeat = parse_heartbeat(h)?;
        }
        if let Some(d) = config.call_dedup {
            cfg.call_dedup = d.into_policy()?;
        }
        cfg.register_name = config.register_name.unwrap_or(false);
        cfg.name_instance = config.name_instance;

        let listener = listener.map(|tsfn| Arc::new(JsClientListener { tsfn: Mutex::new(Some(Arc::new(tsfn))) }));
        let dyn_listener = listener.clone().map(|l| l as Arc<dyn native::ClientListener>);
        let inner = native::NativeClient::new(cfg, dyn_listener).map_err(to_js_error)?;
        Ok(Self { inner, listener })
    }

    #[napi(getter)]
    pub fn instance_id(&self) -> String {
        self.inner.instance_id()
    }

    #[napi(getter)]
    pub fn state(&self) -> JsStateInfo {
        self.inner.state().into()
    }

    /// Host 为当前连接分配的连接 ID（spec/protocol.md 10.3）；未连接时为 `undefined`。
    #[napi(getter)]
    pub fn connection_id(&self) -> Option<String> {
        self.inner.connection_id()
    }

    /// 当前 token（配置带入的或配对后获得的）。
    #[napi(getter)]
    pub fn token(&self) -> Option<String> {
        self.inner.token()
    }

    #[napi]
    pub fn start(&self) {
        self.inner.start();
    }

    /// 设置导航回调（spec/protocol.md 3.4）；`null` 清除。握手时声明能力，建议在 `start()` 之前设置。
    #[napi]
    pub fn set_navigation_handler(&self, handler: Option<WeakTsfn<Navigate>>) {
        let handler = handler.map(|tsfn| Arc::new(JsNavigationHandler { tsfn }) as Arc<dyn native::NavigationHandler>);
        self.inner.set_navigation_handler(handler);
    }

    /// 不可见时导航请求是否仍交给导航回调（spec/protocol.md 3.4）。缺省按平台：桌面 `true`，Android / iOS / 鸿蒙 `false`
    /// （直接以 `USER_ACTION_REQUIRED`（`foreground`）回复）。随时生效，只影响之后到达的请求。
    #[napi]
    pub fn set_navigate_in_background(&self, enabled: bool) {
        self.inner.set_navigate_in_background(enabled);
    }

    /// 声明用户正在 / 不再在 App 内操作（spec/protocol.md 5.3）：期间写调用按 `busyPolicy` 拒绝或排队，只读调用与已开始的调用
    /// 不受影响。随时生效。
    #[napi]
    pub fn set_busy(&self, busy: bool) {
        self.inner.set_busy(busy);
    }

    #[napi]
    pub fn is_busy(&self) -> bool {
        self.inner.is_busy()
    }

    /// 修改用户正在操作期间写调用的处理方式（`'reject'` | `'queue'`），随即对排队中的调用生效。
    #[napi]
    pub fn set_busy_policy(&self, policy: String) -> Result<(), String> {
        self.inner.set_busy_policy(parse_busy_policy(&policy)?);
        Ok(())
    }

    /// 停止：取消所有调用、断开连接、不再重连，并释放监听器的 ThreadsafeFunction。
    ///
    /// 停止后不再投递状态事件（`stopped` 状态由 JS 封装层自行设置）。
    #[napi]
    pub fn stop(&self) {
        self.inner.stop();
        if let Some(listener) = &self.listener {
            listener.release();
        }
    }

    // ---- 生命周期（spec/lifecycle.md 第 8 节）----------------------------

    /// 处理 OS 激活参数 / URL（`app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=`、`#app-mcp-wake=`）。
    /// 不是本 SDK 的唤醒返回 `false`。可在 `start()` 之前调用。
    #[napi]
    pub fn handle_wake(&self, args: String) -> bool {
        self.inner.handle_wake(&args)
    }

    /// App 主动回连。`reason` 缺省 `'app'`（窗口重新可见时用 `'visible'`）。返回是否因此发起了回连。
    #[napi]
    pub fn wake(&self, reason: Option<String>) -> Result<bool, String> {
        Ok(match reason.as_deref() {
            None => self.inner.wake(),
            Some(r) => self.inner.wake_with_reason(parse_wake_reason(r)?),
        })
    }

    /// `on-demand` 模式下主动连接；尚未 `start()` 时等同于 `start()`。
    #[napi]
    pub fn connect_now(&self) -> bool {
        self.inner.connect_now()
    }

    /// 主动请求休眠。`reason` 缺省 `'app'`（进入后台时用 `'background'`）。返回是否有效果。
    #[napi]
    pub fn sleep(&self, reason: Option<String>) -> Result<bool, String> {
        Ok(match reason.as_deref() {
            None => self.inner.sleep(),
            Some(r) => self.inner.sleep_with_reason(parse_sleep_reason(r)?),
        })
    }

    /// 临时阻止自动休眠，直到返回的 `Hold` 被 `release()`。
    #[napi]
    pub fn hold(&self) -> Hold {
        Hold { inner: self.inner.hold() }
    }

    /// 当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）。
    #[napi]
    pub fn tools_hash(&self) -> String {
        self.inner.tools_hash()
    }

    /// 测试用：tokio 运行时当前是否存在（休眠时为 `false`）。
    #[napi]
    pub fn runtime_active(&self) -> bool {
        self.inner.runtime_active()
    }

    #[napi]
    pub fn set_visibility(&self, visibility: String, focused: bool) -> Result<(), String> {
        self.inner.set_visibility(parse_visibility(&visibility)?, focused);
        Ok(())
    }

    #[napi]
    pub fn register_tool(&self, spec: ToolSpecInit, handler: WeakTsfn<Call>) -> Result<Tool, String> {
        let (spec, options) = spec.into_parts()?;
        let inner = self
            .inner
            .register_tool_with(spec, options, Arc::new(JsToolHandler { tsfn: handler }))
            .map_err(to_js_error)?;
        Ok(Tool { inner })
    }

    #[napi]
    pub fn register_resource(&self, spec: ResourceSpecInit, reader: WeakTsfn<Read>) -> Result<Resource, String> {
        let (spec, options) = spec.into_parts()?;
        let inner = self
            .inner
            .register_resource_with(spec, options, Arc::new(JsResourceReader { tsfn: reader }))
            .map_err(to_js_error)?;
        Ok(Resource { inner })
    }

    #[napi]
    pub fn create_scope(&self, name: String) -> Result<Scope, String> {
        let inner = self.inner.create_scope(&name).map_err(to_js_error)?;
        Ok(Scope { inner })
    }
}
