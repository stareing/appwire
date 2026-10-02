/**
 * attachDom：观察 root 下带 data-mcp-* / WebMCP 属性的元素，把它们登记为工具与资源。
 *
 * 性能：
 * - 只用 `querySelectorAll(SELECTOR)` 查询声明过的元素，不遍历整棵 DOM；
 * - MutationObserver 回调只做标记，每帧最多合并处理一次（rAF，后台标签页用定时器兜底）；
 * - 处理时与上次登记的定义逐字段比较，只在确有变化时调用 `update()` / `notifyChanged()`；
 * - 文本提取基于 DOM 树，不读取布局；可见性检查每个工具元素每批最多一次。
 */

import type {
  Activation,
  JsonSchema,
  OutputSchema,
  Registrar,
  ResourceHandle,
  Risk,
  Scope,
  ToolContext,
  ToolDefinition,
  ToolAnnotations,
  ToolHandle,
  ToolSurface,
} from '@app-mcp/web'
import { refreshViewTools, ToolCallError } from '@app-mcp/web'
import {
  ANNOTATION_ATTRS,
  ATTR,
  OBSERVED_ATTRIBUTES,
  SELECTOR,
  WEBMCP,
  attr,
  isActivation,
  isSurface,
  isForm,
  isRisk,
  isStandardForm,
  parseBooleanAttr,
  parseList,
  parseOutputSchemaAttr,
  toolNameOf,
} from './attrs'
import { assertValid, collectFields, fieldsToSchema, fillForm } from './form'
import { invoke } from './invoke'
import { toToolResult } from './result'
import { disabledReason } from './state'
import { truncate, visibleText } from './text'

/** 页面名规则（与工具名相同，spec/protocol.md 3.4）。 */
const PAGE_NAME_RE = /^[a-zA-Z0-9_.-]{1,64}$/

export interface AttachDomOptions {
  /** 观察范围，默认 `document.body`。 */
  root?: Element
  /** 是否登记精简页面快照资源，默认 true；名称默认 `ui.snapshot`。 */
  snapshot?: boolean | { name?: string }
  /** 点击 / 提交后等待页面稳定的时间（毫秒），默认 50。 */
  settleMs?: number
  /**
   * 页面结果（`data-mcp-result` 事件的 `detail`、`respondWith` 的值）按结构化结果解释，默认 false。
   * 开启时形如 `ToolResultEnvelope` 的结果原样透传（判定同 `isToolResultEnvelope`），没有结果时为无返回值
   * （Hub 输出"已完成"）；关闭时结果始终整体作为 `data`，没有结果时为 `{ ok: true }`。
   * @compat 默认关闭以保持既有行为。
   */
  resultEnvelope?: boolean
}

/** 断开观察并注销全部由 DOM 声明的工具与资源。 */
export type DetachDom = () => void

export const DEFAULT_SNAPSHOT_NAME = 'ui.snapshot'
const DEFAULT_TIMEOUT_MS = 10_000
const RESOURCE_TEXT_LIMIT = 4000
const LABEL_LIMIT = 40
const MAX_KEYS_IN_DESC = 30
/** rAF 在后台标签页暂停，用定时器兜底。 */
const FALLBACK_FLUSH_MS = 100
const ROW_SELECTOR = `[${ATTR.row}],li,tr,[role="row"],[role="listitem"],[role="option"]`

interface ScopeRec {
  el: Element
  name: string
  parent: ScopeRec | undefined
  scope: Scope
  disposed: boolean
}

interface Entry {
  el: Element
  key: string | undefined
  label: string
  reason: string | undefined
}

interface ToolDef {
  description: string
  title: string | undefined
  risk: Risk | undefined
  activation: Activation | undefined
  annotations: ToolAnnotations | undefined
  outputSchema: OutputSchema | undefined
  enabled: boolean
  input: JsonSchema
  surface: ToolSurface
  page: string | undefined
}

