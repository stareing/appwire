//! [`NativeClient`] 的方法实现；类型定义在 crate 根。

use super::*;

impl NativeClient {
    pub fn new(
        config: NativeConfig,
        listener: Option<Arc<dyn ClientListener>>,
    ) -> Result<Self, NativeError> {
        let connect_timeout = std::time::Duration::from_millis(u64::from(config.connect_timeout_ms.max(1)));
        let (core_config, host_url, name_request) = build_core_config(config)?;
        let instance_id = core_config.instance_id.clone();
        let shared = Arc::new(Shared {
            state: Mutex::new(CoreState {
                client: Client::new(core_config),
                stopped: false,
                shutdown: false,
                tools: HashMap::new(),
                resources: HashMap::new(),
                scopes: HashMap::new(),
                calls: HashMap::new(),
                navigation: None,
                name_request,
                channel: None,
            }),
            wake: tokio::sync::Notify::new(),
            park: Mutex::new(0),
            park_cv: Condvar::new(),
            runtime_active: AtomicBool::new(false),
            listener,
            instance_id,
        });
        let _ = epoch();

        let rt = runtime::build_runtime()
            .map_err(|e| NativeError::Internal(format!("无法创建运行时：{e}")))?;
        let (job_tx, job_rx) = std::sync::mpsc::channel::<Job>();
        let dispatcher = std::thread::Builder::new()
            .name("app-mcp-dispatch".to_owned())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    // 用户回调 panic 不应拖垮分发线程。
                    let _ = std::panic::catch_unwind(AssertUnwindSafe(job));
                }
            })
            .map_err(|e| NativeError::Internal(format!("无法创建分发线程：{e}")))?;
        let driver_shared = shared.clone();
        let runtime = std::thread::Builder::new()
            .name("app-mcp-runtime".to_owned())
            .spawn(move || {
                runtime::run(
                    rt,
                    driver_shared,
                    job_tx,
                    runtime::Target {
                        endpoint: host_url,
                        connect_timeout,
                    },
                )
            })
            .map_err(|e| NativeError::Internal(format!("无法创建运行时线程：{e}")))?;

        Ok(Self {
            owner: Arc::new(ClientOwner {
                shared,
                threads: Mutex::new(vec![runtime, dispatcher]),
            }),
        })
    }
    /// 设置导航回调（spec/protocol.md 3.4）：`Some` 时握手声明 `capabilities.navigate`，Host 的 `app/navigate` 交给回调；
    /// `None` 时导航请求以 `NAVIGATION_FAILED`（`unsupported`）回复。能力在握手时声明——连接后才设置的回调在下次连接
    /// （回连 / 唤醒）时生效，建议在 `start` 之前设置。
    pub fn set_navigation_handler(&self, handler: Option<Arc<dyn NavigationHandler>>) {
        let mut st = self.owner.shared.lock();
        st.client.set_navigation(handler.is_some());
        st.navigation = handler;
    }

    /// 实例不可见时导航请求是否仍交给导航回调（spec/protocol.md 3.4）。缺省按平台：桌面 `true`（App / 封装层能把窗口
    /// 带到前台）；Android、iOS、鸿蒙 `false`（直接以 `USER_ACTION_REQUIRED`（`foreground`）回复，不等超时）。
    /// App 要在后台自行处理（如发通知请用户点开，再以 [`NavigateHandle::fail_user_action`] 回复）时置为 `true`。随时生效。
    pub fn set_navigate_in_background(&self, enabled: bool) {
        self.owner.shared.lock().client.set_navigate_in_background(enabled);
    }

    /// 声明用户正在 / 不再在 App 内操作（第 16 项 N6，spec/protocol.md 5.3）：期间写调用按 [`NativeConfig::busy_policy`]
    /// 拒绝（`RATE_LIMITED`，`scope: "busy"`）或排队；只读调用与已开始的调用不受影响。何时算"正在操作"由 App 决定。随时生效。
    pub fn set_busy(&self, busy: bool) {
        let shared = &self.owner.shared;
        shared.lock().client.set_busy(busy);
        shared.wake();
    }

    pub fn is_busy(&self) -> bool {
        self.owner.shared.lock().client.is_busy()
    }

    /// 修改用户正在操作期间写调用的处理方式（[`NativeConfig::busy_policy`]），随即对排队中的调用生效。
    pub fn set_busy_policy(&self, policy: BusyPolicy) {
        let shared = &self.owner.shared;
        shared.lock().client.set_busy_policy(policy);
        shared.wake();
    }

    pub fn instance_id(&self) -> String {
        self.owner.shared.instance_id.clone()
    }
    pub fn state(&self) -> StateInfo {
        let st = self.owner.shared.lock();
        state_info(st.client.state(), now_ms())
    }
    /// Host 为当前连接分配的连接 ID（spec/protocol.md 10.3）；未连接或旧 Host 时为 `None`。
    /// 连接期间库产生的日志（[`ClientListener::on_log`]）都以 `[连接 ID] ` 开头。
    pub fn connection_id(&self) -> Option<String> {
        self.owner.shared.lock().client.connection_id().map(str::to_owned)
    }
    /// 当前 token（配置带入的或配对后获得的）。
    pub fn token(&self) -> Option<String> {
        self.owner.shared.lock().client.token().map(str::to_owned)
    }
    /// 开始连接。重复调用无效果。
    pub fn start(&self) {
        let shared = &self.owner.shared;
        shared.lock().client.start(now_ms());
        shared.wake();
    }
    /// 停止：取消所有调用、断开连接、不再重连。之后注册类方法返回 [`NativeError::Stopped`]。
    pub fn stop(&self) {
        let shared = &self.owner.shared;
        shared.lock().stop(false);
        shared.wake();
    }
    pub fn set_visibility(&self, visibility: Visibility, focused: bool) {
        let shared = &self.owner.shared;
        shared
            .lock()
            .client
            .set_visibility(visibility, focused, now_ms());
        shared.wake();
    }
    /// 在根作用域注册工具。
    pub fn register_tool(
        &self,
        spec: ToolSpec,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<ToolHandle, NativeError> {
        self.register_tool_with(spec, ToolOptions::default(), handler)
    }
    /// 同 [`NativeClient::register_tool`]，另带工具选项（MCP 注解、输出 schema，spec/protocol.md 第 3 节）。
    pub fn register_tool_with(
        &self,
        spec: ToolSpec,
        options: ToolOptions,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<ToolHandle, NativeError> {
        self.owner.shared.register_tool(None, spec, options, handler)
    }
    pub fn register_resource(
        &self,
        spec: ResourceSpec,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<ResourceHandle, NativeError> {
        self.register_resource_with(spec, ResourceOptions::default(), reader)
    }
    /// 同 [`NativeClient::register_resource`]，另带资源选项（如 `realtime`，spec/lifecycle.md 第 13 节 B3）。
    pub fn register_resource_with(
        &self,
        spec: ResourceSpec,
        options: ResourceOptions,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<ResourceHandle, NativeError> {
        self.owner.shared.register_resource(None, spec, options, reader)
    }
    pub fn create_scope(&self, name: &str) -> Result<ScopeHandle, NativeError> {
        self.owner.shared.create_scope(None, name)
    }

    // ---- 事件（第 16 项 N3，spec/protocol.md 3.5）--------------------------

    /// 声明本实例可发出的事件（同名替换）。已连接时随即同步给 Host，否则在下次握手成功后同步；不触发连接。
    ///
    /// @error 名称不合法 → [`NativeError::InvalidName`]；已停止 → [`NativeError::Stopped`]。
    pub fn declare_event(&self, info: EventInfo) -> Result<(), NativeError> {
        let shared = &self.owner.shared;
        let mut st = shared.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        st.client.declare_event(info).map_err(core_error)?;
        drop(st);
        shared.wake();
        Ok(())
    }

    /// 撤销事件声明；未声明过（或已停止）返回 `false`。
    pub fn remove_event(&self, name: &str) -> bool {
        let shared = &self.owner.shared;
        let removed = shared.lock().client.remove_event(name);
        if removed {
            shared.wake();
        }
        removed
    }

    /// 发出已声明的事件。`payload_json` 为 JSON 对象文本（`None` = 无载荷）。
    ///
    /// 已连接时发送并返回 `true`；未连接（休眠、断线、重连中、握手中）丢弃并返回 `false`：不缓存、不为此连接或唤醒 Host，
    /// 也不推迟空闲休眠。需要可靠送达的状态变化请改用资源。
    ///
    /// @error 名称不合法、未声明 → [`NativeError::InvalidName`]；`payload_json` 不是合法 JSON、不是对象或序列化后超过
    /// [`MAX_EVENT_PAYLOAD_BYTES`] → [`NativeError::InvalidJson`]；已停止 → [`NativeError::Stopped`]。
    pub fn emit_event(&self, name: &str, payload_json: Option<&str>) -> Result<bool, NativeError> {
        let payload = payload_json
            .map(|text| {
                serde_json::from_str::<Value>(text)
                    .map_err(|e| NativeError::InvalidJson(format!("事件载荷不是合法 JSON：{e}")))
            })
            .transpose()?;
        let shared = &self.owner.shared;
        let mut st = shared.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        let sent = st.client.emit_event(name, payload).map_err(core_error)?;
        drop(st);
        // @why 只在真正排队了消息时唤醒运行时；休眠中不碰条件变量，避免无谓地重建运行时。
        if sent {
            shared.wake();
        }
        Ok(sent)
    }

    // ---- 生命周期 -------------------------------------------------------

    /// 处理操作系统激活参数 / URL（命令行、`onOpenURL`、D-Bus action 参数等），识别
    /// `app-mcp-wake:<token>`、`<scheme>://app-mcp/wake?token=`、`#app-mcp-wake=<token>`。
    /// 不是本 SDK 的唤醒返回 `false`。可以在 `start` 之前调用（冷启动唤醒：`on-demand` 模式也会连接）。
    pub fn handle_wake(&self, args: &str) -> bool {
        let shared = &self.owner.shared;
        let recognized = shared.lock().client.handle_wake(args, now_ms());
        shared.wake();
        recognized
    }
    /// App 主动回连（如用户打开了相关界面）。返回是否因此发起了回连。
    pub fn wake(&self) -> bool {
        self.wake_with_reason(WakeReason::App)
    }
    /// 以指定原因回连（窗口重新可见时用 [`WakeReason::Visible`]）。
    pub fn wake_with_reason(&self, reason: WakeReason) -> bool {
        let shared = &self.owner.shared;
        let started = shared.lock().client.wake_with_reason(reason, now_ms());
        shared.wake();
        started
    }
    /// `on-demand` 模式下主动连接；尚未 `start` 时等同于 `start`。
    pub fn connect_now(&self) -> bool {
        let shared = &self.owner.shared;
        let started = shared.lock().client.connect_now(now_ms());
        shared.wake();
        started
    }
    /// App 主动请求休眠（原因 `app`，不受空闲条件与持有影响）。返回是否有效果。
    pub fn sleep(&self) -> bool {
        self.sleep_with_reason(SleepReason::App)
    }
    /// 以指定原因请求休眠（进入后台时用 [`SleepReason::Background`]）。
    pub fn sleep_with_reason(&self, reason: SleepReason) -> bool {
        let shared = &self.owner.shared;
        let changed = shared.lock().client.sleep_with_reason(reason, now_ms());
        shared.wake();
        changed
    }
    /// 临时阻止自动休眠，直到返回的句柄被释放（或最后一个克隆被丢弃）。
    pub fn hold(&self) -> HoldHandle {
        let shared = &self.owner.shared;
        let id = shared.lock().client.hold(now_ms());
        shared.wake();
        HoldHandle::new(shared.clone(), id)
    }
    /// 接受 Hub 交来的一条已建立的通道（socketpair 的一端，spec/naming.md 第 3 节"被连接方"）：平台名字服务不在本库内
    /// 时由封装层调用——Android `ToolsService` 的 `open()` 创建 socketpair，一端交到这里、另一端经 Binder 返回给 Hub
    /// （4.2）。之后与 D-Bus `Open()` 拨入的通道相同：SDK 在其上作为 WebSocket 客户端先发 `app/hello`
    /// （`wakeReason: "os-activation"`），不发心跳、不做 App 端空闲计时；对端关闭（Hub 宽限到期 / 解绑 / 进程死亡）后
    /// 非 `persistent` 转休眠、不重连。
    ///
    /// @error 已有连接或通道 → [`ChannelRefusal::Busy`]；尚未 `start` / 已停止 → [`ChannelRefusal::Stopped`]；
    /// 不是流式 Unix 套接字 → [`ChannelRefusal::Invalid`]。被拒绝的通道随之关闭。
    #[cfg(unix)]
    pub fn accept_channel(&self, channel: std::os::unix::net::UnixStream) -> Result<(), ChannelRefusal> {
        // @why 不是套接字的 fd（如管道、普通文件）在 getsockname 处失败（ENOTSOCK），不进入核心。
        channel.local_addr().map_err(|e| ChannelRefusal::Invalid(e.to_string()))?;
        names::ChannelSink::offer(&names::ChannelInbox(Arc::downgrade(&self.owner.shared)), channel)
    }

    /// 当前工具与资源定义的摘要（spec/lifecycle.md 第 6 节）。
    pub fn tools_hash(&self) -> String {
        self.owner.shared.lock().client.tools_hash()
    }
    /// 调试 / 测试用：tokio 运行时当前是否存在（休眠时为 `false`）。
    #[doc(hidden)]
    pub fn runtime_active(&self) -> bool {
        self.owner.shared.runtime_active.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for NativeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeClient").finish_non_exhaustive()
    }
}
