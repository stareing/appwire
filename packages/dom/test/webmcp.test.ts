import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { attachDom, type AgentSubmitEvent } from '../src'
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

describe('W3C WebMCP 声明式表单', () => {
  it('toolname / tooldescription / toolparamdescription', () => {
    document.body.innerHTML = `<form toolname="search" tooldescription="搜索商品" toolautosubmit>
      <label for="q">关键词</label><input id="q" name="q" toolparamdescription="搜索关键词" required>
    </form>`
    detach = attachDom(app)
    const t = app.tools.get('search')!
    expect(t.def.description).toBe('搜索商品')
    expect((t.def.input as any).properties.q).toEqual({ type: 'string', description: '搜索关键词' })
  })

  it('标准属性优先于 data-mcp-*', () => {
    document.body.innerHTML = `<form toolname="std" tooldescription="标准描述" data-mcp-tool="ours" data-mcp-desc="我们的描述">
      <input name="a" toolparamdescription="标准参数" data-mcp-desc="我们的参数"></form>`
    detach = attachDom(app)
    expect(app.tools.has('ours')).toBe(false)
    const t = app.tools.get('std')!
    expect(t.def.description).toBe('标准描述')
    expect((t.def.input as any).properties.a.description).toBe('标准参数')
  })

  it('toolautosubmit：自动提交，submit 事件带 agentInvoked 与 respondWith', async () => {
    document.body.innerHTML = `<form toolname="book" tooldescription="订票" toolautosubmit action="/never"><input name="n"></form>`
    const form = document.querySelector('form')!
    let agent: boolean | undefined
    form.addEventListener('submit', (e) => {
      const ev = e as AgentSubmitEvent
      agent = ev.agentInvoked
      const n = new FormData(form).get('n')
      ev.respondWith(Promise.resolve({ booked: n }))
    })
    detach = attachDom(app)
    await expect(app.call('book', { n: '2' })).resolves.toEqual({ data: { booked: '2' } })
    expect(agent).toBe(true)
  })

  it('respondWith 的 promise 失败 → HANDLER_ERROR / 指定类别', async () => {
    document.body.innerHTML = `<form toolname="x" tooldescription="x" toolautosubmit></form>`
    const form = document.querySelector('form')!
    let n = 0
    form.addEventListener('submit', (e) => {
      n++
      ;(e as AgentSubmitEvent).respondWith(
        n === 1 ? Promise.reject(new Error('坏了')) : Promise.reject({ kind: 'USER_REJECTED', message: '不要' }),
      )
    })
    detach = attachDom(app)
    await expect(app.call('x')).rejects.toMatchObject({ kind: 'HANDLER_ERROR', message: '坏了' })
    await expect(app.call('x')).rejects.toMatchObject({ kind: 'USER_REJECTED' })
  })

  it('没有 toolautosubmit：只填值，等待用户提交', async () => {
    document.body.innerHTML = `<form toolname="msg" tooldescription="发消息"><input name="text"><button>发送</button></form>`
    const form = document.querySelector('form')!
    const onSubmit = vi.fn((e: Event) => {
      ;(e as AgentSubmitEvent).respondWith({ sent: true, agent: (e as AgentSubmitEvent).agentInvoked })
    })
    form.addEventListener('submit', onSubmit)
    const activated = vi.fn()
    form.addEventListener('toolactivated', activated)
    detach = attachDom(app)
    const p = app.call('msg', { text: '你好' })
    await settle(30)
    expect(onSubmit).not.toHaveBeenCalled()
    expect(form.querySelector('input')!.value).toBe('你好')
    expect(form.hasAttribute('data-mcp-active')).toBe(true)
    expect(activated).toHaveBeenCalledTimes(1)
    // 用户点击提交
    form.querySelector('button')!.click()
    await expect(p).resolves.toEqual({ data: { sent: true, agent: true } })
    expect(form.hasAttribute('data-mcp-active')).toBe(false)
  })

  it('等待用户提交超时 → TIMEOUT 并派发 toolcancel；重置 → USER_REJECTED', async () => {
    document.body.innerHTML = `<form toolname="msg" tooldescription="发消息" data-mcp-timeout="30"><input name="text"></form>`
    const form = document.querySelector('form')!
    const cancel = vi.fn()
    form.addEventListener('toolcancel', cancel)
    detach = attachDom(app)
    await expect(app.call('msg', { text: 'a' })).rejects.toMatchObject({ kind: 'TIMEOUT' })
    expect(cancel).toHaveBeenCalledTimes(1)
    const p = app.call('msg', { text: 'b' })
    await settle(5)
    form.reset()
    await expect(p).rejects.toMatchObject({ kind: 'USER_REJECTED' })
  })

  it('data-mcp-tool 写法的表单总是自动提交', async () => {
    document.body.innerHTML = `<form data-mcp-tool="f" data-mcp-desc="f"><input name="a"></form>`
    const onSubmit = vi.fn((e: Event) => {
      e.preventDefault()
      expect((e as AgentSubmitEvent).agentInvoked).toBe(true)
    })
    document.querySelector('form')!.addEventListener('submit', onSubmit)
    detach = attachDom(app, { settleMs: 0 })
    await app.call('f', { a: '1' })
    expect(onSubmit).toHaveBeenCalledTimes(1)
  })

  it('respondWith 与 data-mcp-result 可以并存，先到先得', async () => {
    document.body.innerHTML = `<form toolname="f" tooldescription="f" toolautosubmit data-mcp-result="done"></form>`
    const form = document.querySelector('form')!
    form.addEventListener('submit', (e) => {
      e.preventDefault()
      setTimeout(() => form.dispatchEvent(new CustomEvent('done', { detail: 'event' })), 5)
    })
    detach = attachDom(app)
    await expect(app.call('f')).resolves.toEqual({ data: 'event' })
  })
})
