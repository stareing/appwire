//! 驱动层共享状态（[`Shared`]、[`CoreState`]、[`ClientOwner`]）的实现：注册与取事件。

use super::*;

impl Drop for ClientOwner {
    fn drop(&mut self) {
        self.shared.lock().stop(true);
        self.shared.wake();
        let threads = std::mem::take(&mut *lock_ignore_poison(&self.threads));
        let me = std::thread::current().id();
        for t in threads {
            // 最后一个克隆可能在分发线程（用户回调）里被丢弃：不能 join 自己，直接分离。
            // 运行时线程退出后发送端被丢弃，分发线程执行完剩余回调后自行结束。
            if t.thread().id() != me {
                let _ = t.join();
            }
        }
    }
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("instance_id", &self.instance_id)
            .finish_non_exhaustive()
    }
}

impl CoreState {
    pub(crate) fn stop(&mut self, shutdown: bool) {
        self.stopped = true;
        self.shutdown |= shutdown;
        self.client.stop(now_ms());
        // 释放 handler，打破「handler 持有 NativeClient」形成的引用环。
        self.navigation = None;
        self.tools.clear();
        self.resources.clear();
        self.scopes.clear();
    }

    pub(crate) fn dispose_scope(&mut self, scope: ScopeId) {
        if !self.scopes.contains_key(&scope) {
            return;
        }
        // 收集 scope 及全部后代。
        let mut doomed: HashSet<ScopeId> = HashSet::from([scope]);
        loop {
            let before = doomed.len();
            for (id, parent) in &self.scopes {
                if parent.is_some_and(|p| doomed.contains(&p)) {
                    doomed.insert(*id);
                }
            }
            if doomed.len() == before {
                break;
            }
        }
        let _ = self.client.dispose_scope(scope);
        let in_doomed = |s: &Option<ScopeId>| s.is_some_and(|s| doomed.contains(&s));
        self.tools.retain(|_, e| !in_doomed(&e.scope));
        self.resources.retain(|_, e| !in_doomed(&e.scope));
        self.scopes.retain(|id, _| !doomed.contains(id));
    }
}

impl Shared {
    pub(crate) fn lock(&self) -> MutexGuard<'_, CoreState> {
        lock_ignore_poison(&self.state)
    }

    pub(crate) fn wake(&self) {
        self.wake.notify_one();
        *lock_ignore_poison(&self.park) += 1;
        self.park_cv.notify_all();
    }

    /// 是否应在名字服务登记：配置了登记、已 `start` 且未停止。
    pub(crate) fn name_request(&self) -> Option<names::NameRequest> {
        let st = self.lock();
        let started = *st.client.state() != ConnectionState::Idle;
        st.name_request.clone().filter(|_| started && !st.stopped)
    }

    /// 取走待连接的拨入通道。
    pub(crate) fn take_channel(&self) -> Option<names::Channel> {
        self.lock().channel.take()
    }
}

impl Shared {

