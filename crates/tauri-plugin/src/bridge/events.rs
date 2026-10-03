//! 页面声明的事件（spec/protocol.md 3.5，op `event.declare` / `event.remove` / `event.emit`）：按页面记录，转给客户端。
//!
//! 页面注销（刷新、卸载、窗口销毁）时撤销其声明；多个页面声明同名事件时，最后一个页面撤销才撤销客户端上的声明，
//! 其余页面仍声明时以其中一页的声明重新声明。
//!
//! @why Rust 侧代码也可直接 `declare_event` 同名事件：页面全部撤销时会一并撤销它（同名即同一事件，客户端只有一份声明）。

use super::*;

impl Session {
    /// @error 名称不合法 → `INVALID_NAME`；会话已注销 → `DISPOSED`。
    pub(super) fn declare_event(&self, client: &NativeClient, event: EventInfo) -> Result<(), OpError> {
        let mut st = self.live()?;
        client.declare_event(event.clone())?;
        match st.events.iter_mut().find(|e| e.name == event.name) {
            Some(existing) => *existing = event,
            None => st.events.push(event),
        }
        Ok(())
    }

    /// 撤销本页的声明；返回本页是否声明过（之后由 [`Sessions::release_events`] 决定是否撤销客户端上的声明）。
    pub(super) fn remove_event(&self, name: &str) -> bool {
        let mut st = lock(&self.state);
        let before = st.events.len();
        st.events.retain(|e| e.name != name);
        st.events.len() != before
    }

    /// @error 未声明 / 名称不合法 → `INVALID_NAME`；载荷不是对象或超过 8 KiB → `INVALID_JSON`。
    pub(super) fn emit_event(client: &NativeClient, name: &str, payload: Option<&Value>) -> Result<bool, OpError> {
        let payload = payload.map(Value::to_string);
        Ok(client.emit_event(name, payload.as_deref())?)
    }

    /// 本页（未注销时）对该事件的声明。
    fn declared_event(&self, name: &str) -> Option<EventInfo> {
        let st = lock(&self.state);
        if st.disposed {
            return None;
        }
        st.events.iter().find(|e| e.name == name).cloned()
    }
}

impl Sessions {
    /// 页面不再声明这些事件：仍有其他页面声明的按其声明重新声明，否则撤销客户端上的声明。
    pub(super) fn release_events(&self, names: Vec<String>) {
        if names.is_empty() {
            return;
        }
        let sessions: Vec<Arc<Session>> = lock(&self.map).values().cloned().collect();
        let client = lock(&self.client);
        let Some(client) = client.as_ref() else { return };
        for name in names {
            match sessions.iter().find_map(|s| s.declared_event(&name)) {
                // 声明来自页面且已校验过，失败只可能是客户端已停止
                Some(info) => {
                    let _ = client.declare_event(info);
                }
                None => {
                    client.remove_event(&name);
                }
            }
        }
    }
}
