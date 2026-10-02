/**
 * @app-mcp/inspect：没有声明工具的网页界面的兜底操作。
 *
 * 能力超集中的最低一级（语义工具 > 声明式 > 本兜底）。只在显式调用 `attachInspect` 时注册工具，
 * 不在后台观察 DOM，所有内容都在工具调用时计算。
 *
 * ```ts
 * import { attachInspect } from '@app-mcp/inspect'
 * if (import.meta.env.DEV) attachInspect(appMcp)
 * ```
 */

import { type Registrar, ToolCallError, type ToolHandle } from '@app-mcp/web'
import { click, fill, parseKey, press, scroll, settle, submit } from './actions'
import { collect, describe } from './collect'
import { diff, snapshot } from './diff'
import {
  accessibleName,
  classify,
  declaredTool,
  isDisabled,
  isSecure,
  isVisible,
  SECURE_MASK,
  visibleText,
  windowOf,
} from './dom'
import { type OutlineResult, renderOutline } from './outline'
import { REF_PATTERN, RefRegistry } from './refs'

export type { OutlineItem, OutlineResult } from './outline'

export interface InspectOptions {
  /** 大纲与操作的范围，默认 `document.body`（调用时取值）。 */
  root?: Element
  /** 工具名前缀，默认 'ui'。 */
  prefix?: string
  /** 大纲最多列出的元素数，默认 60。 */
  maxItems?: number
  /** 是否提供 `<prefix>.eval`（执行任意脚本，risk 'destructive'），默认 false。 */
  allowScript?: boolean
}

/** 操作类工具的结果：只返回变化摘要，不返回整页。 */
export interface ActionResult {
  ok: true
  changes: string[]
  /** 操作后 URL 有变化时给出。 */
  url?: string
  hint?: string
}

export interface ReadResult {
  ref: string
  text: string
  truncated: boolean
}

const MAX_CHANGES = 15
const READ_DEFAULT = 1000
const READ_MAX = 20000
const EVAL_MAX = 4000

const REF_PROP = { type: 'string', description: '元素引用，如 "e12"（来自 outline）' } as const

/**
 * 注册兜底操作工具：`<prefix>.outline`、`click`、`fill`、`press`、`scroll`、`submit`、`read`，
 * 以及（`allowScript: true` 时）`eval`。返回注销函数。
 */
