/** @app-mcp/hub 的公开类型：App 事件、订阅与信箱（spec/hub-api.md 3.17）。 */

/**
 * 事件信箱上限（`HubConfig.eventLimits`）。每项可选，缺省取默认值；超出时的行为见各字段。
 */
export interface EventLimitsConfig {
  /** 每个订阅方最多的订阅数，缺省 32；超出时 `apps.events.subscribe` 报 `RATE_LIMITED`（`details.scope = 'events'`）。 */
  maxSubscriptions?: number
  /** 每个信箱最多的事件数，缺省 100（至少按 1 处理）；满时丢最旧并计入 `dropped`。 */
  maxInboxEvents?: number
  /** 信箱中事件的保留时长（毫秒），缺省 86400000（24 小时）；过期的在下次读写该信箱时清理。 */
  inboxTtlMs?: number
  /** 每个订阅每分钟（滑动窗口）最多入箱的事件数，缺省 60；0 不限。 */
  perSubscriptionPerMinute?: number
}

/** App 发出、经 Hub 去重与校验后的一个事件（{@link HubEvent} `appEvent`、{@link EventHandler}、内置工具 `apps.events`）。 */
export interface AppEvent {
  /** Hub 分配：`ev-<n>`。 */
  id: string
  appId: string
  instanceId: string
  /** 事件名（App 内的局部名，如 `order.shipped`）。 */
  name: string
  /** 载荷（JSON 对象）；App 未给出时缺省。 */
  payload?: Record<string, unknown>
  /** Hub 收到时的 Unix 毫秒。 */
  at: number
}

/**
 * 厂商 / 机主的事件回调（`Hub.setEventHandler`）：每个通过校验的事件（不论有无订阅）调用一次，在 Node 事件循环上执行。
 * 抛出的异常交给 `HubStartOptions.onListenerError`（缺省写 stderr），不影响投递。
 */
export type EventHandler = (event: AppEvent) => void

/** 事件订阅与丢弃统计（{@link HubStatus.events}）。 */
export interface EventsStatus {
  /** 全部订阅，按订阅方、订阅 ID 排序。 */
  subscriptions: EventSubscriptionStatus[]
  /** 因未声明、载荷不合法或超限而丢弃的事件数（启动以来）。 */
  droppedInvalid: number
}

/** 一个订阅的状态。 */
export interface EventSubscriptionStatus {
  subscriptionId: string
  /** 订阅方：`agent:<名>`，或匿名调用方的调用方键（`mcp:<n>` / `principal:local` / `api` 等）。 */
  subscriber: string
  appId: string
  /** 事件名；缺省 = 该 App 的全部事件。 */
  event?: string
  /** 经本订阅入箱的事件数。 */
  delivered: number
  /** 本订阅因频率上限丢弃的事件数。 */
  dropped: number
  /** 订阅方信箱当前的事件数。 */
  pending: number
}
