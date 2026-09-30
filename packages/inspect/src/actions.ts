/**
 * 模拟输入：点击、填写、按键、滚动、提交。
 *
 * 填写兼容 React 等受控组件：通过原型上的 value / checked setter 赋值（绕过框架在实例上安装的追踪器），
 * 再派发 input / change 事件。
 */

import { ToolCallError } from '@app-mcp/web'
import { windowOf } from './dom'

type Win = Window & typeof globalThis

function scrollIntoView(el: Element): void {
  try {
    el.scrollIntoView?.({ block: 'center', inline: 'nearest' })
  } catch {
    // 部分环境不支持参数对象
  }
}

function focus(el: Element): void {
  const f = (el as HTMLElement).focus
  if (typeof f === 'function') {
    try {
      f.call(el, { preventScroll: true })
    } catch {
      // 忽略
    }
  }
}

/** 沿原型链查找属性 setter 并调用（跳过实例上的自有属性）。 */
function setNative(el: Element, prop: 'value' | 'checked', value: unknown): void {
  let proto: object | null = Object.getPrototypeOf(el)
  while (proto) {
    const d = Object.getOwnPropertyDescriptor(proto, prop)
    if (d?.set) {
      d.set.call(el, value)
      return
    }
    proto = Object.getPrototypeOf(proto)
  }
  ;(el as unknown as Record<string, unknown>)[prop] = value
}

function fire(el: Element, type: 'input' | 'change', data?: string): void {
  const win = windowOf(el)
  const ev =
    type === 'input' && typeof win.InputEvent === 'function'
      ? new win.InputEvent('input', { bubbles: true, composed: true, inputType: 'insertText', data: data ?? null })
      : new win.Event(type, { bubbles: true, composed: type === 'input' })
  el.dispatchEvent(ev)
}

function pointer(el: Element, type: string, x: number, y: number): void {
  const win = windowOf(el)
  const init = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, button: 0, view: win }
  const Ctor = type.startsWith('pointer') && typeof win.PointerEvent === 'function' ? win.PointerEvent : win.MouseEvent
  const extra = type.startsWith('pointer') ? { pointerId: 1, pointerType: 'mouse', isPrimary: true } : {}
  el.dispatchEvent(new Ctor(type, { ...init, ...extra }))
}

export function click(el: Element): void {
  scrollIntoView(el)
  const r = el.getBoundingClientRect()
  const x = r.left + r.width / 2
  const y = r.top + r.height / 2
  pointer(el, 'pointerdown', x, y)
  pointer(el, 'mousedown', x, y)
  focus(el)
  pointer(el, 'pointerup', x, y)
  pointer(el, 'mouseup', x, y)
  const c = (el as HTMLElement).click
  if (typeof c === 'function') c.call(el)
  else pointer(el, 'click', x, y)
}

function toBool(value: unknown): boolean {
  if (typeof value === 'boolean') return value
  if (typeof value === 'number') return value !== 0
  if (typeof value === 'string') {
    const v = value.trim().toLowerCase()
    if (['true', '1', 'on', 'yes', 'checked', '是', '勾选', '选中'].includes(v)) return true
    if (['false', '0', 'off', 'no', 'unchecked', '', '否', '取消', '不选'].includes(v)) return false
  }
  throw new ToolCallError('INVALID_INPUT', '复选框 / 单选框的 value 应为 true 或 false')
}

function optionMatches(select: HTMLSelectElement, wanted: string): HTMLOptionElement | undefined {
  const opts = Array.from(select.options)
  const w = wanted.trim()
  const lower = w.toLowerCase()
  return (
    opts.find((o) => o.value === w) ??
    opts.find((o) => (o.label || o.text).trim() === w) ??
    opts.find((o) => (o.label || o.text).trim().toLowerCase() === lower) ??
    (lower ? opts.find((o) => (o.label || o.text).toLowerCase().includes(lower)) : undefined)
  )
}

function fillSelect(select: HTMLSelectElement, value: unknown): void {
  const wanted = (Array.isArray(value) ? value : [value]).map((v) => String(v))
  if (!select.multiple && wanted.length !== 1) {
    throw new ToolCallError('INVALID_INPUT', '单选下拉框只能选择一个选项')
  }
  const picked: HTMLOptionElement[] = []
  for (const w of wanted) {
    const o = optionMatches(select, w)
    if (!o) {
      const available = Array.from(select.options)
        .slice(0, 15)
        .map((x) => `"${(x.label || x.text).trim()}"`)
        .join('、')
      throw new ToolCallError('INVALID_INPUT', `下拉框中没有与 "${w}" 匹配的选项；可选：${available}`)
    }
    if (o.disabled) throw new ToolCallError('INVALID_INPUT', `选项 "${o.text.trim()}" 已禁用`)
    picked.push(o)
  }
  if (select.multiple) {
    for (const o of Array.from(select.options)) o.selected = picked.includes(o)
  } else {
    setNative(select, 'value', (picked[0] as HTMLOptionElement).value)
    ;(picked[0] as HTMLOptionElement).selected = true
  }
  fire(select, 'input')
  fire(select, 'change')
}