interface ToolSpec {
  name: string
  scope: ScopeRec | undefined
  isForm: boolean
  keyed: boolean
  entries: Entry[]
  reason: string | undefined
  def: ToolDef
  inputJson: string
  /** 快照中显示的参数（表单字段，必填带 *）。 */
  params: string[]
}

interface ToolRec {
  handle: ToolHandle
  scope: ScopeRec | undefined
  isForm: boolean
  applied: ToolDef
  appliedInput: string
  spec: ToolSpec
}

interface ResourceSpec {
  name: string
  el: Element
  scope: ScopeRec | undefined
  description: string
  mode: 'json' | 'text'
}

interface ResourceRec {
  handle: ResourceHandle
  spec: ResourceSpec
  last: string
}

function contentOf(spec: { el: Element; mode: 'json' | 'text' }): string {
  return spec.mode === 'json' ? (spec.el.getAttribute(ATTR.json) ?? '') : visibleText(spec.el, RESOURCE_TEXT_LIMIT)
}

function readResource(spec: { el: Element; mode: 'json' | 'text'; name: string }): unknown {
  if (spec.mode === 'text') return visibleText(spec.el, RESOURCE_TEXT_LIMIT)
  const raw = spec.el.getAttribute(ATTR.json) ?? ''
  try {
    return JSON.parse(raw)
  } catch {
    throw new ToolCallError('HANDLER_ERROR', `资源 ${spec.name} 的 data-mcp-json 不是合法 JSON`)
  }
}

class DomBinding {
  private readonly doc: Document
  private readonly root: Element
  private readonly settleMs: number
  private readonly resultEnvelope: boolean
  private readonly observer: MutationObserver
  private readonly scopes = new Map<Element, ScopeRec>()
  private readonly tools = new Map<string, ToolRec>()
  private readonly resources = new Map<string, ResourceRec>()
  private readonly warned = new Set<string>()
  private toolOrder: string[] = []
  private snapshot: { handle: ResourceHandle; last: string } | undefined
  private dirty = false
  private raf: number | undefined
  private timer: ReturnType<typeof setTimeout> | undefined
  private detached = false

  constructor(
    private readonly base: Registrar,
    options: AttachDomOptions,
  ) {
    const root = options.root ?? globalThis.document?.body
    if (!root) throw new Error('attachDom 需要 root 元素（当前环境没有 document.body）')
    this.root = root
    this.doc = root.ownerDocument
    this.settleMs = options.settleMs ?? 50
    this.resultEnvelope = options.resultEnvelope ?? false

    const win = this.doc.defaultView
    const MO = (win as (Window & typeof globalThis) | null)?.MutationObserver ?? MutationObserver
    this.observer = new MO(() => this.schedule())
    this.observer.observe(root, {
      subtree: true,
      childList: true,
      characterData: true,
      attributes: true,
      attributeFilter: OBSERVED_ATTRIBUTES,
    })

    if (options.snapshot !== false) {
      const name = (typeof options.snapshot === 'object' && options.snapshot.name) || DEFAULT_SNAPSHOT_NAME
      try {
        const handle = base.resource(name, {
          description: '页面精简快照：只列出页面声明的工具与资源及其当前状态（一行一个），用于了解当前页面能做什么',
          mimeType: 'text/plain',
          read: () => this.buildSnapshot(),
        })
        this.snapshot = { handle, last: '' }
      } catch (e) {
        this.warnOnce(`snapshot:${name}`, `[app-mcp/dom] 无法登记快照资源 ${name}：${errorMessage(e)}`)
      }
    }

    this.flush()
  }

  // ---- 调度 -----------------------------------------------------------

  private schedule(): void {
    if (this.dirty || this.detached) return
    this.dirty = true
    const win = this.doc.defaultView
    if (win && typeof win.requestAnimationFrame === 'function') {
      this.raf = win.requestAnimationFrame(() => this.flush())
    }
    this.timer = setTimeout(() => this.flush(), FALLBACK_FLUSH_MS)
  }

