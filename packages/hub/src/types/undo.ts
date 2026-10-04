/** @app-mcp/hub 的公开类型：撤销（第 15 项 X2，spec/hub-api.md 3.23）。 */

/**
 * 本次调用已登记撤销（`CallOutcome.undo`，与 MCP 结果 `_meta` 的 `dev.appwire/undo` 相同）：可用 `apps.undo` 撤销（只能撤销一次）。
 */
export interface UndoOffer {
  /** App 给出的撤销说明；未给出时缺省。 */
  label?: string
  /** 距记录过期的毫秒数（登记时刻起算）。 */
  expiresInMs: number
}

/** 撤销记录的上限（`HubConfig.undo`）。每项可选，缺省取默认值；只在内存，Hub 重启清空。 */
export interface UndoConfig {
  /** 记录自登记起的有效期（毫秒），缺省 1800000（30 分钟）；撤销开启时须大于 0，否则 `Hub.start` 失败。 */
  ttlMs?: number
  /** 每个 Agent 任务保留的记录数，缺省 32，超出时丢最早的一条；`0` 关闭撤销（不登记、不列出 `apps.undo`）。 */
  maxPerTask?: number
}

/** 撤销的生效上限与当前记录数（`HubStatus.undo`）。 */
export interface UndoStatus {
  /** 记录自登记起的有效期（毫秒）。 */
  ttlMs: number
  /** 每个 Agent 任务保留的记录数上限；`0` = 撤销已关闭。 */
  maxPerTask: number
  /** 各任务登记的记录数合计（含尚未惰性丢弃的过期记录）。 */
  records: number
}
