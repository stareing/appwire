/**
 * 驱动层调用与注册：工具调用 / 资源读取的执行与回复，工具、资源、scope 的注册与注销（自 driver.ts 拆出）。
 */

import type { CoreClient, CoreOutcome, CoreToolDef, CoreToolUpdate } from '../core'
import { checkHandlerOrLoad, loadHandler } from '../lazy'
import { toJsonValue } from '../result'
import { describeParseError, isZodLike, toJsonSchema, toOutputSchema } from '../schema'
import type { ToolHubEvent, ToolInfo, ToolView } from '../tool-hub'
import { ToolCallError } from '../types'
import type {
  ContentAnnotations,
  JsonSchema,
  OutputDefinition,
  OutputSchema,
  ResourceDefinition,
  ResourceHandle,
  Scope,
  ScopeOptions,
  ToolDefinition,
  ToolHandle,
} from '../types'
import { checkPageName, ViewDeclaration } from '../view'
import {
  type AnyDef,
  errorMessage,
  errorOutcome,
  isPromise,
  NAME_RE,
  outcomeFromError,
  outcomeFromResult,
  type ResourceRec,
  type ScopeRec,
  scopeChain,
  type ToolRec,
  type ToolSchemas,
  withView,
} from './shared'
import { DriverTransport } from './transport'

export abstract class DriverRegistry extends DriverTransport {
  // ---- 调用 -----------------------------------------------------------

  protected invoke(callId: string, toolId: number, name: string, args: unknown, idempotencyKey?: string): void {
    const rec = this.toolsByCoreId.get(toolId)
    if (!rec) {
      this.input(
        (c) => c.completeCall(callId, errorOutcome('TOOL_NOT_FOUND', `工具 ${name} 不存在或已注销`), this.now()),
        true,
      )
      return
    }
    const controller = new AbortController()
    this.calls.get(callId)?.abort(new ToolCallError('CANCELLED', '重复的调用 ID'))
    this.calls.set(callId, controller)
    void this.runTool(rec, args, callId, controller.signal, idempotencyKey).then((outcome) =>
      this.finishCall(callId, controller, outcome),
    )
  }

  /** 执行 handler（含 zod 校验），结果与异常都转换为 {@link CoreOutcome}。 */
  protected async runTool(
    rec: ToolRec,
    args: unknown,
    callId: string,
    signal: AbortSignal,
    idempotencyKey?: string,
  ): Promise<CoreOutcome> {
    try {
      await Promise.resolve()
      let input = args
      if (rec.parse) {
        try {
          input = rec.parse(args)
        } catch (e) {
          const { message, details } = describeParseError(e)
          return errorOutcome('INVALID_INPUT', message, details)
        }
      }
      if (signal.aborted) throw signal.reason
      let handler = rec.handler
      if (!handler) {
        try {
          handler = await loadHandler(rec)
        } catch (e) {
          return errorOutcome('HANDLER_ERROR', `加载工具 ${rec.name} 的 handler 失败：${errorMessage(e)}`)
        }
        if (signal.aborted) throw signal.reason
      }
      const context = {
        callId,
        ...(idempotencyKey !== undefined && { idempotencyKey }),
        signal,
        hold: () => this.acquireHold(callId),
        progress: (progress: number, total?: number, message?: string) =>
          this.reportProgress(callId, signal, progress, total, message),
      }
      return outcomeFromResult(await handler(input, context))
    } catch (e) {
      return outcomeFromError(e)
    }
  }

  // ---- 内部钩子 -------------------------------------------------------

  protected emitHub(event: ToolHubEvent): void {
    for (const l of [...this.hubListeners]) {
      try {
        l(event)
      } catch (e) {
        this.log.error(`${this.tag} 工具注册监听器出错`, e)
      }
    }
  }

