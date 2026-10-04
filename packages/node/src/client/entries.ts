/**
 * createAppMcp 的注册条目：工具与资源（自 client.ts 拆出）。
 */

import { ToolCallError, nativeErrorCode } from '../errors.js'
import { checkHandlerOrLoad, loadHandler, type LazySlot } from '../lazy.js'
import type {
  NativeCall,
  NativeRead,
  NativeRegistrar,
  NativeResource,
  NativeTool,
  NativeToolSpec,
} from '../native.js'
import { hasResultExtras, normalizeToolResult, type NormalizedResult } from '../result.js'
import { describeParseError } from '../schema.js'
import type {
  HoldHandle,
  LazyToolDefinition,
  Logger,
  ResourceDefinition,
  ResourceHandle,
  ToolDefinition,
  ToolHandle,
  ToolHandler,
  ToolHandlerLoader,
} from '../types.js'
import {
  type Child,
  NOOP_HOLD,
  type Owner,
  resolveTool,
  type ResolvedTool,
  stringifyJson,
  submitFailure,
  wrapHold,
} from './shared.js'

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/** 工具元数据（不含 handler / load）。 */
type ToolMeta = Omit<ToolDefinition<any, any>, 'handler' | 'load'>

export class ToolEntry implements ToolHandle, Child, LazySlot {
  readonly name: string
  private def: ToolMeta
  handler: ToolHandler<any, any> | undefined
  load: ToolHandlerLoader<any, any> | undefined
  loading: Promise<ToolHandler<any, any>> | undefined
  private resolved: ResolvedTool | undefined
  private native: NativeTool | undefined
  private disposed = false
  /** 每次解析输入定义递增，丢弃过期的异步结果。 */
  private resolveSeq = 0

  constructor(
    private readonly registrar: NativeRegistrar | null,
    private readonly owner: Owner | null,
    name: string,
    definition: ToolDefinition<any, any> | LazyToolDefinition<any, any>,
    private readonly logger: Logger,
  ) {
    checkHandlerOrLoad(name, definition)
    this.name = name
    const { handler, load, ...meta } = definition
    this.def = meta
    this.handler = handler
    this.load = load
    if (!registrar) return
    const resolved = resolveTool(definition)
    if (resolved instanceof Promise) {
      const seq = ++this.resolveSeq
      owner?.children.add(this)
      resolved.then(
        (r) => {
          if (this.disposed || seq !== this.resolveSeq) return
          this.resolved = r
          try {
            this.native = registrar.registerTool(this.spec(), (call) => this.onCall(call))
          } catch (error) {
            this.logger.error(`[app-mcp] 注册工具 ${name} 失败`, error)
            this.disposed = true
            owner?.children.delete(this)
          }
        },
        (error: unknown) => {
          this.logger.error(`[app-mcp] 工具 ${name} 的 input / outputSchema 定义无效`, error)
        },
      )
      return
    }
    this.resolved = resolved
    // 同步注册：重名、非法名称等错误直接抛给调用方。
    this.native = registrar.registerTool(this.spec(), (call) => this.onCall(call))
    owner?.children.add(this)
  }

  private spec(): NativeToolSpec {
    const d = this.def
    const spec: NativeToolSpec = { name: this.name, description: d.description, enabled: d.enabled ?? true }
    if (this.resolved?.schemaJson !== undefined) spec.inputSchemaJson = this.resolved.schemaJson
    if (d.risk !== undefined) spec.risk = d.risk
    if (d.activation !== undefined) spec.activation = d.activation
    if (d.title !== undefined) spec.title = d.title
    if (d.annotations !== undefined) spec.annotations = { ...d.annotations }
    if (this.resolved?.outputSchemaJson !== undefined) spec.outputSchemaJson = this.resolved.outputSchemaJson
    if (d.surface !== undefined) spec.surface = d.surface
    if (d.page !== undefined) spec.page = d.page
    if (d.backgroundTool !== undefined) spec.backgroundTool = d.backgroundTool
    if (d.concurrency !== undefined) spec.concurrency = d.concurrency
    if (d.exclusive !== undefined) spec.exclusive = d.exclusive
    if (d.implements !== undefined && d.implements.length > 0) spec.implements = [...d.implements]
    return spec
  }

