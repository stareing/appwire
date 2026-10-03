//! 事件（第 16 项 N3，spec/protocol.md 3.5）：App 声明可发出的事件、发出事件。Hub 只投递，不代 Agent 发起调用。

use super::*;

/// 事件载荷上限（字节，序列化后的 JSON）：SDK 拒绝发出、Hub 丢弃超限事件。
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 8 * 1024;

/// App 可发出的一种事件（清单 `events[]`、`events/sync`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventInfo {
    /// 事件名，规则同工具名（[`is_valid_name`]），如 `order.shipped`。
    pub name: String,
    /// 面向模型：事件何时发生、载荷含义。
    pub description: String,
    /// 载荷的 JSON Schema（描述用，Hub 不校验）。省略 = 无载荷或不描述。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_schema: Option<Value>,
}

/// `events/sync`（SDK → Host，通知）：全量声明本实例可发出的事件；握手成功后（有声明时）与声明变化时发送。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsSyncParams {
    pub events: Vec<EventInfo>,
}

/// `events/emit`（SDK → Host，通知）：发出一个已声明的事件。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventEmitParams {
    pub name: String,
    /// SDK 生成、进程内唯一（Hub 按 `(连接, eventId)` 去重；SDK 不重发事件，不跨连接去重）。
    pub event_id: String,
    /// 载荷（JSON 对象，序列化后不超过 [`MAX_EVENT_PAYLOAD_BYTES`]）。省略 = 无载荷。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}
