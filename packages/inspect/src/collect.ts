/**
 * 收集大纲条目：按 DOM 顺序遍历子树，只保留可交互 / 有意义的元素，并记录分组关系。
 */

import {
  accessibleName,
  classify,
  currentValue,
  declaredTool,
  hasLayout,
  hrefOf,
  type Info,
  invisibleSelf,
  isRequired,
  statesOf,
  subtreeHidden,
  zeroSize,
} from './dom'
import type { RefRegistry } from './refs'

export interface Entry {
  kind: Info['kind']
  el: Element
  info: Info
  /** 可交互元素与容器有引用；标题没有。 */
  ref?: string
  name: string
  value?: string
  href?: string
  states: string[]
  required: boolean
  /** 已由 data-mcp-tool / toolname 声明的工具名。 */
  declared?: string
  /** 所在容器层数（缩进）。 */
  depth: number
  /** 祖先分组（容器与标题）在条目数组中的下标，外层在前。 */
  chain: number[]
  /** 祖先容器元素，外层在前。 */
  containers: Element[]
}

interface Frame {
  container: number
  headings: Array<{ level: number; index: number }>
}

/** 这些元素的子树不再遍历（内容是选项或文本，不是独立可操作元素）。 */
const LEAF_TAGS = new Set(['SELECT', 'TEXTAREA', 'INPUT', 'OPTION', 'OPTGROUP'])

export function collect(root: Element, refs: RefRegistry): Entry[] {
  const entries: Entry[] = []
  const layout = hasLayout(root.ownerDocument)
  const frames: Frame[] = [{ container: -1, headings: [] }]
  const containerEls: Element[] = []

  const chain = (): number[] => {
    const out: number[] = []
    for (const f of frames) {
      if (f.container >= 0) out.push(f.container)
      for (const h of f.headings) out.push(h.index)
    }
    return out
  }

  const walk = (el: Element, parentCursor: string, insideItem: boolean): void => {
    const h = subtreeHidden(el)
    if (h.hidden) return
    const s = h.style
    const cursor = s?.cursor ?? ''
    const pointer = cursor === 'pointer' && parentCursor !== 'pointer' && !insideItem
    const info = classify(el, pointer)
    let pushedFrame = false
    let isItem = false
    if (info) {
      const base = {
        el,
        info,
        depth: frames.length - 1,
        chain: chain(),
        containers: containerEls.slice(),
      }
      if (info.kind === 'container') {
        const index = entries.length
        entries.push({
          ...base,
          kind: 'container',
          ref: refs.refOf(el),
          name: accessibleName(el, info),
          states: [],
          required: false,
          declared: declaredTool(el),
        })
        frames.push({ container: index, headings: [] })
        containerEls.push(el)
        pushedFrame = true
      } else if (info.kind === 'heading') {
        if (!invisibleSelf(s)) {
          const frame = frames[frames.length - 1] as Frame
          const level = info.level ?? 2
          const name = accessibleName(el, info)
          if (name) {
            while (frame.headings.length > 0 && (frame.headings[frame.headings.length - 1]?.level ?? 0) >= level) {
              frame.headings.pop()
            }
            base.chain = chain()
            frame.headings.push({ level, index: entries.length })
            entries.push({ ...base, kind: 'heading', name, states: [], required: false })
          }
        }
      } else {
        isItem = true
        const visible = !invisibleSelf(s) && !(layout && zeroSize(el) && !el.hasAttribute('data-mcp-tool'))
        const name = visible ? accessibleName(el, info) : ''
        // 空的提示区域（尚无消息）不列出
        const emptyStatus = (info.role === 'status' || info.role === 'alert') && !name
        if (visible && !emptyStatus) {
          entries.push({
            ...base,
            kind: 'item',
            ref: refs.refOf(el),
            name,
            value: currentValue(el, info),
            href: info.role === 'link' ? hrefOf(el) : undefined,
            states: statesOf(el, info),
            required: isRequired(el),
            declared: declaredTool(el),
          })
        }
      }
    }
    if (!LEAF_TAGS.has(el.tagName)) {
      const closedDetails = el.tagName === 'DETAILS' && !el.hasAttribute('open')
      for (let child = el.firstElementChild; child; child = child.nextElementSibling) {
        if (closedDetails && child.tagName !== 'SUMMARY') continue
        walk(child, cursor, insideItem || isItem)
      }
    }
    if (pushedFrame) {
      frames.pop()
      containerEls.pop()
    }
  }

  // 根元素的祖先不可见时整棵子树都不可见
  for (let cur = root.parentElement; cur; cur = cur.parentElement) {
    if (subtreeHidden(cur).hidden) return entries
  }
  const parentStyle = root.parentElement ? subtreeHidden(root.parentElement) : null
  const parentCursor = parentStyle && !parentStyle.hidden ? (parentStyle.style?.cursor ?? '') : ''
  walk(root, parentCursor, false)
  refs.prune()
  return entries
}

/** 条目的简短描述：`按钮「结算」`。 */
export function describe(e: Pick<Entry, 'info' | 'name'>): string {
  return e.name ? `${e.info.label}「${e.name}」` : e.info.label
}
