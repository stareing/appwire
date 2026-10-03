/**
 * Electron 桥接实现：桥接客户端（请求队列、事件分发）与共用辅助函数（自 electron-bridge.ts 拆出）。
 */

import { loadHandler } from '../lazy'
import { noopHold } from '../noop'
import { normalizeToolResult, toJsonValue, type NormalizedResult } from '../result'
import { describeParseError, isZodLike, toJsonSchema, toOutputSchema } from '../schema'
import type { ScopeChain } from '../view'
import type {
  ErrorKind,
  HoldHandle,
  JsonSchema,
  LazyToolDefinition,
  Logger,
  OutputSchema,
  ScopeOptions,
  ToolDefinition,
  ToolHandler,
} from '../types'
import type { AppMcpBridge, OpReply, Outcome, RendererOp } from './protocol'
import type { ResourceEntry, ToolEntry } from './entries'

// ---------------------------------------------------------------------------
// 实现
// ---------------------------------------------------------------------------

export const defaultLogger: Logger = {
  debug() {},
  warn: (message, ...args) => console.warn(message, ...args),
  error: (message, ...args) => console.error(message, ...args),
}

/** 与 ToolCallError 同形的错误（按 name + kind 识别）。 */
export function callError(kind: ErrorKind, message: string): Error & { kind: ErrorKind } {
  const error = new Error(message) as Error & { kind: ErrorKind }
  error.name = 'ToolCallError'
  error.kind = kind
  return error
}

/** ToolCallError 的详情 → 可跨 IPC 传递的 JSON 对象；不是对象或无法序列化时丢弃（错误本身照常上报）。 */
export function outcomeDetails(details: unknown): Record<string, unknown> | undefined {
  if (typeof details !== 'object' || details === null || Array.isArray(details)) return undefined
  try {
    const json = toJsonValue(details)
    return typeof json === 'object' && json !== null && !Array.isArray(json) ? (json as Record<string, unknown>) : undefined
  } catch {
    return undefined
  }
}

export function toOutcomeError(error: unknown): Outcome {
  if (typeof error === 'object' && error !== null) {
    const e = error as { name?: unknown; kind?: unknown; message?: unknown }
    const message = typeof e.message === 'string' ? e.message : String(error)
    if (e.name === 'ToolCallError' && typeof e.kind === 'string') {
      const details = outcomeDetails((error as { details?: unknown }).details)
      return details === undefined
        ? { ok: false, kind: e.kind as ErrorKind, message }
        : { ok: false, kind: e.kind as ErrorKind, message, details }
    }
    return { ok: false, kind: 'HANDLER_ERROR', message: message || 'handler 执行失败' }
  }
  return { ok: false, kind: 'HANDLER_ERROR', message: String(error) }
}

export interface ResolvedInput {
  schema: JsonSchema | undefined
  parse?: ((input: unknown) => unknown) | undefined
}

/** 输入定义 → schema 与 zod 校验函数（需要加载 zod 时返回 Promise；定义不合法时同步抛出）。 */
export function resolveInput(input: unknown): ResolvedInput | Promise<ResolvedInput> {
  if (input === undefined) return { schema: undefined }
  const parse = isZodLike(input) ? (value: unknown) => input.parse(value) : undefined
  const schema = toJsonSchema(input as ToolDefinition['input'])
  return schema instanceof Promise ? schema.then((s) => ({ schema: s, parse })) : { schema, parse }
}

/** 输出定义 → JSON Schema（缺省为 undefined；需要加载 zod 时返回 Promise；定义不合法时同步抛出）。 */
export function resolveOutput(output: ToolDefinition['outputSchema']): OutputSchema | undefined | Promise<OutputSchema> {
  return output === undefined ? undefined : toOutputSchema(output)
}

export type AnyDef = ToolDefinition<any, any> | LazyToolDefinition<any, any>

export interface Detachable {
  detach(): void
}

export interface Owner {
  readonly scopeId: number | undefined
  readonly children: Set<Detachable>
  /** scope 的界面声明（{@link ScopeOptions}）与上层；实例本身两者都为 undefined。 */
  readonly viewOptions: ScopeOptions | undefined
  readonly parentOwner: Owner | undefined
}

/** 所在 scope 链（自近到远）上的界面声明。 */
export function ownerChain(owner: Owner): ScopeChain {
  return function* () {
    for (let o: Owner | undefined = owner; o; o = o.parentOwner) yield o.viewOptions
  }
}

export function pageDocument(): Document | undefined {
  return typeof document === 'undefined' ? undefined : document
}

export class Client {
  private queue: Promise<unknown> = Promise.resolve()
  private nextId = 1
  readonly tools = new Map<number, ToolEntry>()
  readonly resources = new Map<number, ResourceEntry>()
  readonly calls = new Map<string, AbortController>()
  private readonly holds = new Set<number>()
  private holdsClosed = false

  constructor(
    readonly bridge: AppMcpBridge | null,
    readonly logger: Logger,
  ) {}

  id(): number {
    return this.nextId++
  }

