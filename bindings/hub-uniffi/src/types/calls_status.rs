//! 调用对象（第 16 项 P5，`HubStatus.calls`；spec/hub-api.md 3.6「调用对象」）的 uniffi 记录。

use app_mcp_hub as hub;

use super::Visibility;

/// 进行中调用的阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CallState {
    /// 已受理：路由、策略与限流检查中。
    Created,
    /// 等待用户确认（审批）。
    Approving,
    /// 唤醒 App 或导航到工具所在页面中。
    Activating,
    /// 已交给 App / 上游 / 内置工具执行。
    Running,
}

impl From<hub::call_objects::CallState> for CallState {
    fn from(s: hub::call_objects::CallState) -> Self {
        match s {
            hub::call_objects::CallState::Created => CallState::Created,
            hub::call_objects::CallState::Approving => CallState::Approving,
            hub::call_objects::CallState::Activating => CallState::Activating,
            hub::call_objects::CallState::Running => CallState::Running,
        }
    }
}

/// 一个进行中的调用。
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct CallStatus {
    pub call_id: String,
    /// 工具全名。
    pub name: String,
    /// 调用方键；为空 = 未给出。
    #[uniffi(default = None)]
    pub caller: Option<String>,
    /// 调用方的记账主体：`agent:<名>` / `local` / `api`。
    pub subject: String,
    pub state: CallState,
    /// 从 Hub 受理起的毫秒数。
    pub elapsed_ms: u64,
    #[uniffi(default = None)]
    pub instance_id: Option<String>,
    /// 最近一次进度（已完成量、总量、说明）。
    #[uniffi(default = None)]
    pub progress: Option<f64>,
    #[uniffi(default = None)]
    pub progress_total: Option<f64>,
    #[uniffi(default = None)]
    pub progress_message: Option<String>,
    /// 诊断：执行实例最近上报的可见性。
    #[uniffi(default = None)]
    pub platform_state: Option<Visibility>,
}

impl From<hub::call_objects::CallStatus> for CallStatus {
    fn from(c: hub::call_objects::CallStatus) -> Self {
        let (progress, progress_total, progress_message) = match c.progress {
            Some(p) => (Some(p.progress), p.total, p.message),
            None => (None, None, None),
        };
        CallStatus {
            call_id: c.call_id,
            name: c.name,
            caller: c.caller,
            subject: c.subject,
            state: c.state.into(),
            elapsed_ms: c.elapsed_ms,
            instance_id: c.instance_id,
            progress,
            progress_total,
            progress_message,
            platform_state: c.platform_state.map(Into::into),
        }
    }
}