  protected toolView(rec: ToolRec): ToolView {
    if (!rec.view) {
      let seq = 0
      const driver = this
      rec.view = {
        name: rec.name,
        get definition() {
          return driver.effectiveEnabled(rec) ? rec.info : { ...rec.info, enabled: false }
        },
        call: (input, signal) => this.runTool(rec, input, `local-${rec.name}-${++seq}`, signal),
      }
    }
    return rec.view
  }

  protected finishCall(callId: string, controller: AbortController, outcome: CoreOutcome): void {
    if (this.calls.get(callId) === controller) this.calls.delete(callId)
    // 已取消 / 超时的调用由核心回复，丢弃结果
    if (controller.signal.aborted || this.disposed) return
    this.input((c) => c.completeCall(callId, outcome, this.now()), true)
  }

  protected readResource(readId: number, resourceId: number, name: string): void {
    const rec = this.resourcesByCoreId.get(resourceId)
    if (!rec) {
      this.input((c) => c.completeRead(readId, errorOutcome('RESOURCE_NOT_FOUND', `资源 ${name} 不存在或已注销`)), true)
      return
    }
    const run = async (): Promise<CoreOutcome> => {
      await Promise.resolve()
      return { data: toJsonValue(await rec.read()) }
    }
    run().then(
      (outcome) => this.finishRead(readId, outcome),
      (e: unknown) => this.finishRead(readId, outcomeFromError(e)),
    )
  }

  protected finishRead(readId: number, outcome: CoreOutcome): void {
    if (this.disposed) return
    this.input((c) => c.completeRead(readId, outcome), true)
  }

  // ---- 注册 -----------------------------------------------------------

  protected assertUsable(kind: string, name: string): boolean {
    if (this.disposed) {
      this.log.warn(`${this.tag} 实例已 dispose，忽略${kind} ${name} 的注册`)
      return false
    }
    return true
  }

  protected registerTool(name: string, def: AnyDef, scope: ScopeRec | undefined): ToolHandle {
    if (!NAME_RE.test(name)) throw new Error(`无效的工具名 ${JSON.stringify(name)}：应匹配 [a-zA-Z0-9_.-]{1,64}`)
    checkHandlerOrLoad(name, def)
    const { handler: _handler, load: _load, anchor: _anchor, visibility: _visibility, ...info } = def
    const rec: ToolRec = {
      name,
      info,
      view: undefined,
      handler: def.handler,
      load: def.load,
      loading: undefined,
      parse: isZodLike(def.input) ? (x) => (def.input as { parse(x: unknown): unknown }).parse(x) : undefined,
      anchor: def.anchor,
      scope,
      coreId: undefined,
      disposed: false,
      decl: undefined,
      coreEnabled: true,
    }
    const handle = this.toolHandle(rec)
    if (!this.assertUsable('工具', name) || scope?.disposed) {
      rec.disposed = true
      return handle
    }
    if (this.toolNames.has(name)) {
      // 可让位的工具（如经 WebMCP 标准接口注册的）让给 appMcp.tool()
      const evict = this.hub.yieldable.get(name)
      if (evict) {
        this.hub.yieldable.delete(name)
        evict()
      }
    }
    if (this.toolNames.has(name)) throw new Error(`工具 ${JSON.stringify(name)} 已注册`)
    const decl = new ViewDeclaration(name, def, scopeChain(scope), this.doc, () => this.pushEnabled(rec))
    rec.decl = decl
    rec.info = withView(rec.info, decl)
    this.toolNames.set(name, rec)
    ;(scope ? scope.tools : this.rootTools).add(rec)
    this.emitHub({ type: 'register', tool: this.toolView(rec) })

    const schemas = this.convertSchemas(name, def.input, def.outputSchema)
    const register = (core: CoreClient, { inputSchema, outputSchema }: ToolSchemas): void => {
      if (rec.disposed) return
      const coreDef: CoreToolDef = { name, description: def.description, inputSchema }
      if (def.title !== undefined) coreDef.title = def.title
      if (def.risk !== undefined) coreDef.risk = def.risk
      if (def.annotations !== undefined) coreDef.annotations = def.annotations
      if (outputSchema !== undefined) coreDef.outputSchema = outputSchema
      if (def.activation !== undefined) coreDef.activation = def.activation
      if (decl.surface === 'view') coreDef.surface = 'view'
      if (decl.page !== undefined) coreDef.page = decl.page
      if (def.backgroundTool !== undefined) coreDef.backgroundTool = def.backgroundTool
      if (def.concurrency !== undefined) coreDef.concurrency = def.concurrency
      if (def.exclusive !== undefined) coreDef.exclusive = def.exclusive
      if (def.implements !== undefined && def.implements.length > 0) coreDef.implements = [...def.implements]
      rec.coreEnabled = this.effectiveEnabled(rec)
      if (!rec.coreEnabled || def.enabled !== undefined) coreDef.enabled = rec.coreEnabled
      if (scope) {
        if (scope.coreId === undefined) throw new Error(`scope ${scope.name} 未创建，无法注册工具 ${name}`)
        coreDef.scope = scope.coreId
      }
      try {
        rec.coreId = core.registerTool(coreDef)
      } catch (e) {
        throw new Error(`注册工具 ${name} 失败：${errorMessage(e)}`)
      }
      this.toolsByCoreId.set(rec.coreId, rec)
    }
    this.enqueue((core) => (isPromise(schemas) ? schemas.then((s) => register(core, s)) : register(core, schemas)))
    return handle
  }

