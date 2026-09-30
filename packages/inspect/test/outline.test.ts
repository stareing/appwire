import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ToolCallError } from '@app-mcp/web'
import { attachInspect } from '../src/index'
import { FakeRegistrar } from './fake'

const PAGE = `
<nav aria-label="主菜单"><a href="/">首页</a><a href="/orders" aria-current="page">订单</a></nav>
<main>
  <h1>商城</h1>
  <h2>购物车</h2>
  <p>共 3 件商品，合计 ¥120</p>
  <button data-mcp-tool="cart.clear">清空</button>
  <button id="checkout" disabled>结算</button>
  <div style="display:none"><button>隐藏1</button></div>
  <button style="visibility:hidden">隐藏2</button>
  <button hidden>隐藏3</button>
  <div inert><button>隐藏4</button></div>
  <div aria-hidden="true"><button>隐藏5</button></div>
  <dialog><button>隐藏6</button></dialog>
  <input type="hidden" name="token" value="x">
  <h2>收货信息</h2>
  <form aria-label="收货" data-mcp-tool="order.submit">
    <label for="addr">收货地址</label><input id="addr" required value="北京市朝阳区某某街道一二三号院四号楼五单元六零一室很长很长的收货地址">
    <label><input type="checkbox" name="agree"> 同意条款</label>
    <select aria-label="城市"><option value="bj">北京</option><option value="sh">上海</option></select>
    <textarea placeholder="备注"></textarea>
    <input type="password" aria-label="密码" value="secret">
    <button type="submit">提交</button>
  </form>
  <div style="cursor:pointer"><span>可点击卡片</span></div>
  <details><summary>更多</summary><button>内部</button></details>
  <div role="status">已加入购物车</div>
  <div role="status"></div>
</main>`

let reg: FakeRegistrar
let detach: () => void

beforeEach(() => {
  document.body.innerHTML = PAGE
  reg = new FakeRegistrar()
  detach = attachInspect(reg)
})

afterEach(() => {
  detach()
  vi.restoreAllMocks()
})

async function outline(input: Record<string, unknown> = {}) {
  return reg.call('ui.outline', input)
}