export function attachInspect(registrar: Registrar, options: InspectOptions = {}): () => void {
  const prefix = options.prefix ?? 'ui'
  const maxItems = Math.max(1, Math.floor(options.maxItems ?? 60))
  const refs = new RefRegistry()
  const handles: ToolHandle[] = []

  const getRoot = (): Element => {
    if (options.root) return options.root
    if (typeof document === 'undefined' || !document.body) {
      throw new ToolCallError('HANDLER_ERROR', '页面尚未就绪（document.body 不存在）')
    }
    return document.body
  }

  const refArg = (input: unknown, key = 'ref', required = true): string | undefined => {
    const v = (input as Record<string, unknown> | null | undefined)?.[key]
    if (v === undefined || v === null || v === '') {
      if (required) throw new ToolCallError('INVALID_INPUT', `缺少参数 ${key}（元素引用，如 "e12"）`)
      return undefined
    }
    if (typeof v !== 'string' || !REF_PATTERN.test(v.trim())) {
      throw new ToolCallError('INVALID_INPUT', `${key} 应为元素引用，如 "e12"`)
    }
    return v.trim()
  }

  const resolve = (ref: string, root: Element): Element => {
    const el = refs.lookup(ref)
    if (!el || (el !== root && !root.contains(el))) {
      throw new ToolCallError('INVALID_INPUT', `引用 ${ref} 已失效，请重新调用 ${prefix}.outline`, { ref })
    }
    return el
  }

  const label = (el: Element, ref: string): string => {
    const info = classify(el)
    return info ? `${ref} ${describe({ info, name: accessibleName(el, info) })}` : ref
  }

  /** 操作前校验：可见且未禁用。 */
  const actionable = (ref: string, root: Element): Element => {
    const el = resolve(ref, root)
    if (!isVisible(el)) {
      throw new ToolCallError('INVALID_INPUT', `${label(el, ref)}当前不可见，请重新调用 ${prefix}.outline`, {
        ref,
        reason: 'hidden',
      })
    }
    if (isDisabled(el)) {
      throw new ToolCallError('INVALID_INPUT', `${label(el, ref)}已禁用，当前无法操作`, {
        ref,
        reason: 'TOOL_DISABLED',
      })
    }
    return el
  }

  /** 写入前核对（spec/ui-fallback.md 7.1）：密码类控件不填写、不按键。 */
  const rejectSecure = (el: Element, ref: string): void => {
    if (!isSecure(el)) return
    throw new ToolCallError('INVALID_INPUT', `${label(el, ref)}是密码类控件，兜底工具不填写`, { ref, reason: 'secure' })
  }

  const act = async (root: Element, el: Element | null, run: () => void | Promise<void>): Promise<ActionResult> => {
    const before = snapshot(root, refs)
    await run()
    await settle(windowOf(root))
    const after = snapshot(root, refs)
    const result: ActionResult = { ok: true, changes: diff(before, after, { max: MAX_CHANGES, prefix }) }
    if (after.url !== before.url) result.url = after.url
    const declared = el ? declaredTool(el) : undefined
    if (declared) result.hint = `该元素已声明为工具 ${declared}，下次可直接调用`
    return result
  }

  const tool = registrar.tool.bind(registrar)

  handles.push(
    tool<{ query?: string; within?: string; limit?: number }, OutlineResult>(`${prefix}.outline`, {
      title: '页面可交互元素大纲',
      description:
        '兜底能力：列出当前页面可见的可交互元素（按钮、链接、输入框等），每行一个，带引用 eN，按标题 / 区域分组。' +
        '页面已有对应的业务工具或标注 [已声明：…] 时请优先使用那些工具。引用用于 click / fill / press / submit / read。',
      risk: 'read',
      input: {
        type: 'object',
        properties: {
          query: { type: 'string', description: '按名称模糊过滤（空格分隔多个词，全部匹配）' },
          within: { type: 'string', description: '只列出该引用元素的子树，如 "e40"' },
          limit: { type: 'integer', minimum: 1, maximum: 500, description: `最多列出的元素数，默认 ${maxItems}` },
        },
        additionalProperties: false,
      },
      handler: (input) => {
        const root = getRoot()
        const within = refArg(input, 'within', false)
        const scope = within ? resolve(within, root) : root
        const q = input?.query
        if (q !== undefined && typeof q !== 'string') throw new ToolCallError('INVALID_INPUT', 'query 应为字符串')
        const limitRaw = input?.limit ?? maxItems
        if (typeof limitRaw !== 'number' || !Number.isFinite(limitRaw)) {
          throw new ToolCallError('INVALID_INPUT', 'limit 应为正整数')
        }
        const limit = Math.min(500, Math.max(1, Math.floor(limitRaw)))
        return renderOutline(collect(scope, refs), { query: q, limit, prefix })
      },
    }),
  )

  handles.push(
    tool<{ ref: string }, ActionResult>(`${prefix}.click`, {
      title: '点击元素',
      description: `兜底能力：点击 ${prefix}.outline 中的元素，返回页面变化摘要。`,
      risk: 'write',
      input: { type: 'object', properties: { ref: REF_PROP }, required: ['ref'], additionalProperties: false },
      handler: (input) => {
        const root = getRoot()
        const el = actionable(refArg(input) as string, root)
        return act(root, el, () => click(el))
      },
    }),
  )

  handles.push(
    tool<{ ref: string; value: unknown }, ActionResult>(`${prefix}.fill`, {
      title: '填写控件',
      description:
        `兜底能力：填写输入框 / 多行输入框 / 下拉框（按选项值或文本匹配，多选传数组）/ 复选框与单选框（true / false）。` +
        '兼容受控组件。返回页面变化摘要。',
      risk: 'write',
      input: {
        type: 'object',
        properties: {
          ref: REF_PROP,
          value: {
            description: '文本、数字、布尔（复选框 / 单选框）或字符串数组（多选下拉框）',
            anyOf: [
              { type: 'string' },
              { type: 'number' },
              { type: 'boolean' },
              { type: 'array', items: { type: 'string' } },
            ],
          },
        },
        required: ['ref', 'value'],
        additionalProperties: false,
      },
      handler: (input) => {
        const root = getRoot()
        const ref = refArg(input) as string
        const el = actionable(ref, root)
        rejectSecure(el, ref)
        if (!input || !('value' in input)) throw new ToolCallError('INVALID_INPUT', '缺少参数 value')
        return act(root, el, () => fill(el, input.value, ref, prefix))
      },
    }),
  )

  handles.push(
    tool<{ ref?: string; key: string }, ActionResult>(`${prefix}.press`, {
      title: '按键',
      description:
        '兜底能力：向元素（缺省为当前焦点元素）派发按键，如 "Enter"、"Escape"、"Tab"、"Control+a"。' +
        '模拟 Enter 提交表单 / 激活按钮、空格激活按钮与复选框、Tab 移动焦点。输入文本请用 fill。返回页面变化摘要。',
      risk: 'write',
      input: {
        type: 'object',
        properties: {
          ref: { ...REF_PROP, description: '目标元素引用；缺省为当前焦点元素' },
          key: { type: 'string', description: '按键，如 "Enter"、"Escape"、"Shift+Tab"、"Control+a"' },
        },
        required: ['key'],
        additionalProperties: false,
      },
      handler: (input) => {
        const root = getRoot()
        const key = input?.key
        if (typeof key !== 'string') throw new ToolCallError('INVALID_INPUT', '缺少参数 key')
        const spec = parseKey(key)
        const ref = refArg(input, 'ref', false)
        let target: Element
        if (ref) {
          target = actionable(ref, root)
        } else {
          const active = root.ownerDocument.activeElement
          target = active && active !== root.ownerDocument.documentElement ? active : root
        }
        rejectSecure(target, ref ?? refs.refOf(target))
        const usable = (el: Element) => root.contains(el) && isVisible(el) && !isDisabled(el)
        return act(root, ref ? target : null, () => press(target, spec, usable))
      },
    }),
  )

  handles.push(
    tool<{ ref: string }, ActionResult>(`${prefix}.scroll`, {
      title: '滚动到元素',
      description: '兜底能力：把元素滚动到视口中央（可能触发懒加载），返回页面变化摘要。',
      risk: 'write',
      input: { type: 'object', properties: { ref: REF_PROP }, required: ['ref'], additionalProperties: false },
      handler: (input) => {
        const root = getRoot()
        const ref = refArg(input) as string
        const el = resolve(ref, root)
        if (!isVisible(el)) throw new ToolCallError('INVALID_INPUT', `${label(el, ref)}当前不可见`, { ref })
        return act(root, null, () => scroll(el))
      },
    }),
  )

  handles.push(
    tool<{ ref: string }, ActionResult>(`${prefix}.submit`, {
      title: '提交表单',
      description:
        '兜底能力：提交表单（ref 为表单本身或其中任意元素，提交按钮会作为 submitter），先做浏览器表单校验。返回页面变化摘要。',
      risk: 'write',
      input: { type: 'object', properties: { ref: REF_PROP }, required: ['ref'], additionalProperties: false },
      handler: async (input) => {
        const root = getRoot()
        const ref = refArg(input) as string
        const el = actionable(ref, root)
        const form = el.tagName === 'FORM' ? el : ((el as HTMLInputElement).form ?? el.closest('form'))
        let invalid: ReturnType<typeof submit> = []
        const result = await act(root, form, () => {
          invalid = submit(el, ref)
        })
        if (invalid.length > 0) {
          const list = invalid
            .slice(0, 10)
            .map((f) => `${label(f.el, refs.refOf(f.el))}：${f.message}`)
            .join('；')
          throw new ToolCallError('INVALID_INPUT', `表单校验未通过，未提交。${list}`, { ref })
        }
        return result
      },
    }),
  )

  handles.push(
    tool<{ ref?: string; maxChars?: number }, ReadResult>(`${prefix}.read`, {
      title: '读取元素文本',
      description: `兜底能力：读取元素的可见文本（折叠空白，默认最多 ${READ_DEFAULT} 字）；ref 缺省为整个页面范围。`,
      risk: 'read',
      input: {
        type: 'object',
        properties: {
          ref: { ...REF_PROP, description: '元素引用；缺省为整个范围' },
          maxChars: { type: 'integer', minimum: 1, maximum: READ_MAX, description: `默认 ${READ_DEFAULT}` },
        },
        additionalProperties: false,
      },
      handler: (input) => {
        const root = getRoot()
        const ref = refArg(input, 'ref', false)
        const el = ref ? resolve(ref, root) : root
        const raw = input?.maxChars ?? READ_DEFAULT
        if (typeof raw !== 'number' || !Number.isFinite(raw)) throw new ToolCallError('INVALID_INPUT', 'maxChars 应为正整数')
        const max = Math.min(READ_MAX, Math.max(1, Math.floor(raw)))
        const tag = el.tagName
        if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') {
          const v =
            tag === 'SELECT'
              ? Array.from((el as HTMLSelectElement).selectedOptions).map((o) => o.text.trim()).join('、')
              : isSecure(el)
                ? (el as HTMLInputElement).value
                  ? SECURE_MASK
                  : ''
                : (el as HTMLInputElement).value
          return { ref: ref ?? 'root', text: v.length > max ? `${v.slice(0, max - 1)}…` : v, truncated: v.length > max }
        }
        const { text, truncated } = visibleText(el, max, { computed: true })
        return { ref: ref ?? 'root', text, truncated }
      },
    }),
  )

  if (options.allowScript) {
    handles.push(
      tool<{ code: string }, { result: unknown; changes: string[]; url?: string }>(`${prefix}.eval`, {
        title: '执行脚本',
        description:
          '在页面中执行任意 JavaScript（表达式或带 return 的语句块，可 await）。可用变量：root（范围根元素）、' +
          'el("e12")（按引用取元素）。元素结果会转为引用。危险：可读写页面上的任何数据。',
        risk: 'destructive',
        input: {
          type: 'object',
          properties: { code: { type: 'string', description: 'JavaScript 代码' } },
          required: ['code'],
          additionalProperties: false,
        },
        handler: async (input) => {
          const root = getRoot()
          const code = input?.code
          if (typeof code !== 'string' || !code.trim()) throw new ToolCallError('INVALID_INPUT', '缺少参数 code')
          const fn = compile(code)
          const el = (ref: string) => resolve(ref, root)
          let value: unknown
          const result = await act(root, null, async () => {
            try {
              value = await fn(root, el)
            } catch (err) {
              if (err instanceof ToolCallError) throw err
              throw new ToolCallError('HANDLER_ERROR', `脚本出错：${err instanceof Error ? err.message : String(err)}`)
            }
          })
          const out: { result: unknown; changes: string[]; url?: string } = {
            result: serialize(value, refs),
            changes: result.changes,
          }
          if (result.url) out.url = result.url
          return out
        },
      }),
    )
  }

  let detached = false
  return () => {
    if (detached) return
    detached = true
    for (const h of handles) h.dispose()
    handles.length = 0
    refs.clear()
  }
}

