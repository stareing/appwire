/**
 * DOM 读取：可见性、可见文本、可访问名称、元素分类与状态。
 *
 * 只在工具调用时同步计算，不注册任何观察者。
 */

/** 连同子树一起跳过的标签（不含可操作内容或内容不可读）。 */
const SKIP_TAGS = new Set([
  'SCRIPT', 'STYLE', 'TEMPLATE', 'NOSCRIPT', 'IFRAME', 'OBJECT', 'EMBED', 'CANVAS', 'svg', 'SVG', 'HEAD', 'META', 'LINK',
])

/** 文本拼接时不插入空格的行内标签。 */
const INLINE_TAGS = new Set([
  'A', 'ABBR', 'B', 'BDI', 'BDO', 'CITE', 'CODE', 'DATA', 'DFN', 'EM', 'I', 'KBD', 'MARK', 'Q', 'S',
  'SAMP', 'SMALL', 'SPAN', 'STRONG', 'SUB', 'SUP', 'TIME', 'U', 'VAR', 'LABEL', 'FONT',
])

export const NAME_MAX = 40
/** 大纲中值的截断长度（值通常不如名称重要）。 */
const VALUE_MAX = 30
const STATUS_MAX = 80
const HREF_MAX = 60

export function collapse(s: string): string {
  return s.replace(/\s+/g, ' ').trim()
}

export function truncate(s: string, max: number): string {
  if (s.length <= max) return s
  return `${s.slice(0, Math.max(0, max - 1)).trimEnd()}…`
}

export function windowOf(el: Element): Window & typeof globalThis {
  return (el.ownerDocument.defaultView ?? globalThis) as Window & typeof globalThis
}

function style(el: Element): CSSStyleDeclaration | null {
  try {
    return windowOf(el).getComputedStyle(el)
  } catch {
    return null
  }
}

// ---------------------------------------------------------------------------
// 可见性
// ---------------------------------------------------------------------------

/**
 * 元素（连同子树）是否被隐藏：跳过的标签、`hidden`、`inert`、`aria-hidden`、`display:none`、未打开的 `<dialog>`。
 * 返回 computed style 供调用方复用（未隐藏时）。
 */
export function subtreeHidden(el: Element): { hidden: true } | { hidden: false; style: CSSStyleDeclaration | null } {
  if (SKIP_TAGS.has(el.tagName)) return { hidden: true }
  if (el.hasAttribute('hidden') || el.hasAttribute('inert')) return { hidden: true }
  if (el.getAttribute('aria-hidden') === 'true') return { hidden: true }
  if (el.tagName === 'DIALOG' && !el.hasAttribute('open')) return { hidden: true }
  if (el.tagName === 'INPUT' && (el as HTMLInputElement).type === 'hidden') return { hidden: true }
  const s = style(el)
  if (s && s.display === 'none') return { hidden: true }
  return { hidden: false, style: s }
}

/** `visibility:hidden` 可被子元素覆盖，因此只对元素自身判断。 */
export function invisibleSelf(s: CSSStyleDeclaration | null): boolean {
  return !!s && (s.visibility === 'hidden' || s.visibility === 'collapse')
}

/** 页面是否有布局信息（测试环境等无布局时不做零尺寸判断）。 */
export function hasLayout(doc: Document): boolean {
  const r = doc.documentElement.getBoundingClientRect()
  return r.width > 0 || r.height > 0
}

export function zeroSize(el: Element): boolean {
  const r = el.getBoundingClientRect()
  return r.width === 0 && r.height === 0
}

/** 元素当前是否可见（检查自身与全部祖先），用于操作前校验。 */
export function isVisible(el: Element): boolean {
  for (let cur: Element | null = el; cur; cur = cur.parentElement) {
    if (subtreeHidden(cur).hidden) return false
    // 未展开的 <details> 只显示 <summary>
    const parent: Element | null = cur.parentElement
    if (parent && parent.tagName === 'DETAILS' && !parent.hasAttribute('open') && cur.tagName !== 'SUMMARY') return false
  }
  if (invisibleSelf(style(el))) return false
  if (hasLayout(el.ownerDocument) && zeroSize(el) && !el.hasAttribute('data-mcp-tool')) return false
  return true
}