  private cancelScheduled(): void {
    if (this.raf !== undefined) this.doc.defaultView?.cancelAnimationFrame?.(this.raf)
    if (this.timer !== undefined) clearTimeout(this.timer)
    this.raf = undefined
    this.timer = undefined
    this.dirty = false
  }

  /** 与 DOM 同步：登记新声明、更新变化、注销消失的声明。 */
  flush(): void {
    this.cancelScheduled()
    if (this.detached) return
    const found: Element[] = []
    if (this.root.matches(SELECTOR)) found.push(this.root)
    found.push(...Array.from(this.root.querySelectorAll(SELECTOR)))

    this.syncScopes(found)
    this.syncTools(found)
    this.syncResources(found)

    if (this.snapshot) {
      const text = this.buildSnapshot()
      if (text !== this.snapshot.last) {
        const first = this.snapshot.last === ''
        this.snapshot.last = text
        if (!first) this.snapshot.handle.notifyChanged()
      }
    }
  }

  // ---- scope ----------------------------------------------------------

  /** 最近的 scope 元素（从 `from` 开始向上，限定在 root 内）。 */
  private scopeElOf(from: Element | null): Element | null {
    const el = from?.closest(`[${ATTR.scope}]`) ?? null
    return el && this.root.contains(el) ? el : null
  }

  private syncScopes(found: Element[]): void {
    const wanted = new Map<Element, { name: string; parentEl: Element | null }>()
    for (const el of found) {
      const name = attr(el, ATTR.scope)
      if (name) wanted.set(el, { name, parentEl: this.scopeElOf(el.parentElement) })
    }
    // 注销：元素消失、改名、父 scope 变化或父 scope 已注销（多轮直到稳定，处理级联）
    let changed = true
    while (changed) {
      changed = false
      for (const [el, rec] of this.scopes) {
        const w = wanted.get(el)
        const stale =
          rec.disposed || !w || w.name !== rec.name || (w.parentEl ?? null) !== (rec.parent?.el ?? null) || rec.parent?.disposed
        if (stale) {
          rec.disposed = true
          rec.scope.dispose()
          this.scopes.delete(el)
          changed = true
        }
      }
    }
    // 登记（文档顺序保证父 scope 先于子 scope）
    for (const [el, w] of wanted) {
      if (this.scopes.has(el)) continue
      const parent = w.parentEl ? this.scopes.get(w.parentEl) : undefined
      if (w.parentEl && !parent) continue
      try {
        const scope = (parent?.scope ?? this.base).scope(w.name)
        this.scopes.set(el, { el, name: w.name, parent, scope, disposed: false })
      } catch (e) {
        this.warnOnce(`scope:${w.name}`, `[app-mcp/dom] 无法创建 scope ${w.name}：${errorMessage(e)}`)
      }
    }
  }

  private scopeFor(el: Element): { rec: ScopeRec | undefined; ok: boolean } {
    const scopeEl = this.scopeElOf(el)
    if (!scopeEl) return { rec: undefined, ok: true }
    const rec = this.scopes.get(scopeEl)
    return { rec, ok: rec !== undefined }
  }

  // ---- 工具 -----------------------------------------------------------

  private syncTools(found: Element[]): void {
    const groups = new Map<string, Element[]>()
    for (const el of found) {
      const name = toolNameOf(el)?.trim()
      if (!name) continue
      const list = groups.get(name)
      if (list) list.push(el)
      else groups.set(name, [el])
    }
    const specs = new Map<string, ToolSpec>()
    for (const [name, els] of groups) {
      const spec = this.computeSpec(name, els)
      if (spec) specs.set(name, spec)
    }

    for (const [name, rec] of this.tools) {
      const spec = specs.get(name)
      if (!spec || spec.scope !== rec.scope || rec.scope?.disposed || spec.isForm !== rec.isForm) {
        rec.handle.dispose()
        this.tools.delete(name)
      }
    }

    const order: string[] = []
    for (const spec of specs.values()) {
      const rec = this.tools.get(spec.name)
      if (rec) {
        rec.spec = spec
        this.applyUpdate(rec, spec)
        order.push(spec.name)
      } else if (this.register(spec)) {
        order.push(spec.name)
      }
    }
    this.toolOrder = order
    // 条目（锚点）可能变化：立即重新评估 view 工具的可见性门控
    refreshViewTools(this.root.ownerDocument ?? undefined)
  }

