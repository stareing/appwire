/**
 * 每个文档一份的界面跟踪器：层栈（{@link ViewLayer}）与 `view` 工具的可见性门控（spec/protocol.md 3.4）。
 *
 * - 门控（{@link ViewGate}）在以下时机重新求值：层打开 / 关闭（同步）、`visibilitychange`、锚点的
 *   IntersectionObserver 回调、`inert` / `hidden` / `open` 属性变化（MutationObserver），后三者合并到一个微任务。
 * - 观察器在第一个门控创建时启动，最后一个门控注销时停止。
 *
 * @invariant 门控的 `active` 只在 {@link ViewTracker.evaluateAll} 中改变，改变时恰好调用一次 `onChange`。
 */

import type { ViewLayer } from '../types'
import { elementVisible } from './visible'

/** 门控的输入：锚点（`undefined` = 未声明锚点）与所属层（`undefined` = 基础层）。 */
export interface GateSpec {
  anchor: (() => Element | null) | undefined
  layer: ViewLayer | undefined
}

export interface ViewGate {
  /** 当前是否启用。 */
  readonly active: boolean
  /** 修改锚点 / 层后立即重新求值（可能同步触发 `onChange`）。 */
  update(spec: GateSpec): void
  dispose(): void
}

interface GateRec {
  spec: GateSpec
  active: boolean
  observed: Element | null
  onChange: (active: boolean) => void
}

/** 观察 DOM 属性变化中会影响可见性的部分（`style` / `class` 由 IntersectionObserver 兜底，避免频繁求值）。 */
const WATCHED_ATTRIBUTES = ['inert', 'hidden', 'open']

function resolveAnchor(anchor: () => Element | null): Element | null {
  try {
    return anchor()
  } catch {
    return null
  }
}

export class ViewTracker {
  private readonly layerStack: LayerHandle[] = []
  private readonly gates = new Set<GateRec>()
  private readonly observedCount = new Map<Element, number>()
  private io: IntersectionObserver | undefined
  private mo: MutationObserver | undefined
  private listening = false
  private scheduled = false
  private readonly onVisibility = (): void => this.schedule()

  constructor(readonly doc: Document) {}

  // ---- 层 -------------------------------------------------------------

  /** 打开的层，自下而上。 */
  get layers(): readonly ViewLayer[] {
    return this.layerStack
  }

  get top(): ViewLayer | undefined {
    return this.layerStack[this.layerStack.length - 1]
  }

  pushLayer(layer: LayerHandle): void {
    if (this.layerStack.includes(layer)) return
    this.layerStack.push(layer)
    this.evaluateAll()
  }

  removeLayer(layer: LayerHandle): void {
    const i = this.layerStack.indexOf(layer)
    if (i < 0) return
    this.layerStack.splice(i, 1)
    this.evaluateAll()
  }

  // ---- 门控 -----------------------------------------------------------

  gate(spec: GateSpec, onChange: (active: boolean) => void): ViewGate {
    const rec: GateRec = { spec, active: false, observed: null, onChange }
    this.gates.add(rec)
    this.start()
    rec.active = this.evaluate(rec)
    return {
      get active() {
        return rec.active
      },
      update: (next) => {
        if (!this.gates.has(rec)) return
        rec.spec = next
        this.apply(rec)
      },
      dispose: () => {
        if (!this.gates.delete(rec)) return
        this.observe(rec, null)
        if (this.gates.size === 0) this.stop()
      },
    }
  }

  /** 立即重新求值全部门控。 */
  evaluateAll(): void {
    for (const rec of [...this.gates]) {
      if (this.gates.has(rec)) this.apply(rec)
    }
  }

  private apply(rec: GateRec): void {
    const next = this.evaluate(rec)
    if (next === rec.active) return
    rec.active = next
    rec.onChange(next)
  }

  /** 页面可见 → 处于最上层 → 锚点（声明了时）可见。 */
  private evaluate(rec: GateRec): boolean {
    const { anchor, layer } = rec.spec
    const el = anchor ? resolveAnchor(anchor) : null
    this.observe(rec, el)
    if (this.doc.visibilityState === 'hidden') return false
    if (layer ? this.top !== layer : this.top !== undefined) return false
    if (!anchor) return true
    return el !== null && elementVisible(el)
  }