// ---------------------------------------------------------------------------
// 文本
// ---------------------------------------------------------------------------

/**
 * 元素的可见文本：折叠空白后截断到 `limit` 字符。
 * `computed` 为 true 时额外按 computed style 跳过 `display:none` 的子树（用于 ui.read）。
 * `skip` 返回 true 的子元素（连同子树）不计入。
 */
export function visibleText(
  root: Element,
  limit: number,
  opts: { computed?: boolean; skip?: (el: Element) => boolean } = {},
): { text: string; truncated: boolean } {
  const parts: string[] = []
  let length = 0
  const budget = limit * 2 + 64
  const hidden = (el: Element): boolean => {
    if (SKIP_TAGS.has(el.tagName) || el.hasAttribute('hidden') || el.getAttribute('aria-hidden') === 'true') return true
    if (el.tagName === 'DIALOG' && !el.hasAttribute('open')) return true
    if (opts.computed) return subtreeHidden(el).hidden
    const s = (el as HTMLElement).style
    return s !== undefined && s.display === 'none'
  }
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
        if (hidden(el) || opts.skip?.(el)) continue
        if (node.nodeName === 'DETAILS' && !(node as Element).hasAttribute('open') && el.tagName !== 'SUMMARY') continue
        const block = !INLINE_TAGS.has(el.tagName)
        if (block) parts.push(' ')
        if (el.tagName === 'IMG') {
          const alt = el.getAttribute('alt')
          if (alt) parts.push(alt)
        }
        if (!walk(el)) return false
        if (block) parts.push(' ')
      }
    }
    return true
  }
  walk(root)
  const full = collapse(parts.join(''))
  return { text: truncate(full, limit), truncated: full.length > limit || length > budget }
}

function text(el: Element, limit = NAME_MAX): string {
  return visibleText(el, limit).text
}

// ---------------------------------------------------------------------------
// 分类
// ---------------------------------------------------------------------------

export type Kind = 'item' | 'container' | 'heading'

export interface Info {
  kind: Kind
  /** 英文角色名（ARIA 风格）。 */
  role: string
  /** 中文角色名，用于输出。 */
  label: string
  /** 标题级别（kind 为 heading 时）。 */
  level?: number
}

const ROLE_LABELS: Record<string, string> = {
  button: '按钮',
  link: '链接',
  textbox: '输入框',
  searchbox: '搜索框',
  checkbox: '复选框',
  radio: '单选框',
  switch: '开关',
  combobox: '下拉框',
  listbox: '列表框',
  option: '选项',
  slider: '滑块',
  spinbutton: '数字框',
  tab: '标签页',
  menuitem: '菜单项',
  menuitemcheckbox: '菜单项',
  menuitemradio: '菜单项',
  treeitem: '树节点',
  gridcell: '单元格',
  status: '提示',
  alert: '警告',
  navigation: '导航',
  main: '主区域',
  form: '表单',
  search: '搜索区',
  dialog: '对话框',
  alertdialog: '警告框',
  heading: '标题',
  generic: '元素',
}

const ITEM_ROLES = new Set([
  'button', 'link', 'textbox', 'searchbox', 'checkbox', 'radio', 'switch', 'combobox', 'listbox', 'option',
  'slider', 'spinbutton', 'tab', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'treeitem',
])
const CONTAINER_ROLES = new Set(['navigation', 'main', 'form', 'search', 'dialog', 'alertdialog'])
const STATUS_ROLES = new Set(['status', 'alert', 'log'])

const INPUT_LABELS: Record<string, [string, string]> = {
  text: ['textbox', '输入框'],
  email: ['textbox', '邮箱框'],
  tel: ['textbox', '电话框'],
  url: ['textbox', '网址框'],
  password: ['textbox', '密码框'],
  search: ['searchbox', '搜索框'],
  number: ['spinbutton', '数字框'],
  range: ['slider', '滑块'],
  checkbox: ['checkbox', '复选框'],
  radio: ['radio', '单选框'],
  date: ['textbox', '日期框'],
  time: ['textbox', '时间框'],
  'datetime-local': ['textbox', '日期时间框'],
  month: ['textbox', '月份框'],
  week: ['textbox', '周框'],
  color: ['textbox', '颜色框'],
  file: ['button', '文件选择'],
  submit: ['button', '按钮'],
  reset: ['button', '按钮'],
  button: ['button', '按钮'],
  image: ['button', '按钮'],
}