  private computeSpec(name: string, els: Element[]): ToolSpec | undefined {
    const keyedEls = els.filter((el) => el.hasAttribute(ATTR.key))
    const keyed = keyedEls.length > 0
    if (keyed && keyedEls.length < els.length) {
      this.warnOnce(`mixed:${name}`, `[app-mcp/dom] 工具 ${name}：部分元素缺少 data-mcp-key，已忽略这些元素`)
    }
    const members = keyed ? keyedEls : els
    const rep = members[0] as Element
    const { rec: scope, ok } = this.scopeFor(rep)
    if (!ok) return undefined
    const form = isForm(rep)

    const entries: Entry[] = []
    const seen = new Set<string>()
    for (const el of members) {
      if (isForm(el) !== form) continue
      let key: string | undefined
      if (keyed) {
        key = el.getAttribute(ATTR.key) ?? ''
        if (seen.has(key)) {
          this.warnOnce(`dupkey:${name}:${key}`, `[app-mcp/dom] 工具 ${name}：data-mcp-key=${JSON.stringify(key)} 重复，只使用第一个`)
          continue
        }
        seen.add(key)
      }
      entries.push({ el, key, label: keyed ? this.labelOf(el) : '', reason: disabledReason(el) })
    }
    const usable = entries.filter((e) => !e.reason)
    const enabled = usable.length > 0
    const reason = enabled ? undefined : keyed && entries.length > 1 ? '没有可操作的条目' : entries[0]?.reason

    let description = this.descOf(rep, name)
    const properties: Record<string, unknown> = {}
    const required: string[] = []
    let params: string[] = []
    if (keyed) {
      const listed = usable.length > 0 ? usable : entries
      const shown = listed.slice(0, MAX_KEYS_IN_DESC).map((e) => (e.label ? `${e.key}=${e.label}` : `${e.key}`))
      const more = listed.length > shown.length ? ` 等 ${listed.length} 项` : ''
      description = `${description}（key：${shown.join(', ')}${more}）`
      properties.key = {
        type: 'string',
        enum: listed.map((e) => e.key as string),
        description: '要操作的条目',
      }
      required.push('key')
    }
    if (form) {
      const fields = collectFields(rep as HTMLFormElement)
      const s = fieldsToSchema(fields)
      for (const [k, v] of Object.entries(s.properties)) {
        if (k === 'key' && keyed) {
          this.warnOnce(`keyfield:${name}`, `[app-mcp/dom] 工具 ${name}：表单字段 key 与集合参数 key 冲突，已忽略该字段`)
          continue
        }
        properties[k] = v
      }
      required.push(...s.required.filter((k) => !(k === 'key' && keyed)))
      params = fields.filter((f) => !(f.name === 'key' && keyed)).map((f) => (f.required ? `${f.name}*` : f.name))
    }
    const input: JsonSchema = {
      type: 'object',
      properties,
      ...(required.length > 0 && { required }),
      additionalProperties: false,
    }

    const riskAttr = attr(rep, ATTR.risk)
    let risk: Risk | undefined
    if (riskAttr) {
      if (isRisk(riskAttr)) risk = riskAttr
      else this.warnOnce(`risk:${name}`, `[app-mcp/dom] 工具 ${name}：无效的 data-mcp-risk=${JSON.stringify(riskAttr)}，已忽略`)
    }
    const activationAttr = attr(rep, ATTR.activation)
    let activation: Activation | undefined
    if (activationAttr) {
      if (isActivation(activationAttr)) activation = activationAttr
      else
        this.warnOnce(
          `activation:${name}`,
          `[app-mcp/dom] 工具 ${name}：无效的 data-mcp-activation=${JSON.stringify(activationAttr)}，已忽略`,
        )
    }

    return {
      name,
      scope,
      isForm: form,
      keyed,
      entries,
      reason,
      params,
      def: {
        description,
        title: attr(rep, ATTR.title),
        risk,
        activation,
        annotations: this.annotationsOf(rep, name),
        outputSchema: this.outputSchemaOf(rep, name),
        enabled,
        input,
        surface: this.surfaceOf(rep, name),
        page: this.pageOf(rep, name),
      },
      inputJson: JSON.stringify(input),
    }
  }

