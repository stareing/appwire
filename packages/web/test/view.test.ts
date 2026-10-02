/**
 * 界面级暴露（spec/protocol.md 3.4，第 4c 项 D）：view 工具的可见性门控、层栈、scope 继承与 surface / page 传递。
 * 真实 DOM（happy-dom）+ 假核心：断言核心收到的注册 / 更新。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { CoreToolDef } from '../src/core'
import { getToolHub } from '../src/tool-hub'
import { createViewLayer, openViewLayers, refreshViewTools } from '../src/view'
import { connected, settle } from './fakes'

const handler = (): null => null

function registered(h: Awaited<ReturnType<typeof connected>>, name: string): CoreToolDef | undefined {
  return h.core.callsOf('registerTool').map((c) => c[0] as CoreToolDef).find((d) => d.name === name)
}

/** 核心收到的启用状态更新（按顺序）。 */
function enabledUpdates(h: Awaited<ReturnType<typeof connected>>): boolean[] {
  return h.core.callsOf('updateTool').map((c) => (c[1] as { enabled?: boolean }).enabled).filter((e) => e !== undefined) as boolean[]
}

function setDocumentVisible(visible: boolean): void {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (visible ? 'visible' : 'hidden') })
  document.dispatchEvent(new Event('visibilitychange'))
}

beforeEach(() => {
  document.body.innerHTML = ''
  sessionStorage.clear()
  localStorage.clear()
})

afterEach(() => {
  for (const layer of [...openViewLayers()]) layer.close()
  setDocumentVisible(true)
  vi.restoreAllMocks()
})

describe('surface / page 声明', () => {
  it('view 工具带 surface 与 page；app 工具（缺省）不带 surface', async () => {
    const h = await connected()
    h.app.tool('a', { description: 'A', handler })
    h.app.tool('v', { description: 'V', surface: 'view', page: 'cart', handler })
    h.app.tool('p', { description: 'P', surface: 'app', page: 'orders', handler })
    expect(registered(h, 'a')).not.toHaveProperty('surface')
    expect(registered(h, 'a')).not.toHaveProperty('page')
    expect(registered(h, 'v')).toMatchObject({ surface: 'view', page: 'cart' })
    expect(registered(h, 'v')).not.toHaveProperty('enabled')
    expect(registered(h, 'p')).toMatchObject({ page: 'orders' })
    expect(registered(h, 'p')).not.toHaveProperty('surface')
    h.app.dispose()
  })

  it('非法页面名在注册时抛错', async () => {
    const h = await connected()
    expect(() => h.app.tool('x', { description: 'X', page: 'bad page', handler })).toThrow(/页面名/)
    expect(() => h.app.scope('s', { page: '' })).toThrow(/页面名/)
    h.app.dispose()
  })

  it('update 修改与清除 page / surface', async () => {
    const h = await connected()
    const t = h.app.tool('v', { description: 'V', page: 'cart', handler })
    t.update({ page: undefined })
    t.update({ surface: 'view', page: 'orders' })
    const updates = h.core.callsOf('updateTool').map((c) => c[1])
    expect(updates).toContainEqual({ page: null })
    expect(updates).toContainEqual({ surface: 'view', page: 'orders' })
    h.app.dispose()
  })

  it('backgroundTool：注册时声明（缺省不带）；update 修改与清除；不继承 scope', async () => {
    const h = await connected()
    h.app.tool('cart.add', { description: '加入', handler })
    const t = h.app.tool('cart.viewAdd', { description: 'V', surface: 'view', page: 'cart', backgroundTool: 'cart.add', handler })
    h.app.scope('s', { page: 'cart' }).tool('cart.inner', { description: 'I', handler })
    expect(registered(h, 'cart.add')).not.toHaveProperty('backgroundTool')
    expect(registered(h, 'cart.viewAdd')).toMatchObject({ surface: 'view', page: 'cart', backgroundTool: 'cart.add' })
    expect(registered(h, 'cart.inner')).not.toHaveProperty('backgroundTool')
    t.update({ backgroundTool: 'cart.add2' })
    t.update({ backgroundTool: undefined })
    t.update({ description: 'V2' })
    const updates = h.core.callsOf('updateTool').map((c) => c[1])
    expect(updates).toContainEqual({ backgroundTool: 'cart.add2' })
    expect(updates).toContainEqual({ backgroundTool: null })
    expect(updates).toContainEqual({ description: 'V2' })
    h.app.dispose()
  })

  it('scope 的界面声明被其下工具（含子 scope）继承，工具自身声明优先', async () => {
    const h = await connected()
    const page = h.app.scope('cart', { page: 'cart', surface: 'view', visibility: 'always' })
    page.tool('cart.add', { description: '加入', handler })
    page.scope('inner').tool('cart.remove', { description: '移除', surface: 'app', handler })
    page.tool('cart.other', { description: '其他', page: 'other', handler })
    expect(registered(h, 'cart.add')).toMatchObject({ surface: 'view', page: 'cart' })
    expect(registered(h, 'cart.remove')).toMatchObject({ page: 'cart' })
    expect(registered(h, 'cart.remove')).not.toHaveProperty('surface')
    expect(registered(h, 'cart.other')).toMatchObject({ page: 'other' })
    h.app.dispose()
  })
})