function explicitRole(el: Element): string | undefined {
  const r = el.getAttribute('role')
  if (!r) return undefined
  return r.trim().split(/\s+/)[0]?.toLowerCase()
}

function item(role: string, label = ROLE_LABELS[role] ?? '元素'): Info {
  return { kind: 'item', role, label }
}

/** 元素是否为可编辑区域。 */
export function isContentEditable(el: Element): boolean {
  const v = el.getAttribute('contenteditable')
  if (v === null) return false
  if (v === 'false') return false
  // 只取可编辑区域的最外层
  const parent = el.parentElement?.closest('[contenteditable]')
  return !parent || parent.getAttribute('contenteditable') === 'false'
}

/**
 * 元素分类：可交互元素（item）、分组容器（container：nav / main / form / dialog 等）、标题（h1–h3）。
 * 不属于三者时返回 null。`pointer` 表示元素是可点击区域的最外层（computed cursor 为 pointer 且父元素不是）。
 */
export function classify(el: Element, pointer = false): Info | null {
  const tag = el.tagName
  const role = explicitRole(el)
  if (role === 'none' || role === 'presentation') {
    return el.hasAttribute('data-mcp-tool') ? item('generic') : null
  }
  if (role) {
    if (ITEM_ROLES.has(role)) return item(role)
    if (CONTAINER_ROLES.has(role)) return { kind: 'container', role, label: ROLE_LABELS[role] ?? role }
    if (STATUS_ROLES.has(role)) return item(role === 'alert' ? 'alert' : 'status')
    if (role === 'heading') {
      const level = Number(el.getAttribute('aria-level') ?? '2')
      return level >= 1 && level <= 3 ? { kind: 'heading', role, label: '标题', level } : null
    }
  }
  switch (tag) {
    case 'A':
    case 'AREA':
      if (el.hasAttribute('href')) return item('link')
      break
    case 'BUTTON':
      return item('button')
    case 'INPUT': {
      const type = (el as HTMLInputElement).type
      if (type === 'hidden') return null
      const [r, l] = INPUT_LABELS[type] ?? ['textbox', '输入框']
      return item(r, l)
    }
    case 'TEXTAREA':
      return item('textbox', '多行输入框')
    case 'SELECT': {
      const s = el as HTMLSelectElement
      return s.multiple || s.size > 1 ? item('listbox') : item('combobox')
    }
    case 'SUMMARY':
      return item('button', '展开项')
    case 'H1':
    case 'H2':
    case 'H3':
      return { kind: 'heading', role: 'heading', label: '标题', level: Number(tag[1]) }
    case 'NAV':
      return { kind: 'container', role: 'navigation', label: '导航' }
    case 'MAIN':
      return { kind: 'container', role: 'main', label: '主区域' }
    case 'FORM':
      return { kind: 'container', role: 'form', label: '表单' }
    case 'DIALOG':
      return { kind: 'container', role: 'dialog', label: '对话框' }
    case 'OUTPUT':
      return item('status')
  }
  const live = el.getAttribute('aria-live')
  if (live === 'polite' || live === 'assertive') return item('status')
  if (isContentEditable(el)) return item('textbox', '编辑区')
  if (el.hasAttribute('data-mcp-tool')) return item('generic')
  const tabindex = el.getAttribute('tabindex')
  if (tabindex !== null && Number(tabindex) >= 0) return item('generic')
  if (pointer && tag !== 'LABEL' && tag !== 'HTML' && tag !== 'BODY') return item('generic', '可点击元素')
  return null
}

// ---------------------------------------------------------------------------
// 名称、值、状态
// ---------------------------------------------------------------------------

const LABELABLE = new Set(['INPUT', 'SELECT', 'TEXTAREA', 'METER', 'PROGRESS', 'OUTPUT', 'BUTTON'])
const CONTROL_TAGS = new Set(['INPUT', 'SELECT', 'TEXTAREA', 'BUTTON'])