  /** 标准 MCP 工具注解：data-mcp-readonly / destructive / idempotent / open-world；都缺省时为 undefined。 */
  private annotationsOf(el: Element, name: string): ToolAnnotations | undefined {
    const annotations: ToolAnnotations = {}
    for (const [attrName, hint] of ANNOTATION_ATTRS) {
      const parsed = parseBooleanAttr(el, attrName)
      if (parsed.kind === 'value') annotations[hint] = parsed.value
      if (parsed.kind === 'invalid')
        this.warnOnce(
          `${attrName}:${name}`,
          `[app-mcp/dom] 工具 ${name}：无效的 ${attrName}=${JSON.stringify(parsed.raw)}（应为空值、true 或 false），已忽略`,
        )
    }
    return Object.keys(annotations).length > 0 ? annotations : undefined
  }

  /** data-mcp-surface：缺省 `view`（元素工具依赖界面，SDK 按可见性 / 层级门控）；无效值警告并按缺省。 */
  private surfaceOf(el: Element, name: string): ToolSurface {
    const value = attr(el, ATTR.surface)
    if (value === undefined) return 'view'
    if (isSurface(value)) return value
    this.warnOnce(`surface:${name}`, `[app-mcp/dom] 工具 ${name}：无效的 ${ATTR.surface}=${JSON.stringify(value)}，按 view 处理`)
    return 'view'
  }

  /** data-mcp-page：元素或最近祖先上的页面名；名称不合法时警告并忽略。 */
  private pageOf(el: Element, name: string): string | undefined {
    const value = el.closest(`[${ATTR.page}]`)?.getAttribute(ATTR.page) ?? undefined
    if (value === undefined || PAGE_NAME_RE.test(value)) return value
    this.warnOnce(`page:${name}`, `[app-mcp/dom] 工具 ${name}：无效的 ${ATTR.page}=${JSON.stringify(value)}，已忽略`)
    return undefined
  }

  /** 结果 schema：data-mcp-output-schema（JSON 对象）。 */
  private outputSchemaOf(el: Element, name: string): OutputSchema | undefined {
    const parsed = parseOutputSchemaAttr(el)
    if (parsed.kind === 'invalid')
      this.warnOnce(
        `${ATTR.outputSchema}:${name}`,
        `[app-mcp/dom] 工具 ${name}：${ATTR.outputSchema} 不是 JSON 对象，已忽略`,
      )
    return parsed.kind === 'value' ? parsed.value : undefined
  }

  /** 描述：tooldescription（标准表单）> data-mcp-desc > 可见文本 / aria-label / title。 */
  private descOf(el: Element, name: string): string {
    const explicit = (isStandardForm(el) ? attr(el, WEBMCP.tooldescription) : undefined) ?? attr(el, ATTR.desc)
    if (explicit) return explicit
    const text = () => visibleText(el, 80)
    const candidates = isForm(el)
      ? [attr(el, 'aria-label'), attr(el, 'title'), text()]
      : [text(), attr(el, 'aria-label'), attr(el, 'title')]
    const fallback = candidates.find(Boolean) ?? name
    this.warnOnce(`desc:${name}`, `[app-mcp/dom] 工具 ${name} 缺少 data-mcp-desc，已使用 ${JSON.stringify(fallback)} 作为描述`)
    return fallback
  }

