//! Hub 公开数据类型（spec/hub-api.md 3.1、3.3）。
//!
//! 所有类型都实现 `Serialize` / `Deserialize`（camelCase），方便绑定层以 JSON 传递。

mod apps;
mod callbacks;
mod calls;
mod events;
mod status;
#[cfg(test)]
mod tests;
mod tools;

pub use apps::{AppInfo, AppKind, InstanceInfo};
pub use tools::{
    AppOverviewInfo, Availability, HubResource, HubTool, McpProtocolMode, ResourceContent, ToolExposure, ToolFilter,
    risk_rank, risk_str,
};
pub use calls::{CallOutcome, CallRequest, HubError};
pub use events::HubEvent;
pub use status::{
    AgentTaskStatus, AppState, AppStatus, AuthStatus, AwakeReason, DiagnosticReport, HubStatus, InstancePower,
    InstanceState, InstanceStatus, LastError, TaskLeaseStatus, TaskSelectionStatus, ToolDeclaration,
};
pub use callbacks::{ApprovalHandler, ApprovalPolicy, ApprovalRequest, PairingHandler, PairingRequest};

// ---------------------------------------------------------------------------
// serde 辅助
// ---------------------------------------------------------------------------

/// `Option<Duration>` ↔ 毫秒数。
mod opt_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(d) => s.serialize_some(&(d.as_millis() as u64)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        Ok(Option::<u64>::deserialize(d)?.map(Duration::from_millis))
    }
}

/// `Result<Value, ToolError>` ↔ `{"ok": value}` / `{"error": {kind, message, details}}`。
mod result_json {
    use app_mcp_protocol::{ErrorKind, ToolError};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    #[derive(Serialize, Deserialize)]
    struct ErrorJson {
        kind: ErrorKind,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    enum Repr {
        Ok(Value),
        Error(ErrorJson),
    }

    pub fn serialize<S: Serializer>(v: &Result<Value, ToolError>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Ok(v) => Repr::Ok(v.clone()),
            Err(e) => Repr::Error(ErrorJson {
                kind: e.kind,
                message: e.message.clone(),
                details: e.details.clone(),
            }),
        }
        .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Result<Value, ToolError>, D::Error> {
        Ok(match Repr::deserialize(d)? {
            Repr::Ok(v) => Ok(v),
            Repr::Error(e) => Err(ToolError {
                kind: e.kind,
                message: e.message,
                details: e.details,
            }),
        })
    }
}
