//! 公开句柄（调用、持有、读取、导航、工具、资源、scope）的方法实现；类型定义在 crate 根。

use super::*;

impl CallHandle {
    pub fn call_id(&self) -> String {
        self.inner.call_id.clone()
    }
    pub fn tool_name(&self) -> String {
        self.inner.tool_name.clone()
    }
    /// 已由 Host 按 inputSchema 校验过的参数（JSON 对象文本）。
    pub fn arguments_json(&self) -> String {
        self.inner.arguments_json.clone()
    }
    /// Agent 给出的幂等键（原样，spec/protocol.md 3.3）；没有时为 `None`。App 决定如何使用（如作为业务去重键）。
    pub fn idempotency_key(&self) -> Option<String> {
        self.inner.idempotency_key.clone()
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.lock().cancelled.is_some()
    }
    /// 设置取消监听。已取消时立即（在当前线程）回调一次。
    pub fn set_cancel_listener(&self, listener: Arc<dyn CancelListener>) {
        let mut st = self.inner.lock();
        match st.cancelled {
            Some(reason) => {
                drop(st);
                listener.on_cancel(reason);
            }
            None => st.listener = Some(listener),
        }
    }
    /// 成功完成。`data_json` 为 `None` 表示 `null`；非法 JSON 返回 [`NativeError::InvalidJson`]（调用仍未完成）。
    pub fn complete(
        &self,
        data_json: Option<&str>,
        state_hints: Vec<String>,
    ) -> Result<(), NativeError> {
        self.complete_with(CallResult {
            data_json: data_json.map(str::to_owned),
            state_hints,
            ..CallResult::default()
        })
    }
    /// 成功完成，附带业务状态、摘要与内容标注（spec/protocol.md 3.2）。非法 JSON 返回
    /// [`NativeError::InvalidJson`]（调用仍未完成）。
    pub fn complete_with(&self, result: CallResult) -> Result<(), NativeError> {
        let data = match result.data_json.as_deref() {
            None => Value::Null,
            Some(text) => {
                serde_json::from_str(text).map_err(|e| NativeError::InvalidJson(e.to_string()))?
            }
        };
        self.inner.finish(Ok(CallOutput {
            data,
            state_hints: result.state_hints,
            annotations: result.annotations,
            status: result.status,
            state_resource: result.state_resource,
            summary: result.summary,
        }))
    }
    /// 失败完成。
    pub fn fail(&self, kind: ErrorKind, message: &str) -> Result<(), NativeError> {
        self.inner.finish(Err(ToolError::new(kind, message)))
    }
    /// 失败完成，附带结构化详情（JSON 文本；对象的字段合并进错误的 `data`，其他值放在 `data.details`）。`details_json` 为 `None`
    /// 等同于 [`CallHandle::fail`]；非法 JSON 返回 [`NativeError::InvalidJson`]（调用仍未完成）。
    pub fn fail_with_details(
        &self,
        kind: ErrorKind,
        message: &str,
        details_json: Option<&str>,
    ) -> Result<(), NativeError> {
        self.inner.finish(Err(tool_error_with_details(kind, message, details_json)?))
    }
    /// 以 `USER_ACTION_REQUIRED` 失败完成（spec/protocol.md 第 4 节）：需要用户本人操作后才能继续
    /// （登录过期、系统权限未授予、需切到前台、需在 App 内确认等）。
    ///
    /// @input message 面向用户的说明（Agent 转告用户）。
    /// @input reason 可选类别，建议取 `user_action_reason` 中的值（`login` / `permission` / `foreground` / `confirm`）。
    /// @input uri 可选的 App 内入口（深链接等）。
    /// @output `None` 的字段不出现在错误的 `data` 中。
    pub fn fail_user_action(
        &self,
        message: &str,
        reason: Option<&str>,
        uri: Option<&str>,
    ) -> Result<(), NativeError> {
        self.inner
            .finish(Err(ToolError::user_action_required(message, reason, uri)))
    }
    /// 报告进度（spec/protocol.md 3.3）：Host 合并后转发给 Agent（MCP `notifications/progress`）。`progress` 应递增；
    /// `total` 未知时为 `None`。未连接时丢弃；调用已结束（完成、取消）时返回 [`NativeError::AlreadyCompleted`]。
    pub fn report_progress(
        &self,
        progress: f64,
        total: Option<f64>,
        message: Option<&str>,
    ) -> Result<(), NativeError> {
        if self.inner.lock().cancelled.is_some() {
            return Err(NativeError::AlreadyCompleted);
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        st.client
            .report_progress(&self.inner.call_id, progress, total, message.map(str::to_owned), now_ms())
            .map_err(core_error)?;
        drop(st);
        shared.wake();
        Ok(())
    }
    /// 延长持有：调用完成后仍阻止自动休眠（handler 发起的长任务），直到返回的句柄被释放。
    /// 调用已结束（完成、取消）时返回 [`NativeError::AlreadyCompleted`]。
    pub fn hold(&self) -> Result<HoldHandle, NativeError> {
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        let id = st
            .client
            .hold_for_call(&self.inner.call_id, now_ms())
            .map_err(core_error)?;
        drop(st);
        shared.wake();
        Ok(HoldHandle::new(shared.clone(), id))
    }
}

impl HoldHandle {
    pub(crate) fn new(shared: Arc<Shared>, id: HoldId) -> Self {
        Self {
            inner: Arc::new(HoldInner {
                shared,
                id,
                released: AtomicBool::new(false),
            }),
        }
    }
    /// 释放持有。重复调用无效果。
    pub fn release(&self) {
        self.inner.release();
    }
}

impl ReadHandle {
    pub fn resource_name(&self) -> String {
        self.inner.name.clone()
    }
    pub fn complete(&self, contents_json: &str) -> Result<(), NativeError> {
        let contents: Value = serde_json::from_str(contents_json)
            .map_err(|e| NativeError::InvalidJson(e.to_string()))?;
        self.inner.finish(Ok(contents))
    }
    pub fn fail(&self, kind: ErrorKind, message: &str) -> Result<(), NativeError> {
        self.inner.finish(Err(ToolError::new(kind, message)))
    }
    /// 失败完成，附带结构化详情；语义同 [`CallHandle::fail_with_details`]（非法 JSON 返回
    /// [`NativeError::InvalidJson`]，读取仍未完成）。
    pub fn fail_with_details(
        &self,
        kind: ErrorKind,
        message: &str,
        details_json: Option<&str>,
    ) -> Result<(), NativeError> {
        self.inner.finish(Err(tool_error_with_details(kind, message, details_json)?))
    }
    /// 以 `USER_ACTION_REQUIRED` 失败完成；语义同 [`CallHandle::fail_user_action`]（`None` 的字段不出现在错误的 `data` 中）。
    pub fn fail_user_action(
        &self,
        message: &str,
        reason: Option<&str>,
        uri: Option<&str>,
    ) -> Result<(), NativeError> {
        self.inner
            .finish(Err(ToolError::user_action_required(message, reason, uri)))
    }
}

/// 带可选详情（JSON 文本）的错误；`details_json` 非法时返回 [`NativeError::InvalidJson`]。
fn tool_error_with_details(
    kind: ErrorKind,
    message: &str,
    details_json: Option<&str>,
) -> Result<ToolError, NativeError> {
    let err = ToolError::new(kind, message);
    let Some(text) = details_json else {
        return Ok(err);
    };
    let details: Value =
        serde_json::from_str(text).map_err(|e| NativeError::InvalidJson(e.to_string()))?;
    Ok(err.with_details(details))
}

impl NavigateHandle {
    /// 目标页面名。
    pub fn page(&self) -> String {
        self.inner.page.clone()
    }
    /// 页面参数（JSON 文本）；Host 没有给出时为 `None`。
    pub fn params_json(&self) -> Option<String> {
        self.inner.params_json.clone()
    }
    /// 导航完成。
    pub fn complete(&self) -> Result<(), NativeError> {
        self.inner.finish(Ok(()))
    }
    /// 导航失败（`NAVIGATION_FAILED`，`data.reason` = `error`）：页面不存在、参数不合法等。
    pub fn fail(&self, message: &str) -> Result<(), NativeError> {
        self.inner.finish(Err(ToolError::navigation_failed(message, navigation_reason::ERROR)))
    }
    /// 拒绝本次导航（`NAVIGATION_DENIED`）：如用户正在输入。`message` 面向模型 / 用户。
    pub fn deny(&self, message: &str) -> Result<(), NativeError> {
        self.inner.finish(Err(ToolError::navigation_denied(message)))
    }
    /// 需要用户本人操作（`USER_ACTION_REQUIRED`，spec/protocol.md 3.4）：如 App 在后台、已发通知请用户点开
    /// （`reason` 取 `foreground`，`uri` 为该页面的 App 内入口）。语义同 [`CallHandle::fail_user_action`]。
    pub fn fail_user_action(&self, message: &str, reason: Option<&str>, uri: Option<&str>) -> Result<(), NativeError> {
        self.inner.finish(Err(ToolError::user_action_required(message, reason, uri)))
    }
}

impl ToolHandle {
    pub fn name(&self) -> String {
        self.inner.name.clone()
    }
    /// 用新定义整体替换（名称不可变，`spec.name` 被忽略）。已声明的 [`ToolOptions`] 保持不变。
    pub fn update(&self, spec: ToolSpec) -> Result<(), NativeError> {
        self.apply(spec_update(spec)?)
    }
    /// 用新定义与选项整体替换（选项中的 `None` 表示清除该声明）。
    pub fn update_with(&self, spec: ToolSpec, options: ToolOptions) -> Result<(), NativeError> {
        let output_schema = parse_output_schema(options.output_schema_json.as_deref())?;
        self.apply(ToolUpdate {
            annotations: Some(options.annotations),
            output_schema: Some(output_schema),
            surface: Some(options.surface),
            page: Some(options.page),
            background_tool: Some(options.background_tool),
            implements: Some(options.implements),
            cache: Some(options.cache),
            concurrency: Some(options.concurrency),
            exclusive: Some(options.exclusive),
            ..spec_update(spec)?
        })
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<(), NativeError> {
        self.apply(ToolUpdate {
            enabled: Some(enabled),
            ..ToolUpdate::default()
        })
    }
    pub fn dispose(&self) {
        if self.inner.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.tools.remove(&self.inner.id).is_some() {
            let _ = st.client.unregister_tool(self.inner.id);
        }
        drop(st);
        shared.wake();
    }

    fn apply(&self, update: ToolUpdate) -> Result<(), NativeError> {
        if self.inner.disposed.load(Ordering::SeqCst) {
            return Err(NativeError::Disposed);
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        if !st.tools.contains_key(&self.inner.id) {
            return Err(NativeError::Disposed);
        }
        st.client
            .update_tool(self.inner.id, update)
            .map_err(core_error)?;
        drop(st);
        shared.wake();
        Ok(())
    }
}

impl ResourceHandle {
    pub fn name(&self) -> String {
        self.inner.name.clone()
    }
    pub fn notify_changed(&self) -> Result<(), NativeError> {
        if self.inner.disposed.load(Ordering::SeqCst) {
            return Err(NativeError::Disposed);
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.stopped {
            return Err(NativeError::Stopped);
        }
        if !st.resources.contains_key(&self.inner.id) {
            return Err(NativeError::Disposed);
        }
        st.client
            .notify_resource_changed(self.inner.id, now_ms())
            .map_err(core_error)?;
        drop(st);
        shared.wake();
        Ok(())
    }
    pub fn dispose(&self) {
        if self.inner.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        if st.resources.remove(&self.inner.id).is_some() {
            let _ = st.client.unregister_resource(self.inner.id);
        }
        drop(st);
        shared.wake();
    }
}

impl ScopeHandle {
    pub fn register_tool(
        &self,
        spec: ToolSpec,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<ToolHandle, NativeError> {
        self.register_tool_with(spec, ToolOptions::default(), handler)
    }
    /// 同 [`ScopeHandle::register_tool`]，另带工具选项（MCP 注解、输出 schema）。
    pub fn register_tool_with(
        &self,
        spec: ToolSpec,
        options: ToolOptions,
        handler: Arc<dyn ToolHandler>,
    ) -> Result<ToolHandle, NativeError> {
        self.check()?;
        self.inner
            .shared
            .register_tool(Some(self.inner.id), spec, options, handler)
    }
    pub fn register_resource(
        &self,
        spec: ResourceSpec,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<ResourceHandle, NativeError> {
        self.register_resource_with(spec, ResourceOptions::default(), reader)
    }
    /// 同 [`ScopeHandle::register_resource`]，另带资源选项（如 `realtime`）。
    pub fn register_resource_with(
        &self,
        spec: ResourceSpec,
        options: ResourceOptions,
        reader: Arc<dyn ResourceReader>,
    ) -> Result<ResourceHandle, NativeError> {
        self.check()?;
        self.inner
            .shared
            .register_resource(Some(self.inner.id), spec, options, reader)
    }
    pub fn create_scope(&self, name: &str) -> Result<ScopeHandle, NativeError> {
        self.check()?;
        self.inner.shared.create_scope(Some(self.inner.id), name)
    }
    pub fn dispose(&self) {
        if self.inner.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = &self.inner.shared;
        let mut st = shared.lock();
        st.dispose_scope(self.inner.id);
        drop(st);
        shared.wake();
    }

    fn check(&self) -> Result<(), NativeError> {
        if self.inner.disposed.load(Ordering::SeqCst) {
            Err(NativeError::Disposed)
        } else {
            Ok(())
        }
    }
}
