import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { attachDom } from '../src'
import { createFakeAppMcp, settle, type FakeAppMcp } from './fake'

let app: FakeAppMcp
let detach: (() => void) | undefined

beforeEach(() => {
  document.body.innerHTML = ''
  app = createFakeAppMcp()
})
afterEach(() => {
  detach?.()
  detach = undefined
  vi.restoreAllMocks()
})

const FULL_FORM = `
<form data-mcp-tool="profile.save" data-mcp-desc="保存资料">
  <label for="n">姓名</label><input id="n" name="name" required minlength="2" maxlength="20" pattern="[a-z]+">
  <label>年龄 <input name="age" type="number" min="0" max="150"></label>
  <input name="score" type="range" min="0.5" max="10" step="0.5">
  <input name="agree" type="checkbox" data-mcp-desc="同意条款">
  <fieldset><legend>性别</legend>
    <label><input type="radio" name="sex" value="m" required>男</label>
    <label><input type="radio" name="sex" value="f">女</label>
  </fieldset>
  <select name="city" aria-label="城市"><option value="">请选择</option><option value="bj">北京</option><option value="sh">上海</option></select>
  <select name="tags" multiple><option>a</option><option>b</option></select>
  <input name="email" type="email" placeholder="邮箱地址">
  <input name="site" type="url">
  <input name="day" type="date">
  <input name="at" type="datetime-local">
  <input name="clock" type="time">
  <textarea name="bio" data-mcp-desc="简介"></textarea>
  <input name="secret" data-mcp-ignore>
  <div data-mcp-ignore><input name="ignored2"></div>
  <input type="hidden" name="h"><input type="file" name="f"><input type="submit" name="s">
  <button type="button" name="b">b</button>
  <input name="off" disabled>
  <input name="colors" type="checkbox" value="red"><input name="colors" type="checkbox" value="blue">
  <button>提交</button>
</form>`

describe('表单 schema 推导', () => {
  it('各字段类型', () => {
    document.body.innerHTML = FULL_FORM
    detach = attachDom(app)
    const input = app.tools.get('profile.save')!.def.input as any
    expect(input.required).toEqual(['name', 'sex'])
    expect(input.additionalProperties).toBe(false)
    const p = input.properties
    expect(Object.keys(p)).toEqual(['name', 'age', 'score', 'agree', 'sex', 'city', 'tags', 'email', 'site', 'day', 'at', 'clock', 'bio', 'colors'])
    expect(p.name).toEqual({ type: 'string', minLength: 2, maxLength: 20, pattern: '^(?:[a-z]+)$', description: '姓名' })
    expect(p.age).toEqual({ type: 'number', minimum: 0, maximum: 150, multipleOf: 1, description: '年龄' })
    expect(p.score).toEqual({ type: 'number', minimum: 0.5, maximum: 10 })
    expect(p.agree).toEqual({ type: 'boolean', description: '同意条款' })
    expect(p.sex).toEqual({ type: 'string', enum: ['m', 'f'], description: '性别；可选值：m=男, f=女' })
    expect(p.city).toEqual({ type: 'string', enum: ['bj', 'sh'], description: '城市；可选值：bj=北京, sh=上海' })
    expect(p.tags).toEqual({ type: 'array', items: { type: 'string', enum: ['a', 'b'] }, uniqueItems: true })
    expect(p.email).toEqual({ type: 'string', format: 'email', description: '邮箱地址' })
    expect(p.site).toEqual({ type: 'string', format: 'uri' })
    expect(p.day).toEqual({ type: 'string', format: 'date' })
    expect(p.at.format).toBe('date-time')
    expect(p.clock.format).toBe('time')
    expect(p.bio).toEqual({ type: 'string', description: '简介' })
    expect(p.colors).toEqual({ type: 'array', items: { type: 'string', enum: ['red', 'blue'] }, uniqueItems: true })
  })

  it('字段变化后更新 schema', async () => {
    document.body.innerHTML = `<form data-mcp-tool="f" data-mcp-desc="f"><input name="a"></form>`
    detach = attachDom(app)
    document.querySelector('form')!.insertAdjacentHTML('beforeend', '<input name="b" required>')
    await settle()
    const t = app.tools.get('f')!
    expect(t.updates).toHaveLength(1)
    expect((t.current.input as any).required).toEqual(['b'])
  })
})

