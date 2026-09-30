/**
 * 表单 → 参数 schema，以及按参数填写表单（兼容 React 等框架的受控组件）。
 */

import { ToolCallError } from '@app-mcp/web'
import { ATTR, WEBMCP, attr } from './attrs'
import { collapse, truncate, visibleText } from './text'

type Control = HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement

export type FieldKind = 'string' | 'number' | 'boolean' | 'radio' | 'checkboxes' | 'select' | 'select-multiple'

export interface Field {
  name: string
  kind: FieldKind
  controls: Control[]
  required: boolean
  schema: Record<string, unknown>
}

const EXCLUDED_INPUT_TYPES = new Set(['hidden', 'submit', 'button', 'file', 'reset', 'image'])
const MAX_OPTIONS_IN_DESC = 30

function isControl(el: Element): el is Control {
  return el.tagName === 'INPUT' || el.tagName === 'SELECT' || el.tagName === 'TEXTAREA'
}

function inputType(el: Control): string {
  return el.tagName === 'INPUT' ? (el as HTMLInputElement).type.toLowerCase() : el.tagName.toLowerCase()
}

function controlsOf(form: HTMLFormElement): Control[] {
  const list = form.elements ? Array.from(form.elements) : Array.from(form.querySelectorAll('input,select,textarea'))
  return list.filter((el): el is Control => isControl(el))
}

function ignored(control: Control, form: HTMLFormElement): boolean {
  const ign = control.closest(`[${ATTR.ignore}]`)
  return ign !== null && (ign === control || form.contains(ign))
}

/** 标签文本中不计入的子元素：控件本身、按钮、下拉选项。 */
function skipInLabel(el: Element): boolean {
  return isControl(el) || el.tagName === 'BUTTON'
}

function labelText(control: Control): string | undefined {
  const texts: string[] = []
  const labels = control.labels
  if (labels && labels.length > 0) {
    for (const l of Array.from(labels)) texts.push(visibleText(l, 100, skipInLabel))
  } else {
    const wrap = control.closest('label')
    if (wrap) texts.push(visibleText(wrap, 100, skipInLabel))
  }
  const t = collapse(texts.join(' '))
  return t ? truncate(t, 100) : undefined
}

/** 字段描述：toolparamdescription > data-mcp-desc > 关联 label > aria-label > placeholder。 */
function controlDesc(control: Control): string | undefined {
  return (
    attr(control, WEBMCP.toolparamdescription) ??
    attr(control, ATTR.desc) ??
    labelText(control) ??
    attr(control, 'aria-label') ??
    attr(control, 'placeholder')
  )
}

/** radio / checkbox 组的描述：任一控件上的显式描述 > fieldset 的 legend > aria-label。 */
function groupDesc(controls: Control[], form: HTMLFormElement): string | undefined {
  for (const c of controls) {
    const d = attr(c, WEBMCP.toolparamdescription) ?? attr(c, ATTR.desc)
    if (d) return d
  }
  const fieldset = controls[0]?.closest('fieldset')
  if (fieldset && form.contains(fieldset)) {
    const legend = fieldset.querySelector('legend')
    if (legend) {
      const t = visibleText(legend, 100)
      if (t) return t
    }
    const a = attr(fieldset, 'aria-label')
    if (a) return a
  }
  return undefined
}

function joinDesc(...parts: Array<string | undefined>): string | undefined {
  const s = parts.filter(Boolean).join('；')
  return s || undefined
}

function optionsNote(pairs: Array<[string, string]>): string | undefined {
  const described = pairs.filter(([v, l]) => l && l !== v)
  if (described.length === 0) return undefined
  const shown = described.slice(0, MAX_OPTIONS_IN_DESC).map(([v, l]) => `${v}=${l}`)
  const more = described.length > shown.length ? ` 等 ${described.length} 项` : ''
  return `可选值：${shown.join(', ')}${more}`
}

function num(v: string | null): number | undefined {
  if (v === null || v.trim() === '') return undefined
  const n = Number(v)
  return Number.isFinite(n) ? n : undefined
}

