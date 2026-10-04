/**
 * createAppMcp 的共用部分：状态映射、schema 解析、持有包装与失败回复辅助函数（自 client.ts 拆出）。
 */

import { toFailure } from '../errors.js'
import type { NativeCall, NativeNavigate, NativeStateInfo, NativeUndo } from '../native.js'
import { resolveInput, resolveOutput, type ResolvedInput } from '../schema.js'
import type { ConnectionState, HoldHandle, Logger, ToolDefinition, UndoAction } from '../types.js'

export const defaultLogger: Logger = {
  debug() {},
  warn: (message, ...args) => console.warn(message, ...args),
  error: (message, ...args) => console.error(message, ...args),
}

/** 可被父级（scope / 客户端）批量注销的子项。 */
export interface Child {
  /** 父级已在原生侧注销：只更新本地状态，不再调用原生 dispose。 */
  detach(): void
}

export interface Owner {
  readonly children: Set<Child>
}

export function mapState(info: NativeStateInfo): ConnectionState {
  switch (info.status) {
    case 'backoff':
      return {
        status: 'backoff',
        retryAt: Date.now() + (info.retryInMs ?? 0),
        ...(info.reason != null && { reason: info.reason }),
        ...(info.code != null && { code: info.code }),
      }
    case 'rejected':
      return { status: 'rejected', reason: info.reason ?? '', code: info.code ?? 'REJECTED' }
    case 'host-mismatch':
      return { status: 'host-mismatch', reason: info.reason ?? '', code: info.code ?? 'HOST_NOT_APP_MCP' }
    default:
      return { status: info.status }
  }
}

/** 输入与输出定义解析后的结果。 */
export interface ResolvedTool extends ResolvedInput {
  /** 输出 schema 的 JSON 文本；`undefined` 表示未声明。 */
  outputSchemaJson: string | undefined
}

/** 解析输入与输出定义：都能同步完成时同步返回（同步注册的错误直接抛给调用方），否则返回 Promise。 */
export function resolveTool(def: Pick<ToolDefinition<any, any>, 'input' | 'outputSchema'>): ResolvedTool | Promise<ResolvedTool> {
  const input = resolveInput(def.input)
  const output = resolveOutput(def.outputSchema)
  if (!(input instanceof Promise) && !(output instanceof Promise)) return { ...input, outputSchemaJson: output }
  return Promise.all([input, output]).then(([i, o]) => ({ ...i, outputSchemaJson: o }))
}

/** 空操作的持有（未启用 / 旧版原生模块）。 */
export const NOOP_HOLD: HoldHandle = Object.freeze({ release() {} })

/** 包装原生持有：`release` 幂等。 */
export function wrapHold(hold: { release(): void }): HoldHandle {
  let released = false
  return {
    release() {
      if (released) return
      released = true
      hold.release()
    },
  }
}

/**
 * 以失败完成调用或资源读取：`ToolCallError` 的类别与详情（含 `userActionRequired` 的 `reason` / `uri`）原样提交。
 *
 * @error 详情无法序列化为 JSON 时记警告并按无详情提交；原生模块旧（无 `failWithDetails`）时同样丢弃详情。
 */
export function submitFailure(
  target: Pick<NativeCall, 'fail' | 'failWithDetails'>,
  error: unknown,
  logger: Logger,
  label: string,
): void {
  const { kind, message, details } = toFailure(error)
  let detailsJson: string | undefined
  if (details !== undefined && target.failWithDetails) {
    try {
      detailsJson = JSON.stringify(details)
    } catch (e) {
      logger.warn(`[app-mcp] ${label} 的错误详情无法序列化为 JSON，已忽略`, e)
    }
  }
  if (detailsJson !== undefined) target.failWithDetails!(kind, message, detailsJson)
  else target.fail(kind, message)
}

/**
 * 导航回调的失败 → 原生导航句柄：`NAVIGATION_DENIED` 拒绝；`USER_ACTION_REQUIRED` 带详情中的 `reason` / `uri`；其他按失败。
 * @compat 旧版原生模块没有 `failUserAction`：`USER_ACTION_REQUIRED` 按失败（`NAVIGATION_FAILED`）提交。
 */
export function submitNavigationFailure(navigate: NativeNavigate, failure: ReturnType<typeof toFailure>): void {
  const { kind, message, details } = failure
  if (kind === 'NAVIGATION_DENIED') return navigate.deny(message)
  if (kind === 'USER_ACTION_REQUIRED' && navigate.failUserAction) {
    return navigate.failUserAction(message, detailText(details, 'reason'), detailText(details, 'uri'))
  }
  navigate.fail(message)
}

export function detailText(details: Record<string, unknown> | undefined, key: string): string | undefined {
  const value = details?.[key]
  return typeof value === 'string' ? value : undefined
}

export function stringifyJson(value: unknown): string {
  const text = JSON.stringify(value === undefined ? null : value)
  return text === undefined ? 'null' : text
}

/**
 * 撤销信息 → 原生 `UndoInit`：`arguments` 序列化为 JSON 文本（缺省时原生取 `{}`）。只转换不校验——不合法的由原生核心去掉并告警。
 *
 * @error `arguments` 无法序列化为 JSON（循环引用、BigInt）时抛出（调用方转为 `HANDLER_ERROR`）。
 */
export function nativeUndo(undo: UndoAction): NativeUndo {
  return {
    tool: undo.tool,
    ...(undo.arguments !== undefined && { argumentsJson: stringifyJson(undo.arguments) }),
    ...(undo.label !== undefined && { label: undo.label }),
  }
}
