import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { attachDom } from '../src'
import { toErrorKind } from '../src/attrs'
import { createFakeAppMcp, settle, type FakeAppMcp } from './fake'

let app: FakeAppMcp
let detach: (() => void) | undefined

beforeEach(() => {
  document.body.innerHTML = ''
  document.title = '测试页'
  app = createFakeAppMcp()
})
afterEach(() => {
  detach?.()
  detach = undefined
  vi.restoreAllMocks()
})

describe('按钮工具', () => {
  it('登记并点击', async () => {
    document.body.innerHTML = `<button data-mcp-tool="cart.clear" data-mcp-desc="清空购物车" data-mcp-title="清空" data-mcp-risk="destructive">清空</button>`
    const btn = document.querySelector('button')!
    const onClick = vi.fn()
    btn.addEventListener('click', onClick)
    detach = attachDom(app, { settleMs: 0 })
    const t = app.tools.get('cart.clear')!
    expect(t.def.description).toBe('清空购物车')
    expect(t.def.title).toBe('清空')
    expect(t.def.risk).toBe('destructive')
    expect(t.def.enabled).toBe(true)
    expect(t.def.input).toEqual({ type: 'object', properties: {}, additionalProperties: false })
    await expect(app.call('cart.clear')).resolves.toEqual({ data: { ok: true } })
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it('缺少描述时用可见文本兜底并警告', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    document.body.innerHTML = `<button data-mcp-tool="a.b">  删除 <span>全部</span></button><a data-mcp-tool="c" aria-label="设置"></a>`
    detach = attachDom(app)
    expect(app.tools.get('a.b')!.def.description).toBe('删除 全部')
    expect(app.tools.get('c')!.def.description).toBe('设置')
    expect(warn).toHaveBeenCalled()
  })

  it('无参数工具拒绝多余参数', async () => {
    document.body.innerHTML = `<button data-mcp-tool="x" data-mcp-desc="x">x</button>`
    detach = attachDom(app)
    await expect(app.call('x', { a: 1 })).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
  })

  it('stateHints', async () => {
    document.body.innerHTML = `<button data-mcp-tool="x" data-mcp-desc="x" data-mcp-hints="cart.state, todos.list">x</button>`
    detach = attachDom(app, { settleMs: 0 })
    await expect(app.call('x')).resolves.toEqual({ data: { ok: true }, stateHints: ['cart.state', 'todos.list'] })
  })

  it('无效名称只警告不抛出', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    document.body.innerHTML = `<button data-mcp-tool="有 空格" data-mcp-desc="x">x</button>`
    expect(() => (detach = attachDom(app))).not.toThrow()
    expect(app.tools.size).toBe(0)
    expect(warn).toHaveBeenCalled()
  })
})

describe('结构化结果', () => {
  it('data-mcp-result 等待 CustomEvent', async () => {
    document.body.innerHTML = `<button data-mcp-tool="x" data-mcp-desc="x" data-mcp-result="done">x</button>`
    const btn = document.querySelector('button')!
    btn.addEventListener('click', () => {
      setTimeout(() => btn.dispatchEvent(new CustomEvent('done', { detail: { count: 3 } })), 10)
    })
    detach = attachDom(app)
    await expect(app.call('x')).resolves.toEqual({ data: { count: 3 } })
  })

  it('超时返回 TIMEOUT', async () => {
    document.body.innerHTML = `<button data-mcp-tool="x" data-mcp-desc="x" data-mcp-result="done" data-mcp-timeout="30">x</button>`
    detach = attachDom(app)
    await expect(app.call('x')).rejects.toMatchObject({ kind: 'TIMEOUT' })
  })

  it('mcp:error 表示失败', async () => {
    document.body.innerHTML = `<button data-mcp-tool="x" data-mcp-desc="x">x</button>`
    const btn = document.querySelector('button')!
    btn.addEventListener('click', () => {
      btn.dispatchEvent(new CustomEvent('mcp:error', { detail: { kind: 'UNAUTHORIZED', message: '请先登录' } }))
    })
    detach = attachDom(app, { settleMs: 20 })
    await expect(app.call('x')).rejects.toMatchObject({ kind: 'UNAUTHORIZED', message: '请先登录' })
  })

  it('未知错误类别归为 HANDLER_ERROR', async () => {
    document.body.innerHTML = `<button data-mcp-tool="x" data-mcp-desc="x" data-mcp-result="done">x</button>`
    const btn = document.querySelector('button')!
    btn.addEventListener('click', () => btn.dispatchEvent(new CustomEvent('mcp:error', { detail: { kind: 'NOPE' } })))
    detach = attachDom(app)
    await expect(app.call('x')).rejects.toMatchObject({ kind: 'HANDLER_ERROR' })
  })

  it('取消', async () => {
    document.body.innerHTML = `<button data-mcp-tool="x" data-mcp-desc="x" data-mcp-result="done">x</button>`
    detach = attachDom(app)
    const ac = new AbortController()
    const p = app.call('x', {}, ac.signal)
    ac.abort()
    await expect(p).rejects.toMatchObject({ kind: 'CANCELLED' })
  })
})