function stringSchema(control: HTMLInputElement | HTMLTextAreaElement, type: string): Record<string, unknown> {
  const s: Record<string, unknown> = { type: 'string' }
  const notes: string[] = []
  switch (type) {
    case 'email':
      s.format = 'email'
      break
    case 'url':
      s.format = 'uri'
      break
    case 'date':
      s.format = 'date'
      break
    case 'datetime-local':
      s.format = 'date-time'
      notes.push('本地时间，形如 2024-05-01T09:30')
      break
    case 'time':
      s.format = 'time'
      notes.push('形如 09:30')
      break
    case 'month':
      s.pattern = '^\\d{4}-\\d{2}$'
      break
    case 'week':
      s.pattern = '^\\d{4}-W\\d{2}$'
      break
    case 'color':
      s.pattern = '^#[0-9a-fA-F]{6}$'
      break
  }
  const min = control.getAttribute('min')
  const max = control.getAttribute('max')
  if ((min || max) && ['date', 'datetime-local', 'time', 'month', 'week'].includes(type)) {
    notes.push(`范围 ${min ?? '…'} ~ ${max ?? '…'}`)
  }
  const minLength = num(control.getAttribute('minlength'))
  const maxLength = num(control.getAttribute('maxlength'))
  if (minLength !== undefined && minLength >= 0) s.minLength = minLength
  if (maxLength !== undefined && maxLength >= 0) s.maxLength = maxLength
  const pattern = control.getAttribute('pattern')
  if (pattern && s.pattern === undefined) s.pattern = `^(?:${pattern})$`
  const desc = joinDesc(controlDesc(control), ...notes)
  if (desc) s.description = desc
  return s
}

function numberSchema(control: HTMLInputElement): Record<string, unknown> {
  const s: Record<string, unknown> = { type: 'number' }
  const min = num(control.getAttribute('min'))
  const max = num(control.getAttribute('max'))
  if (min !== undefined) s.minimum = min
  if (max !== undefined) s.maximum = max
  // HTML 默认步长为 1（以 min 为基准）；基准为 0 时可以精确表达为 multipleOf。
  const stepAttr = control.getAttribute('step')
  if (stepAttr?.trim().toLowerCase() !== 'any') {
    const step = num(stepAttr) ?? 1
    if (step > 0 && (min === undefined || min === 0)) s.multipleOf = step
  }
  const desc = controlDesc(control)
  if (desc) s.description = desc
  return s
}

function buildField(name: string, controls: Control[], form: HTMLFormElement): Field {
  const first = controls[0] as Control
  const type = inputType(first)
  if (type === 'radio') {
    const radios = controls.filter((c) => inputType(c) === 'radio') as HTMLInputElement[]
    const values = [...new Set(radios.map((r) => r.value))]
    const desc = joinDesc(
      groupDesc(radios, form),
      optionsNote(radios.map((r) => [r.value, labelText(r) ?? ''])),
    )
    return {
      name,
      kind: 'radio',
      controls: radios,
      required: radios.some((r) => r.required),
      schema: { type: 'string', enum: values, ...(desc && { description: desc }) },
    }
  }
  if (type === 'checkbox') {
    const boxes = controls.filter((c) => inputType(c) === 'checkbox') as HTMLInputElement[]
    if (boxes.length === 1) {
      const desc = controlDesc(first)
      return {
        name,
        kind: 'boolean',
        controls: boxes,
        required: (first as HTMLInputElement).required,
        schema: { type: 'boolean', ...(desc && { description: desc }) },
      }
    }
    const values = [...new Set(boxes.map((b) => b.value))]
    const desc = joinDesc(groupDesc(boxes, form), optionsNote(boxes.map((b) => [b.value, labelText(b) ?? ''])))
    return {
      name,
      kind: 'checkboxes',
      controls: boxes,
      required: false,
      schema: {
        type: 'array',
        items: { type: 'string', enum: values },
        uniqueItems: true,
        ...(desc && { description: desc }),
      },
    }
  }
  if (type === 'select') {
    const select = first as HTMLSelectElement
    const options = Array.from(select.options).filter((o) => !o.disabled && o.value !== '')
    const values = [...new Set(options.map((o) => o.value))]
    const desc = joinDesc(
      controlDesc(select),
      optionsNote(options.map((o) => [o.value, collapse(o.label || o.text || '')])),
    )
    const item = { type: 'string', enum: values }
    if (select.multiple) {
      return {
        name,
        kind: 'select-multiple',
        controls: [select],
        required: select.required,
        schema: { type: 'array', items: item, uniqueItems: true, ...(desc && { description: desc }) },
      }
    }
    return {
      name,
      kind: 'select',
      controls: [select],
      required: select.required,
      schema: { ...item, ...(desc && { description: desc }) },
    }
  }
  if (type === 'number' || type === 'range') {
    return {
      name,
      kind: 'number',
      controls: [first],
      required: first.required,
      schema: numberSchema(first as HTMLInputElement),
    }
  }
  return {
    name,
    kind: 'string',
    controls: [first],
    required: first.required,
    schema: stringSchema(first as HTMLInputElement | HTMLTextAreaElement, type),
  }
}

