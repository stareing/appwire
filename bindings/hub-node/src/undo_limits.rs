//! 配置 `undo`（spec/hub-api.md 3.23）：撤销记录上限的可选覆盖。

use app_mcp_hub::UndoLimits;
use serde::Deserialize;

/// [`UndoLimits`] 的可选覆盖（camelCase）；缺省字段沿用默认值（30 分钟 / 每任务 32 条）。`maxPerTask: 0` 关闭撤销。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UndoLimitOverrides {
    /// 记录保留的毫秒数；撤销开启时须大于 0（`Hub.start` 校验）。
    ttl_ms: Option<u64>,
    /// 每个任务保留的记录数；`0` = 关闭撤销。
    max_per_task: Option<usize>,
}

impl UndoLimitOverrides {
    pub(crate) fn apply(&self, target: &mut UndoLimits) {
        *target = UndoLimits::with_overrides(self.ttl_ms, self.max_per_task);
    }
}