describe('启用状态', () => {
  it('disabled / aria-disabled / hidden / inert / display:none / 祖先禁用', () => {
    document.body.innerHTML = `
      <button data-mcp-tool="a" data-mcp-desc="a" disabled>a</button>
      <button data-mcp-tool="b" data-mcp-desc="b" aria-disabled="true">b</button>
      <button data-mcp-tool="c" data-mcp-desc="c" hidden>c</button>
      <div inert><button data-mcp-tool="d" data-mcp-desc="d">d</button></div>
      <div style="display:none"><button data-mcp-tool="e" data-mcp-desc="e">e</button></div>
      <fieldset disabled><button data-mcp-tool="f" data-mcp-desc="f">f</button></fieldset>
      <button data-mcp-tool="g" data-mcp-desc="g">g</button>`
    detach = attachDom(app)
    const enabled = Object.fromEntries([...app.tools].map(([k, t]) => [k, t.current.enabled]))
    expect(enabled).toEqual({ a: false, b: false, c: false, d: false, e: false, f: false, g: true })
  })

  it('属性变化后更新 enabled，调用禁用工具返回 TOOL_DISABLED', async () => {
    document.body.innerHTML = `<button data-mcp-tool="a" data-mcp-desc="a">a</button>`
    detach = attachDom(app)
    const btn = document.querySelector('button')!
    btn.disabled = true
    await settle()
    const t = app.tools.get('a')!
    expect(t.current.enabled).toBe(false)
    expect(t.updates).toEqual([{ enabled: false }])
    await expect(app.call('a')).rejects.toMatchObject({ kind: 'TOOL_DISABLED' })
    btn.disabled = false
    await settle()
    expect(t.current.enabled).toBe(true)
  })

  it('同一帧内多次变化只 update 一次', async () => {
    document.body.innerHTML = `<button data-mcp-tool="a" data-mcp-desc="a">a</button>`
    detach = attachDom(app)
    const btn = document.querySelector('button')!
    btn.disabled = true
    btn.setAttribute('data-mcp-desc', '新描述')
    btn.setAttribute('aria-disabled', 'true')
    btn.disabled = false
    btn.setAttribute('class', 'x')
    await settle()
    const t = app.tools.get('a')!
    expect(t.updates).toEqual([{ description: '新描述', enabled: false }])
    // 无关变化不产生 update
    document.body.appendChild(document.createElement('p')).textContent = 'hello'
    await settle()
    expect(t.updates).toHaveLength(1)
  })

  it('title / risk 移除时显式传 undefined 以恢复默认', async () => {
    document.body.innerHTML = `<button data-mcp-tool="a" data-mcp-desc="a" data-mcp-title="T" data-mcp-risk="payment">a</button>`
    detach = attachDom(app)
    const btn = document.querySelector('button')!
    btn.removeAttribute('data-mcp-title')
    btn.removeAttribute('data-mcp-risk')
    await settle()
    const u = app.tools.get('a')!.updates
    expect(u).toHaveLength(1)
    expect('title' in u[0]! && u[0]!.title === undefined).toBe(true)
    expect('risk' in u[0]! && u[0]!.risk === undefined).toBe(true)
  })

  it('标准注解属性与 data-mcp-output-schema 传给 SDK；缺省时不声明', () => {
    document.body.innerHTML = `
      <button data-mcp-tool="a" data-mcp-desc="a" data-mcp-readonly data-mcp-open-world="false">a</button>
      <button data-mcp-tool="b" data-mcp-desc="b" data-mcp-risk="write" data-mcp-destructive="TRUE" data-mcp-idempotent="true"
        data-mcp-output-schema='{"type":"object","properties":{"id":{"type":"string"}}}'>b</button>
      <button data-mcp-tool="c" data-mcp-desc="c">c</button>`
    detach = attachDom(app)
    expect(app.tools.get('a')!.def.annotations).toEqual({ readOnlyHint: true, openWorldHint: false })
    expect(app.tools.get('a')!.def.outputSchema).toBeUndefined()
    const b = app.tools.get('b')!.def
    expect(b.risk).toBe('write')
    expect(b.annotations).toEqual({ destructiveHint: true, idempotentHint: true })
    expect(b.outputSchema).toEqual({ type: 'object', properties: { id: { type: 'string' } } })
    const c = app.tools.get('c')!.def
    expect('annotations' in c).toBe(false)
    expect('outputSchema' in c).toBe(false)
  })

  it('非法注解取值与非 JSON 对象的 output schema 只警告并忽略', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    document.body.innerHTML = `
      <button data-mcp-tool="a" data-mcp-desc="a" data-mcp-readonly="yes" data-mcp-idempotent data-mcp-output-schema="{oops">a</button>
      <button data-mcp-tool="b" data-mcp-desc="b" data-mcp-output-schema="[1]">b</button>`
    detach = attachDom(app)
    expect(app.tools.get('a')!.def.annotations).toEqual({ idempotentHint: true })
    expect(app.tools.get('a')!.def.outputSchema).toBeUndefined()
    expect(app.tools.get('b')!.def.outputSchema).toBeUndefined()
    const messages = warn.mock.calls.map((c) => String(c[0])).join('\n')
    expect(messages).toContain('data-mcp-readonly="yes"')
    expect(messages).toMatch(/工具 a：data-mcp-output-schema 不是 JSON 对象/)
    expect(messages).toMatch(/工具 b：data-mcp-output-schema 不是 JSON 对象/)
  })

  it('注解与 output schema 变化时只提交变化，移除时显式传 undefined', async () => {
    document.body.innerHTML = `<button data-mcp-tool="a" data-mcp-desc="a" data-mcp-readonly data-mcp-output-schema='{"type":"string"}'>a</button>`
    detach = attachDom(app)
    const t = app.tools.get('a')!
    const btn = document.querySelector('button')!
    document.body.appendChild(document.createElement('p')).textContent = '无关变化'
    btn.setAttribute('data-mcp-readonly', '')
    await settle()
    expect(t.updates).toHaveLength(0)

    btn.setAttribute('data-mcp-readonly', 'false')
    await settle()
    expect(t.updates).toEqual([{ annotations: { readOnlyHint: false } }])

    btn.removeAttribute('data-mcp-readonly')
    btn.removeAttribute('data-mcp-output-schema')
    await settle()
    expect(t.updates).toHaveLength(2)
    const last = t.updates[1]!
    expect('annotations' in last && last.annotations === undefined).toBe(true)
    expect('outputSchema' in last && last.outputSchema === undefined).toBe(true)
    expect(t.current.annotations).toBeUndefined()
  })

  it('同名无 key 的多个元素：使用第一个可用的', async () => {
    document.body.innerHTML = `
      <button data-mcp-tool="save" data-mcp-desc="保存" hidden id="m">移动端</button>
      <button data-mcp-tool="save" data-mcp-desc="保存" id="d">桌面端</button>`
    const clicked: string[] = []
    for (const b of document.querySelectorAll('button')) b.addEventListener('click', () => clicked.push(b.id))
    detach = attachDom(app, { settleMs: 0 })
    expect(app.tools.get('save')!.def.enabled).toBe(true)
    await app.call('save')
    expect(clicked).toEqual(['d'])
  })
})

