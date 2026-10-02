/**
 * 工具的界面声明（spec/protocol.md 3.4）：`surface` / `page` / `visibility` / `anchor` 的继承解析、启用意愿与可见性门控。
 * 驱动层（WASM 核心）与桥接实现（Electron / Tauri 页面侧）共用；对 Host 可见 = App 启用 且（`view` 工具）门控为真。
 */

import type { ScopeOptions, ToolDefinition, ToolSurface, ViewLayer, ViewVisibility } from '../types'
import { type ViewGate, viewTracker } from './tracker'

const NAME_RE = /^[a-zA-Z0-9_.-]{1,64}$/

/** 工具定义中的界面字段与启用意愿。 */
export type ViewFields = Pick<ToolDefinition, 'surface' | 'page' | 'visibility' | 'anchor' | 'enabled'>

/** 从最近的 scope 到最远的 scope 依次给出 scope 的界面声明。 */
export type ScopeChain = () => Iterable<ScopeOptions | undefined>

/**
 * 最近的 scope 中声明的值。`page` 在声明了 `layer` 的 scope 处截断：层内工具不继承层外的页面
 * （层只在打开时存在，不能作为导航目标）。
 */
export function inheritedOption<K extends keyof ScopeOptions>(chain: ScopeChain, key: K): ScopeOptions[K] {
  for (const options of chain()) {
    const value = options?.[key]
    if (value !== undefined) return value
    if (key === 'page' && options?.layer !== undefined) return undefined
  }
  return undefined
}

/** 页面名与工具名同规则。@error 不合法时抛出。 */
export function checkPageName(owner: string, page: string | undefined): void {
  if (page !== undefined && !NAME_RE.test(page)) {
    throw new Error(`${owner} 的页面名 ${JSON.stringify(page)} 无效：应匹配 [a-zA-Z0-9_.-]{1,64}`)
  }
}

function anchorResolver(anchor: ToolDefinition['anchor']): (() => Element | null) | undefined {
  if (anchor === undefined) return undefined
  return typeof anchor === 'function' ? anchor : () => anchor
}

/** {@link ViewDeclaration.apply} 的结果：哪些协议字段需要更新。 */
export interface ViewChange {
  surface: boolean
  page: boolean
  /** 生效的启用状态可能变化（启用意愿、门控输入变化）。 */
  enabled: boolean
}

export class ViewDeclaration {
  surface: ToolSurface
  page: string | undefined
  visibility: ViewVisibility
  userEnabled: boolean
  private anchor: ToolDefinition['anchor']
  private gate: ViewGate | undefined
  private disposed = false

  /**
   * @input owner 错误信息中的工具名；chain 所在 scope 链；doc 门控所在文档（没有时不门控）；
   *   onActiveChange 门控结果变化时调用（不在构造中调用）。
   * @error 页面名不合法时抛出。
   */
  constructor(
    private readonly owner: string,
    def: ViewFields,
    private readonly chain: ScopeChain,
    private readonly doc: Document | undefined,
    private readonly onActiveChange: () => void,
  ) {
    this.surface = def.surface ?? inheritedOption(chain, 'surface') ?? 'app'
    this.page = def.page ?? inheritedOption(chain, 'page')
    checkPageName(owner, this.page)
    this.visibility = def.visibility ?? inheritedOption(chain, 'visibility') ?? 'auto'
    this.userEnabled = def.enabled ?? true
    this.anchor = def.anchor
    this.syncGate()
  }

  /** 门控是否生效（`view` 工具、未关闭门控、有文档）。 */
  get gated(): boolean {
    return this.gate !== undefined
  }

  /** 对 Host 可见。 */
  get enabled(): boolean {
    return this.userEnabled && (this.gate?.active ?? true)
  }

  /** 应用 `update()` 中出现的字段（显式 `undefined` 恢复为继承值 / 缺省）。@error 页面名不合法时抛出（不修改状态）。 */
  apply(changes: Partial<ViewFields>): ViewChange {
    const change: ViewChange = { surface: false, page: false, enabled: false }
    if ('page' in changes) {
      const page = changes.page ?? inheritedOption(this.chain, 'page')
      checkPageName(this.owner, page)
      this.page = page
      change.page = true
    }
    let gateInput = false
    if ('anchor' in changes) {
      this.anchor = changes.anchor
      gateInput = true
    }
    if ('surface' in changes) {
      this.surface = changes.surface ?? inheritedOption(this.chain, 'surface') ?? 'app'
      change.surface = true
      gateInput = true
    }
    if ('visibility' in changes) {
      this.visibility = changes.visibility ?? inheritedOption(this.chain, 'visibility') ?? 'auto'
      gateInput = true
    }
    if ('enabled' in changes) {
      this.userEnabled = changes.enabled ?? true
      change.enabled = true
    }
    if (gateInput) {
      this.syncGate()
      change.enabled = true
    }
    return change
  }

  dispose(): void {
    this.disposed = true
    this.gate?.dispose()
    this.gate = undefined
  }

  private syncGate(): void {
    const doc = this.doc
    if (this.disposed || this.surface !== 'view' || this.visibility === 'always' || !doc) {
      this.gate?.dispose()
      this.gate = undefined
      return
    }
    const spec = {
      anchor: anchorResolver(this.anchor ?? inheritedOption(this.chain, 'anchor')),
      layer: inheritedOption(this.chain, 'layer') as ViewLayer | undefined,
    }
    if (this.gate) this.gate.update(spec)
    else this.gate = viewTracker(doc).gate(spec, () => this.onActiveChange())
  }
}
