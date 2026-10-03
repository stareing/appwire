/**
 * 事件（spec/protocol.md 3.5）：JS 声明 / 载荷 → 原生参数。校验（名称、是否声明、载荷是否为对象、8 KiB 上限）在原生核心，
 * 这里只负责序列化；无法序列化时抛出与原生同类的 `INVALID_JSON`。
 */

import type { NativeEventSpec } from '../native.js'
import type { EventDefinition } from '../types.js'

function invalidJson(message: string): Error & { code: 'INVALID_JSON' } {
  return Object.assign(new Error(message), { code: 'INVALID_JSON' as const })
}

function stringify(what: string, value: unknown): string {
  let json: string | undefined
  try {
    json = JSON.stringify(value)
  } catch (error) {
    throw invalidJson(`${what}无法序列化为 JSON：${error instanceof Error ? error.message : String(error)}`)
  }
  if (json === undefined) throw invalidJson(`${what}无法序列化为 JSON`)
  return json
}

export function toNativeEventSpec(event: EventDefinition): NativeEventSpec {
  return {
    name: event.name,
    description: event.description,
    ...(event.payloadSchema !== undefined && {
      payloadSchemaJson: stringify(`事件 ${event.name} 的 payloadSchema `, event.payloadSchema),
    }),
  }
}

/** 载荷 → JSON 文本；`undefined` → `null`（无载荷）。是否为对象由原生核心判断。 */
export function encodePayload(name: string, payload: unknown): string | null {
  return payload === undefined ? null : stringify(`事件 ${name} 的载荷`, payload)
}