/** 收集表单中可填写的字段（按名称分组，保持文档顺序）。 */
export function collectFields(form: HTMLFormElement): Field[] {
  const groups = new Map<string, Control[]>()
  for (const control of controlsOf(form)) {
    const name = control.name
    if (!name) continue
    if (control.disabled) continue
    if (control.tagName === 'INPUT' && EXCLUDED_INPUT_TYPES.has(inputType(control))) continue
    if (ignored(control, form)) continue
    const list = groups.get(name)
    if (list) list.push(control)
    else groups.set(name, [control])
  }
  return [...groups].map(([name, controls]) => buildField(name, controls, form))
}

/** 字段 → JSON Schema 的 properties 与 required。 */
export function fieldsToSchema(fields: Field[]): { properties: Record<string, unknown>; required: string[] } {
  const properties: Record<string, unknown> = {}
  const required: string[] = []
  for (const f of fields) {
    properties[f.name] = f.schema
    if (f.required) required.push(f.name)
  }
  return { properties, required }
}

// ---------------------------------------------------------------------------
// 填写
// ---------------------------------------------------------------------------

/**
 * 找到原型链上的属性 setter。
 * React 在受控组件实例上定义了 `value` / `checked` 的追踪属性：直接赋值会被追踪器记下，
 * 随后派发的 input 事件被认为"值没变"而不触发 onChange。调用原型上的原生 setter 可以绕过追踪器。
 */
function nativeSetter(el: object, prop: string): ((v: unknown) => void) | undefined {
  for (let proto = Object.getPrototypeOf(el); proto; proto = Object.getPrototypeOf(proto)) {
    const desc = Object.getOwnPropertyDescriptor(proto, prop)
    if (desc?.set) return desc.set
  }
  return undefined
}

function fire(el: Element, type: string): void {
  const Ev = el.ownerDocument.defaultView?.Event ?? Event
  el.dispatchEvent(new Ev(type, { bubbles: true }))
}

export function setValue(el: Control, value: string): void {
  const set = nativeSetter(el, 'value')
  if (set) set.call(el, value)
  else el.value = value
  fire(el, 'input')
  fire(el, 'change')
}

export function setChecked(el: HTMLInputElement, checked: boolean): void {
  if (el.checked === checked) return
  // 原生 click 会切换选中状态并依次派发 click / input / change，React 通过 click 事件感知 checkbox / radio。
  el.click()
  if (el.checked !== checked) {
    // 页面阻止了默认行为等情况：退回原生 setter + 事件。
    const set = nativeSetter(el, 'checked')
    if (set) set.call(el, checked)
    else el.checked = checked
    fire(el, 'input')
    fire(el, 'change')
  }
}

function setSelected(select: HTMLSelectElement, values: Set<string>): void {
  for (const option of Array.from(select.options)) {
    const want = values.has(option.value)
    if (option.selected === want) continue
    const set = nativeSetter(option, 'selected')
    if (set) set.call(option, want)
    else option.selected = want
  }
  fire(select, 'input')
  fire(select, 'change')
}

function invalid(message: string): ToolCallError {
  return new ToolCallError('INVALID_INPUT', message)
}

function toBool(v: unknown, name: string): boolean {
  if (typeof v === 'boolean') return v
  if (v === 'true') return true
  if (v === 'false') return false
  throw invalid(`参数 ${name} 应为布尔值`)
}