  protected convertSchema(name: string, input: unknown): JsonSchema | Promise<JsonSchema> {
    try {
      return toJsonSchema(input as ToolDefinition['input'])
    } catch (e) {
      return Promise.reject(new Error(`工具 ${name} 的输入 schema 无效：${errorMessage(e)}`))
    }
  }

  protected convertOutputSchema(name: string, output: OutputDefinition<unknown>): OutputSchema | Promise<OutputSchema> {
    try {
      return toOutputSchema(output)
    } catch (e) {
      return Promise.reject(new Error(`工具 ${name} 的输出 schema 无效：${errorMessage(e)}`))
    }
  }

  /** 输入与输出 schema；都能同步转换时同步返回（注册顺序不受影响），否则返回 Promise。 */
  protected convertSchemas(
    name: string,
    input: unknown,
    output: OutputDefinition<unknown> | undefined,
  ): ToolSchemas | Promise<ToolSchemas> {
    const inputSchema = this.convertSchema(name, input)
    const outputSchema = output === undefined ? undefined : this.convertOutputSchema(name, output)
    if (!isPromise(inputSchema) && !isPromise(outputSchema)) return { inputSchema, outputSchema }
    return Promise.all([inputSchema, outputSchema]).then(([i, o]) => ({ inputSchema: i, outputSchema: o }))
  }

