/**
 * 大纲渲染：过滤、截断、分组，每个元素一行。
 */

import { describe, type Entry } from './collect'

export interface OutlineItem {
  ref: string
  /** ARIA 风格的英文角色名。 */
  role: string
  name: string
  value?: string
  href?: string
  states?: string[]
  required?: boolean
  /** 已声明的工具名：请优先直接调用该工具。 */
  declared?: string
  /** 所在分组路径，如 `主区域 › 购物车`。 */
  group?: string
}

export interface OutlineResult {
  /** 每行一个元素的文本大纲。 */
  text: string
  items: OutlineItem[]
  /** 匹配的元素总数。 */
  total: number
  /** 未列出的元素数（超过 limit 时）。 */
  remaining?: number
  hint?: string
}

export interface RenderOptions {
  query?: string
  limit: number
  prefix: string
}

const INDENT = '  '

function groupLabel(e: Entry): string {
  return e.kind === 'heading' ? e.name : describe(e)
}

function haystack(e: Entry, entries: Entry[]): string {
  const parts = [e.info.label, e.info.role, e.name, e.value ?? '', e.href ?? '', e.declared ?? '']
  for (const i of e.chain) {
    const g = entries[i]
    if (g) parts.push(groupLabel(g))
  }
  return parts.join(' ').toLowerCase()
}

/** 一个元素的单行描述（不含缩进）。 */
export function itemLine(e: Entry): string {
  let line = `${e.ref} ${describe(e)}`
  if (e.value !== undefined) line += `= "${e.value}"`
  if (e.href) line += `→ ${e.href}`
  if (e.states.length > 0) line += ` ${e.states.join(' ')}`
  if (e.required) line += ' (必填)'
  if (e.declared) line += ` [已声明：${e.declared}]`
  return line
}

function containerLine(e: Entry): string {
  let line = `» ${e.ref} ${describe(e)}`
  if (e.declared) line += ` [已声明：${e.declared}]`
  return line
}

function headingLine(e: Entry): string {
  return `${'#'.repeat(e.info.level ?? 2)} ${e.name}`
}

export function renderOutline(entries: Entry[], opts: RenderOptions): OutlineResult {
  const tokens = (opts.query ?? '')
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
  const matched: number[] = []
  entries.forEach((e, i) => {
    if (e.kind !== 'item') return
    if (tokens.length > 0) {
      const h = haystack(e, entries)
      if (!tokens.every((t) => h.includes(t))) return
    }
    matched.push(i)
  })
  const shown = matched.slice(0, opts.limit)
  const shownSet = new Set(shown)
  const keptGroups = new Set<number>()
  for (const i of shown) for (const g of (entries[i] as Entry).chain) keptGroups.add(g)

  const lines: string[] = []
  const items: OutlineItem[] = []
  let declared = false
  entries.forEach((e, i) => {
    const indent = INDENT.repeat(e.depth)
    if (e.kind === 'item') {
      if (!shownSet.has(i)) return
      lines.push(indent + itemLine(e))
      const group = e.chain.map((g) => groupLabel(entries[g] as Entry)).join(' › ')
      const item: OutlineItem = { ref: e.ref as string, role: e.info.role, name: e.name }
      if (e.value !== undefined) item.value = e.value
      if (e.href) item.href = e.href
      if (e.states.length > 0) item.states = e.states
      if (e.required) item.required = true
      if (e.declared) {
        item.declared = e.declared
        declared = true
      }
      if (group) item.group = group
      items.push(item)
    } else if (keptGroups.has(i)) {
      if (e.kind === 'container') {
        lines.push(indent + containerLine(e))
        if (e.declared) declared = true
      } else {
        lines.push(indent + headingLine(e))
      }
    }
  })

  const result: OutlineResult = { text: '', items, total: matched.length }
  const remaining = matched.length - shown.length
  if (lines.length === 0) {
    lines.push(tokens.length > 0 ? `（没有与「${opts.query}」匹配的可交互元素）` : '（没有可见的可交互元素）')
  }
  if (remaining > 0) {
    result.remaining = remaining
    lines.push(`…另有 ${remaining} 个元素未列出，可用 query 或 within 缩小范围`)
  }
  if (declared) {
    result.hint = '标注 [已声明：…] 的元素已有对应工具，请优先直接调用该工具'
  }
  result.text = lines.join('\n')
  return result
}