  // ---- 观察器 ---------------------------------------------------------

  private schedule(): void {
    if (this.scheduled) return
    this.scheduled = true
    queueMicrotask(() => {
      this.scheduled = false
      this.evaluateAll()
    })
  }

  private start(): void {
    if (this.listening) return
    this.listening = true
    const win = this.doc.defaultView
    this.doc.addEventListener('visibilitychange', this.onVisibility)
    if (win && typeof win.IntersectionObserver === 'function') {
      this.io = new win.IntersectionObserver(() => this.schedule())
    }
    if (win && typeof win.MutationObserver === 'function' && this.doc.documentElement) {
      this.mo = new win.MutationObserver(() => this.schedule())
      this.mo.observe(this.doc.documentElement, { attributes: true, subtree: true, attributeFilter: WATCHED_ATTRIBUTES })
    }
  }

  private stop(): void {
    if (!this.listening) return
    this.listening = false
    this.doc.removeEventListener('visibilitychange', this.onVisibility)
    this.io?.disconnect()
    this.io = undefined
    this.mo?.disconnect()
    this.mo = undefined
    this.observedCount.clear()
  }

  /** 让 IntersectionObserver 跟随门控当前的锚点元素（引用计数，多个门控共用同一元素）。 */
  private observe(rec: GateRec, el: Element | null): void {
    if (rec.observed === el) return
    const prev = rec.observed
    rec.observed = el
    if (prev) {
      const n = (this.observedCount.get(prev) ?? 1) - 1
      if (n <= 0) {
        this.observedCount.delete(prev)
        this.io?.unobserve(prev)
      } else this.observedCount.set(prev, n)
    }
    if (el) {
      const n = this.observedCount.get(el) ?? 0
      this.observedCount.set(el, n + 1)
      if (n === 0) this.io?.observe(el)
    }
  }
}

/** {@link ViewLayer} 的实现：没有文档（服务端渲染）时只记录打开状态，永远不是最上层。 */
export class LayerHandle implements ViewLayer {
  private opened = false

  constructor(
    readonly name: string,
    private readonly tracker: ViewTracker | undefined,
  ) {}

  get isOpen(): boolean {
    return this.opened
  }

  get isTop(): boolean {
    return this.opened && this.tracker !== undefined && this.tracker.top === this
  }

  open(): void {
    if (this.opened) return
    this.opened = true
    this.tracker?.pushLayer(this)
  }

  close(): void {
    if (!this.opened) return
    this.opened = false
    this.tracker?.removeLayer(this)
  }
}

const trackers = new WeakMap<Document, ViewTracker>()

/** 取得（必要时创建）文档的跟踪器。 */
export function viewTracker(doc: Document): ViewTracker {
  let tracker = trackers.get(doc)
  if (!tracker) {
    tracker = new ViewTracker(doc)
    trackers.set(doc, tracker)
  }
  return tracker
}

function defaultDocument(): Document | undefined {
  return typeof document === 'undefined' ? undefined : document
}

/**
 * 创建界面层（对话框、抽屉、模态框），打开后压在之前打开的层之上。用法：层打开时 `open()`、关闭时 `close()`，
 * 层内工具经 `appMcp.scope(name, { layer })` 注册（React 用 `<ToolLayer>`）。
 *
 * @input doc 层所在文档，缺省为全局 `document`；没有文档时返回的层永远不是最上层。
 */
export function createViewLayer(name: string, doc: Document | undefined = defaultDocument()): ViewLayer {
  return new LayerHandle(name, doc ? viewTracker(doc) : undefined)
}

/** 文档中当前打开的层（自下而上）。 */
export function openViewLayers(doc: Document | undefined = defaultDocument()): readonly ViewLayer[] {
  return doc ? viewTracker(doc).layers : []
}

/**
 * 立即重新评估文档中全部 `view` 工具的可见性。界面以门控观察不到的方式变化（如只改 `style` / `class`
 * 且不影响视口相交）后可以调用；通常不需要。
 */
export function refreshViewTools(doc: Document | undefined = defaultDocument()): void {
  if (doc) viewTracker(doc).evaluateAll()
}
