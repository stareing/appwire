/**
 * 元素是否"真正可见"的判定（`view` 工具门控的 DOM 部分，spec/protocol.md 3.4）。纯函数：只读 DOM，不订阅。
 *
 * @invariant 判定顺序与 {@link elementVisible} 中的规则表一致；任一规则不满足即不可见。
 */

type CheckVisibility = (options?: Record<string, boolean>) => boolean

/** 祖先（含自身）带 `inert`：不可交互。 */
function inert(el: Element): boolean {
  return el.closest('[inert]') !== null
}

/** 祖先（含自身）带 `hidden` 属性（浏览器缺省样式为 `display:none`；不依赖样式计算）。 */
function hiddenAttribute(el: Element): boolean {
  return el.closest('[hidden]') !== null
}

/** 有打开的模态 `<dialog>`（`showModal()`）且它不包含该元素：其余文档被浏览器设为惰性。 */
function coveredByModal(el: Element): boolean {
  const doc = el.ownerDocument
  for (const dialog of Array.from(doc.querySelectorAll('dialog[open]'))) {
    let modal = false
    try {
      modal = dialog.matches(':modal')
    } catch {
      // @compat 不支持 `:modal` 的环境：无法区分模态与非模态，按非模态处理
      modal = false
    }
    if (modal && !dialog.contains(el)) return true
  }
  return false
}

/** 未渲染：`display:none`（含祖先）、`visibility:hidden`、`content-visibility` 跳过；不支持 `checkVisibility` 时不判断（由其他规则兜底）。 */
function notRendered(el: Element): boolean {
  const check = (el as Element & { checkVisibility?: CheckVisibility }).checkVisibility
  if (typeof check === 'function') {
    try {
      return !check.call(el, { checkVisibilityCSS: true, visibilityProperty: true })
    } catch {
      // 视为已渲染
    }
  }
  return false
}

/**
 * 不在视口内（滚出屏幕）。按布局盒判断；没有布局信息（盒为 0×0 且位于原点，如测试环境）时视为在视口内。
 * @why 用同步的几何判断而不是 IntersectionObserver 的最近一次结果：后者在 DOM 变化后的下一帧才更新，
 *   导航完成后立即求值时会读到旧值。IntersectionObserver 只用作"可能变化了"的触发器（tracker.ts）。
 */
function outsideViewport(el: Element): boolean {
  const win = el.ownerDocument.defaultView
  if (!win) return false
  const r = el.getBoundingClientRect()
  const width = win.innerWidth || el.ownerDocument.documentElement.clientWidth
  const height = win.innerHeight || el.ownerDocument.documentElement.clientHeight
  if (!width || !height) return false
  return r.bottom < 0 || r.right < 0 || r.top > height || r.left > width
}

/** 不可见规则表：依次检查，命中任一条即不可见。 */
const HIDDEN_RULES: ReadonlyArray<(el: Element) => boolean> = [
  inert,
  hiddenAttribute,
  coveredByModal,
  notRendered,
  outsideViewport,
]

/** 元素已挂载、可交互、已渲染且在视口内。 */
export function elementVisible(el: Element): boolean {
  if (!el.isConnected) return false
  return !HIDDEN_RULES.some((rule) => rule(el))
}
