/**
 * 精简可见文本：基于 DOM 树（textContent 语义），不读取布局，不会引起重排。
 */

const SKIP_TAGS = new Set(['SCRIPT', 'STYLE', 'TEMPLATE', 'NOSCRIPT', 'IFRAME', 'OBJECT', 'CANVAS', 'svg', 'SVG'])
const INLINE_TAGS = new Set([
  'A', 'ABBR', 'B', 'BDI', 'BDO', 'CITE', 'CODE', 'DATA', 'DFN', 'EM', 'I', 'KBD', 'MARK', 'Q', 'S',
  'SAMP', 'SMALL', 'SPAN', 'STRONG', 'SUB', 'SUP', 'TIME', 'U', 'VAR', 'LABEL', 'FONT',
])

export function collapse(s: string): string {
  return s.replace(/\s+/g, ' ').trim()
}

export function truncate(s: string, max: number): string {
  if (s.length <= max) return s
  return `${s.slice(0, Math.max(0, max - 1))}…`
}

/** 明显不可见的元素（只看属性与内联样式，不计算样式）。 */
function isHiddenByMarkup(el: Element): boolean {
  if (SKIP_TAGS.has(el.tagName)) return true
  if (el.hasAttribute('hidden')) return true
  if (el.getAttribute('aria-hidden') === 'true') return true
  const style = (el as HTMLElement).style
  return style !== undefined && style.display === 'none'
}

/**
 * 元素的可见文本：折叠空白后截断到 `limit` 字符。
 * `skip` 返回 true 的子元素（连同子树）不计入。
 */
export function visibleText(root: Element, limit: number, skip?: (el: Element) => boolean): string {
  const parts: string[] = []
  let length = 0
  const budget = limit * 2 + 64
  const walk = (node: Node): boolean => {
    for (let child = node.firstChild; child; child = child.nextSibling) {
      if (child.nodeType === 3) {
        const t = child.nodeValue
        if (t) {
          parts.push(t)
          length += t.length
          if (length > budget) return false
        }
      } else if (child.nodeType === 1) {
        const el = child as Element
        if (isHiddenByMarkup(el) || skip?.(el)) continue
        const block = !INLINE_TAGS.has(el.tagName)
        if (block) parts.push(' ')
        if (!walk(el)) return false
        if (block) parts.push(' ')
      }
    }
    return true
  }
  walk(root)
  return truncate(collapse(parts.join('')), limit)
}
