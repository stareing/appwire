/**
 * Electron 桥接的注册条目：工具与资源（自 electron-bridge.ts 拆出）。
 */

import { checkHandlerOrLoad, type LazySlot } from '../lazy'
import { toJsonValue } from '../result'
import { ViewDeclaration } from '../view'
import type {
  ContentAnnotations,
  OutputSchema,
  ResourceDefinition,
  ResourceHandle,
  ToolHandle,
  ToolHandler,
  ToolHandlerLoader,
} from '../types'
import type { ToolSpecMessage } from './protocol'
import {
  type AnyDef,
  Client,
  type Detachable,
  ownerChain,
  type Owner,
  pageDocument,
  resolveInput,
  resolveOutput,
  type ResolvedInput,
  scopeField,
} from './client'

export class ToolEntry implements ToolHandle, Detachable, LazySlot {
  readonly id: number
  private def: Omit<AnyDef, 'handler' | 'load'>
  handler: ToolHandler<any, any> | undefined
  load: ToolHandlerLoader<any, any> | undefined
  loading: Promise<ToolHandler<any, any>> | undefined
  schema: ResolvedInput['schema']
  parse: ResolvedInput['parse']
  outputSchema: OutputSchema | undefined
  /** 界面声明与可见性门控（只在有桥接时创建）。 */
  private decl: ViewDeclaration | undefined
  private disposed = false

  constructor(
    private readonly client: Client,
    private readonly owner: Owner,
    readonly name: string,
    definition: AnyDef,
  ) {
    const { handler, load, ...meta } = definition
    this.def = meta
    this.handler = handler
    this.load = load
    this.id = client.id()
    if (!client.bridge) return
    // 定义不合法时同步抛出（与驱动层一致）；需要加载 zod 时异步解析。
    checkHandlerOrLoad(name, definition)
    // 门控变化：重新发送定义（`enabled` 为生效值）
    this.decl = new ViewDeclaration(name, definition, ownerChain(owner), pageDocument(), () => this.resend())
    const resolved = resolveInput(definition.input)
    const output = resolveOutput(definition.outputSchema)
    client.tools.set(this.id, this)
    owner.children.add(this)
    client.enqueue(async () => {
      const [r, o] = await Promise.all([resolved, output])
      if (this.disposed) return null
      this.schema = r.schema
      this.parse = r.parse
      this.outputSchema = o
      return { op: 'tool.register', id: this.id, ...scopeField(owner), name, spec: this.spec() }
    })
  }

  private spec(): ToolSpecMessage {
    const d = this.def
    return {
      description: d.description,
      ...(d.title !== undefined && { title: d.title }),
      ...(this.schema !== undefined && { inputSchema: this.schema }),
      ...(d.risk !== undefined && { risk: d.risk }),
      ...(d.annotations !== undefined && { annotations: d.annotations }),
      ...(this.outputSchema !== undefined && { outputSchema: this.outputSchema }),
      ...(d.activation !== undefined && { activation: d.activation }),
      ...((d.enabled !== undefined || this.decl?.gated) && { enabled: this.decl?.enabled ?? d.enabled }),
      ...(this.decl?.surface === 'view' && { surface: 'view' as const }),
      ...(this.decl?.page !== undefined && { page: this.decl.page }),
      ...(d.backgroundTool !== undefined && { backgroundTool: d.backgroundTool }),
      ...(d.concurrency !== undefined && { concurrency: d.concurrency }),
      ...(d.exclusive !== undefined && { exclusive: d.exclusive }),
    }
  }

  private resend(): void {
    if (this.disposed) return
    this.client.enqueue(() => (this.disposed ? null : { op: 'tool.update', id: this.id, spec: this.spec() }))
  }

  update(changes: Parameters<ToolHandle['update']>[0]): void {
    if (this.disposed) return
    const { handler: _h, load: _l, ...meta } = changes as Record<string, unknown>
    this.decl?.apply(changes)
    this.def = { ...this.def, ...meta }
    const resolved = 'input' in changes ? resolveInput(changes.input) : undefined
    const output = 'outputSchema' in changes ? resolveOutput(changes.outputSchema) : undefined
    this.client.enqueue(async () => {
      if (resolved) {
        const r = await resolved
        this.schema = r.schema
        this.parse = r.parse
      }
      if ('outputSchema' in changes) this.outputSchema = await output
      return this.disposed ? null : { op: 'tool.update', id: this.id, spec: this.spec() }
    })
  }

  setHandler(handler: ToolHandler<any, any>): void {
    this.handler = handler
    this.load = undefined
    this.loading = undefined
  }

  dispose(): void {
    if (this.disposed) return
    this.detach()
    this.owner.children.delete(this)
    this.client.enqueue(() => ({ op: 'tool.dispose', id: this.id }))
  }

  detach(): void {
    this.disposed = true
    this.decl?.dispose()
    this.client.tools.delete(this.id)
  }
}

export class ResourceEntry implements ResourceHandle, Detachable {
  readonly id: number
  private disposed = false

  constructor(
    private readonly client: Client,
    private readonly owner: Owner,
    readonly name: string,
    definition: ResourceDefinition<any>,
    public reader: ResourceDefinition<any>['read'],
  ) {
    this.id = client.id()
    if (!client.bridge) return
    client.resources.set(this.id, this)
    owner.children.add(this)
    client.enqueue(() => ({
      op: 'resource.register',
      id: this.id,
      ...scopeField(owner),
      name,
      description: definition.description,
      ...(definition.mimeType !== undefined && { mimeType: definition.mimeType }),
      ...(definition.realtime && { realtime: true }),
      ...(definition.annotations !== undefined && { annotations: toJsonValue(definition.annotations) as ContentAnnotations }),
    }))
  }

  notifyChanged(): void {
    if (this.disposed) return
    this.client.enqueue(() => ({ op: 'resource.notify', id: this.id }))
  }

  setReader(read: ResourceDefinition<any>['read']): void {
    this.reader = read
  }

  dispose(): void {
    if (this.disposed) return
    this.detach()
    this.owner.children.delete(this)
    this.client.enqueue(() => ({ op: 'resource.dispose', id: this.id }))
  }

  detach(): void {
    this.disposed = true
    this.client.resources.delete(this.id)
  }
}