describe('表单调用', () => {
  it('填值并提交', async () => {
    document.body.innerHTML = FULL_FORM
    const form = document.querySelector('form')!
    let submitted: FormData | undefined
    form.addEventListener('submit', (e) => {
      e.preventDefault()
      submitted = new FormData(form)
    })
    detach = attachDom(app, { settleMs: 0 })
    const res = await app.call('profile.save', {
      name: 'abc',
      age: 30,
      agree: true,
      sex: 'f',
      city: 'sh',
      tags: ['b'],
      bio: '你好',
      colors: ['blue'],
    })
    expect(res).toEqual({ data: { ok: true } })
    expect(submitted!.get('name')).toBe('abc')
    expect(submitted!.get('age')).toBe('30')
    expect(submitted!.get('agree')).toBe('on')
    expect(submitted!.get('sex')).toBe('f')
    expect(submitted!.get('city')).toBe('sh')
    expect(submitted!.getAll('tags')).toEqual(['b'])
    expect(submitted!.get('bio')).toBe('你好')
    expect(submitted!.getAll('colors')).toEqual(['blue'])
  })

  it('校验失败返回 INVALID_INPUT 且不提交', async () => {
    document.body.innerHTML = `<form data-mcp-tool="f" data-mcp-desc="f"><input name="a" required><input name="n" type="number"></form>`
    const onSubmit = vi.fn((e: Event) => e.preventDefault())
    document.querySelector('form')!.addEventListener('submit', onSubmit)
    detach = attachDom(app)
    await expect(app.call('f', {})).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
    await expect(app.call('f', { a: 'x', zz: 1 })).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
    await expect(app.call('f', { a: 'x', n: 'abc' })).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
    expect(onSubmit).not.toHaveBeenCalled()
  })

  it('非法枚举值返回 INVALID_INPUT', async () => {
    document.body.innerHTML = `<form data-mcp-tool="f" data-mcp-desc="f"><select name="s"><option>a</option></select></form>`
    detach = attachDom(app)
    await expect(app.call('f', { s: 'zz' })).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
  })

  it('兼容 React 受控组件：绕过实例上的值追踪器并派发 input/change', async () => {
    document.body.innerHTML = `<form data-mcp-tool="f" data-mcp-desc="f"><input name="title"><input name="done" type="checkbox"></form>`
    const form = document.querySelector('form')!
    form.addEventListener('submit', (e) => e.preventDefault())
    const input = form.querySelector<HTMLInputElement>('input[name=title]')!
    const box = form.querySelector<HTMLInputElement>('input[name=done]')!

    // 模拟 React 的 inputValueTracking：在实例上覆盖 value，记录"最后已知值"
    const proto = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!
    let tracked = ''
    Object.defineProperty(input, 'value', {
      configurable: true,
      get() {
        return proto.get!.call(this)
      },
      set(v: string) {
        tracked = v
        proto.set!.call(this, v)
      },
    })
    const onChange = vi.fn()
    // React 只在"当前 DOM 值 ≠ 追踪值"时触发 onChange
    input.addEventListener('input', () => {
      const current = proto.get!.call(input)
      if (current !== tracked) {
        tracked = current
        onChange(current)
      }
    })
    const events: string[] = []
    box.addEventListener('click', () => events.push('click'))
    box.addEventListener('change', () => events.push('change'))

    detach = attachDom(app, { settleMs: 0 })
    await app.call('f', { title: '买菜', done: true })
    expect(onChange).toHaveBeenCalledWith('买菜')
    expect(box.checked).toBe(true)
    expect(events).toEqual(['click', 'change'])
  })

  it('checkbox 页面阻止默认行为时退回 setter', async () => {
    document.body.innerHTML = `<form data-mcp-tool="f" data-mcp-desc="f"><input name="done" type="checkbox"></form>`
    const form = document.querySelector('form')!
    form.addEventListener('submit', (e) => e.preventDefault())
    const box = form.querySelector('input')!
    box.addEventListener('click', (e) => e.preventDefault())
    detach = attachDom(app, { settleMs: 0 })
    await app.call('f', { done: true })
    expect(box.checked).toBe(true)
  })

  it('data-mcp-result 结果', async () => {
    document.body.innerHTML = `<form data-mcp-tool="f" data-mcp-desc="f" data-mcp-result="saved"><input name="a"></form>`
    const form = document.querySelector('form')!
    form.addEventListener('submit', (e) => {
      e.preventDefault()
      const a = new FormData(form).get('a')
      queueMicrotask(() => form.dispatchEvent(new CustomEvent('saved', { detail: { id: 7, a } })))
    })
    detach = attachDom(app)
    await expect(app.call('f', { a: 'x' })).resolves.toEqual({ data: { id: 7, a: 'x' } })
  })

  it('表单集合：每行一个表单，key 选择行', async () => {
    document.body.innerHTML = `<ul>
      <li>买菜<form data-mcp-tool="todo.rename" data-mcp-desc="重命名" data-mcp-key="a1"><input name="title"></form></li>
      <li>写周报<form data-mcp-tool="todo.rename" data-mcp-desc="重命名" data-mcp-key="a2"><input name="title"></form></li>
    </ul>`
    const got: string[] = []
    for (const f of document.querySelectorAll('form'))
      f.addEventListener('submit', (e) => {
        e.preventDefault()
        got.push(`${f.dataset.mcpKey}:${new FormData(f).get('title')}`)
      })
    detach = attachDom(app, { settleMs: 0 })
    const t = app.tools.get('todo.rename')!
    expect(t.def.description).toBe('重命名（key：a1=买菜, a2=写周报）')
    expect((t.def.input as any).required).toEqual(['key'])
    expect(Object.keys((t.def.input as any).properties)).toEqual(['key', 'title'])
    await app.call('todo.rename', { key: 'a2', title: '写月报' })
    expect(got).toEqual(['a2:写月报'])
  })
})