function byIds(el: Element, ids: string): string {
  const doc = el.ownerDocument
  return collapse(
    ids
      .split(/\s+/)
      .map((id) => {
        const t = id ? doc.getElementById(id) : null
        return t ? text(t, NAME_MAX * 2) : ''
      })
      .join(' '),
  )
}

function labelText(el: Element): string {
  if (!LABELABLE.has(el.tagName)) return ''
  const labels = (el as HTMLInputElement).labels
  const list: Element[] = labels ? Array.from(labels) : []
  if (list.length === 0) {
    const wrap = el.closest('label')
    if (wrap) list.push(wrap)
    const id = el.getAttribute('id')
    if (id) {
      for (const l of Array.from(el.ownerDocument.querySelectorAll('label[for]'))) {
        if (l.getAttribute('for') === id && !list.includes(l)) list.push(l)
      }
    }
  }
  // 标签里嵌套的控件本身（如 <label>数量 <select>…</select></label>）不计入名称
  const skip = (c: Element) => CONTROL_TAGS.has(c.tagName)
  return collapse(list.map((l) => visibleText(l, NAME_MAX * 2, { skip }).text).join(' '))
}

/** 可访问名称：aria-labelledby → aria-label → label → 文本 → title → placeholder，截断到 40 字。 */
export function accessibleName(el: Element, info: Info): string {
  const labelledby = el.getAttribute('aria-labelledby')
  let name = labelledby ? byIds(el, labelledby) : ''
  if (!name) name = collapse(el.getAttribute('aria-label') ?? '')
  if (!name) name = labelText(el)
  if (!name && el.tagName === 'INPUT') {
    const input = el as HTMLInputElement
    if (input.type === 'submit' || input.type === 'reset' || input.type === 'button') {
      name = input.value || (input.type === 'submit' ? '提交' : input.type === 'reset' ? '重置' : '')
    } else if (input.type === 'image') {
      name = input.getAttribute('alt') ?? ''
    }
  }
  if (!name && info.kind === 'container') {
    if (info.role === 'dialog' || info.role === 'alertdialog') {
      const h = el.querySelector('h1,h2,h3,h4,[role="heading"]')
      if (h) name = text(h)
    }
  } else if (!name && !(el.tagName === 'INPUT' || el.tagName === 'SELECT' || el.tagName === 'TEXTAREA')) {
    if (info.role !== 'textbox') name = text(el, info.role === 'status' || info.role === 'alert' ? STATUS_MAX : NAME_MAX)
    if (!name) {
      const img = el.querySelector('img[alt],[aria-label]')
      if (img) name = collapse(img.getAttribute('alt') || img.getAttribute('aria-label') || '')
    }
  }
  if (!name) name = collapse(el.getAttribute('title') ?? '')
  if (!name) name = collapse(el.getAttribute('placeholder') ?? el.getAttribute('aria-placeholder') ?? '')
  if (!name && info.kind === 'item') {
    const n = el.getAttribute('name')
    if (n) name = n
  }
  if (!name && info.kind === 'container' && info.role === 'form') {
    name = el.getAttribute('toolname') ?? el.getAttribute('data-mcp-tool') ?? ''
  }
  return truncate(name, info.role === 'status' || info.role === 'alert' ? STATUS_MAX : NAME_MAX)
}

/** 密码类控件的值掩码（spec/ui-fallback.md 8.1：固定 4 个点，不泄露长度）。 */
export const SECURE_MASK = '••••'

/**
 * 密码类控件（spec/ui-fallback.md 8.1 网页映射：`input[type=password]`）。
 * @security 值只显示 SECURE_MASK；fill 与对其按键一律拒绝。
 */
export function isSecure(el: Element): boolean {
  return el.tagName === 'INPUT' && (el as HTMLInputElement).type === 'password'
}