describe('集合工具', () => {
  const list = (items: Array<[string, string]>) =>
    `<ul>${items
      .map(([k, t]) => `<li><span>${t}</span> <button data-mcp-tool="todos.remove" data-mcp-desc="删除待办" data-mcp-key="${k}">删除</button></li>`)
      .join('')}</ul>`

  it('合并为一个工具，key 为 enum，描述列出标签', async () => {
    document.body.innerHTML = list([
      ['a1', '买菜'],
      ['a2', '写周报'],
    ])
    const clicked: string[] = []
    for (const b of document.querySelectorAll('button')) b.addEventListener('click', () => clicked.push(b.dataset.mcpKey!))
    detach = attachDom(app, { settleMs: 0 })
    expect(app.tools.size).toBe(1)
    const t = app.tools.get('todos.remove')!
    expect(t.def.description).toBe('删除待办（key：a1=买菜, a2=写周报）')
    expect(t.def.input).toEqual({
      type: 'object',
      properties: { key: { type: 'string', enum: ['a1', 'a2'], description: '要操作的条目' } },
      required: ['key'],
      additionalProperties: false,
    })
    await app.call('todos.remove', { key: 'a2' })
    expect(clicked).toEqual(['a2'])
    await expect(app.call('todos.remove', { key: 'zz' })).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
    await expect(app.call('todos.remove', {})).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
  })

  it('列表变化时更新 schema；禁用的 key 从 enum 去掉；全部移除后注销', async () => {
    document.body.innerHTML = list([['a1', '买菜']])
    detach = attachDom(app)
    const ul = document.querySelector('ul')!
    ul.insertAdjacentHTML('beforeend', list([['a2', '写周报']]).replace(/<\/?ul>/g, ''))
    ul.insertAdjacentHTML('beforeend', list([['a3', '健身']]).replace(/<\/?ul>/g, ''))
    await settle()
    const t = app.tools.get('todos.remove')!
    expect(t.updates).toHaveLength(1)
    expect((t.current.input as any).properties.key.enum).toEqual(['a1', 'a2', 'a3'])
    ;(document.querySelector('[data-mcp-key="a1"]') as HTMLButtonElement).disabled = true
    await settle()
    expect((t.current.input as any).properties.key.enum).toEqual(['a2', 'a3'])
    expect(t.current.description).toBe('删除待办（key：a2=写周报, a3=健身）')
    await expect(app.call('todos.remove', { key: 'a1' })).rejects.toMatchObject({ kind: 'TOOL_DISABLED' })
    ul.innerHTML = ''
    await settle()
    expect(app.tools.size).toBe(0)
    expect(t.disposed).toBe(true)
  })

  it('没有可用条目时禁用', async () => {
    document.body.innerHTML = list([
      ['a1', '买菜'],
      ['a2', '写周报'],
    ]).replace(/<button/g, '<button disabled')
    detach = attachDom(app)
    expect(app.tools.get('todos.remove')!.def.enabled).toBe(false)
  })

  it('data-mcp-label 与标签截断', () => {
    const long = '很长'.repeat(40)
    document.body.innerHTML = `<div><button data-mcp-tool="r" data-mcp-desc="删除" data-mcp-key="k1" data-mcp-label="自定义">x</button></div>
      <ul><li>${long}<button data-mcp-tool="r" data-mcp-desc="删除" data-mcp-key="k2">x</button></li></ul>`
    detach = attachDom(app)
    const d = app.tools.get('r')!.def.description
    expect(d).toContain('k1=自定义')
    const label = d.split('k2=')[1]!.replace('）', '')
    expect(label.length).toBe(40)
    expect(label.endsWith('…')).toBe(true)
  })
})