/** 填写输入类控件；不是输入类控件时抛出 INVALID_INPUT。 */
export function fill(el: Element, value: unknown, ref: string, prefix: string): void {
  const tag = el.tagName
  scrollIntoView(el)
  if (tag === 'SELECT') {
    focus(el)
    fillSelect(el as HTMLSelectElement, value)
    return
  }
  if (tag === 'INPUT' || tag === 'TEXTAREA') {
    const input = el as HTMLInputElement
    const type = tag === 'INPUT' ? input.type : 'textarea'
    if (type === 'checkbox' || type === 'radio') {
      const want = toBool(value)
      if (input.checked === want) return
      if (want || type === 'checkbox') {
        // 与用户点击一致：切换状态并派发 click / input / change（React 监听 click）
        click(input)
      } else {
        setNative(input, 'checked', false)
        fire(input, 'input')
        fire(input, 'change')
      }
      return
    }
    if (type === 'file') {
      throw new ToolCallError('INVALID_INPUT', `${ref} 是文件选择控件，不支持通过 ${prefix}.fill 选择文件`)
    }
    if (['submit', 'reset', 'button', 'image'].includes(type)) {
      throw new ToolCallError('INVALID_INPUT', `${ref} 是按钮，请使用 ${prefix}.click`)
    }
    if (input.readOnly) throw new ToolCallError('INVALID_INPUT', `${ref} 是只读字段，不能填写`)
    if (Array.isArray(value) || (value !== null && typeof value === 'object')) {
      throw new ToolCallError('INVALID_INPUT', '输入框的 value 应为字符串或数字')
    }
    const text = value === null || value === undefined ? '' : String(value)
    focus(el)
    setNative(el, 'value', text)
    fire(el, 'input', text)
    fire(el, 'change')
    return
  }
  const editable = el.getAttribute('contenteditable')
  if (editable !== null && editable !== 'false') {
    const text = value === null || value === undefined ? '' : String(value)
    focus(el)
    el.textContent = text
    fire(el, 'input', text)
    return
  }
  throw new ToolCallError('INVALID_INPUT', `${ref} 不是输入类控件，请使用 ${prefix}.click 等操作`)
}

// ---------------------------------------------------------------------------
// 按键
// ---------------------------------------------------------------------------

const KEY_ALIASES: Record<string, string> = {
  enter: 'Enter',
  return: 'Enter',
  esc: 'Escape',
  escape: 'Escape',
  tab: 'Tab',
  space: ' ',
  spacebar: ' ',
  backspace: 'Backspace',
  delete: 'Delete',
  del: 'Delete',
  up: 'ArrowUp',
  down: 'ArrowDown',
  left: 'ArrowLeft',
  right: 'ArrowRight',
  arrowup: 'ArrowUp',
  arrowdown: 'ArrowDown',
  arrowleft: 'ArrowLeft',
  arrowright: 'ArrowRight',
  home: 'Home',
  end: 'End',
  pageup: 'PageUp',
  pagedown: 'PageDown',
}

export interface KeySpec {
  key: string
  code: string
  ctrlKey: boolean
  shiftKey: boolean
  altKey: boolean
  metaKey: boolean
}

/** 解析 `Enter`、`Control+a`、`Shift+Tab`、`Meta+Enter` 这类按键描述。 */
export function parseKey(spec: string): KeySpec {
  const raw = spec.trim()
  if (!raw) throw new ToolCallError('INVALID_INPUT', 'key 不能为空')
  const parts = raw === '+' ? ['+'] : raw.endsWith('++') ? [...raw.slice(0, -2).split('+'), '+'] : raw.split('+')
  const out: KeySpec = { key: '', code: '', ctrlKey: false, shiftKey: false, altKey: false, metaKey: false }
  parts.forEach((p, i) => {
    const lower = p.trim().toLowerCase()
    if (i < parts.length - 1) {
      if (lower === 'ctrl' || lower === 'control') out.ctrlKey = true
      else if (lower === 'shift') out.shiftKey = true
      else if (lower === 'alt' || lower === 'option') out.altKey = true
      else if (lower === 'meta' || lower === 'cmd' || lower === 'command') out.metaKey = true
      else throw new ToolCallError('INVALID_INPUT', `无法识别的修饰键 "${p}"`)
      return
    }
    const key = KEY_ALIASES[lower] ?? (p.length === 1 ? p : p.trim())
    out.key = key
    if (key.length === 1) {
      if (/[a-z]/i.test(key)) out.code = `Key${key.toUpperCase()}`
      else if (/[0-9]/.test(key)) out.code = `Digit${key}`
      else if (key === ' ') out.code = 'Space'
      else out.code = key
    } else {
      out.code = key
    }
  })
  return out
}

const TEXT_INPUT_TYPES = new Set([
  'text', 'email', 'tel', 'url', 'password', 'search', 'number', 'date', 'time', 'datetime-local', 'month', 'week',
])

const FOCUSABLE = 'a[href],button,input,select,textarea,summary,[tabindex],[contenteditable]'

