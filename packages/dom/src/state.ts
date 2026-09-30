/**
 * 元素可用性：disabled、aria-disabled、hidden、inert、display:none。
 * 先检查属性（不涉及样式），最后才做一次可见性检查。
 */

/** 返回不可用原因；可用时返回 undefined。 */
export function disabledReason(el: Element): string | undefined {
  for (let cur: Element | null = el; cur; cur = cur.parentElement) {
    const self = cur === el
    if (cur.hasAttribute('disabled')) {
      if (!self) return '所在区域已禁用'
      return cur.tagName === 'BUTTON' ? '按钮已禁用' : '元素已禁用'
    }
    if (cur.getAttribute('aria-disabled') === 'true') return self ? '元素已禁用（aria-disabled）' : '所在区域已禁用（aria-disabled）'
    if (cur.hasAttribute('hidden')) return self ? '元素已隐藏' : '所在区域已隐藏'
    if (cur.hasAttribute('inert')) return '元素不可交互（inert）'
  }
  if (!isDisplayed(el)) return '元素不可见'
  return undefined
}

function isDisplayed(el: Element): boolean {
  if (!el.isConnected) return false
  const check = (el as Element & { checkVisibility?: () => boolean }).checkVisibility
  if (typeof check === 'function') return check.call(el)
  const win = el.ownerDocument.defaultView
  if (!win) return true
  for (let cur: Element | null = el; cur; cur = cur.parentElement) {
    if (win.getComputedStyle(cur).display === 'none') return false
  }
  return true
}