describe('可见性门控', () => {
  it('锚点隐藏时注册为禁用，显示后启用；display:none / inert / hidden 都视为不可见', async () => {
    const h = await connected()
    const el = document.createElement('div')
    el.style.display = 'none'
    document.body.append(el)
    h.app.tool('v', { description: 'V', surface: 'view', anchor: el, handler })
    expect(registered(h, 'v')).toMatchObject({ enabled: false })

    el.style.display = ''
    refreshViewTools()
    expect(enabledUpdates(h)).toEqual([true])

    el.setAttribute('inert', '')
    await settle()
    expect(enabledUpdates(h)).toEqual([true, false])
    el.removeAttribute('inert')
    await settle()
    expect(enabledUpdates(h)).toEqual([true, false, true])

    const wrapper = document.createElement('section')
    document.body.append(wrapper)
    wrapper.append(el)
    wrapper.hidden = true
    await settle()
    expect(enabledUpdates(h)).toEqual([true, false, true, false])
    h.app.dispose()
  })

  it('锚点函数返回 null（未挂载）时不启用', async () => {
    const h = await connected()
    let el: Element | null = null
    h.app.tool('v', { description: 'V', surface: 'view', anchor: () => el, handler })
    expect(registered(h, 'v')).toMatchObject({ enabled: false })
    el = document.body.appendChild(document.createElement('div'))
    refreshViewTools()
    expect(enabledUpdates(h)).toEqual([true])
    h.app.dispose()
  })

  it('页面隐藏时 view 工具暂停，app 工具不受影响；visibility: always 关闭门控', async () => {
    const h = await connected()
    h.app.tool('v', { description: 'V', surface: 'view', handler })
    h.app.tool('always', { description: 'A', surface: 'view', visibility: 'always', handler })
    h.app.tool('a', { description: 'App', handler })
    h.core.calls = []
    setDocumentVisible(false)
    await settle()
    expect(h.core.callsOf('updateTool')).toEqual([[1, { enabled: false }]])
    setDocumentVisible(true)
    await settle()
    expect(h.core.callsOf('updateTool')).toEqual([
      [1, { enabled: false }],
      [1, { enabled: true }],
    ])
    h.app.dispose()
  })

  it('enabled: false 优先于门控；恢复启用时按当前可见性', async () => {
    const h = await connected()
    const t = h.app.tool('v', { description: 'V', surface: 'view', enabled: false, handler })
    expect(registered(h, 'v')).toMatchObject({ enabled: false })
    setDocumentVisible(false)
    await settle()
    expect(enabledUpdates(h)).toEqual([])
    t.update({ enabled: true })
    expect(enabledUpdates(h)).toEqual([])
    setDocumentVisible(true)
    await settle()
    expect(enabledUpdates(h)).toEqual([true])
    h.app.dispose()
  })

  it('改为 app 工具后不再门控', async () => {
    const h = await connected()
    const el = document.body.appendChild(document.createElement('div'))
    el.hidden = true
    const t = h.app.tool('v', { description: 'V', surface: 'view', anchor: el, handler })
    t.update({ surface: 'app' })
    expect(h.core.callsOf('updateTool').map((c) => c[1])).toContainEqual({ enabled: true })
    h.app.dispose()
  })

  it('WebMCP 等本地入口看到的定义反映门控结果', async () => {
    const h = await connected()
    h.app.tool('v', { description: 'V', surface: 'view', handler })
    const hub = getToolHub(h.app)!
    expect(hub.list()[0]?.definition.enabled).not.toBe(false)
    setDocumentVisible(false)
    await settle()
    expect(hub.list()[0]?.definition.enabled).toBe(false)
    h.app.dispose()
  })
})