function moveFocus(from: Element, backwards: boolean, isUsable: (el: Element) => boolean): void {
  const doc = from.ownerDocument
  const list = Array.from(doc.querySelectorAll(FOCUSABLE)).filter((el) => {
    const t = el.getAttribute('tabindex')
    if (t !== null && Number(t) < 0) return false
    if (el.tagName === 'INPUT' && (el as HTMLInputElement).type === 'hidden') return false
    return isUsable(el)
  })
  if (list.length === 0) return
  const i = list.indexOf(from)
  const next = backwards ? list[(i <= 0 ? list.length : i) - 1] : list[(i + 1) % list.length]
  if (next) focus(next)
}

/** 派发 keydown / keypress / keyup；未被阻止时模拟 Enter、空格、Tab 的默认行为。 */
export function press(target: Element, spec: KeySpec, isUsable: (el: Element) => boolean): void {
  const win = windowOf(target)
  const init = {
    key: spec.key,
    code: spec.code,
    ctrlKey: spec.ctrlKey,
    shiftKey: spec.shiftKey,
    altKey: spec.altKey,
    metaKey: spec.metaKey,
    bubbles: true,
    cancelable: true,
    composed: true,
    view: win,
  }
  const down = new win.KeyboardEvent('keydown', init)
  const proceed = target.dispatchEvent(down)
  const printable = spec.key.length === 1 || spec.key === 'Enter'
  if (proceed && printable && !spec.ctrlKey && !spec.metaKey) {
    target.dispatchEvent(new win.KeyboardEvent('keypress', init))
  }
  if (proceed && !spec.ctrlKey && !spec.metaKey && !spec.altKey) {
    const tag = target.tagName
    const role = target.getAttribute('role')
    if (spec.key === 'Enter') {
      if (tag === 'INPUT' && TEXT_INPUT_TYPES.has((target as HTMLInputElement).type)) {
        const form = (target as HTMLInputElement).form
        if (form) requestSubmit(form)
      } else if (tag === 'BUTTON' || tag === 'A' || tag === 'SUMMARY' || role === 'button' || role === 'link') {
        click(target)
      }
    } else if (spec.key === 'Tab') {
      moveFocus(target, spec.shiftKey, isUsable)
    }
  }
  target.dispatchEvent(new win.KeyboardEvent('keyup', init))
  if (proceed && spec.key === ' ' && !spec.ctrlKey && !spec.metaKey && !spec.altKey) {
    const tag = target.tagName
    const type = (target as HTMLInputElement).type
    const role = target.getAttribute('role') ?? ''
    if (
      tag === 'BUTTON' ||
      tag === 'SUMMARY' ||
      (tag === 'INPUT' && ['checkbox', 'radio', 'button', 'submit', 'reset'].includes(type)) ||
      ['button', 'checkbox', 'switch', 'radio', 'menuitem', 'tab', 'option'].includes(role)
    ) {
      click(target)
    }
  }
}

// ---------------------------------------------------------------------------
// 提交、滚动
// ---------------------------------------------------------------------------

function requestSubmit(form: HTMLFormElement, submitter?: HTMLElement): void {
  if (typeof form.requestSubmit === 'function') {
    form.requestSubmit(submitter)
    return
  }
  const win = windowOf(form)
  const ev = new win.Event('submit', { bubbles: true, cancelable: true })
  if (form.dispatchEvent(ev)) form.submit()
}

export interface InvalidField {
  el: Element
  message: string
}

/** 表单或其中元素 → requestSubmit；校验未通过时返回无效字段（不提交）。 */
export function submit(el: Element, ref: string): InvalidField[] {
  const form: HTMLFormElement | null =
    el.tagName === 'FORM' ? (el as HTMLFormElement) : ((el as HTMLInputElement).form ?? el.closest('form'))
  if (!form) throw new ToolCallError('INVALID_INPUT', `${ref} 不是表单，也不在表单内`)
  const invalid: InvalidField[] = []
  if (!form.noValidate) {
    for (const field of Array.from(form.elements)) {
      const f = field as HTMLInputElement
      if (typeof f.checkValidity === 'function' && f.willValidate !== false && !f.checkValidity()) {
        invalid.push({ el: f, message: f.validationMessage || '不符合要求' })
      }
    }
  }
  if (invalid.length > 0) return invalid
  const isSubmitter =
    (el.tagName === 'BUTTON' && (el as HTMLButtonElement).type === 'submit') ||
    (el.tagName === 'INPUT' && ['submit', 'image'].includes((el as HTMLInputElement).type))
  requestSubmit(form, isSubmitter ? (el as HTMLElement) : undefined)
  return []
}

export function scroll(el: Element): void {
  scrollIntoView(el)
}

/** 等待页面稳定：一帧（后台标签页不触发 rAF 时最多等 100ms）再加 50ms。 */
export async function settle(win: Win): Promise<void> {
  await new Promise<void>((resolve) => {
    let done = false
    const finish = () => {
      if (done) return
      done = true
      resolve()
    }
    if (typeof win.requestAnimationFrame === 'function') win.requestAnimationFrame(() => finish())
    setTimeout(finish, 100)
  })
  await new Promise<void>((resolve) => setTimeout(resolve, 50))
}