  protected toolHandle(rec: ToolRec): ToolHandle {
    return {
      name: rec.name,
      update: (changes) => {
        if (rec.disposed) return
        // 未出现的字段保持不变；显式给出 undefined 的字段恢复默认值（便于框架适配整体同步定义）。
        if ('input' in changes) {
          rec.parse = isZodLike(changes.input)
            ? (x) => (changes.input as { parse(x: unknown): unknown }).parse(x)
            : undefined
        }
        const update: CoreToolUpdate = {}
        const decl = rec.decl
        const viewChange = decl?.apply(changes)
        if (decl && viewChange?.surface) update.surface = decl.surface
        if (decl && viewChange?.page) update.page = decl.page ?? null
        const info: Record<string, unknown> = { ...rec.info }
        for (const [k, v] of Object.entries(changes)) {
          if (k === 'anchor' || k === 'handler' || k === 'load' || k === 'visibility') continue
          if (k === 'description' && v === undefined) continue
          if (v === undefined) delete info[k]
          else info[k] = v
        }
        rec.info = decl ? withView(info as ToolInfo, decl) : (info as ToolInfo)
        this.emitHub({ type: 'update', tool: this.toolView(rec) })
        if (viewChange?.enabled) this.pushEnabled(rec)
        if (changes.description !== undefined) update.description = changes.description
        if ('risk' in changes) update.risk = changes.risk ?? 'write'
        if ('title' in changes) update.title = changes.title ?? null
        if ('activation' in changes) update.activation = changes.activation ?? null
        if ('annotations' in changes) update.annotations = changes.annotations ?? null
        if ('backgroundTool' in changes) update.backgroundTool = changes.backgroundTool ?? null
        if ('concurrency' in changes) update.concurrency = changes.concurrency ?? 0
        if ('exclusive' in changes) update.exclusive = changes.exclusive ?? null
        if ('implements' in changes) update.implements = [...(changes.implements ?? [])]
        if ('outputSchema' in changes && changes.outputSchema === undefined) update.outputSchema = null
        const schema = 'input' in changes ? this.convertSchema(rec.name, changes.input) : undefined
        const output =
          changes.outputSchema !== undefined ? this.convertOutputSchema(rec.name, changes.outputSchema) : undefined
        if (schema === undefined && output === undefined && Object.keys(update).length === 0) return
        const apply = (core: CoreClient, inputSchema: JsonSchema | undefined, outputSchema: OutputSchema | undefined): void => {
          if (rec.disposed || rec.coreId === undefined) return
          if (inputSchema !== undefined) update.inputSchema = inputSchema
          if (outputSchema !== undefined) update.outputSchema = outputSchema
          try {
            core.updateTool(rec.coreId, update)
          } catch (e) {
            throw new Error(`更新工具 ${rec.name} 失败：${errorMessage(e)}`)
          }
        }
        this.enqueue((core) =>
          isPromise(schema) || isPromise(output)
            ? Promise.all([schema, output]).then(([i, o]) => apply(core, i, o))
            : apply(core, schema, output),
        )
      },
      setHandler: (handler) => {
        rec.handler = handler
        rec.load = undefined
        rec.loading = undefined
      },
      dispose: () => {
        if (rec.disposed) return
        this.forgetTool(rec)
        this.enqueue((core) => {
          if (rec.coreId === undefined) return
          this.toolsByCoreId.delete(rec.coreId)
          core.unregisterTool(rec.coreId)
        })
      },
    }
  }

  // ---- view 工具（spec/protocol.md 3.4）---------------------------------

  /** 对 Host 可见 = App 启用 且（有门控时）门控为真。 */
  protected effectiveEnabled(rec: ToolRec): boolean {
    return rec.decl?.enabled ?? true
  }

  /** 生效的启用状态变化时告诉核心（核心据此发 `tools/changed`）。 */
  protected pushEnabled(rec: ToolRec): void {
    if (rec.disposed) return
    this.emitHub({ type: 'update', tool: this.toolView(rec) })
    this.enqueue((core) => {
      if (rec.disposed || rec.coreId === undefined) return
      const enabled = this.effectiveEnabled(rec)
      if (enabled === rec.coreEnabled) return
      rec.coreEnabled = enabled
      core.updateTool(rec.coreId, { enabled })
    })
  }

  /** JS 侧注销（不操作核心）。 */
  protected forgetTool(rec: ToolRec): void {
    rec.disposed = true
    rec.decl?.dispose()
    ;(rec.scope ? rec.scope.tools : this.rootTools).delete(rec)
    if (this.toolNames.get(rec.name) === rec) {
      this.toolNames.delete(rec.name)
      this.emitHub({ type: 'unregister', name: rec.name })
    }
  }

  protected forgetResource(rec: ResourceRec): void {
    rec.disposed = true
    if (this.resourceNames.get(rec.name) === rec) this.resourceNames.delete(rec.name)
    ;(rec.scope ? rec.scope.resources : this.rootResources).delete(rec)
  }

