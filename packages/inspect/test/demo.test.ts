import { expect, it } from 'vitest'
import { attachInspect } from '../src/index'
import { FakeRegistrar } from './fake'

/** 接近真实的商城页面：大量布局节点、图片、价格文本，少量可交互元素。 */
function shopPage(): string {
  const products = Array.from({ length: 8 }, (_, i) => `
    <div class="card"><div class="card-inner"><div class="thumb"><img src="/p${i}.jpg" alt=""></div>
      <div class="meta"><div class="title"><span>商品 ${i + 1} 号 · 限时特惠</span></div>
      <div class="price"><span class="currency">¥</span><span class="amount">${99 + i}</span><s>¥${199 + i}</s></div>
      <div class="tags"><span class="tag">包邮</span><span class="tag">七天无理由</span></div></div>
      <div class="actions"><button class="btn btn-primary" aria-label="加入购物车：商品 ${i + 1}"><svg viewBox="0 0 24 24"><path d="M0 0h24v24H0z"/></svg></button></div>
    </div></div>`).join('')
  return `
  <header class="site-header"><div class="container"><div class="logo"><a href="/"><img alt="示例商城"></a></div>
    <nav aria-label="主菜单"><ul><li><a href="/">首页</a></li><li><a href="/cats">分类</a></li><li><a href="/orders">我的订单</a></li></ul></nav>
    <div class="search"><form role="search" aria-label="搜索"><input type="search" placeholder="搜索商品"><button>搜索</button></form></div></div></header>
  <main><div class="container"><div class="layout"><aside class="filters"><h3>筛选</h3>
    <label><input type="checkbox"> 仅看有货</label><label>排序 <select><option>综合</option><option>价格</option></select></label></aside>
    <section class="grid"><h2>热门商品</h2>${products}</section>
    <section class="cart"><h2>购物车</h2><div class="cart-summary"><p>共 3 件，合计 <b>¥297</b></p>
      <button data-mcp-tool="cart.clear">清空</button><button disabled>结算</button></div></section>
  </div></div></main>
  <footer><div class="container"><p>© 2026 示例商城 · 客服电话 400-000-0000</p><a href="/about">关于我们</a></div></footer>`
}

it('大纲远短于页面 HTML', async () => {
  document.body.innerHTML = shopPage()
  const reg = new FakeRegistrar()
  attachInspect(reg)
  const r = await reg.call('ui.outline')
  const html = document.body.innerHTML.length
  if (process.env.INSPECT_DEMO) process.stdout.write(`\n${r.text}\n\n大纲 ${r.text.length} 字符 / HTML ${html} 字符\n`)
  expect(r.items.length).toBe(19)
  expect(r.text.length * 8).toBeLessThan(html)
})