  update(changes: Partial<Omit<ToolDefinition<any, any>, 'handler'>>): void {
    if (this.disposed) return
    const { handler: _h, load: _l, ...meta } = changes as Partial<ToolDefinition<any, any>>
    this.def = { ...this.def, ...meta }
    if (!this.registrar) return
    if ('input' in changes || 'outputSchema' in changes) {
      const resolved = resolveTool(this.def)
      const seq = ++this.resolveSeq
      if (resolved instanceof Promise) {
        resolved.then(
          (r) => {
            if (this.disposed || seq !== this.resolveSeq) return
            this.resolved = r
            this.pushUpdate()
          },
          (error: unknown) => this.logger.error(`[app-mcp] 工具 ${this.name} 的 input / outputSchema 定义无效`, error),
        )
        return
      }
      this.resolved = resolved
    }
    this.pushUpdate()
  }

  private pushUpdate(): void {
    if (!this.native) return // 尚在异步注册中：注册时会使用最新定义
    // @compat 旧版原生模块没有 updateWith：注解与输出 schema 保持注册时的声明
    if (this.native.updateWith) this.native.updateWith(this.spec())
    else this.native.update(this.spec())
  }

  setHandler(handler: ToolDefinition<any, any>['handler']): void {
    this.handler = handler
    this.load = undefined
    this.loading = undefined
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.owner?.children.delete(this)
    this.native?.dispose()
  }

  detach(): void {
    this.disposed = true
  }

  /**
   * 提交成功结果：带业务状态 / 摘要 / 内容标注时用 `completeWith`，否则用 `complete`（与旧版行为一致）。
   * @compat 旧版原生模块没有 `completeWith`：只提交 data 与 stateHints，并警告一次丢弃的字段。
   * @error `completeWith` 拒绝取值（如非法 audience）时以 `HANDLER_ERROR` 失败完成，不让调用挂起到超时。
   */
  private complete(
    call: NativeCall,
    json: string,
    result: NormalizedResult,
    finish: (action: () => void) => void,
    fail: (error: unknown) => void,
  ): void {
    const hints = result.stateHints ?? []
    if (!hasResultExtras(result)) {
      finish(() => call.complete(json, hints))
      return
    }
    if (!call.completeWith) {
      this.logger.warn(`[app-mcp] 原生模块版本过旧，工具 ${this.name} 结果中的 status / summary / annotations 已忽略`)
      finish(() => call.complete(json, hints))
      return
    }
    try {
      call.completeWith({
        dataJson: json,
        stateHints: hints,
        ...(result.status !== undefined && { status: result.status }),
        ...(result.stateResource !== undefined && { stateResource: result.stateResource }),
        ...(result.summary !== undefined && { summary: result.summary }),
        ...(result.annotations !== undefined && { annotations: { ...result.annotations } }),
      })
      finish(() => {})
    } catch (error) {
      if (nativeErrorCode(error) === 'ALREADY_COMPLETED') return finish(() => {})
      fail(new ToolCallError('HANDLER_ERROR', `返回的结构化结果无效：${error instanceof Error ? error.message : String(error)}`))
    }
  }