function toStr(v: unknown, name: string): string {
  if (typeof v === 'string') return v
  if (typeof v === 'number' && Number.isFinite(v)) return String(v)
  throw invalid(`参数 ${name} 应为字符串`)
}

function toStrArray(v: unknown, name: string): string[] {
  if (!Array.isArray(v)) throw invalid(`参数 ${name} 应为数组`)
  return v.map((x) => toStr(x, name))
}

/** 按参数填写表单；未提供的字段保持原值。 */
export function fillForm(fields: Field[], input: Record<string, unknown>): void {
  const byName = new Map(fields.map((f) => [f.name, f]))
  const unknown = Object.keys(input).filter((k) => !byName.has(k))
  if (unknown.length > 0) {
    throw invalid(`未知参数：${unknown.join(', ')}；可用参数：${fields.map((f) => f.name).join(', ') || '（无）'}`)
  }
  // 先校验，全部通过后再写入，避免填了一半
  const plan: Array<() => void> = []
  for (const f of fields) {
    if (!Object.prototype.hasOwnProperty.call(input, f.name)) continue
    const v = input[f.name]
    if (v === undefined || v === null) continue
    switch (f.kind) {
      case 'string': {
        const s = toStr(v, f.name)
        plan.push(() => setValue(f.controls[0] as Control, s))
        break
      }
      case 'number': {
        const n = typeof v === 'number' ? v : Number(toStr(v, f.name))
        if (!Number.isFinite(n)) throw invalid(`参数 ${f.name} 应为数字`)
        plan.push(() => setValue(f.controls[0] as Control, String(n)))
        break
      }
      case 'boolean': {
        const b = toBool(v, f.name)
        plan.push(() => setChecked(f.controls[0] as HTMLInputElement, b))
        break
      }
      case 'radio': {
        const s = toStr(v, f.name)
        const radio = (f.controls as HTMLInputElement[]).find((r) => r.value === s)
        if (!radio) throw invalid(`参数 ${f.name} 的值 ${JSON.stringify(s)} 不在可选值中`)
        plan.push(() => setChecked(radio, true))
        break
      }
      case 'checkboxes': {
        const values = new Set(toStrArray(v, f.name))
        const known = new Set((f.controls as HTMLInputElement[]).map((c) => c.value))
        const bad = [...values].filter((x) => !known.has(x))
        if (bad.length > 0) throw invalid(`参数 ${f.name} 包含未知值：${bad.join(', ')}`)
        plan.push(() => {
          for (const c of f.controls as HTMLInputElement[]) setChecked(c, values.has(c.value))
        })
        break
      }
      case 'select':
      case 'select-multiple': {
        const select = f.controls[0] as HTMLSelectElement
        const values = f.kind === 'select' ? [toStr(v, f.name)] : toStrArray(v, f.name)
        const known = new Set(Array.from(select.options).map((o) => o.value))
        const bad = values.filter((x) => !known.has(x))
        if (bad.length > 0) throw invalid(`参数 ${f.name} 的值 ${bad.map((x) => JSON.stringify(x)).join(', ')} 不在可选值中`)
        plan.push(() => {
          if (f.kind === 'select') setValue(select, values[0] as string)
          else setSelected(select, new Set(values))
        })
        break
      }
    }
  }
  for (const step of plan) step()
}

/** 约束校验失败时抛出 INVALID_INPUT，列出每个字段的原因。 */
export function assertValid(form: HTMLFormElement): void {
  if (form.noValidate || typeof form.checkValidity !== 'function') return
  if (form.checkValidity()) return
  const problems: Record<string, string> = {}
  for (const control of controlsOf(form)) {
    const validity = control.validity
    if (!control.willValidate || !validity || validity.valid) continue
    const key = control.name || control.id || control.tagName.toLowerCase()
    problems[key] ??= control.validationMessage || '不满足约束'
  }
  const summary = Object.entries(problems)
    .map(([k, m]) => `${k}: ${m}`)
    .join('; ')
  throw new ToolCallError('INVALID_INPUT', `表单校验未通过：${summary || '存在无效字段'}`, { fields: problems })
}