  /** 集合条目标签：data-mcp-label > 所在行的可见文本（不含其他工具元素与按钮）> 元素自身文本。 */
  private labelOf(el: Element): string {
    const explicit = attr(el, ATTR.label)
    if (explicit) return truncate(explicit, LABEL_LIMIT)
    const row = el.closest(ROW_SELECTOR)
    const skip = (x: Element): boolean =>
      x !== row &&
      (x.hasAttribute(ATTR.tool) ||
        isStandardForm(x) ||
        x.tagName === 'BUTTON' ||
        x.tagName === 'SELECT' ||
        x.tagName === 'TEXTAREA')
    let label = ''
    if (row && row !== el && this.root.contains(row)) label = visibleText(row, LABEL_LIMIT, skip)
    if (!label) label = visibleText(el, LABEL_LIMIT, (x) => x.tagName === 'SELECT' || x.tagName === 'TEXTAREA')
    return label
  }

  private register(spec: ToolSpec): boolean {
    const registrar: Registrar = spec.scope?.scope ?? this.base
    const d = spec.def
    const definition: ToolDefinition<Record<string, unknown>, unknown> = {
      description: d.description,
      input: d.input,
      enabled: d.enabled,
      surface: d.surface,
      // 注册返回前（门控首次求值时）记录尚未写入：用本次的 spec
      anchor: () => this.anchorOf(spec.name, spec),
      handler: (input, ctx) => this.call(spec.name, input, ctx),
    }
    if (d.title !== undefined) definition.title = d.title
    if (d.risk !== undefined) definition.risk = d.risk
    if (d.activation !== undefined) definition.activation = d.activation
    if (d.annotations !== undefined) definition.annotations = d.annotations
    if (d.outputSchema !== undefined) definition.outputSchema = d.outputSchema
    if (d.page !== undefined) definition.page = d.page
    let handle: ToolHandle
    try {
      handle = registrar.tool(spec.name, definition)
    } catch (e) {
      this.warnOnce(`register:${spec.name}`, `[app-mcp/dom] 无法登记工具 ${spec.name}：${errorMessage(e)}`)
      return false
    }
    this.tools.set(spec.name, {
      handle,
      scope: spec.scope,
      isForm: spec.isForm,
      applied: d,
      appliedInput: spec.inputJson,
      spec,
    })
    return true
  }

  /** 只提交变化的字段；显式 undefined 表示恢复默认（SDK 的 update 语义）。 */
  private applyUpdate(rec: ToolRec, spec: ToolSpec): void {
    const a = rec.applied
    const d = spec.def
    const changes: Partial<Omit<ToolDefinition<unknown, unknown>, 'handler'>> = {}
    if (d.description !== a.description) changes.description = d.description
    if (d.title !== a.title) changes.title = d.title
    if (d.risk !== a.risk) changes.risk = d.risk
    if (d.activation !== a.activation) changes.activation = d.activation
    if (!sameJson(d.annotations, a.annotations)) changes.annotations = d.annotations
    if (!sameJson(d.outputSchema, a.outputSchema)) changes.outputSchema = d.outputSchema
    if (d.enabled !== a.enabled) changes.enabled = d.enabled
    if (d.surface !== a.surface) changes.surface = d.surface
    if (d.page !== a.page) changes.page = d.page
    if (spec.inputJson !== rec.appliedInput) changes.input = d.input
    if (Object.keys(changes).length === 0) return
    rec.applied = d
    rec.appliedInput = spec.inputJson
    rec.handle.update(changes)
  }