  /** 按顺序发送操作（`build` 在前一个操作完成后才执行，可等待 schema 解析）。 */
  enqueue(build: () => RendererOp | null | Promise<RendererOp | null>): Promise<OpReply | undefined> {
    const bridge = this.bridge
    if (!bridge) return Promise.resolve(undefined)
    const next = this.queue.then(async () => {
      const op = await build()
      if (!op) return undefined
      const reply = await bridge.request(op)
      if (!reply.ok) this.logger.error(`[app-mcp] ${op.op} 失败：${reply.message}`)
      return reply
    })
    this.queue = next.catch((error: unknown) => this.logger.error('[app-mcp] IPC 请求失败', error))
    return next
  }

  /** 持有：经主进程转给 @app-mcp/node 的 `hold()`；`release()` 幂等。 */
  hold(): HoldHandle {
    if (!this.bridge || this.holdsClosed) return noopHold()
    const holdId = this.id()
    this.holds.add(holdId)
    void this.enqueue(() => ({ op: 'lifecycle.hold', holdId }))
    return {
      release: () => {
        if (!this.holds.delete(holdId)) return
        void this.enqueue(() => ({ op: 'lifecycle.release', holdId }))
      },
    }
  }

  /** 实例注销：主进程随 `reset` 释放全部持有，这里只清本地记录（之后的 release 为空操作）。 */
  dropHolds(): void {
    this.holdsClosed = true
    this.holds.clear()
  }

  /** 结果不排队：避免被等待中的注册阻塞。 */
  send(op: RendererOp): void {
    this.bridge?.request(op).then(
      (reply) => {
        if (!reply.ok) this.logger.warn(`[app-mcp] ${op.op} 失败：${reply.message}`)
      },
      (error: unknown) => this.logger.error('[app-mcp] IPC 请求失败', error),
    )
  }

  onCall(callId: string, toolId: number, input: unknown, idempotencyKey?: string): void {
    const entry = this.tools.get(toolId)
    if (!entry) {
      this.send({ op: 'call.result', callId, ok: false, kind: 'TOOL_NOT_FOUND', message: '工具已注销' })
      return
    }
    const controller = new AbortController()
    this.calls.set(callId, controller)
    const finish = (outcome: Outcome) => {
      if (controller.signal.aborted) return
      this.calls.delete(callId)
      this.send({ op: 'call.result', callId, ...outcome })
    }
    let value = input
    const parse = entry.parse
    if (parse) {
      try {
        value = parse(input)
      } catch (error) {
        finish({ ok: false, kind: 'INVALID_INPUT', message: describeParseError(error).message })
        return
      }
    }
    const handler: Promise<ToolHandler<any, any>> = entry.handler
      ? Promise.resolve(entry.handler)
      : loadHandler(entry).catch((error: unknown) => {
          throw callError(
            'HANDLER_ERROR',
            `加载工具 ${entry.name} 的 handler 失败：${error instanceof Error ? error.message : String(error)}`,
          )
        })
    handler
      .then((fn) =>
        fn(value, {
          callId,
          ...(idempotencyKey !== undefined && { idempotencyKey }),
          signal: controller.signal,
          hold: () => this.hold(),
          progress: (progress: number, total?: number, message?: string) => {
            if (controller.signal.aborted || !this.calls.has(callId)) return
            this.send({
              op: 'call.progress',
              callId,
              progress,
              ...(total !== undefined && { total }),
              ...(message !== undefined && { message }),
            })
          },
        }),
      )
      .then(
        (result) => {
          let normalized: NormalizedResult
          try {
            normalized = normalizeToolResult(result)
          } catch (error) {
            finish({ ok: false, kind: 'HANDLER_ERROR', message: `返回值无法序列化为 JSON：${String(error)}` })
            return
          }
          finish({ ok: true, ...normalized })
        },
        (error: unknown) => finish(toOutcomeError(error)),
      )
  }

  onRead(readId: number, resourceId: number): void {
    const entry = this.resources.get(resourceId)
    if (!entry) {
      this.send({ op: 'read.result', readId, ok: false, kind: 'RESOURCE_NOT_FOUND', message: '资源已注销' })
      return
    }
    const reader = entry.reader
    Promise.resolve()
      .then(() => reader())
      .then(
        (value) => {
          let json: unknown
          try {
            json = toJsonValue(value)
          } catch (error) {
            this.send({ op: 'read.result', readId, ok: false, kind: 'HANDLER_ERROR', message: String(error) })
            return
          }
          this.send({ op: 'read.result', readId, ok: true, data: json })
        },
        (error: unknown) => this.send({ op: 'read.result', readId, ...toOutcomeError(error) }),
      )
  }

  onCancel(callId: string, kind: ErrorKind, message: string): void {
    const controller = this.calls.get(callId)
    if (!controller) return
    this.calls.delete(callId)
    controller.abort(callError(kind, message))
  }
}

export function scopeField(owner: Owner): { scopeId?: number } {
  return owner.scopeId === undefined ? {} : { scopeId: owner.scopeId }
}
