import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ToolCallError } from '@app-mcp/web'
import { attachInspect } from '../src/index'
import { FakeRegistrar } from './fake'

let reg: FakeRegistrar
let detach: () => void

function setup(html: string, options: Parameters<typeof attachInspect>[1] = {}) {
  document.body.innerHTML = html
  reg = new FakeRegistrar()
  detach = attachInspect(reg, options)
}

afterEach(() => {
  detach?.()
  vi.restoreAllMocks()
})

/** 名称 → 引用。 */
async function refs(): Promise<Record<string, string>> {
  const r = await reg.call('ui.outline', { limit: 500 })
  return Object.fromEntries(r.items.map((i: any) => [i.name, i.ref]))
}

function descriptor(el: object, prop: string): PropertyDescriptor {
  for (let p = Object.getPrototypeOf(el); p; p = Object.getPrototypeOf(p)) {
    const d = Object.getOwnPropertyDescriptor(p, prop)
    if (d) return d
  }
  throw new Error(`找不到 ${prop}`)
}

/**
 * 模拟 React 受控组件：实例上安装 value 追踪器，input 事件时只有 DOM 值与追踪值不同才触发 onChange。
 * 直接 `el.value = x` 会同步更新追踪值，从而被 React 忽略。
 */
function reactControlled(input: HTMLInputElement | HTMLTextAreaElement, onChange: (v: string) => void) {
  const d = descriptor(input, 'value')
  let tracked = input.value
  Object.defineProperty(input, 'value', {
    configurable: true,
    get() {
      return d.get!.call(this)
    },
    set(v) {
      tracked = String(v)
      d.set!.call(this, v)
    },
  })
  input.addEventListener('input', () => {
    const v = d.get!.call(input) as string
    if (v !== tracked) {
      tracked = v
      onChange(v)
    }
  })
}