/** 当前值（用于大纲展示；密码只显示是否已填写）。 */
export function currentValue(el: Element, info: Info): string | undefined {
  const tag = el.tagName
  if (tag === 'INPUT') {
    const input = el as HTMLInputElement
    switch (input.type) {
      case 'checkbox':
      case 'radio':
      case 'submit':
      case 'reset':
      case 'button':
      case 'image':
        return undefined
      case 'password':
        return input.value ? SECURE_MASK : undefined
      case 'file':
        return input.files && input.files.length > 0
          ? Array.from(input.files).map((f) => f.name).join('、')
          : undefined
    }
    return input.value ? truncate(collapse(input.value), VALUE_MAX) : undefined
  }
  if (tag === 'TEXTAREA') {
    const v = (el as HTMLTextAreaElement).value
    return v ? truncate(collapse(v), VALUE_MAX) : undefined
  }
  if (tag === 'SELECT') {
    const s = el as HTMLSelectElement
    const t = Array.from(s.selectedOptions ?? [])
      .map((o) => collapse(o.label || o.text))
      .join('、')
    return t ? truncate(t, VALUE_MAX) : undefined
  }
  if (info.role === 'slider' || info.role === 'spinbutton') {
    return el.getAttribute('aria-valuetext') ?? el.getAttribute('aria-valuenow') ?? undefined
  }
  if (info.label === '编辑区') {
    const t = text(el)
    return t || undefined
  }
  return undefined
}

/** 元素（或祖先 fieldset）是否禁用。 */
export function isDisabled(el: Element): boolean {
  if ((el as HTMLButtonElement).disabled === true) return true
  if (el.getAttribute('aria-disabled') === 'true') return true
  if (CONTROL_TAGS.has(el.tagName)) {
    const fs = el.closest('fieldset[disabled]')
    if (fs) {
      const legend = fs.querySelector(':scope > legend')
      if (!legend || !legend.contains(el)) return true
    }
  }
  return !!el.closest('[inert]')
}

/** 状态标记（英文短词，与 ARIA 状态对应）。 */
export function statesOf(el: Element, info: Info): string[] {
  const states: string[] = []
  if (info.kind !== 'item') return states
  if (isDisabled(el)) states.push('disabled')
  const tag = el.tagName
  if (tag === 'INPUT' && ((el as HTMLInputElement).type === 'checkbox' || (el as HTMLInputElement).type === 'radio')) {
    const input = el as HTMLInputElement
    states.push(input.indeterminate ? 'mixed' : input.checked ? 'checked' : 'unchecked')
  } else {
    const checked = el.getAttribute('aria-checked')
    if (checked === 'true') states.push('checked')
    else if (checked === 'mixed') states.push('mixed')
    else if (checked === 'false') states.push('unchecked')
  }
  if (tag === 'SUMMARY') {
    const details = el.parentElement
    if (details?.tagName === 'DETAILS') states.push(details.hasAttribute('open') ? 'expanded' : 'collapsed')
  } else {
    const expanded = el.getAttribute('aria-expanded')
    if (expanded === 'true') states.push('expanded')
    else if (expanded === 'false') states.push('collapsed')
  }
  if (el.getAttribute('aria-selected') === 'true') states.push('selected')
  if (el.getAttribute('aria-pressed') === 'true') states.push('pressed')
  const current = el.getAttribute('aria-current')
  if (current && current !== 'false') states.push('current')
  if ((tag === 'INPUT' || tag === 'TEXTAREA') && (el as HTMLInputElement).readOnly) states.push('readonly')
  if (el.getAttribute('aria-invalid') === 'true') states.push('invalid')
  if (el.ownerDocument.activeElement === el) states.push('focused')
  return states
}

export function isRequired(el: Element): boolean {
  return (el as HTMLInputElement).required === true || el.getAttribute('aria-required') === 'true'
}

/** 链接目标：同源时只保留路径，`javascript:` 不显示。 */
export function hrefOf(el: Element): string | undefined {
  const raw = el.getAttribute('href')
  if (raw === null || /^\s*javascript:/i.test(raw)) return undefined
  let out = raw.trim()
  try {
    const loc = windowOf(el).location
    const url = new URL(out, loc.href)
    if (url.origin === loc.origin) out = `${url.pathname}${url.search}${url.hash}`
  } catch {
    // 保留原值
  }
  return truncate(out, HREF_MAX)
}

/** 元素声明的工具名（`@app-mcp/dom` 的 data-mcp-tool，表单上的标准 toolname 优先）。 */
export function declaredTool(el: Element): string | undefined {
  if (el.tagName === 'FORM') {
    const std = el.getAttribute('toolname')
    if (std) return std
  }
  return el.getAttribute('data-mcp-tool') ?? undefined
}
