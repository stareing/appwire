//! 句柄对象：[`Call`]、[`Hold`]、[`Read`]、[`Navigate`]、[`Tool`]、[`Resource`]、[`Scope`]。

use std::sync::Arc;

use app_mcp_native as native;

use crate::callbacks::{CancelListener, CancelListenerAdapter, ResourceReader, ResourceReaderAdapter, ToolHandler, ToolHandlerAdapter};
use crate::error::{AppMcpError, parse_error_kind};
use crate::records::{CallResult, ResourceSpec, ToolSpec};

/// 一次工具调用。可跨线程传递；`complete` / `fail` 只能成功一次。
#[derive(Debug, uniffi::Object)]
pub struct Call {
    pub(crate) inner: native::CallHandle,
}

#[uniffi::export]
impl Call {
    pub fn call_id(&self) -> String {
        self.inner.call_id()
    }
    pub fn tool_name(&self) -> String {
        self.inner.tool_name()
    }
    /// 已由 Host 按 inputSchema 校验过的参数（JSON 对象文本）。
    pub fn arguments_json(&self) -> String {
        self.inner.arguments_json()
    }
    /// Agent 给出的幂等键（原样，spec/protocol.md 3.3）；没有时为 `None`。App 决定如何使用（如作为业务去重键）。
    pub fn idempotency_key(&self) -> Option<String> {
        self.inner.idempotency_key()
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    /// 设置取消监听。已取消时立即（在当前线程）回调一次。
    pub fn set_cancel_listener(&self, listener: Arc<dyn CancelListener>) {
        self.inner
            .set_cancel_listener(Arc::new(CancelListenerAdapter(listener)));
    }
    /// 成功完成。`data_json` 为空表示 `null`；非法 JSON 返回 `InvalidJson`（调用仍未完成）。
    pub fn complete(
        &self,
        data_json: Option<String>,
        state_hints: Vec<String>,
    ) -> Result<(), AppMcpError> {
        Ok(self.inner.complete(data_json.as_deref(), state_hints)?)
    }
    /// 以完整结果成功完成（业务状态、摘要、内容标注）。非法 JSON 返回 `InvalidJson`（调用仍未完成）。
    pub fn complete_with(&self, result: CallResult) -> Result<(), AppMcpError> {
        Ok(self.inner.complete_with(result.into())?)
    }
    /// 失败完成。`kind` 为错误类别字符串（如 `"HANDLER_ERROR"`），未知类别返回 `UnknownErrorKind`。
    pub fn fail(&self, kind: String, message: String) -> Result<(), AppMcpError> {
        let kind = parse_error_kind(&kind)?;
        Ok(self.inner.fail(kind, &message)?)
    }
    /// 失败完成并附带结构化详情（JSON 文本；对象的字段合并进错误的 `data`，其他值放在 `data.details`）。
    /// `details_json` 为空等同于 `fail`；非法 JSON 返回 `InvalidJson`（调用仍未完成）。
    pub fn fail_with_details(
        &self,
        kind: String,
        message: String,
        details_json: Option<String>,
    ) -> Result<(), AppMcpError> {
        let kind = parse_error_kind(&kind)?;
        Ok(self
            .inner
            .fail_with_details(kind, &message, details_json.as_deref())?)
    }
    /// 以 `USER_ACTION_REQUIRED` 失败完成（spec/protocol.md 第 4 节）：需要用户本人操作后才能继续。
    /// `reason` 建议取 `login` / `permission` / `foreground` / `confirm`；`uri` 为 App 内入口。为空的字段不出现在错误的 `data` 中。
    pub fn fail_user_action(
        &self,
        message: String,
        reason: Option<String>,
        uri: Option<String>,
    ) -> Result<(), AppMcpError> {
        Ok(self
            .inner
            .fail_user_action(&message, reason.as_deref(), uri.as_deref())?)
    }
    /// 报告进度（spec/protocol.md 3.3）：Host 合并后转发给 Agent。`progress` 应递增，`total` 未知时为 `None`。
    /// 未连接时丢弃；调用已结束时返回 `AlreadyCompleted`。
    pub fn report_progress(
        &self,
        progress: f64,
        total: Option<f64>,
        message: Option<String>,
    ) -> Result<(), AppMcpError> {
        Ok(self.inner.report_progress(progress, total, message.as_deref())?)
    }
    /// 延长持有：调用完成后仍阻止自动休眠（handler 发起的长任务），直到返回的 `Hold` 被释放。
    /// 调用已结束时返回 `AlreadyCompleted`。
    pub fn hold(&self) -> Result<Arc<Hold>, AppMcpError> {
        Ok(Arc::new(Hold {
            inner: self.inner.hold()?,
        }))
    }
}

/// 阻止自动休眠的持有。`release` 幂等；对象被外部语言释放时自动释放。
#[derive(Debug, uniffi::Object)]
pub struct Hold {
    pub(crate) inner: native::HoldHandle,
}

#[uniffi::export]
impl Hold {
    /// 释放持有。重复调用无效果。
    pub fn release(&self) {
        self.inner.release()
    }
}

/// 一次资源读取。完成只能一次。
#[derive(Debug, uniffi::Object)]
pub struct Read {
    pub(crate) inner: native::ReadHandle,
}

#[uniffi::export]
impl Read {
    pub fn resource_name(&self) -> String {
        self.inner.resource_name()
    }
    /// 以 JSON 文本完成读取。
    pub fn complete(&self, contents_json: String) -> Result<(), AppMcpError> {
        Ok(self.inner.complete(&contents_json)?)
    }
    pub fn fail(&self, kind: String, message: String) -> Result<(), AppMcpError> {
        let kind = parse_error_kind(&kind)?;
        Ok(self.inner.fail(kind, &message)?)
    }
    /// 失败完成并附带结构化详情；语义同 [`Call::fail_with_details`]（非法 JSON 返回 `InvalidJson`，读取仍未完成）。
    pub fn fail_with_details(
        &self,
        kind: String,
        message: String,
        details_json: Option<String>,
    ) -> Result<(), AppMcpError> {
        let kind = parse_error_kind(&kind)?;
        Ok(self
            .inner
            .fail_with_details(kind, &message, details_json.as_deref())?)
    }
    /// 以 `USER_ACTION_REQUIRED` 失败完成；语义同 [`Call::fail_user_action`]。
    pub fn fail_user_action(
        &self,
        message: String,
        reason: Option<String>,
        uri: Option<String>,
    ) -> Result<(), AppMcpError> {
        Ok(self
            .inner
            .fail_user_action(&message, reason.as_deref(), uri.as_deref())?)
    }
}

/// 一次导航请求（[`NavigationHandler::navigate`]）。完成只能一次。
#[derive(Debug, uniffi::Object)]
pub struct Navigate {
    pub(crate) inner: native::NavigateHandle,
}

#[uniffi::export]
impl Navigate {
    /// 目标页面名。
    pub fn page(&self) -> String {
        self.inner.page()
    }
    /// 页面参数 JSON 文本；Host 没有给出时为空。
    pub fn params_json(&self) -> Option<String> {
        self.inner.params_json()
    }
    /// 导航完成。
    pub fn complete(&self) -> Result<(), AppMcpError> {
        Ok(self.inner.complete()?)
    }
    /// 导航失败（`NAVIGATION_FAILED`）：页面不存在、参数不合法等。
    pub fn fail(&self, message: String) -> Result<(), AppMcpError> {
        Ok(self.inner.fail(&message)?)
    }
    /// 拒绝导航（`NAVIGATION_DENIED`），如用户正在输入。
    pub fn deny(&self, message: String) -> Result<(), AppMcpError> {
        Ok(self.inner.deny(&message)?)
    }
    /// 以 `USER_ACTION_REQUIRED` 失败完成，如 App 在后台无法自行前置界面、已发通知请用户点开
    /// （`reason` 取 `foreground`，`uri` 为该页面的 App 内入口）；语义同 [`Call::fail_user_action`]。
    pub fn fail_user_action(
        &self,
        message: String,
        reason: Option<String>,
        uri: Option<String>,
    ) -> Result<(), AppMcpError> {
        Ok(self
            .inner
            .fail_user_action(&message, reason.as_deref(), uri.as_deref())?)
    }
}

/// 已注册的工具。`dispose` 幂等；丢弃对象**不会**注销工具。
#[derive(Debug, uniffi::Object)]
pub struct Tool {
    pub(crate) inner: native::ToolHandle,
}

#[uniffi::export]
impl Tool {
    pub fn name(&self) -> String {
        self.inner.name()
    }
    /// 用新定义整体替换（名称不可变，`spec.name` 被忽略）。`annotations` / `output_schema_json` / `cache` 为空表示清除该声明。
    pub fn update(&self, spec: ToolSpec) -> Result<(), AppMcpError> {
        let (spec, options) = spec.into();
        Ok(self.inner.update_with(spec, options)?)
    }
    pub fn set_enabled(&self, enabled: bool) -> Result<(), AppMcpError> {
        Ok(self.inner.set_enabled(enabled)?)
    }
    pub fn dispose(&self) {
        self.inner.dispose()
    }
}

/// 已注册的资源。`dispose` 幂等。
#[derive(Debug, uniffi::Object)]
pub struct Resource {
    pub(crate) inner: native::ResourceHandle,
}

#[uniffi::export]
impl Resource {
    pub fn name(&self) -> String {
        self.inner.name()
    }
    pub fn notify_changed(&self) -> Result<(), AppMcpError> {
        Ok(self.inner.notify_changed()?)
    }
    pub fn dispose(&self) {
        self.inner.dispose()
    }
}

/// Scope：注销时递归注销其下所有工具、资源与子 scope。`dispose` 幂等。
#[derive(Debug, uniffi::Object)]
pub struct Scope {
    pub(crate) inner: native::ScopeHandle,
}

#[uniffi::export]
impl Scope {
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
    pub fn dispose(&self) {
        self.inner.dispose()
    }
}
