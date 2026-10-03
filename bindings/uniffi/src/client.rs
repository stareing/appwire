//! 客户端对象 [`AppMcpClient`] 与本地通道接入结果 [`ChannelOffer`]。

use std::sync::Arc;

use app_mcp_native as native;

use crate::callbacks::{ClientListener, ClientListenerAdapter, NavigationHandler, NavigationHandlerAdapter, ResourceReader, ResourceReaderAdapter, ToolHandler, ToolHandlerAdapter};
use crate::config::ClientConfig;
use crate::enums::{BusyPolicy, SleepReason, Visibility, WakeReason};
use crate::error::AppMcpError;
use crate::handles::{Hold, Resource, Scope, Tool};
use crate::records::{EventInfo, ResourceSpec, StateInfo, ToolSpec};

/// 客户端。创建时启动后台运行时（不连接），`start` 后开始连接 Host。
/// 对象被外部语言释放（最后一个引用消失）时自动停止。
#[derive(Debug, uniffi::Object)]
pub struct AppMcpClient {
    pub(crate) inner: native::NativeClient,
}

#[uniffi::export]
impl AppMcpClient {
    #[uniffi::constructor]
    pub fn new(
        config: ClientConfig,
        listener: Option<Arc<dyn ClientListener>>,
    ) -> Result<Arc<Self>, AppMcpError> {
        let listener: Option<Arc<dyn native::ClientListener>> =
            listener.map(|l| Arc::new(ClientListenerAdapter(l)) as Arc<dyn native::ClientListener>);
        let inner = native::NativeClient::new(config.into(), listener)?;
        Ok(Arc::new(Self { inner }))
    }
    pub fn instance_id(&self) -> String {
        self.inner.instance_id()
    }
    pub fn state(&self) -> StateInfo {
        self.inner.state().into()
    }
    /// 当前 token（配置带入的或配对后获得的）。
    pub fn token(&self) -> Option<String> {
        self.inner.token()
    }
    /// Host 为当前连接分配的连接 ID（spec/protocol.md 10.3）；未连接时为 `None`。
    pub fn connection_id(&self) -> Option<String> {
        self.inner.connection_id()
    }
    /// 开始连接。重复调用无效果。
    pub fn start(&self) {
        self.inner.start()
    }
    /// 停止：取消所有调用、断开连接、不再重连。之后注册类方法返回 `Stopped`。
    pub fn stop(&self) {
        self.inner.stop()
    }
    /// 设置导航回调（spec/protocol.md 3.4）；为空时清除（导航请求以 `NAVIGATION_FAILED` 回复）。
    /// 握手时声明能力，建议在 `start` 之前设置。
    pub fn set_navigation_handler(&self, handler: Option<Arc<dyn NavigationHandler>>) {
        let handler = handler.map(|h| Arc::new(NavigationHandlerAdapter(h)) as Arc<dyn native::NavigationHandler>);
        self.inner.set_navigation_handler(handler)
    }
    pub fn set_visibility(&self, visibility: Visibility, focused: bool) {
        self.inner.set_visibility(visibility.into(), focused)
    }
    /// 后台时是否仍把导航请求交给导航回调（spec/protocol.md 3.4「后台与前台」）。为 `false` 时
    /// App 不可见（`Hidden` / `Frozen`）收到的导航立即以 `USER_ACTION_REQUIRED`（reason `foreground`）回复，
    /// 不调用回调。缺省取平台默认：桌面为 `true`，Android / iOS / 鸿蒙为 `false`。随时生效。
    pub fn set_navigate_in_background(&self, enabled: bool) {
        self.inner.set_navigate_in_background(enabled)
    }
    /// 声明用户正在 / 不再在 App 内操作（spec/protocol.md 5.3）：期间写调用按 `busy_policy` 拒绝或排队，
    /// 只读调用与已开始的调用不受影响。何时算"正在操作"由 App 决定。随时生效。
    pub fn set_busy(&self, busy: bool) {
        self.inner.set_busy(busy)
    }
    pub fn is_busy(&self) -> bool {
        self.inner.is_busy()
    }
    /// 修改用户正在操作期间写调用的处理方式，随即对排队中的调用生效。
    pub fn set_busy_policy(&self, policy: BusyPolicy) {
        self.inner.set_busy_policy(policy.into())
    }
    /// 在根作用域注册工具。
    pub fn register_tool(
        &self,
        spec: ToolSpec,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<Arc<Tool>, AppMcpError> {
        let (spec, options) = spec.into();
        let inner = self
            .inner
            .register_tool_with(spec, options, Arc::new(ToolHandlerAdapter(handler)))?;
        Ok(Arc::new(Tool { inner }))
    }
    pub fn register_resource(
        &self,
        spec: ResourceSpec,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<Arc<Resource>, AppMcpError> {
        let (spec, options) = spec.into();
        let inner = self
            .inner
            .register_resource_with(spec, options, Arc::new(ResourceReaderAdapter(reader)))?;
        Ok(Arc::new(Resource { inner }))
    }
    pub fn create_scope(&self, name: String) -> Result<Arc<Scope>, AppMcpError> {
        Ok(Arc::new(Scope {
            inner: self.inner.create_scope(&name)?,
        }))
    }

    // ---- 事件（spec/protocol.md 3.5） ----

    /// 声明本实例可发出的事件（同名替换）。已连接时随即同步给 Host，否则在下次握手成功后同步；不触发连接。
    ///
    /// @error 名称不合法 → `InvalidName`；`payload_schema_json` 不是合法 JSON → `InvalidJson`；已停止 → `Stopped`。
    pub fn declare_event(&self, info: EventInfo) -> Result<(), AppMcpError> {
        Ok(self.inner.declare_event(info.try_into()?)?)
    }
    /// 撤销事件声明；未声明过（或已停止）返回 `false`。
    pub fn remove_event(&self, name: String) -> bool {
        self.inner.remove_event(&name)
    }
    /// 发出已声明的事件，`payload_json` 为 JSON 对象文本（为空 = 无载荷）。已连接时发送并返回 `true`；未连接时丢弃并返回
    /// `false`：不缓存、不为此连接或唤醒 Host，也不推迟空闲休眠。需要可靠送达的状态变化请改用资源。
    ///
    /// @error 名称不合法、未声明 → `InvalidName`；载荷不是合法 JSON、不是对象或序列化后超过 8 KiB → `InvalidJson`；
    /// 已停止 → `Stopped`。
    pub fn emit_event(&self, name: String, payload_json: Option<String>) -> Result<bool, AppMcpError> {
        Ok(self.inner.emit_event(&name, payload_json.as_deref())?)
    }

    // ---- 生命周期（spec/lifecycle.md 第 8 节） ----

    /// 处理操作系统激活参数 / URL。不是本 SDK 的唤醒返回 `false`。可以在 `start` 之前调用。
    pub fn handle_wake(&self, args: String) -> bool {
        self.inner.handle_wake(&args)
    }
    /// App 主动回连。返回是否因此发起了回连。
    pub fn wake(&self) -> bool {
        self.inner.wake()
    }
    /// 以指定原因回连（窗口重新可见时用 `Visible`）。
    pub fn wake_with_reason(&self, reason: WakeReason) -> bool {
        self.inner.wake_with_reason(reason.into())
    }
    /// `OnDemand` 模式下主动连接；尚未 `start` 时等同于 `start`。
    pub fn connect_now(&self) -> bool {
        self.inner.connect_now()
    }
    /// App 主动请求休眠（原因 `App`）。返回是否有效果。
    pub fn sleep(&self) -> bool {
        self.inner.sleep()
    }
    /// 以指定原因请求休眠（进入后台时用 `Background`）。
    pub fn sleep_with_reason(&self, reason: SleepReason) -> bool {
        self.inner.sleep_with_reason(reason.into())
    }
    /// 临时阻止自动休眠，直到返回的 `Hold` 被释放。
    pub fn hold(&self) -> Arc<Hold> {
        Arc::new(Hold {
            inner: self.inner.hold(),
        })
    }
    /// 当前工具与资源定义的摘要。
    pub fn tools_hash(&self) -> String {
        self.inner.tools_hash()
    }
    /// 接受 Hub 交来的通道（socketpair 的一端，spec/naming.md 4.2）：Android `ToolsService` 的 `open()` 用
    /// `ParcelFileDescriptor.createSocketPair()` 创建一对，`detachFd()` 一端交到这里，另一端经 Binder 返回给 Hub。
    ///
    /// @input `fd` 的所有权随调用转移给本库（无论接受与否；被拒绝时随即关闭）；负数不接管、返回 `Invalid`。
    /// @output 是否接受；`Busy` 时 Hub 记 `CHANNEL_LIMIT`。非 Unix 平台总是 `Invalid`。
    pub fn accept_channel_fd(&self, fd: i32) -> ChannelOffer {
        accept_fd(&self.inner, fd)
    }
}

/// [`AppMcpClient::accept_channel_fd`] 的结果。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ChannelOffer {
    /// 已接受：SDK 随即在通道上发 `app/hello`。
    Accepted,
    /// 已有连接或通道（同时只接受一条），`detail` 为说明。
    Busy { detail: String },
    /// SDK 尚未启动或已停止。
    Stopped { detail: String },
    /// fd 不是可用的 Unix 流式套接字（或平台不支持）。
    Invalid { detail: String },
}

impl From<Result<(), native::ChannelRefusal>> for ChannelOffer {
    fn from(r: Result<(), native::ChannelRefusal>) -> Self {
        use native::ChannelRefusal as R;
        match r {
            Ok(()) => ChannelOffer::Accepted,
            Err(e @ R::Busy) => ChannelOffer::Busy { detail: e.to_string() },
            Err(e @ R::Stopped) => ChannelOffer::Stopped { detail: e.to_string() },
            Err(e @ R::Invalid(_)) => ChannelOffer::Invalid { detail: e.to_string() },
        }
    }
}

#[cfg(unix)]
pub(crate) fn accept_fd(client: &native::NativeClient, fd: i32) -> ChannelOffer {
    use std::os::fd::{FromRawFd, OwnedFd};
    if fd < 0 {
        return ChannelOffer::Invalid { detail: format!("fd 无效：{fd}") };
    }
    // @security 调用方按契约转移一个打开的 fd 的所有权（Kotlin `ParcelFileDescriptor.detachFd()`），之后不再使用它；
    // 本库唯一持有并负责关闭（被拒绝时在此处随 `OwnedFd` 丢弃而关闭）。
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    client.accept_channel(std::os::unix::net::UnixStream::from(owned)).into()
}

#[cfg(not(unix))]
pub(crate) fn accept_fd(_client: &native::NativeClient, fd: i32) -> ChannelOffer {
    ChannelOffer::Invalid { detail: format!("本平台不支持以 fd 交来通道（fd {fd}）") }
}
