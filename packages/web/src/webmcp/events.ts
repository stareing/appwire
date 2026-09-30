/**
 * 标准事件 `ToolActivatedEvent` / `ToolCancelEvent`（原生不存在时使用）。
 */

export interface ToolEventInit extends EventInit {
  toolName?: string
}

/** 工具开始执行时在 modelContext 上派发（`toolactivated`）。 */
export class ToolActivatedEvent extends Event {
  readonly toolName: string
  constructor(type: string, init: ToolEventInit = {}) {
    super(type, init)
    this.toolName = init.toolName ?? ''
  }
}

/** 工具执行被取消时在 modelContext 上派发（`toolcancel`）。 */
export class ToolCancelEvent extends Event {
  readonly toolName: string
  constructor(type: string, init: ToolEventInit = {}) {
    super(type, init)
    this.toolName = init.toolName ?? ''
  }
}

type EventCtor = new (type: string, init?: ToolEventInit) => Event

/** 创建事件：优先使用全局（原生）构造函数。 */
export function createToolEvent(type: 'toolactivated' | 'toolcancel', toolName: string): Event {
  const globalName = type === 'toolactivated' ? 'ToolActivatedEvent' : 'ToolCancelEvent'
  const ctor = (globalThis as Record<string, unknown>)[globalName]
  const fallback: EventCtor = type === 'toolactivated' ? ToolActivatedEvent : ToolCancelEvent
  if (typeof ctor === 'function' && ctor !== fallback) {
    try {
      return new (ctor as EventCtor)(type, { toolName })
    } catch {
      // 退回自有实现
    }
  }
  return new fallback(type, { toolName })
}
