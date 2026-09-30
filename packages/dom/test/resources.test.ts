import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { attachDom } from '../src'
import { createFakeAppMcp, settle, type FakeAppMcp } from './fake'

let app: FakeAppMcp
let detach: (() => void) | undefined

beforeEach(() => {
  document.body.innerHTML = ''
  document.title = '购物车'
  app = createFakeAppMcp()
})
afterEach(() => {
  detach?.()
  detach = undefined
  vi.restoreAllMocks()
})

describe('资源', () => {
  it('精简可见文本', async () => {
    document.body.innerHTML = `<div data-mcp-resource="cart.state" data-mcp-desc="购物车状态">
      <h2>购物车</h2>
      <ul><li>苹果 × <b>2</b></li><li>香蕉 × 1</li></ul>
      <script>var x = 1</script><p hidden>隐藏</p><p style="display:none">也隐藏</p>
    </div>`
    detach = attachDom(app)
    const r = app.resources.get('cart.state')!
    expect(r.def.description).toBe('购物车状态')
    expect(r.def.mimeType).toBe('text/plain')
    expect(await app.read('cart.state')).toBe('购物车 苹果 × 2 香蕉 × 1')
  })

  it('截断到 4000 字', async () => {
    document.body.innerHTML = `<div data-mcp-resource="big" data-mcp-desc="大">${'字'.repeat(5000)}</div>`
    detach = attachDom(app)
    const text = (await app.read('big')) as string
    expect(text.length).toBe(4000)
    expect(text.endsWith('…')).toBe(true)
  })

  it('data-mcp-json', async () => {
    document.body.innerHTML = `<div data-mcp-resource="todos.list" data-mcp-desc="待办" data-mcp-json='[{"id":"a1","title":"买菜"}]'>x</div>`
    detach = attachDom(app)
    expect(app.resources.get('todos.list')!.def.mimeType).toBe('application/json')
    expect(await app.read('todos.list')).toEqual([{ id: 'a1', title: '买菜' }])
    document.querySelector('div')!.setAttribute('data-mcp-json', '{bad')
    await expect(app.read('todos.list')).rejects.toMatchObject({ kind: 'HANDLER_ERROR' })
  })

  it('子树或属性变化时合并通知', async () => {
    document.body.innerHTML = `<div data-mcp-resource="t" data-mcp-desc="t"><span>1</span></div>
      <div data-mcp-resource="j" data-mcp-desc="j" data-mcp-json="1"></div>`
    detach = attachDom(app)
    const span = document.querySelector('span')!
    span.textContent = '2'
    span.firstChild!.nodeValue = '3'
    span.append('4')
    await settle()
    expect(app.resources.get('t')!.notifications).toBe(1)
    expect(app.resources.get('j')!.notifications).toBe(0)
    document.querySelector('[data-mcp-resource=j]')!.setAttribute('data-mcp-json', '2')
    await settle()
    expect(app.resources.get('j')!.notifications).toBe(1)
    expect(app.resources.get('t')!.notifications).toBe(1)
  })

  it('元素被替换时沿用登记', async () => {
    document.body.innerHTML = `<div id="w"><div data-mcp-resource="t" data-mcp-desc="t">旧</div></div>`
    detach = attachDom(app)
    const r = app.resources.get('t')!
    document.getElementById('w')!.innerHTML = `<div data-mcp-resource="t" data-mcp-desc="t">新</div>`
    await settle()
    expect(app.resources.get('t')).toBe(r)
    expect(r.notifications).toBe(1)
    expect(await app.read('t')).toBe('新')
  })

  it('移除时注销', async () => {
    document.body.innerHTML = `<div data-mcp-resource="t" data-mcp-desc="t">x</div>`
    detach = attachDom(app)
    document.querySelector('div')!.remove()
    await settle()
    expect(app.resources.has('t')).toBe(false)
  })
})

describe('ui.snapshot', () => {
  it('只列出声明的工具与资源', async () => {
    history.replaceState(null, '', '/#cart')
    document.body.innerHTML = `
      <nav><a href="#a">很多无关节点</a><div><div><span>布局</span></div></div></nav>
      <button data-mcp-tool="cart.clear" data-mcp-desc="清空购物车">清空</button>
      <ul>
        <li>买菜 <button data-mcp-tool="todos.remove" data-mcp-desc="删除待办" data-mcp-key="a1">删除</button></li>
        <li>写周报 <button data-mcp-tool="todos.remove" data-mcp-desc="删除待办" data-mcp-key="a2">删除</button></li>
      </ul>
      <button data-mcp-tool="cart.checkout" data-mcp-desc="结算" disabled>结算</button>
      <form data-mcp-tool="todos.add" data-mcp-desc="新建待办"><input name="title" required><input name="pinned" type="checkbox"></form>
      <div data-mcp-resource="cart.state" data-mcp-desc="购物车">x</div>
      <div data-mcp-resource="todos.list" data-mcp-desc="待办" data-mcp-json="[]"></div>`
    detach = attachDom(app)
    const snap = await app.read('ui.snapshot')
    expect(snap).toBe(
      [
        `[页面] 购物车 · ${location.href}`,
        '工具：',
        '- cart.clear  清空购物车  可用',
        '- todos.remove  删除待办（key：a1=买菜, a2=写周报）  可用',
        '- cart.checkout  结算  不可用：按钮已禁用',
        '- todos.add  新建待办（参数：title*, pinned）  可用',
        '资源：cart.state、todos.list',
      ].join('\n'),
    )
    expect(location.href.endsWith('/#cart')).toBe(true)
    expect(app.resources.get('ui.snapshot')!.def.mimeType).toBe('text/plain')
  })

  it('状态变化时通知快照变化', async () => {
    document.body.innerHTML = `<button data-mcp-tool="a" data-mcp-desc="a">a</button>`
    detach = attachDom(app)
    const r = app.resources.get('ui.snapshot')!
    document.querySelector('button')!.disabled = true
    await settle()
    expect(r.notifications).toBe(1)
    expect(await app.read('ui.snapshot')).toContain('- a  a  不可用：按钮已禁用')
  })

  it('可关闭与改名', () => {
    detach = attachDom(app, { snapshot: false })
    expect(app.resources.size).toBe(0)
    detach()
    detach = attachDom(app, { snapshot: { name: 'page.snapshot' } })
    expect(app.resources.has('page.snapshot')).toBe(true)
    expect(app.resources.has('ui.snapshot')).toBe(false)
  })

  it('没有声明时', async () => {
    detach = attachDom(app)
    const snap = (await app.read('ui.snapshot')) as string
    expect(snap.split('\n').slice(1)).toEqual(['工具：', '- （无）', '资源：（无）'])
  })
})

describe('root 选项', () => {
  it('只观察 root 子树', () => {
    document.body.innerHTML = `<div id="app"><button data-mcp-tool="in" data-mcp-desc="in">a</button></div>
      <button data-mcp-tool="out" data-mcp-desc="out">b</button>`
    detach = attachDom(app, { root: document.getElementById('app')! })
    expect([...app.tools.keys()]).toEqual(['in'])
  })
})