  private anchorOf(name: string, fallback?: ToolSpec): Element | null {
    const spec = this.tools.get(name)?.spec ?? fallback
    if (!spec) return null
    return (spec.entries.find((e) => !e.reason) ?? spec.entries[0])?.el ?? null
  }

  private async call(name: string, raw: Record<string, unknown> | undefined, ctx: ToolContext): Promise<unknown> {
    if (this.dirty) this.flush()
    const rec = this.tools.get(name)
    if (!rec) throw new ToolCallError('TOOL_NOT_FOUND', `工具 ${name} 已不在页面上`)
    const spec = rec.spec
    const input: Record<string, unknown> = { ...(raw ?? {}) }

    let entry: Entry | undefined
    if (spec.keyed) {
      const key = input.key
      delete input.key
      if (typeof key !== 'string') throw new ToolCallError('INVALID_INPUT', '缺少参数 key')
      entry = spec.entries.find((e) => e.key === key)
      if (!entry) {
        const keys = spec.entries.filter((e) => !e.reason).map((e) => e.key)
        throw new ToolCallError('INVALID_INPUT', `key ${JSON.stringify(key)} 不存在；当前可用：${keys.join(', ') || '（无）'}`)
      }
      if (entry.reason) throw new ToolCallError('TOOL_DISABLED', `条目 ${key} 当前不可用：${entry.reason}`)
    } else {
      entry = spec.entries.find((e) => !e.reason)
      if (!entry) throw new ToolCallError('TOOL_DISABLED', `工具 ${name} 当前不可用：${spec.reason ?? '元素不可用'}`)
    }
    const el = entry.el
    // 调用前复查：页面可能在上次同步后刚刚禁用了元素
    const now = disabledReason(el)
    if (now) throw new ToolCallError('TOOL_DISABLED', `工具 ${name} 当前不可用：${now}`)

    let action: Parameters<typeof invoke>[0]['action']
    if (isForm(el)) {
      fillForm(collectFields(el), input)
      const autosubmit = el.hasAttribute(WEBMCP.toolautosubmit) || !isStandardForm(el)
      if (autosubmit) assertValid(el)
      action = { kind: 'form', form: el, autosubmit }
    } else {
      const extra = Object.keys(input)
      if (extra.length > 0) throw new ToolCallError('INVALID_INPUT', `工具 ${name} 不接受参数：${extra.join(', ')}`)
      action = { kind: 'click' }
    }

    const timeoutAttr = Number(attr(el, ATTR.timeout) ?? NaN)
    const outcome = await invoke({
      toolName: name,
      element: el,
      action,
      resultEvent: attr(el, ATTR.result),
      timeoutMs: Number.isFinite(timeoutAttr) && timeoutAttr > 0 ? timeoutAttr : DEFAULT_TIMEOUT_MS,
      settleMs: this.settleMs,
      signal: ctx.signal,
    })
    const fields = { hints: parseList(attr(el, ATTR.hints)), summary: attr(el, ATTR.summary) }
    return toToolResult(outcome, fields, this.resultEnvelope)
  }

  // ---- 资源 -----------------------------------------------------------