  protected registerResource(name: string, def: ResourceDefinition<any>, scope: ScopeRec | undefined): ResourceHandle {
    if (!NAME_RE.test(name)) throw new Error(`无效的资源名 ${JSON.stringify(name)}：应匹配 [a-zA-Z0-9_.-]{1,64}`)
    const rec: ResourceRec = { name, read: def.read, scope, coreId: undefined, disposed: false }
    const handle: ResourceHandle = {
      name,
      notifyChanged: () => {
        // 核心加载前不可能有订阅，直接忽略
        if (rec.disposed || !this.core) return
        this.enqueue((core) => {
          if (rec.coreId !== undefined && !rec.disposed) core.notifyResourceChanged(rec.coreId, this.now())
        })
      },
      setReader: (read) => {
        rec.read = read
      },
      dispose: () => {
        if (rec.disposed) return
        this.forgetResource(rec)
        this.enqueue((core) => {
          if (rec.coreId === undefined) return
          this.resourcesByCoreId.delete(rec.coreId)
          core.unregisterResource(rec.coreId)
        })
      },
    }
    if (!this.assertUsable('资源', name) || scope?.disposed) {
      rec.disposed = true
      return handle
    }
    if (this.resourceNames.has(name)) throw new Error(`资源 ${JSON.stringify(name)} 已注册`)
    this.resourceNames.set(name, rec)
    ;(scope ? scope.resources : this.rootResources).add(rec)
    this.enqueue((core) => {
      if (rec.disposed) return
      if (scope && scope.coreId === undefined) throw new Error(`scope ${scope.name} 未创建，无法注册资源 ${name}`)
      try {
        rec.coreId = core.registerResource({
          name,
          description: def.description,
          ...(def.mimeType !== undefined && { mimeType: def.mimeType }),
          ...(def.realtime && { realtime: true }),
          ...(def.annotations !== undefined && { annotations: toJsonValue(def.annotations) as ContentAnnotations }),
          ...(scope && { scope: scope.coreId }),
        })
      } catch (e) {
        throw new Error(`注册资源 ${name} 失败：${errorMessage(e)}`)
      }
      this.resourcesByCoreId.set(rec.coreId, rec)
    })
    return handle
  }

  protected createScope(name: string, parent: ScopeRec | undefined, options?: ScopeOptions): Scope {
    if (options?.page !== undefined) checkPageName(`scope ${name}`, options.page)
    const rec: ScopeRec = {
      name,
      options: options === undefined ? undefined : { ...options },
      parent,
      coreId: undefined,
      disposed: false,
      children: new Set(),
      tools: new Set(),
      resources: new Set(),
    }
    const scope: Scope = {
      name,
      tool: (toolName, definition) => this.registerTool(toolName, definition as AnyDef, rec),
      resource: (resName, definition) => this.registerResource(resName, definition, rec),
      scope: (childName, childOptions) => this.createScope(childName, rec, childOptions),
      dispose: () => {
        if (rec.disposed) return
        this.forgetScope(rec)
        ;(parent ? parent.children : this.rootScopes).delete(rec)
        this.enqueue((core) => {
          if (rec.coreId === undefined) return
          core.disposeScope(rec.coreId)
        })
      },
    }
    if (this.disposed || parent?.disposed) {
      rec.disposed = true
      return scope
    }
    ;(parent ? parent.children : this.rootScopes).add(rec)
    this.enqueue((core) => {
      if (rec.disposed) return
      if (parent && parent.coreId === undefined) throw new Error(`父 scope ${parent.name} 未创建`)
      rec.coreId = core.createScope(name, parent?.coreId)
    })
    return scope
  }

  /** 递归标记 scope 及其内容为已注销（核心侧由 disposeScope 递归处理）。 */
  protected forgetScope(rec: ScopeRec): void {
    rec.disposed = true
    for (const t of [...rec.tools]) {
      this.forgetTool(t)
      if (t.coreId !== undefined) this.toolsByCoreId.delete(t.coreId)
    }
    for (const r of [...rec.resources]) {
      this.forgetResource(r)
      if (r.coreId !== undefined) this.resourcesByCoreId.delete(r.coreId)
    }
    for (const c of [...rec.children]) this.forgetScope(c)
    rec.children.clear()
  }
}
