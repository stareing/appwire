//! 撤销（第 15 项 X2，spec/hub-api.md 3.23）：App 在调用结果中显式给出逆操作（spec/protocol.md 3.8），Hub 登记到调用方的
//! Agent 任务，`apps.undo` 时按 Agent 的请求转发为一次普通的 App 工具调用。Hub 不推断、不核对、不代 Agent 发起调用。
//!
//! - [`store`]：任务名下的撤销记录（纯状态，不读时钟）：条数上限、惰性 TTL、只能取出一次。
//! - [`hooks`]：接入调用（登记、`apps.undo` 转发），时钟取 `tokio::time::Instant`。
//!
//! @invariant 不新增定时器或线程：过期在取用 / 登记时惰性判定，记录随任务回收（CLAUDE.md「召之即来」）。

use std::time::Duration;

use serde::Serialize;

mod hooks;
mod store;
#[cfg(test)]
mod tests;

pub(crate) use store::{TaskUndo, UndoRecord};

/// [`UndoLimits::ttl`] 的默认值（30 分钟）。
///
/// @why 撤销通常紧跟操作（用户看到结果后说"撤销"）；30 分钟覆盖一轮对话的停顿，又不让过时的逆操作长期留存。
pub const DEFAULT_UNDO_TTL: Duration = Duration::from_secs(30 * 60);
/// [`UndoLimits::max_per_task`] 的默认值。
pub const DEFAULT_UNDO_MAX_PER_TASK: usize = 32;

/// 撤销记录的上限（`HubConfig::undo`，B-07）。只在内存，Hub 重启后记录丢失。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UndoLimits {
    /// 记录自登记起的有效期；过期的记录在取用时丢弃。
    pub ttl: Duration,
    /// 每个 Agent 任务保留的记录数上限；超出时丢弃最早的一条。`0` = 关闭撤销（不登记、不列出 `apps.undo`）。
    pub max_per_task: usize,
}

impl Default for UndoLimits {
    fn default() -> Self {
        Self { ttl: DEFAULT_UNDO_TTL, max_per_task: DEFAULT_UNDO_MAX_PER_TASK }
    }
}

impl UndoLimits {
    /// 撤销是否开启（`max_per_task > 0`）。
    pub fn enabled(&self) -> bool {
        self.max_per_task > 0
    }

    /// 由可选覆盖得到上限（Host 配置 `undo`、命令行共用）：未给的字段取默认值。
    pub fn with_overrides(ttl_ms: Option<u64>, max_per_task: Option<usize>) -> Self {
        Self {
            ttl: ttl_ms.map_or(DEFAULT_UNDO_TTL, Duration::from_millis),
            max_per_task: max_per_task.unwrap_or(DEFAULT_UNDO_MAX_PER_TASK),
        }
    }

    /// 校验上限（Hub 启动时，[`crate::Hub::start`]）；关闭撤销（`max_per_task == 0`）时不校验其余字段。
    ///
    /// @error 开启时 `ttl` 为 0；消息按配置形式（`undo.*`）称呼字段。
    pub fn validate(&self) -> Result<(), String> {
        if self.enabled() && self.ttl.is_zero() {
            return Err("undo.ttlMs 必须大于 0（关闭撤销请设 undo.maxPerTask 为 0）".to_owned());
        }
        Ok(())
    }
}

/// 登记成功时写进原调用结果 `_meta` `dev.appwire/undo` 的内容：`{label?, expiresInMs}`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UndoGrant {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub expires_in_ms: u64,
}
