//! 事件（第 16 项 N3，spec/protocol.md 3.5）：运行时声明（`events/sync`）与发出（`events/emit`）。
//!
//! @invariant 未连接时发事件直接丢弃：不缓存、不触发连接、不动空闲计时（原则 4）。

use app_mcp_protocol as proto;
use proto::{EventEmitParams, EventInfo, EventsSyncParams, MAX_EVENT_PAYLOAD_BYTES, method};
use serde_json::Value;

use crate::{Client, CoreError};

/// 运行时声明的事件与 `eventId` 计数（跨连接保留）。
#[derive(Debug, Default)]
pub(crate) struct Events {
    /// 按首次声明顺序；同名替换保持原位置。
    declared: Vec<EventInfo>,
    /// 已发出的事件数，`eventId` = `e<序号>`。
    emitted: u64,
}

impl Events {
    /// 声明或替换；返回声明是否变化。
    fn declare(&mut self, info: EventInfo) -> bool {
        match self.declared.iter_mut().find(|e| e.name == info.name) {
            Some(existing) if *existing == info => false,
            Some(existing) => {
                *existing = info;
                true
            }
            None => {
                self.declared.push(info);
                true
            }
        }
    }

    fn remove(&mut self, name: &str) -> bool {
        let before = self.declared.len();
        self.declared.retain(|e| e.name != name);
        self.declared.len() != before
    }

    fn contains(&self, name: &str) -> bool {
        self.declared.iter().any(|e| e.name == name)
    }

    fn next_event_id(&mut self) -> String {
        self.emitted = self.emitted.saturating_add(1);
        format!("e{}", self.emitted)
    }

    fn sync_params(&self) -> EventsSyncParams {
        EventsSyncParams { events: self.declared.clone() }
    }
}

fn check_name(name: &str) -> Result<(), CoreError> {
    if proto::is_valid_name(name) { Ok(()) } else { Err(CoreError::InvalidName(name.to_owned())) }
}

/// @error 不是 JSON 对象，或序列化后超过 [`MAX_EVENT_PAYLOAD_BYTES`] → [`CoreError::InvalidEventPayload`]。
fn check_payload(payload: Option<&Value>) -> Result<(), CoreError> {
    let Some(value) = payload else { return Ok(()) };
    if !value.is_object() {
        return Err(CoreError::InvalidEventPayload("payload must be a json object".to_owned()));
    }
    let size = serde_json::to_vec(value).map_or(usize::MAX, |v| v.len());
    if size > MAX_EVENT_PAYLOAD_BYTES {
        return Err(CoreError::InvalidEventPayload(format!(
            "payload is {size} bytes, exceeds the limit of {MAX_EVENT_PAYLOAD_BYTES} bytes"
        )));
    }
    Ok(())
}

impl Client {
    pub(crate) fn on_declare_event(&mut self, info: EventInfo) -> Result<(), CoreError> {
        check_name(&info.name)?;
        if self.declared_events.declare(info) {
            self.resync_events();
        }
        Ok(())
    }

    pub(crate) fn on_remove_event(&mut self, name: &str) -> bool {
        let removed = self.declared_events.remove(name);
        if removed {
            self.resync_events();
        }
        removed
    }

    pub(crate) fn on_emit_event(&mut self, name: &str, payload: Option<Value>) -> Result<bool, CoreError> {
        check_name(name)?;
        if !self.declared_events.contains(name) {
            return Err(CoreError::UnknownEvent(name.to_owned()));
        }
        check_payload(payload.as_ref())?;
        if !self.connected() {
            return Ok(false);
        }
        let event_id = self.declared_events.next_event_id();
        self.notify(method::EVENTS_EMIT, &EventEmitParams { name: name.to_owned(), event_id, payload });
        Ok(true)
    }

    /// 握手成功后：有声明时发一次全量 `events/sync`。
    pub(crate) fn send_event_declarations(&mut self) {
        if !self.declared_events.declared.is_empty() {
            let params = self.declared_events.sync_params();
            self.notify(method::EVENTS_SYNC, &params);
        }
    }

    /// 已连接时声明变化：重发全量（含清空后的空列表）；未连接时等下次握手。
    fn resync_events(&mut self) {
        if self.connected() {
            let params = self.declared_events.sync_params();
            self.notify(method::EVENTS_SYNC, &params);
        }
    }
}
