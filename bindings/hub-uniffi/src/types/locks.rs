//! 对象锁（第 16 项 N6，`HubStatus.locks`；spec/hub-api.md 3.6「对象锁」）的 uniffi 记录。

use app_mcp_hub as hub;

/// 一把未到期的对象锁。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LockStatus {
    pub app_id: String,
    /// 命名锁的名字；App 锁为空。
    #[uniffi(default = None)]
    pub key: Option<String>,
    /// 持有者的调用方键（`mcp:<n>` / `principal:<主体>` / `api` / `api:<session>` 等）。
    pub caller: String,
    /// 持有者的记账主体：`agent:<名>` / `local` / `api`。
    pub holder: String,
    /// 距到期的毫秒数。
    pub expires_in_ms: u64,
}

impl From<hub::object_lock::LockStatus> for LockStatus {
    fn from(l: hub::object_lock::LockStatus) -> Self {
        LockStatus { app_id: l.app_id, key: l.key, caller: l.caller, holder: l.holder, expires_in_ms: l.expires_in_ms }
    }
}
