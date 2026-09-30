/**
 * 变化摘要：操作前后各收集一次大纲条目，只比较大纲范围内的元素，输出简短的变化描述。
 */

import { collect, describe, type Entry } from './collect'
import type { RefRegistry } from './refs'

export interface Snapshot {
  url: string
  entries: Entry[]
  byEl: Map<Element, Entry>
}

export function snapshot(root: Element, refs: RefRegistry): Snapshot {
  const entries = collect(root, refs)
  const byEl = new Map<Element, Entry>()
  for (const e of entries) byEl.set(e.el, e)
  const win = root.ownerDocument.defaultView
  return { url: win?.location.href ?? '', entries, byEl }
}

/** 焦点变化不算作页面变化（点击按钮必然引起）。 */
const IGNORED_STATES = new Set(['focused'])
/** 互斥状态：出现新状态时不再单独说明旧状态消失。 */
const EXCLUSIVE = [
  ['checked', 'unchecked', 'mixed'],
  ['expanded', 'collapsed'],
]

function stateChange(before: string[], after: string[]): string[] {
  const b = before.filter((s) => !IGNORED_STATES.has(s))
  const a = after.filter((s) => !IGNORED_STATES.has(s))
  const added = a.filter((s) => !b.includes(s))
  let removed = b.filter((s) => !a.includes(s))
  removed = removed.filter((r) => !EXCLUSIVE.some((g) => g.includes(r) && added.some((x) => g.includes(x))))
  const parts: string[] = []
  if (added.length > 0) parts.push(`变为 ${added.join('、')}`)
  if (removed.length > 0) parts.push(`不再 ${removed.join('、')}`)
  return parts
}

function visibleStates(e: Entry): string {
  const s = e.states.filter((x) => !IGNORED_STATES.has(x))
  return s.length > 0 ? ` ${s.join(' ')}` : ''
}

function label(e: Entry): string {
  return e.kind === 'heading' ? `标题「${e.name}」` : `${describe(e)}(${e.ref})`
}

/** 外层最先出现的"同样是新增 / 同样被移除"的祖先容器。 */
function outerChanged(e: Entry, set: Map<Element, Entry>, other: Map<Element, Entry>): Element | undefined {
  return e.containers.find((c) => set.has(c) && !other.has(c))
}

export interface DiffOptions {
  max: number
  prefix: string
}

export function diff(before: Snapshot, after: Snapshot, opts: DiffOptions): string[] {
  const out: string[] = []

  // 新增：属于新增容器的元素只计数
  const addedChildren = new Map<Element, number>()
  for (const e of after.entries) {
    if (before.byEl.has(e.el)) continue
    const outer = outerChanged(e, after.byEl, before.byEl)
    if (outer) addedChildren.set(outer, (addedChildren.get(outer) ?? 0) + (e.kind === 'item' ? 1 : 0))
  }
  const removedChildren = new Map<Element, number>()
  for (const e of before.entries) {
    if (after.byEl.has(e.el)) continue
    const outer = outerChanged(e, before.byEl, after.byEl)
    if (outer) removedChildren.set(outer, (removedChildren.get(outer) ?? 0) + (e.kind === 'item' ? 1 : 0))
  }

  for (const e of after.entries) {
    const old = before.byEl.get(e.el)
    if (!old) {
      if (outerChanged(e, after.byEl, before.byEl)) continue
      let msg = `新增${label(e)}`
      if (e.kind === 'item') {
        if (e.value !== undefined) msg += ` = "${e.value}"`
        msg += visibleStates(e)
      }
      const n = addedChildren.get(e.el)
      if (n) msg += `，含 ${n} 个可交互元素（可用 ${opts.prefix}.outline({ within: "${e.ref}" }) 查看）`
      out.push(msg)
      continue
    }
    if (e.kind === 'heading') {
      if (old.name !== e.name) out.push(`标题「${old.name}」变为「${e.name}」`)
      continue
    }
    const parts: string[] = []
    if (old.name !== e.name) parts.push(`名称变为「${e.name}」`)
    if (old.value !== e.value) parts.push(e.value === undefined ? '值已清空' : `值变为 "${e.value}"`)
    parts.push(...stateChange(old.states, e.states))
    if (parts.length > 0) out.push(`${e.ref} ${old.name || old.info.label} ${parts.join('，')}`)
  }

  for (const e of before.entries) {
    if (after.byEl.has(e.el)) continue
    if (outerChanged(e, before.byEl, after.byEl)) continue
    const n = removedChildren.get(e.el)
    out.push(`${label(e)} 已消失${n ? `（含 ${n} 个可交互元素）` : ''}`)
  }

  if (out.length > opts.max) {
    const rest = out.length - opts.max
    out.length = opts.max
    out.push(`…另有 ${rest} 项变化，请调用 ${opts.prefix}.outline 查看`)
  }
  return out
}