  private onCall(call: NativeCall): void {
    const controller = new AbortController()
    let settled = false
    const finish = (action: () => void) => {
      if (settled) return
      settled = true
      try {
        action()
      } catch (error) {
        // 已取消 / 已完成：原生侧拒绝重复完成，属正常情况。
        if (nativeErrorCode(error) !== 'ALREADY_COMPLETED') {
          this.logger.error(`[app-mcp] 提交工具 ${this.name} 的结果失败`, error)
        }
      }
    }
    const fail = (error: unknown) => finish(() => submitFailure(call, error, this.logger, `工具 ${this.name}`))

    try {
      call.setCancelListener((reason) => {
        const kind = reason === 'timeout' ? 'TIMEOUT' : 'CANCELLED'
        settled = true
        controller.abort(new ToolCallError(kind, `调用已取消（${reason}）`))
      })
    } catch (error) {
      this.logger.warn(`[app-mcp] 设置取消监听失败`, error)
    }

    let input: unknown
    try {
      input = call.argumentsJson ? JSON.parse(call.argumentsJson) : {}
    } catch {
      fail(new ToolCallError('INVALID_INPUT', '参数不是合法的 JSON'))
      return
    }
    const parse = this.resolved?.parse
    if (parse) {
      try {
        input = parse(input)
      } catch (error) {
        const { message, details } = describeParseError(error)
        fail(new ToolCallError('INVALID_INPUT', message, details))
        return
      }
    }

    const handler: Promise<ToolHandler<any, any>> = this.handler
      ? Promise.resolve(this.handler)
      : loadHandler(this).catch((error: unknown) => {
          throw new ToolCallError(
            'HANDLER_ERROR',
            `加载工具 ${this.name} 的 handler 失败：${error instanceof Error ? error.message : String(error)}`,
          )
        })
    const idempotencyKey = call.idempotencyKey ?? undefined
    const context = {
      callId: call.callId,
      ...(idempotencyKey !== undefined && { idempotencyKey }),
      signal: controller.signal,
      hold: (): HoldHandle => (call.hold ? wrapHold(call.hold()) : NOOP_HOLD),
      progress: (progress: number, total?: number, message?: string): void => {
        if (controller.signal.aborted) return
        try {
          call.reportProgress?.(progress, total ?? null, message ?? null)
        } catch {
          // @why 调用刚结束时原生层报 ALREADY_COMPLETED：进度只是提示，不影响结果。
        }
      },
    }
    handler
      .then((fn) => {
        if (controller.signal.aborted) throw controller.signal.reason
        return fn(input, context)
      })
      .then(
        (result) => {
          if (controller.signal.aborted) return
          const normalized = normalizeToolResult(result)
          let json: string
          try {
            json = stringifyJson(normalized.data)
          } catch (error) {
            fail(new ToolCallError('HANDLER_ERROR', `返回值无法序列化为 JSON：${String(error)}`))
            return
          }
          this.complete(call, json, normalized, finish, fail)
        },
        (error: unknown) => {
          if (controller.signal.aborted) return
          fail(error)
        },
      )
  }
}

// ---------------------------------------------------------------------------
// 资源
// ---------------------------------------------------------------------------

export class ResourceEntry implements ResourceHandle, Child {
  readonly name: string
  private reader: ResourceDefinition<any>['read']
  private readonly native: NativeResource | undefined
  private disposed = false

  constructor(
    registrar: NativeRegistrar | null,
    private readonly owner: Owner | null,
    name: string,
    definition: ResourceDefinition<any>,
    private readonly logger: Logger,
  ) {
    this.name = name
    this.reader = definition.read
    if (!registrar) return
    this.native = registrar.registerResource(
      {
        name,
        description: definition.description,
        // @compat 未声明时不发送（spec/protocol.md 8.4 的 toolsHash 输入与其他 SDK 一致；Host 按 application/json 处理）
        ...(definition.mimeType !== undefined && { mimeType: definition.mimeType }),
        ...(definition.realtime && { realtime: true }),
        ...(definition.annotations !== undefined && { annotations: { ...definition.annotations } }),
      },
      (read) => this.onRead(read),
    )
    owner?.children.add(this)
  }

  notifyChanged(): void {
    if (this.disposed || !this.native) return
    try {
      this.native.notifyChanged()
    } catch (error) {
      this.logger.warn(`[app-mcp] 资源 ${this.name} 变更通知失败`, error)
    }
  }

  setReader(read: ResourceDefinition<any>['read']): void {
    this.reader = read
  }

  dispose(): void {
    if (this.disposed) return
    this.disposed = true
    this.owner?.children.delete(this)
    this.native?.dispose()
  }

  detach(): void {
    this.disposed = true
  }

  private onRead(read: NativeRead): void {
    const reader = this.reader
    const submit = (action: () => void) => {
      try {
        action()
      } catch (error) {
        if (nativeErrorCode(error) !== 'ALREADY_COMPLETED') {
          this.logger.error(`[app-mcp] 提交资源 ${this.name} 的内容失败`, error)
        }
      }
    }
    Promise.resolve()
      .then(() => reader())
      .then(
        (value) => {
          let json: string
          try {
            json = stringifyJson(value)
          } catch (error) {
            submit(() => read.fail('HANDLER_ERROR', `资源内容无法序列化为 JSON：${String(error)}`))
            return
          }
          submit(() => read.complete(json))
        },
        (error: unknown) => submit(() => submitFailure(read, error, this.logger, `资源 ${this.name}`)),
      )
  }
}