describe('ui.click', () => {
  beforeEach(() => {
    setup(`
      <main>
        <h2>购物车</h2>
        <button id="checkout">结算</button>
        <button id="off" disabled>不可用</button>
        <button id="nav">去订单</button>
        <span id="count">0</span>
      </main>`)
  })

  it('派发指针事件并点击；返回变化摘要（状态变化、新增对话框）', async () => {
    const events: string[] = []
    const btn = document.getElementById('checkout') as HTMLButtonElement
    for (const t of ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click']) {
      btn.addEventListener(t, () => events.push(t))
    }
    btn.addEventListener('click', () => {
      btn.disabled = true
      const dlg = document.createElement('dialog')
      dlg.setAttribute('open', '')
      dlg.innerHTML = '<h2>确认支付</h2><button id="ok">确定</button><button id="cancel">取消</button>'
      dlg.querySelector('#cancel')!.addEventListener('click', () => dlg.remove())
      document.querySelector('main')!.append(dlg)
    })
    const r0 = await refs()
    const r = await reg.call('ui.click', { ref: r0['结算'] })
    expect(events).toEqual(['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click'])
    expect(r.ok).toBe(true)
    expect(r.url).toBeUndefined()
    expect(r.changes).toHaveLength(2)
    expect(r.changes[0]).toBe(`${r0['结算']} 结算 变为 disabled`)
    expect(r.changes[1]).toMatch(/^新增对话框「确认支付」\((e\d+)\)，含 2 个可交互元素（可用 ui\.outline\(\{ within: "\1" \}\) 查看）$/)
    const dialogRef = /\((e\d+)\)/.exec(r.changes[1])![1]
    const inner = await reg.call('ui.outline', { within: dialogRef })
    expect(inner.text).toMatch(/^» e\d+ 对话框「确认支付」\n {2}## 确认支付\n {2}e\d+ 按钮「确定」\n {2}e\d+ 按钮「取消」$/)
    const cancel = inner.items.find((i: any) => i.name === '取消').ref
    const r2 = await reg.call('ui.click', { ref: cancel })
    expect(r2.changes).toEqual([`对话框「确认支付」(${dialogRef}) 已消失（含 2 个可交互元素）`])
  })

  it('无变化时 changes 为空；URL 变化时返回 url', async () => {
    const r0 = await refs()
    document.getElementById('nav')!.addEventListener('click', () => history.pushState({}, '', '/orders?page=2'))
    const r = await reg.call('ui.click', { ref: r0['去订单'] })
    expect(r.changes).toEqual([])
    expect(r.url).toMatch(/\/orders\?page=2$/)
  })

  it('禁用元素报错（说明原因），不触发点击', async () => {
    const r0 = await refs()
    const clicked = vi.fn()
    document.getElementById('off')!.addEventListener('click', clicked)
    const err = await reg.call('ui.click', { ref: r0['不可用'] }).catch((e) => e)
    expect(err).toBeInstanceOf(ToolCallError)
    expect(err.kind).toBe('INVALID_INPUT')
    expect(err.message).toBe(`${r0['不可用']} 按钮「不可用」已禁用，当前无法操作`)
    expect(err.details).toEqual({ ref: r0['不可用'], reason: 'TOOL_DISABLED' })
    expect(clicked).not.toHaveBeenCalled()
  })

  it('元素变为不可见时报错', async () => {
    const r0 = await refs()
    ;(document.getElementById('checkout') as HTMLElement).style.display = 'none'
    const err = await reg.call('ui.click', { ref: r0['结算'] }).catch((e) => e)
    expect(err.message).toContain('当前不可见')
  })

  it('已声明元素在结果中提示直接调用对应工具', async () => {
    document.getElementById('checkout')!.setAttribute('data-mcp-tool', 'cart.checkout')
    const r0 = await refs()
    const r = await reg.call('ui.click', { ref: r0['结算'] })
    expect(r.hint).toBe('该元素已声明为工具 cart.checkout，下次可直接调用')
  })

  it('缺少或格式错误的 ref', async () => {
    await expect(reg.call('ui.click', {})).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
    await expect(reg.call('ui.click', { ref: 'button' })).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
  })
})

describe('ui.fill', () => {
  beforeEach(() => {
    setup(`
      <form aria-label="收货">
        <label>收货地址 <input id="addr" name="addr"></label>
        <label>备注 <textarea id="note"></textarea></label>
        <label>城市 <select id="city"><option value="bj">北京</option><option value="sh">上海</option><option value="gz" disabled>广州</option></select></label>
        <label>标签 <select id="tags" multiple><option value="a">甲</option><option value="b">乙</option><option value="c">丙</option></select></label>
        <label><input type="checkbox" id="agree"> 同意条款</label>
        <label><input type="radio" name="pay" id="wx" value="wx"> 微信</label>
        <label><input type="radio" name="pay" id="ali" value="ali"> 支付宝</label>
        <label>只读 <input readonly value="x"></label>
        <label>附件 <input type="file"></label>
        <div contenteditable="true" aria-label="正文"></div>
        <button type="button">按钮</button>
      </form>`)
  })

  it('兼容 React 受控组件：原型 setter + input / change 事件', async () => {
    const addr = document.getElementById('addr') as HTMLInputElement
    const onChange = vi.fn()
    reactControlled(addr, onChange)
    // 模拟本身的正确性：直接赋值会被"React"忽略
    addr.value = '直接赋值'
    addr.dispatchEvent(new Event('input', { bubbles: true }))
    expect(onChange).not.toHaveBeenCalled()

    const events: string[] = []
    addr.addEventListener('input', () => events.push('input'))
    addr.addEventListener('change', () => events.push('change'))
    const r0 = await refs()
    const r = await reg.call('ui.fill', { ref: r0['收货地址'], value: '上海市浦东新区' })
    expect(onChange).toHaveBeenCalledWith('上海市浦东新区')
    expect(addr.value).toBe('上海市浦东新区')
    expect(events).toEqual(['input', 'change'])
    expect(r.changes).toEqual([`${r0['收货地址']} 收货地址 值变为 "上海市浦东新区"`])

    const note = document.getElementById('note') as HTMLTextAreaElement
    const onNote = vi.fn()
    reactControlled(note, onNote)
    await reg.call('ui.fill', { ref: r0['备注'], value: 42 })
    expect(note.value).toBe('42')
    expect(onNote).toHaveBeenCalledWith('42')
    const cleared = await reg.call('ui.fill', { ref: r0['收货地址'], value: '' })
    expect(cleared.changes).toEqual([`${r0['收货地址']} 收货地址 值已清空`])
  })

  it('select 按 value 或选项文本匹配；多选传数组；不匹配时列出可选项', async () => {
    const city = document.getElementById('city') as HTMLSelectElement
    const change = vi.fn()
    city.addEventListener('change', change)
    const r0 = await refs()
    let r = await reg.call('ui.fill', { ref: r0['城市'], value: 'sh' })
    expect(city.value).toBe('sh')
    expect(change).toHaveBeenCalledTimes(1)
    expect(r.changes).toEqual([`${r0['城市']} 城市 值变为 "上海"`])
    r = await reg.call('ui.fill', { ref: r0['城市'], value: '北京' })
    expect(city.value).toBe('bj')
    const err = await reg.call('ui.fill', { ref: r0['城市'], value: '深圳' }).catch((e) => e)
    expect(err.kind).toBe('INVALID_INPUT')
    expect(err.message).toBe('下拉框中没有与 "深圳" 匹配的选项；可选："北京"、"上海"、"广州"')
    await expect(reg.call('ui.fill', { ref: r0['城市'], value: '广州' })).rejects.toThrow('已禁用')

    const tags = document.getElementById('tags') as HTMLSelectElement
    await reg.call('ui.fill', { ref: r0['标签'], value: ['甲', 'c'] })
    expect(Array.from(tags.selectedOptions).map((o) => o.value)).toEqual(['a', 'c'])
  })

  it('checkbox / radio 用 checked', async () => {
    const agree = document.getElementById('agree') as HTMLInputElement
    const clicks = vi.fn()
    agree.addEventListener('click', clicks)
    const r0 = await refs()
    let r = await reg.call('ui.fill', { ref: r0['同意条款'], value: true })
    expect(agree.checked).toBe(true)
    expect(clicks).toHaveBeenCalledTimes(1)
    expect(r.changes).toEqual([`${r0['同意条款']} 同意条款 变为 checked`])
    // 已是目标状态时不再点击
    await reg.call('ui.fill', { ref: r0['同意条款'], value: 'true' })
    expect(clicks).toHaveBeenCalledTimes(1)
    await reg.call('ui.fill', { ref: r0['同意条款'], value: false })
    expect(agree.checked).toBe(false)

    r = await reg.call('ui.fill', { ref: r0['支付宝'], value: true })
    expect((document.getElementById('ali') as HTMLInputElement).checked).toBe(true)
    expect(r.changes).toEqual([`${r0['支付宝']} 支付宝 变为 checked`])
    await reg.call('ui.fill', { ref: r0['微信'], value: true })
    expect((document.getElementById('ali') as HTMLInputElement).checked).toBe(false)
    await expect(reg.call('ui.fill', { ref: r0['微信'], value: 'maybe' })).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
  })

  it('contenteditable 与不支持的控件', async () => {
    const r0 = await refs()
    await reg.call('ui.fill', { ref: r0['正文'], value: '你好' })
    expect(document.querySelector('[contenteditable]')!.textContent).toBe('你好')
    await expect(reg.call('ui.fill', { ref: r0['只读'], value: 'y' })).rejects.toThrow('只读')
    await expect(reg.call('ui.fill', { ref: r0['附件'], value: 'a.txt' })).rejects.toThrow('文件选择')
    await expect(reg.call('ui.fill', { ref: r0['按钮'], value: 'x' })).rejects.toThrow('ui.click')
    await expect(reg.call('ui.fill', { ref: r0['收货'] ?? 'e1', value: 'x' })).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
  })
})

describe('ui.press', () => {
  beforeEach(() => {
    setup(`
      <form aria-label="搜索"><input id="q" aria-label="关键词"><button type="submit">搜</button></form>
      <button id="b">菜单</button>
      <input id="other" aria-label="其他">`)
  })

  it('派发 keydown / keypress / keyup，带修饰键', async () => {
    const q = document.getElementById('q') as HTMLInputElement
    const seen: string[] = []
    for (const t of ['keydown', 'keypress', 'keyup']) {
      q.addEventListener(t, (e) => {
        const k = e as KeyboardEvent
        seen.push(`${t}:${k.key}:${k.code}:${k.ctrlKey ? 'C' : ''}${k.shiftKey ? 'S' : ''}`)
      })
    }
    const r0 = await refs()
    await reg.call('ui.press', { ref: r0['关键词'], key: 'a' })
    await reg.call('ui.press', { ref: r0['关键词'], key: 'Control+Shift+K' })
    expect(seen).toEqual([
      'keydown:a:KeyA:',
      'keypress:a:KeyA:',
      'keyup:a:KeyA:',
      'keydown:K:KeyK:CS',
      'keyup:K:KeyK:CS',
    ])
    await expect(reg.call('ui.press', { ref: r0['关键词'], key: 'Hyper+x' })).rejects.toMatchObject({ kind: 'INVALID_INPUT' })
  })

  it('Enter 在输入框中提交表单；缺省目标为焦点元素；Escape 交给页面处理', async () => {
    const submit = vi.fn((e: Event) => e.preventDefault())
    document.querySelector('form')!.addEventListener('submit', submit)
    const r0 = await refs()
    await reg.call('ui.press', { ref: r0['关键词'], key: 'Enter' })
    expect(submit).toHaveBeenCalledTimes(1)

    const esc = vi.fn()
    const b = document.getElementById('b') as HTMLButtonElement
    b.addEventListener('keydown', (e) => (e as KeyboardEvent).key === 'Escape' && esc())
    b.focus()
    await reg.call('ui.press', { key: 'Esc' })
    expect(esc).toHaveBeenCalledTimes(1)
  })

  it('keydown 被阻止时不执行默认行为；Enter / 空格激活按钮；Tab 移动焦点', async () => {
    const b = document.getElementById('b') as HTMLButtonElement
    const click = vi.fn()
    b.addEventListener('click', click)
    const r0 = await refs()
    await reg.call('ui.press', { ref: r0['菜单'], key: 'Enter' })
    await reg.call('ui.press', { ref: r0['菜单'], key: 'Space' })
    expect(click).toHaveBeenCalledTimes(2)
    const block = (e: Event) => e.preventDefault()
    b.addEventListener('keydown', block)
    await reg.call('ui.press', { ref: r0['菜单'], key: 'Enter' })
    expect(click).toHaveBeenCalledTimes(2)
    b.removeEventListener('keydown', block)

    b.focus()
    await reg.call('ui.press', { key: 'Tab' })
    expect(document.activeElement?.id).toBe('other')
    await reg.call('ui.press', { key: 'Shift+Tab' })
    expect(document.activeElement?.id).toBe('b')
  })
})

describe('ui.submit / ui.scroll / ui.read', () => {
  it('表单或其中元素 → requestSubmit，提交按钮作为 submitter；校验失败时报错且不提交', async () => {
    setup(`
      <form aria-label="登录">
        <label>用户名 <input name="u" required></label>
        <button type="submit" name="act" value="login">登录</button>
      </form>`)
    const form = document.querySelector('form')!
    const submitter: Array<string | undefined> = []
    form.addEventListener('submit', (e) => {
      e.preventDefault()
      submitter.push((e as SubmitEvent).submitter?.getAttribute('value') ?? undefined)
      form.insertAdjacentHTML('beforeend', '<p role="alert">登录成功</p>')
    })
    const r0 = await refs()
    const err = await reg.call('ui.submit', { ref: r0['用户名'] }).catch((e) => e)
    expect(err.kind).toBe('INVALID_INPUT')
    expect(err.message).toMatch(/^表单校验未通过，未提交。e\d+ 输入框「用户名」：/)
    expect(submitter).toEqual([])

    await reg.call('ui.fill', { ref: r0['用户名'], value: 'alice' })
    const r = await reg.call('ui.submit', { ref: r0['登录'] })
    expect(submitter).toEqual(['login'])
    expect(r.changes).toEqual([expect.stringMatching(/^新增警告「登录成功」\(e\d+\)$/)])
    await reg.call('ui.submit', { ref: r0['用户名'] })
    expect(submitter).toEqual(['login', undefined])
  })

  it('不在表单内时报错', async () => {
    setup('<button>孤立</button>')
    const r0 = await refs()
    await expect(reg.call('ui.submit', { ref: r0['孤立'] })).rejects.toThrow('不是表单')
  })

  it('scroll 调用 scrollIntoView', async () => {
    setup('<button>底部</button>')
    const spy = vi.spyOn(HTMLElement.prototype, 'scrollIntoView')
    const r0 = await refs()
    const r = await reg.call('ui.scroll', { ref: r0['底部'] })
    expect(spy).toHaveBeenCalledWith({ block: 'center', inline: 'nearest' })
    expect(r).toEqual({ ok: true, changes: [] })
  })

  it('read 返回精简可见文本并按 maxChars 截断', async () => {
    setup(`
      <main>
        <h2>订单详情</h2>
        <p>订单号   123456</p>
        <p style="display:none">内部备注</p>
        <script>var x = 1</script>
        <p>${'长'.repeat(2000)}</p>
        <input aria-label="数量" value="3">
        <input type="password" aria-label="口令" value="pw">
      </main>`)
    const r0 = await refs()
    const all = await reg.call('ui.read', {})
    expect(all.ref).toBe('root')
    expect(all.text.startsWith('订单详情 订单号 123456 长长')).toBe(true)
    expect(all.text).not.toContain('内部备注')
    expect(all.text).not.toContain('var x')
    expect(all.text).toHaveLength(1000)
    expect(all.truncated).toBe(true)
    const short = await reg.call('ui.read', { maxChars: 10 })
    expect(short).toEqual({ ref: 'root', text: '订单详情 订单号…', truncated: true })
    expect(await reg.call('ui.read', { ref: r0['数量'] })).toEqual({ ref: r0['数量'], text: '3', truncated: false })
    expect((await reg.call('ui.read', { ref: r0['口令'] })).text).toBe('••••')
  })
})

describe('密码类控件（spec/ui-fallback.md 7.1 / 8.1）', () => {
  const html = `
    <form id="f">
      <input type="password" aria-label="短口令" value="pw">
      <input type="password" aria-label="长口令" value="a-much-longer-secret-value">
      <input type="password" aria-label="空口令">
      <input aria-label="用户名" value="u">
    </form>`

  it('大纲、ui.read 与变化摘要中只显示 ••••，不泄露长度与内容', async () => {
    setup(html)
    const out = await reg.call('ui.outline', { limit: 500 })
    const byName = Object.fromEntries(out.items.map((i: any) => [i.name, i]))
    expect(byName['短口令'].value).toBe('••••')
    expect(byName['长口令'].value).toBe('••••')
    expect(byName['空口令'].value).toBeUndefined()
    expect(out.text).not.toContain('pw')
    expect(out.text).not.toContain('secret')
    expect((await reg.call('ui.read', { ref: byName['短口令'].ref })).text).toBe('••••')
    expect((await reg.call('ui.read', { ref: byName['长口令'].ref })).text).toBe('••••')
    expect((await reg.call('ui.read', { ref: byName['空口令'].ref })).text).toBe('')
    expect((await reg.call('ui.read', {})).text).not.toContain('secret')
    const changed = await reg.call('ui.fill', { ref: byName['用户名'].ref, value: 'v' })
    const pw = document.querySelector('input[aria-label="长口令"]') as HTMLInputElement
    pw.value = 'x'
    const after = await reg.call('ui.click', { ref: byName['用户名'].ref })
    expect(JSON.stringify([changed, after])).not.toMatch(/secret|"x"/)
  })

  it('ui.fill 拒绝并给出 reason: secure，值不变、不派发事件', async () => {
    setup(html)
    const r0 = await refs()
    const pw = document.querySelector('input[aria-label="短口令"]') as HTMLInputElement
    const onInput = vi.fn()
    pw.addEventListener('input', onInput)
    const err = await reg.call('ui.fill', { ref: r0['短口令'], value: 'hacked' }).catch((e) => e)
    expect(err).toBeInstanceOf(ToolCallError)
    expect(err.kind).toBe('INVALID_INPUT')
    expect(err.message).toBe(`${r0['短口令']} 密码框「短口令」是密码类控件，兜底工具不填写`)
    expect(err.details).toEqual({ ref: r0['短口令'], reason: 'secure' })
    expect(pw.value).toBe('pw')
    expect(onInput).not.toHaveBeenCalled()
  })

  it('ui.press 指定引用或作用于焦点密码框时都拒绝，不派发按键', async () => {
    setup(html)
    const r0 = await refs()
    const pw = document.querySelector('input[aria-label="短口令"]') as HTMLInputElement
    const form = document.getElementById('f') as HTMLFormElement
    const onKey = vi.fn()
    const onSubmit = vi.fn((e: Event) => e.preventDefault())
    pw.addEventListener('keydown', onKey)
    form.addEventListener('submit', onSubmit)
    for (const key of ['Enter', 'a', 'Tab']) {
      const err = await reg.call('ui.press', { ref: r0['短口令'], key }).catch((e) => e)
      expect(err.kind).toBe('INVALID_INPUT')
      expect(err.details).toEqual({ ref: r0['短口令'], reason: 'secure' })
    }
    pw.focus()
    const err = await reg.call('ui.press', { key: 'Enter' }).catch((e) => e)
    expect(err.details).toEqual({ ref: r0['短口令'], reason: 'secure' })
    expect(onKey).not.toHaveBeenCalled()
    expect(onSubmit).not.toHaveBeenCalled()
  })

  it('禁用的密码框先报 TOOL_DISABLED（核对顺序）', async () => {
    setup(`<input type="password" aria-label="口令" disabled>`)
    const r0 = await refs()
    const err = await reg.call('ui.fill', { ref: r0['口令'], value: 'x' }).catch((e) => e)
    expect(err.details).toEqual({ ref: r0['口令'], reason: 'TOOL_DISABLED' })
  })
})

describe('变化摘要', () => {
  it('名称、值、状态、标题、新增与移除；超过上限时截断', async () => {
    setup(`
      <main>
        <h2 id="title">购物车</h2>
        <button id="go">结算</button>
        <label>数量 <input id="n" value="1"></label>
        <button id="toggle" aria-expanded="false">更多</button>
        <button id="gone">删除商品</button>
        <div id="list"></div>
      </main>`)
    const r0 = await refs()
    document.getElementById('go')!.addEventListener('click', () => {
      document.getElementById('go')!.textContent = '结算中…'
      ;(document.getElementById('n') as HTMLInputElement).value = '2'
      document.getElementById('toggle')!.setAttribute('aria-expanded', 'true')
      document.getElementById('gone')!.remove()
      document.getElementById('title')!.textContent = '确认订单'
      document.getElementById('list')!.innerHTML = '<a href="/pay">去支付</a>'
    })
    const r = await reg.call('ui.click', { ref: r0['结算'] })
    expect(r.changes).toEqual([
      '标题「购物车」变为「确认订单」',
      `${r0['结算']} 结算 名称变为「结算中…」`,
      `${r0['数量']} 数量 值变为 "2"`,
      `${r0['更多']} 更多 变为 expanded`,
      expect.stringMatching(/^新增链接「去支付」\(e\d+\)$/),
      `按钮「删除商品」(${r0['删除商品']}) 已消失`,
    ])

    document.getElementById('list')!.addEventListener('click', () => {})
    const btn = document.createElement('button')
    btn.textContent = '加很多'
    btn.addEventListener('click', () => {
      for (let i = 0; i < 30; i++) document.getElementById('list')!.insertAdjacentHTML('beforeend', `<button>新${i}</button>`)
    })
    document.querySelector('main')!.append(btn)
    const r1 = await refs()
    const many = await reg.call('ui.click', { ref: r1['加很多'] })
    expect(many.changes).toHaveLength(16)
    expect(many.changes[15]).toBe('…另有 15 项变化，请调用 ui.outline 查看')
  })
})

describe('allowScript', () => {
  it('默认不注册 ui.eval；开启后 risk 为 destructive，可执行表达式与语句块', async () => {
    setup('<button id="x">按钮</button>')
    expect(reg.tools.has('ui.eval')).toBe(false)
    detach()
    reg = new FakeRegistrar()
    detach = attachInspect(reg, { allowScript: true })
    const t = reg.getTool('ui.eval')
    expect(t.def.risk).toBe('destructive')
    expect((await reg.call('ui.eval', { code: '1 + 2' })).result).toBe(3)
    const r0 = await refs()
    const stmt = await reg.call('ui.eval', {
      code: 'const b = el("' + r0['按钮'] + '"); b.textContent = "改名"; return [b.id, root.tagName, b]',
    })
    expect(stmt.result).toEqual(['x', 'BODY', `${r0['按钮']} 按钮「改名」`])
    expect(stmt.changes).toEqual([`${r0['按钮']} 按钮 名称变为「改名」`])
    await expect(reg.call('ui.eval', { code: 'throw new Error("坏了")' })).rejects.toThrow('脚本出错：坏了')
    await expect(reg.call('ui.eval', { code: 'await Promise.resolve(5)' })).resolves.toMatchObject({ result: 5 })
    const big = await reg.call('ui.eval', { code: '"x".repeat(10000)' })
    expect(typeof big.result).toBe('string')
    expect(big.result.length).toBeLessThan(4100)
  })
})

describe('attach / detach', () => {
  it('detach 注销全部工具；root 与 prefix 选项生效', async () => {
    document.body.innerHTML = '<div id="a"><button>里面</button></div><button>外面</button>'
    reg = new FakeRegistrar()
    detach = attachInspect(reg, { root: document.getElementById('a')!, prefix: 'dev', allowScript: true })
    expect([...reg.tools.keys()].every((n) => n.startsWith('dev.'))).toBe(true)
    expect(reg.tools.size).toBe(8)
    const r = await reg.call('dev.outline')
    expect(r.text).toBe(`${r.items[0].ref} 按钮「里面」`)
    detach()
    expect(reg.tools.size).toBe(0)
    detach()
  })
})
