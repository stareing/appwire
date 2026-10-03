/**
 * 事件（spec/protocol.md 3.5）的页面侧声明表与发出前校验。驱动层（核心加载前也要能声明 / 校验）与桥接实现
 * （Electron / Tauri 页面侧）共用；错误码与原生层（@app-mcp/node）一致：名称不合法或未声明为 `INVALID_NAME`，
 * 载荷不合法为 `INVALID_JSON`。核心在其后再做同样的校验，以核心为准。
 */

import type { EventDefinition } from './types'

/** 载荷序列化后的上限（与协议 `MAX_EVENT_PAYLOAD_BYTES` 相同）。 */
export const MAX_EVENT_PAYLOAD_BYTES = 8 * 1024

const EVENT_NAME_RE = /^[a-zA-Z0-9_.-]{1,64}$/

export type EventErrorCode = 'INVALID_NAME' | 'INVALID_SCHEMA' | 'INVALID_JSON'

/** 规范化后的事件声明（与协议 `EventInfo` 同形，可直接 JSON 化）。 */
export interface EventInfo {
  name: string
  description: string
  payloadSchema?: Record<string, unknown>
}

function eventError(code: EventErrorCode, message: string): Error & { code: EventErrorCode } {
  return Object.assign(new Error(message), { code })
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function checkEventName(name: unknown): asserts name is string {
  if (typeof name !== 'string' || !EVENT_NAME_RE.test(name)) {
    throw eventError('INVALID_NAME', `无效的事件名 ${JSON.stringify(name)}：应匹配 [a-zA-Z0-9_.-]{1,64}`)
  }
}

/**
 * 校验并规范化声明。
 * @error 名称不合法 → `INVALID_NAME`；`payloadSchema` 不是 JSON 对象 → `INVALID_SCHEMA`。
 */
export function toEventInfo(event: EventDefinition): EventInfo {
  checkEventName(event.name)
  const { payloadSchema } = event
  if (payloadSchema !== undefined && !isPlainObject(payloadSchema)) {
    throw eventError('INVALID_SCHEMA', `事件 ${event.name} 的 payloadSchema 必须是 JSON 对象`)
  }
  return {
    name: event.name,
    description: String(event.description ?? ''),
    ...(payloadSchema !== undefined && { payloadSchema: JSON.parse(JSON.stringify(payloadSchema)) as Record<string, unknown> }),
  }
}

/**
 * 载荷 → JSON 文本（`undefined` = 无载荷）。
 * @error 不是 JSON 对象、无法序列化或序列化后超过 {@link MAX_EVENT_PAYLOAD_BYTES} → `INVALID_JSON`。
 */
export function encodeEventPayload(name: string, payload: unknown): string | undefined {
  if (payload === undefined) return undefined
  if (!isPlainObject(payload)) throw eventError('INVALID_JSON', `事件 ${name} 的载荷必须是 JSON 对象`)
  let json: string
  try {
    json = JSON.stringify(payload)
  } catch (e) {
    throw eventError('INVALID_JSON', `事件 ${name} 的载荷无法序列化为 JSON：${e instanceof Error ? e.message : String(e)}`)
  }
  const size = new TextEncoder().encode(json).length
  if (size > MAX_EVENT_PAYLOAD_BYTES) {
    throw eventError('INVALID_JSON', `事件 ${name} 的载荷为 ${size} 字节，超过上限 ${MAX_EVENT_PAYLOAD_BYTES} 字节`)
  }
  return json
}

/** 已声明的事件（按首次声明顺序，同名替换保持原位置）。 */
export class EventDeclarations {
  private readonly declared = new Map<string, EventInfo>()

  /** 声明或替换，返回规范化后的声明。@error 同 {@link toEventInfo}。 */
  declare(event: EventDefinition): EventInfo {
    const info = toEventInfo(event)
    this.declared.set(info.name, info)
    return info
  }

  remove(name: string): boolean {
    return this.declared.delete(name)
  }

  values(): EventInfo[] {
    return [...this.declared.values()]
  }

  /**
   * 发出前校验，返回载荷 JSON 文本。
   * @error 名称不合法或未声明 → `INVALID_NAME`；载荷不合法 → `INVALID_JSON`（{@link encodeEventPayload}）。
   */
  prepareEmit(name: string, payload: unknown): string | undefined {
    checkEventName(name)
    if (!this.declared.has(name)) {
      throw eventError('INVALID_NAME', `事件 ${JSON.stringify(name)} 未声明：请先 declareEvent`)
    }
    return encodeEventPayload(name, payload)
  }
}