  private syncResources(found: Element[]): void {
    const specs = new Map<string, ResourceSpec>()
    for (const el of found) {
      const name = attr(el, ATTR.resource)
      if (!name) continue
      if (specs.has(name)) {
        this.warnOnce(`dupres:${name}`, `[app-mcp/dom] 资源 ${name} 重复声明，只使用第一个`)
        continue
      }
      const { rec: scope, ok } = this.scopeFor(el)
      if (!ok) continue
      let description = attr(el, ATTR.desc)
      if (!description) {
        description = attr(el, 'aria-label') ?? attr(el, 'title') ?? name
        this.warnOnce(`resdesc:${name}`, `[app-mcp/dom] 资源 ${name} 缺少 data-mcp-desc，已使用 ${JSON.stringify(description)}`)
      }
      specs.set(name, { name, el, scope, description, mode: el.hasAttribute(ATTR.json) ? 'json' : 'text' })
    }

    for (const [name, rec] of this.resources) {
      const spec = specs.get(name)
      const s = rec.spec
      if (
        !spec ||
        spec.scope !== s.scope ||
        s.scope?.disposed ||
        spec.description !== s.description ||
        spec.mode !== s.mode
      ) {
        rec.handle.dispose()
        this.resources.delete(name)
      }
    }

    for (const spec of specs.values()) {
      const rec = this.resources.get(spec.name)
      if (rec) {
        // 元素可能被替换（列表重新渲染），沿用同一登记，只切换读取的元素
        rec.spec = spec
        const content = contentOf(spec)
        if (content !== rec.last) {
          rec.last = content
          rec.handle.notifyChanged()
        }
        continue
      }
      const registrar: Registrar = spec.scope?.scope ?? this.base
      const newRec: ResourceRec = { handle: undefined as unknown as ResourceHandle, spec, last: contentOf(spec) }
      try {
        newRec.handle = registrar.resource(spec.name, {
          description: spec.description,
          mimeType: spec.mode === 'json' ? 'application/json' : 'text/plain',
          read: () => readResource(newRec.spec),
        })
      } catch (e) {
        this.warnOnce(`regres:${spec.name}`, `[app-mcp/dom] 无法登记资源 ${spec.name}：${errorMessage(e)}`)
        continue
      }
      this.resources.set(spec.name, newRec)
    }
  }

  // ---- 快照 -----------------------------------------------------------

  buildSnapshot(): string {
    if (this.dirty) this.flush()
    const title = this.doc.title?.trim() || '（无标题）'
    const url = this.doc.location?.href ?? this.doc.URL
    const lines = [`[页面] ${title} · ${url}`, '工具：']
    let count = 0
    for (const name of this.toolOrder) {
      const rec = this.tools.get(name)
      if (!rec) continue
      const spec = rec.spec
      const params = spec.params.length > 0 ? `（参数：${spec.params.join(', ')}）` : ''
      const state = spec.def.enabled ? '可用' : `不可用：${spec.reason ?? '元素不可用'}`
      lines.push(`- ${name}  ${spec.def.description}${params}  ${state}`)
      count++
    }
    if (count === 0) lines.push('- （无）')
    const resources = [...this.resources.keys()]
    lines.push(`资源：${resources.length > 0 ? resources.join('、') : '（无）'}`)
    return lines.join('\n')
  }

  // ---- 其他 -----------------------------------------------------------

  private warnOnce(key: string, message: string): void {
    if (this.warned.has(key)) return
    this.warned.add(key)
    console.warn(message)
  }

  detach(): void {
    if (this.detached) return
    this.detached = true
    this.observer.disconnect()
    this.cancelScheduled()
    for (const rec of this.tools.values()) rec.handle.dispose()
    for (const rec of this.resources.values()) rec.handle.dispose()
    // 先子后父（Map 按创建顺序，父先于子）
    for (const rec of [...this.scopes.values()].reverse()) {
      rec.disposed = true
      rec.scope.dispose()
    }
    this.snapshot?.handle.dispose()
    this.tools.clear()
    this.resources.clear()
    this.scopes.clear()
    this.snapshot = undefined
    this.toolOrder = []
  }
}

/** @why 注解与 schema 每批从属性重新解析成新对象，按序列化结果判断是否变化（键序由解析顺序固定）。 */
function sameJson(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b)
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/**
 * 把 root 下用 data-mcp-* 属性（以及 W3C WebMCP 表单属性）声明的元素登记为工具与资源。
 *
 * ```ts
 * const detach = attachDom(appMcp)
 * // …
 * detach()
 * ```
 */
export function attachDom(registrar: Registrar, options: AttachDomOptions = {}): DetachDom {
  const binding = new DomBinding(registrar, options)
  return () => binding.detach()
}