describe('ui.outline', () => {
  it('注册工具与风险等级；默认不注册 eval', () => {
    expect([...reg.tools.keys()].sort()).toEqual(
      ['ui.click', 'ui.fill', 'ui.outline', 'ui.press', 'ui.read', 'ui.scroll', 'ui.submit'].sort(),
    )
    expect(reg.getTool('ui.outline').def.risk).toBe('read')
    expect(reg.getTool('ui.read').def.risk).toBe('read')
    for (const n of ['click', 'fill', 'press', 'scroll', 'submit']) expect(reg.getTool(`ui.${n}`).def.risk).toBe('write')
  })

  it('每个元素一行，含名称、值、链接、状态、必填', async () => {
    const r = await outline()
    const lines: string[] = r.text.split('\n')
    expect(lines).toContain('  e3 链接「订单」→ /orders current')
    expect(lines).toContain('  e6 按钮「结算」 disabled')
    const addr = (document.getElementById('addr') as HTMLInputElement).value
    expect(r.text).toContain(`输入框「收货地址」= "${addr.slice(0, 29)}…" (必填)`)
    expect(r.text).toMatch(/e\d+ 复选框「同意条款」 unchecked/)
    expect(r.text).toMatch(/e\d+ 下拉框「城市」= "北京"/)
    expect(r.text).toMatch(/e\d+ 多行输入框「备注」\n/)
    // 密码不泄露
    expect(r.text).toMatch(/e\d+ 密码框「密码」= "••••"/)
    expect(r.text).not.toContain('secret')
    expect(r.text).toMatch(/e\d+ 可点击元素「可点击卡片」/)
    expect(r.text).toMatch(/e\d+ 展开项「更多」 collapsed/)
    expect(r.text).toMatch(/e\d+ 提示「已加入购物车」/)
    // 非交互文本不出现
    expect(r.text).not.toContain('合计')
    expect(r.items.find((i: any) => i.name === '结算')).toEqual({
      ref: 'e6',
      role: 'button',
      name: '结算',
      states: ['disabled'],
      group: '主区域 › 商城 › 购物车',
    })
    expect(r.total).toBe(r.items.length)
    expect(r.remaining).toBeUndefined()
  })

  it('跳过不可见元素：display:none、visibility:hidden、hidden、inert、aria-hidden、未打开的 dialog、未展开的 details', async () => {
    const r = await outline()
    for (let i = 1; i <= 6; i++) expect(r.text).not.toContain(`隐藏${i}`)
    expect(r.text).not.toContain('内部')
    expect(r.text).not.toContain('token')
    document.querySelector('details')!.setAttribute('open', '')
    const r2 = await outline()
    expect(r2.text).toMatch(/e\d+ 按钮「内部」/)
    expect(r2.text).toMatch(/展开项「更多」 expanded/)
  })

  it('有布局信息时跳过零尺寸元素', async () => {
    document.body.innerHTML = '<button>正常</button><button data-zero>零尺寸</button>'
    vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
      const size = this.hasAttribute('data-zero') ? 0 : 20
      return { x: 0, y: 0, left: 0, top: 0, width: size, height: size, right: size, bottom: size, toJSON() {} } as DOMRect
    })
    const r = await outline()
    expect(r.text).toContain('正常')
    expect(r.text).not.toContain('零尺寸')
  })

  it('按标题与 landmark 分组并缩进；已声明元素标注并给出提示', async () => {
    const r = await outline()
    const lines: string[] = r.text.split('\n')
    expect(lines.slice(0, 7)).toEqual([
      '» e1 导航「主菜单」',
      '  e2 链接「首页」→ /',
      '  e3 链接「订单」→ /orders current',
      '» e4 主区域',
      '  # 商城',
      '  ## 购物车',
      '  e5 按钮「清空」 [已声明：cart.clear]',
    ])
    expect(lines).toContain('  ## 收货信息')
    expect(lines).toContain('  » e7 表单「收货」 [已声明：order.submit]')
    expect(lines.some((l) => /^ {4}e\d+ 按钮「提交」$/.test(l))).toBe(true)
    expect(r.hint).toContain('已声明')
    const addr = r.items.find((i: any) => i.name === '收货地址')
    expect(addr.group).toBe('主区域 › 商城 › 收货信息 › 表单「收货」')
    expect(addr.required).toBe(true)
  })

  it('query 模糊过滤（名称与分组），只保留相关分组标题', async () => {
    const r = await outline({ query: '订单' })
    expect(r.text).toBe('» e1 导航「主菜单」\n  e3 链接「订单」→ /orders current')
    const g = await outline({ query: '收货' })
    // 分组「收货信息」下的元素全部匹配
    expect(g.items.map((i: any) => i.name)).toEqual([
      '收货地址', '同意条款', '城市', '备注', '密码', '提交', '可点击卡片', '更多', '已加入购物车',
    ])
    const multi = await outline({ query: '收货 城市' })
    expect(multi.items.map((i: any) => i.name)).toEqual(['城市'])
    expect(multi.text).toBe('» e4 主区域\n  # 商城\n  ## 收货信息\n  » e7 表单「收货」 [已声明：order.submit]\n    e10 下拉框「城市」= "北京"')
    const none = await outline({ query: '不存在的东西' })
    expect(none.items).toEqual([])
    expect(none.text).toContain('没有与「不存在的东西」匹配')
  })

  it('within 限定子树', async () => {
    await outline()
    const r = await outline({ within: 'e7' })
    expect(r.text.split('\n')[0]).toBe('» e7 表单「收货」 [已声明：order.submit]')
    expect(r.items).toHaveLength(6)
    expect(r.text).not.toContain('清空')
  })

  it('超过 limit 时截断并说明剩余数量', async () => {
    const r = await outline({ limit: 3 })
    expect(r.items).toHaveLength(3)
    expect(r.remaining).toBe(r.total - 3)
    expect(r.text.split('\n').at(-1)).toBe(`…另有 ${r.total - 3} 个元素未列出，可用 query 或 within 缩小范围`)
    // maxItems 选项
    detach()
    const reg2 = new FakeRegistrar()
    detach = attachInspect(reg2, { maxItems: 2, prefix: 'page' })
    const r2 = await reg2.call('page.outline')
    expect(r2.items).toHaveLength(2)
  })

  it('引用在多次调用间稳定；新元素分配新引用；移除后失效', async () => {
    const a = await outline()
    const b = await outline({ query: '结算' })
    expect(b.items[0].ref).toBe(a.items.find((i: any) => i.name === '结算').ref)
    const btn = document.createElement('button')
    btn.textContent = '新按钮'
    document.querySelector('main')!.prepend(btn)
    const c = await outline()
    const fresh = c.items.find((i: any) => i.name === '新按钮')
    expect(Number(fresh.ref.slice(1))).toBeGreaterThan(15)
    expect(c.items.find((i: any) => i.name === '结算').ref).toBe('e6')
    document.getElementById('checkout')!.remove()
    const err = await reg.call('ui.click', { ref: 'e6' }).catch((e) => e)
    expect(err).toBeInstanceOf(ToolCallError)
    expect(err.kind).toBe('INVALID_INPUT')
    expect(err.message).toBe('引用 e6 已失效，请重新调用 ui.outline')
    const bad = await reg.call('ui.read', { ref: 'x1' }).catch((e) => e)
    expect(bad.kind).toBe('INVALID_INPUT')
    const unknown = await outline({ within: 'e999' }).catch((e) => e)
    expect(unknown.message).toContain('已失效')
  })

  it('名称来源：aria-labelledby、title、placeholder、图片 alt，并截断到 40 字', async () => {
    document.body.innerHTML = `
      <span id="lb">外部标签</span><input aria-labelledby="lb">
      <button title="设置"></button>
      <input placeholder="搜索商品">
      <a href="https://example.com/x"><img alt="官网"></a>
      <button>${'很'.repeat(60)}</button>`
    const r = await outline()
    expect(r.items.map((i: any) => i.name)).toEqual(['外部标签', '设置', '搜索商品', '官网', `${'很'.repeat(39)}…`])
    expect(r.items[3].href).toBe('https://example.com/x')
  })
})