describe('scope', () => {
  it('子树使用 scope，移除时 dispose', async () => {
    document.body.innerHTML = `
      <section data-mcp-scope="cart">
        <button data-mcp-tool="cart.clear" data-mcp-desc="清空">x</button>
        <div data-mcp-scope="inner"><button data-mcp-tool="cart.inner" data-mcp-desc="内">y</button></div>
      </section>
      <button data-mcp-tool="top" data-mcp-desc="顶层">z</button>`
    detach = attachDom(app)
    expect(app.tools.get('cart.clear')!.scope).toBe('cart')
    expect(app.tools.get('cart.inner')!.scope).toBe('cart/inner')
    expect(app.tools.get('top')!.scope).toBeUndefined()
    const cart = app.scopes.find((s) => s.name === 'cart')!
    document.querySelector('section')!.remove()
    await settle()
    expect(cart.disposed).toBe(true)
    expect(app.scopes.find((s) => s.name === 'inner')!.disposed).toBe(true)
    expect(app.tools.has('cart.clear')).toBe(false)
    expect(app.tools.has('cart.inner')).toBe(false)
    expect(app.tools.has('top')).toBe(true)
  })

  it('动态新增的 scope 子树', async () => {
    detach = attachDom(app)
    document.body.innerHTML = `<div data-mcp-scope="page"><button data-mcp-tool="p.go" data-mcp-desc="去">x</button></div>`
    await settle()
    expect(app.tools.get('p.go')!.scope).toBe('page')
  })
})

describe('detach', () => {
  it('注销全部并停止观察', async () => {
    document.body.innerHTML = `<div data-mcp-scope="s"><button data-mcp-tool="a" data-mcp-desc="a">a</button></div>
      <button data-mcp-tool="b" data-mcp-desc="b">b</button><div data-mcp-resource="r" data-mcp-desc="r">t</div>`
    const d = attachDom(app)
    expect(app.tools.size).toBe(2)
    expect(app.resources.size).toBe(2)
    d()
    d()
    expect(app.tools.size).toBe(0)
    expect(app.resources.size).toBe(0)
    expect(app.scopes.every((s) => s.disposed)).toBe(true)
    document.body.insertAdjacentHTML('beforeend', `<button data-mcp-tool="c" data-mcp-desc="c">c</button>`)
    await settle()
    expect(app.tools.size).toBe(0)
  })

  it('支持在 Scope 上挂载', () => {
    document.body.innerHTML = `<button data-mcp-tool="a" data-mcp-desc="a">a</button>`
    const scope = app.scope('dom')
    detach = attachDom(scope, { snapshot: false })
    expect(app.tools.get('a')!.scope).toBe('dom')
  })
})

describe('toErrorKind', () => {
  it('识别全部协议错误类别（含 Host 侧的 RATE_LIMITED / PAYLOAD_TOO_LARGE），未知值归为 HANDLER_ERROR', () => {
    expect(toErrorKind('RATE_LIMITED')).toBe('RATE_LIMITED')
    expect(toErrorKind('PAYLOAD_TOO_LARGE')).toBe('PAYLOAD_TOO_LARGE')
    expect(toErrorKind('NOPE')).toBe('HANDLER_ERROR')
  })
})
