//! 句柄类：一次调用 / 读取 / 导航、阻止休眠的持有，以及已注册的工具、资源与 scope。

use std::sync::Arc;

use app_mcp_native as native;
use napi_derive::napi;

use super::WeakTsfn;
use super::callbacks::{JsCancelListener, JsResourceReader, JsToolHandler};
use super::convert::{parse_error_kind, to_js_error};
use super::objects::{CallResultInit, ResourceSpecInit, ToolSpecInit};

/// 本绑定抛出的错误：`status` 字符串成为 JS 错误的 `code`（napi-derive 按名称 `Result` 识别返回类型）。
use napi::Result;

/// 一次工具调用（由原生运行时创建，JS 不能构造）。
#[napi]
pub struct Call {
    pub(super) inner: native::CallHandle,
}

#[napi]
impl Call {
    #[napi(getter)]
    pub fn call_id(&self) -> String {
        self.inner.call_id()
    }

    #[napi(getter)]
    pub fn tool_name(&self) -> String {
        self.inner.tool_name()
    }

    /// 已由 Host 校验过的参数（JSON 对象文本）。
    #[napi(getter)]
    pub fn arguments_json(&self) -> String {
        self.inner.arguments_json()
    }

    /// Agent 给出的幂等键（原样，spec/protocol.md 3.3）；没有时为 `null`。
    #[napi(getter)]
    pub fn idempotency_key(&self) -> Option<String> {
        self.inner.idempotency_key()
    }

    #[napi]
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    /// 设置取消监听：`listener(reason)`，reason 为 `'requested' | 'timeout' | 'disconnected' | 'stopped'`。
    /// 已取消时也会回调一次（经事件循环异步投递）。
    #[napi]
    pub fn set_cancel_listener(&self, listener: WeakTsfn<String>) {
        self.inner.set_cancel_listener(Arc::new(JsCancelListener { tsfn: listener }));
    }

    /// 成功完成。`dataJson` 为 `null` / 省略表示 `null`。
    #[napi]
    pub fn complete(&self, data_json: Option<String>, state_hints: Option<Vec<String>>) -> Result<(), String> {
        self.inner.complete(data_json.as_deref(), state_hints.unwrap_or_default()).map_err(to_js_error)
    }

    /// 成功完成，附带业务状态、摘要与内容标注（spec/protocol.md 3.2）。取值不合法时抛出 `INVALID_ARG`（调用仍未完成）。
    #[napi]
    pub fn complete_with(&self, result: CallResultInit) -> Result<(), String> {
        self.inner.complete_with(result.into_result()?).map_err(to_js_error)
    }

    /// 失败完成。`kind` 为协议错误类别（如 `'HANDLER_ERROR'`）。
    #[napi]
    pub fn fail(&self, kind: String, message: String) -> Result<(), String> {
        let kind = parse_error_kind(&kind)?;
        self.inner.fail(kind, &message).map_err(to_js_error)
    }

    /// 失败完成并附带结构化详情（JSON 文本；对象的字段合并进错误的 `data`，其他值放在 `data.details`）。
    /// `detailsJson` 为 `null` / 省略时等同于 `fail`。
    #[napi]
    pub fn fail_with_details(&self, kind: String, message: String, details_json: Option<String>) -> Result<(), String> {
        let kind = parse_error_kind(&kind)?;
        self.inner.fail_with_details(kind, &message, details_json.as_deref()).map_err(to_js_error)
    }

    /// 报告进度（spec/protocol.md 3.3）：Host 合并后转发给 Agent。`progress` 应递增，`total` 未知时省略。
    /// 未连接时丢弃；调用已结束时抛出 `ALREADY_COMPLETED`。
    #[napi]
    pub fn report_progress(&self, progress: f64, total: Option<f64>, message: Option<String>) -> Result<(), String> {
        self.inner.report_progress(progress, total, message.as_deref()).map_err(to_js_error)
    }

    /// 调用完成后仍阻止自动休眠（handler 发起的长任务），直到返回的 `Hold` 被 `release()`。
    /// 调用已结束时抛出 `ALREADY_COMPLETED`。
    #[napi]
    pub fn hold(&self) -> Result<Hold, String> {
        let inner = self.inner.hold().map_err(to_js_error)?;
        Ok(Hold { inner })
    }
}

/// 阻止自动休眠的持有。`release()` 幂等；JS 对象被垃圾回收时也会释放（不要依赖这一点）。
#[napi]
pub struct Hold {
    pub(super) inner: native::HoldHandle,
}

#[napi]
impl Hold {
    #[napi]
    pub fn release(&self) {
        self.inner.release();
    }
}