describe('层栈', () => {
  it('打开层：下层 view 工具暂停、层内工具启用；关闭后恢复；app 工具不受影响', async () => {
    const h = await connected()
    const dialog = createViewLayer('dialog')
    h.app.tool('page.v', { description: '页面', surface: 'view', handler })
    h.app.tool('page.app', { description: '后台', handler })
    const layerScope = h.app.scope('dialog', { layer: dialog, surface: 'view' })
    layerScope.tool('dialog.ok', { description: '确认', handler })
    expect(registered(h, 'dialog.ok')).toMatchObject({ enabled: false })
    h.core.calls = []

    dialog.open()
    expect(dialog.isTop).toBe(true)
    // 句柄：page.v = 1、page.app = 2、scope = 3、dialog.ok = 4
    expect(h.core.callsOf('updateTool')).toEqual([
      [1, { enabled: false }],
      [4, { enabled: true }],
    ])

    h.core.calls = []
    dialog.close()
    expect(h.core.callsOf('updateTool')).toEqual([
      [1, { enabled: true }],
      [4, { enabled: false }],
    ])
    h.app.dispose()
  })

  it('层内工具不继承层外的 page（层不是导航目标）', async () => {
    const h = await connected()
    const layer = createViewLayer('dialog')
    const page = h.app.scope('products', { page: 'products', surface: 'view' })
    page.scope('dialog', { layer }).tool('dialog.ok', { description: '确认', handler })
    page.scope('dialog2', { layer, page: 'products' }).tool('dialog.explicit', { description: '显式', handler })
    expect(registered(h, 'dialog.ok')).toMatchObject({ surface: 'view' })
    expect(registered(h, 'dialog.ok')).not.toHaveProperty('page')
    expect(registered(h, 'dialog.explicit')).toMatchObject({ page: 'products' })
    h.app.dispose()
  })

  it('嵌套层：只有最上层启用', async () => {
    const h = await connected()
    const a = createViewLayer('a')
    const b = createViewLayer('b')
    a.open()
    h.app.scope('a', { layer: a, surface: 'view' }).tool('a.t', { description: 'A', handler })
    h.app.scope('b', { layer: b, surface: 'view' }).tool('b.t', { description: 'B', handler })
    expect(registered(h, 'a.t')).not.toHaveProperty('enabled')
    expect(registered(h, 'b.t')).toMatchObject({ enabled: false })
    b.open()
    expect(a.isTop).toBe(false)
    expect(enabledUpdates(h)).toEqual([false, true])
    b.close()
    a.close()
    h.app.dispose()
  })

  it('打开模态 <dialog> 之外的锚点不可见（浏览器把其余文档设为惰性）', async () => {
    const h = await connected()
    const page = document.body.appendChild(document.createElement('main'))
    const dialog = document.body.appendChild(document.createElement('dialog'))
    dialog.setAttribute('open', '')
    vi.spyOn(dialog, 'matches').mockImplementation((sel: string) => sel === ':modal')
    h.app.tool('page.v', { description: '页面', surface: 'view', anchor: page, handler })
    h.app.tool('dialog.v', { description: '对话框', surface: 'view', anchor: dialog, handler })
    expect(registered(h, 'page.v')).toMatchObject({ enabled: false })
    expect(registered(h, 'dialog.v')).not.toHaveProperty('enabled')
    h.app.dispose()
  })

  it('注销后门控不再更新核心', async () => {
    const h = await connected()
    const layer = createViewLayer('l')
    const t = h.app.tool('v', { description: 'V', surface: 'view', handler })
    t.dispose()
    h.core.calls = []
    layer.open()
    expect(h.core.callsOf('updateTool')).toEqual([])
    layer.close()
    h.app.dispose()
  })
})
