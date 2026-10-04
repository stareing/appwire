//! 撤销（spec/hub-api.md 3.23）：调用结果的撤销登记与状态记录。

use app_mcp_hub as hub;

/// `CallOutcome.undo`：本次调用已登记撤销，可用 `apps.undo` 撤销（同 MCP 结果 `_meta` `dev.appwire/undo`）。
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct UndoOffer {
    /// App 给出的撤销说明；未给出为空。
    #[uniffi(default = None)]
    pub label: Option<String>,
    /// 距记录过期的毫秒数（登记时刻起算）。
    pub expires_in_ms: u64,
}

impl From<hub::UndoOffer> for UndoOffer {
    fn from(o: hub::UndoOffer) -> Self {
        UndoOffer { label: o.label, expires_in_ms: o.expires_in_ms }
    }
}

/// `HubConfig.undo`：撤销记录上限的可选覆盖；缺省字段沿用默认值（30 分钟 / 每任务 32 条）。
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct UndoLimitOverrides {
    /// 记录自登记起的有效期（毫秒）；撤销开启时须大于 0，否则启动失败。
    #[uniffi(default = None)]
    pub ttl_ms: Option<u64>,
    /// 每个 Agent 任务保留的记录数；超出时丢最早的一条。`0` = 关闭撤销（不登记、不列出 `apps.undo`）。
    #[uniffi(default = None)]
    pub max_per_task: Option<u32>,
}

impl UndoLimitOverrides {
    pub(crate) fn apply(&self, target: &mut hub::UndoLimits) {
        *target = hub::UndoLimits::with_overrides(self.ttl_ms, self.max_per_task.map(|v| v as usize));
    }
}

/// `HubStatus.undo`：生效上限与当前记录数。
#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct UndoStatus {
    /// 记录有效期（毫秒）。
    pub ttl_ms: u64,
    /// 每个 Agent 任务保留的记录数上限；`0` = 撤销已关闭。
    pub max_per_task: u64,
    /// 各任务登记的记录数合计（含尚未惰性丢弃的过期记录）。
    pub records: u64,
}

impl From<hub::UndoStatus> for UndoStatus {
    fn from(s: hub::UndoStatus) -> Self {
        let to_u64 = |v: usize| u64::try_from(v).unwrap_or(u64::MAX);
        UndoStatus { ttl_ms: s.ttl_ms, max_per_task: to_u64(s.max_per_task), records: to_u64(s.records) }
    }
}