/// 一次资源读取。
#[napi]
pub struct Read {
    pub(super) inner: native::ReadHandle,
}

#[napi]
impl Read {
    #[napi(getter)]
    pub fn resource_name(&self) -> String {
        self.inner.resource_name()
    }

    #[napi]
    pub fn complete(&self, contents_json: String) -> Result<(), String> {
        self.inner.complete(&contents_json).map_err(to_js_error)
    }

    #[napi]
    pub fn fail(&self, kind: String, message: String) -> Result<(), String> {
        let kind = parse_error_kind(&kind)?;
        self.inner.fail(kind, &message).map_err(to_js_error)
    }

    /// 失败完成并附带结构化详情；语义同 `Call.failWithDetails`（`USER_ACTION_REQUIRED` 的 `reason` / `uri` 放在详情对象中）。
    #[napi]
    pub fn fail_with_details(&self, kind: String, message: String, details_json: Option<String>) -> Result<(), String> {
        let kind = parse_error_kind(&kind)?;
        self.inner.fail_with_details(kind, &message, details_json.as_deref()).map_err(to_js_error)
    }
}

/// 一次导航请求（Host 的 `app/navigate`，spec/protocol.md 3.4）。完成只能一次。
#[napi]
pub struct Navigate {
    pub(super) inner: native::NavigateHandle,
}

#[napi]
impl Navigate {
    #[napi(getter)]
    pub fn page(&self) -> String {
        self.inner.page()
    }

    /// 页面参数 JSON 文本；Host 没有给出时为 `undefined`。
    #[napi(getter)]
    pub fn params_json(&self) -> Option<String> {
        self.inner.params_json()
    }

    #[napi]
    pub fn complete(&self) -> Result<(), String> {
        self.inner.complete().map_err(to_js_error)
    }

    /// 导航失败（`NAVIGATION_FAILED`）。
    #[napi]
    pub fn fail(&self, message: String) -> Result<(), String> {
        self.inner.fail(&message).map_err(to_js_error)
    }

    /// 拒绝导航（`NAVIGATION_DENIED`）。
    #[napi]
    pub fn deny(&self, message: String) -> Result<(), String> {
        self.inner.deny(&message).map_err(to_js_error)
    }

    /// 需要用户操作（`USER_ACTION_REQUIRED`）：如 App 在后台、已发通知请用户点开（`reason` 常为 `foreground`，
    /// `uri` 为 App 内入口）。
    #[napi]
    pub fn fail_user_action(&self, message: String, reason: Option<String>, uri: Option<String>) -> Result<(), String> {
        self.inner.fail_user_action(&message, reason.as_deref(), uri.as_deref()).map_err(to_js_error)
    }
}

/// 已注册的工具。
#[napi]
pub struct Tool {
    pub(super) inner: native::ToolHandle,
}

#[napi]
impl Tool {
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.name()
    }

    /// 整体替换定义（`spec.name` 被忽略）。`annotations` / `outputSchemaJson` 被忽略、已声明的保持不变
    /// （旧行为；要一并替换用 `updateWith`）。
    #[napi]
    pub fn update(&self, spec: ToolSpecInit) -> Result<(), String> {
        self.inner.update(spec.into_spec()?).map_err(to_js_error)
    }

    /// 整体替换定义与选项（`spec.name` 被忽略）：`annotations` / `outputSchemaJson` 未给出表示清除该声明。
    #[napi]
    pub fn update_with(&self, spec: ToolSpecInit) -> Result<(), String> {
        let (spec, options) = spec.into_parts()?;
        self.inner.update_with(spec, options).map_err(to_js_error)
    }

    #[napi]
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.inner.set_enabled(enabled).map_err(to_js_error)
    }

    #[napi]
    pub fn dispose(&self) {
        self.inner.dispose();
    }
}

/// 已注册的资源。
#[napi]
pub struct Resource {
    pub(super) inner: native::ResourceHandle,
}

#[napi]
impl Resource {
    #[napi(getter)]
    pub fn name(&self) -> String {
        self.inner.name()
    }

    #[napi]
    pub fn notify_changed(&self) -> Result<(), String> {
        self.inner.notify_changed().map_err(to_js_error)
    }

    #[napi]
    pub fn dispose(&self) {
        self.inner.dispose();
    }
}

/// Scope：注销时递归注销其下全部工具、资源与子 scope。
#[napi]
pub struct Scope {
    pub(super) inner: native::ScopeHandle,
}

#[napi]
impl Scope {
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

    #[napi]
    pub fn dispose(&self) {
        self.inner.dispose();
    }
}