type ScriptFn = (root: Element, el: (ref: string) => Element) => Promise<unknown>

function compile(code: string): ScriptFn {
  const AsyncFunction = Object.getPrototypeOf(async () => {}).constructor as new (
    ...args: string[]
  ) => ScriptFn
  try {
    return new AsyncFunction('root', 'el', `return (${code}\n)`)
  } catch {
    // 不是表达式，按语句块处理
  }
  try {
    return new AsyncFunction('root', 'el', code)
  } catch (err) {
    throw new ToolCallError('INVALID_INPUT', `脚本无法编译：${err instanceof Error ? err.message : String(err)}`)
  }
}

/** 把脚本结果转为可 JSON 序列化的精简值：元素转为引用，超长时截断。 */
function serialize(value: unknown, refs: RefRegistry): unknown {
  const seen = new WeakSet<object>()
  const conv = (v: unknown, depth: number): unknown => {
    if (v === undefined) return null
    if (v === null || typeof v === 'boolean' || typeof v === 'string') return v
    if (typeof v === 'number') return Number.isFinite(v) ? v : String(v)
    if (typeof v === 'bigint') return v.toString()
    if (typeof v === 'function' || typeof v === 'symbol') return `[${typeof v}]`
    if (typeof v !== 'object') return String(v)
    const Elem = typeof Element === 'undefined' ? undefined : Element
    if (Elem && v instanceof Elem) {
      const info = classify(v)
      const ref = refs.refOf(v)
      return info ? `${ref} ${describe({ info, name: accessibleName(v, info) })}` : `${ref} <${v.tagName.toLowerCase()}>`
    }
    if (seen.has(v)) return '[循环引用]'
    seen.add(v)
    if (depth > 4) return '[…]'
    if (Array.isArray(v) || (typeof (v as { length?: unknown }).length === 'number' && Symbol.iterator in v)) {
      const arr = Array.from(v as unknown as ArrayLike<unknown>)
      const out = arr.slice(0, 50).map((x) => conv(x, depth + 1))
      if (arr.length > 50) out.push(`…另有 ${arr.length - 50} 项`)
      return out
    }
    const out: Record<string, unknown> = {}
    for (const [k, x] of Object.entries(v).slice(0, 50)) out[k] = conv(x, depth + 1)
    return out
  }
  const converted = conv(value, 0)
  const json = JSON.stringify(converted) ?? 'null'
  if (json.length <= EVAL_MAX) return converted
  return `${json.slice(0, EVAL_MAX - 1)}…（已截断，共 ${json.length} 字符）`
}