    pub(crate) fn register_tool(
        self: &Arc<Self>,
        scope: Option<ScopeId>,
        spec: ToolSpec,
        options: ToolOptions,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<ToolHandle, NativeError> {
        let mut st = self.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        let input_schema = parse_schema(spec.input_schema_json.as_deref())?;
        let output_schema = parse_output_schema(options.output_schema_json.as_deref())?;
        let name = spec.name.clone();
        let id = st
            .client
            .register_tool(ToolDef {
                name: spec.name,
                description: spec.description,
                input_schema,
                risk: spec.risk,
                activation: spec.activation,
                title: spec.title,
                enabled: spec.enabled,
                scope,
                annotations: options.annotations,
                output_schema,
                surface: options.surface,
                page: options.page,
                background_tool: options.background_tool,
                implements: options.implements,
                concurrency: options.concurrency,
                exclusive: options.exclusive,
            })
            .map_err(core_error)?;
        st.tools.insert(id, ToolEntry { handler, scope });
        drop(st);
        self.wake();
        Ok(ToolHandle {
            inner: Arc::new(ToolInner {
                shared: self.clone(),
                id,
                name,
                disposed: AtomicBool::new(false),
            }),
        })
    }

    pub(crate) fn register_resource(
        self: &Arc<Self>,
        scope: Option<ScopeId>,
        spec: ResourceSpec,
        options: ResourceOptions,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<ResourceHandle, NativeError> {
        let mut st = self.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        let name = spec.name.clone();
        let id = st
            .client
            .register_resource(ResourceDef {
                name: spec.name,
                description: spec.description,
                mime_type: spec.mime_type,
                scope,
                realtime: options.realtime,
                annotations: options.annotations,
            })
            .map_err(core_error)?;
        st.resources.insert(id, ResourceEntry { reader, scope });
        drop(st);
        self.wake();
        Ok(ResourceHandle {
            inner: Arc::new(ResourceInner {
                shared: self.clone(),
                id,
                name,
                disposed: AtomicBool::new(false),
            }),
        })
    }

    pub(crate) fn create_scope(
        self: &Arc<Self>,
        parent: Option<ScopeId>,
        name: &str,
    ) -> Result<ScopeHandle, NativeError> {
        let mut st = self.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        let id = st.client.create_scope(name, parent).map_err(core_error)?;
        st.scopes.insert(id, parent);
        Ok(ScopeHandle {
            inner: Arc::new(ScopeInner {
                shared: self.clone(),
                id,
                disposed: AtomicBool::new(false),
            }),
        })
    }

    /// 在分发线程上调用 listener。
    pub(crate) fn listener_job(&self, f: impl FnOnce(&dyn ClientListener) + Send + 'static) -> Option<Action> {
        let listener = self.listener.clone()?;
        Some(Action::Dispatch(Box::new(move || f(listener.as_ref()))))
    }

    /// 持锁取出全部事件并翻译成动作。不在这里调用任何用户回调。
    pub(crate) fn drain(self: &Arc<Self>) -> Batch {
        let mut st = self.lock();
        let mut actions = Vec::new();
        while let Some(event) = st.client.poll_event() {
            match event {
                Event::Connect => actions.push(Action::Connect),
                Event::Send(text) => actions.push(Action::Send(text)),
                Event::Disconnect => actions.push(Action::Disconnect),
                Event::InvokeTool {
                    call_id,
                    tool,
                    name,
                    arguments,
                    idempotency_key,
                } => {
                    let Some(handler) = st.tools.get(&tool).map(|e| e.handler.clone()) else {
                        let err =
                            ToolError::new(ErrorKind::ToolNotFound, format!("工具 {name} 已注销"));
                        let _ = st.client.complete_call(&call_id, Err(err), now_ms());
                        continue;
                    };
                    let call = Arc::new(CallInner {
                        shared: self.clone(),
                        call_id: call_id.clone(),
                        tool_name: name,
                        arguments_json: arguments.to_string(),
                        idempotency_key,
                        state: Mutex::new(CallState::default()),
                    });
                    st.calls.insert(call_id, call.clone());
                    let handle = CallHandle { inner: call };
                    actions.push(Action::Dispatch(Box::new(move || {
                        let fallback = handle.clone();
                        if std::panic::catch_unwind(AssertUnwindSafe(|| handler.invoke(handle)))
                            .is_err()
                        {
                            let _ =
                                fallback.fail(ErrorKind::HandlerError, "handler 执行时发生 panic");
                        }
                    })));
                }
                Event::CancelTool { call_id, reason } => {
                    let Some(call) = st.calls.remove(&call_id) else {
                        continue;
                    };
                    let reason = cancel_reason(reason);
                    if let Some(listener) = call.mark_cancelled(reason) {
                        actions.push(Action::Dispatch(Box::new(move || {
                            listener.on_cancel(reason)
                        })));
                    }
                }
                Event::ReadResource {
                    read,
                    resource,
                    name,
                } => {
                    let Some(reader) = st.resources.get(&resource).map(|e| e.reader.clone()) else {
                        let err = ToolError::new(
                            ErrorKind::ResourceNotFound,
                            format!("资源 {name} 已注销"),
                        );
                        let _ = st.client.complete_read(read, Err(err));
                        continue;
                    };
                    let handle = ReadHandle {
                        inner: Arc::new(ReadInner {
                            shared: self.clone(),
                            read,
                            name,
                            done: AtomicBool::new(false),
                        }),
                    };
                    actions.push(Action::Dispatch(Box::new(move || {
                        let fallback = handle.clone();
                        if std::panic::catch_unwind(AssertUnwindSafe(|| reader.read(handle)))
                            .is_err()
                        {
                            let _ = fallback.fail(ErrorKind::HandlerError, "资源读取时发生 panic");
                        }
                    })));
                }
                Event::Navigate { navigate, page, params } => {
                    let Some(handler) = st.navigation.clone() else {
                        // 回调在请求到达后被清除：按不支持回复。
                        let err = ToolError::navigation_failed(
                            format!("App 不支持由 Agent 导航（页面「{page}」）。"),
                            navigation_reason::UNSUPPORTED,
                        );
                        let _ = st.client.complete_navigate(navigate, Err(err));
                        continue;
                    };
                    let handle = NavigateHandle {
                        inner: Arc::new(NavigateInner {
                            shared: self.clone(),
                            navigate,
                            page,
                            params_json: (!params.is_null()).then(|| params.to_string()),
                            done: AtomicBool::new(false),
                        }),
                    };
                    actions.push(Action::Dispatch(Box::new(move || {
                        let fallback = handle.clone();
                        if std::panic::catch_unwind(AssertUnwindSafe(|| handler.navigate(handle))).is_err() {
                            let _ = fallback.fail("导航回调执行时发生 panic");
                        }
                    })));
                }
                Event::StateChanged(state) => {
                    if state == ConnectionState::Connected {
                        let message = match st.client.connection_id() {
                            Some(cid) => format!("[{cid}] 已连接 Host（连接 ID {cid}）"),
                            None => "已连接 Host（Host 未提供连接 ID）".to_owned(),
                        };
                        actions.extend(self.listener_job(move |l| l.on_log(LogLevel::Info, message)));
                    }
                    let info = state_info(&state, now_ms());
                    actions.extend(self.listener_job(move |l| l.on_state_changed(info)));
                }
                Event::Paired { token } => {
                    actions.extend(self.listener_job(move |l| l.on_paired(token)))
                }
                Event::Warning(message) => {
                    let message = format!("{}{message}", cid_prefix(&st.client));
                    actions.extend(self.listener_job(move |l| l.on_log(LogLevel::Warn, message)));
                }
                Event::IdleExit => actions.extend(self.listener_job(|l| l.on_idle_exit())),
            }
        }
        Batch {
            actions,
            timeout: st.client.poll_timeout(),
            shutdown: st.shutdown,
            dormant: *st.client.state() == ConnectionState::Dormant,
        }
    }
}
