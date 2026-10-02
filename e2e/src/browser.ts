/**
 * 极简 CDP 驱动：启动无头 Chromium（Playwright 下载的 headless_shell 即可），经 Node 内置 WebSocket
 * 连接 DevTools 协议，用扁平会话（flatten）控制多个标签页。不依赖 Playwright / Puppeteer 包。
 */
import { type ChildProcess, spawn } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { lineSplitter, waitFor } from './util'

interface Pending {
  resolve(value: any): void
  reject(error: Error): void
}

export class Browser {
  private nextId = 1
  private readonly pending = new Map<number, Pending>()
  private readonly listeners = new Set<(msg: { method: string; params: any; sessionId?: string }) => void>()
  readonly pages = new Set<Page>()

  private constructor(
    private readonly child: ChildProcess,
    private readonly ws: WebSocket,
    private readonly profileDir: string,
  ) {
    ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(String(ev.data))
      if (msg.id !== undefined) {
        const p = this.pending.get(msg.id)
        if (!p) return
        this.pending.delete(msg.id)
        if (msg.error) p.reject(new Error(`CDP ${msg.error.code}: ${msg.error.message}`))
        else p.resolve(msg.result)
        return
      }
      for (const l of this.listeners) l(msg)
    })
  }

  static async launch(bin: string): Promise<Browser> {
    const profileDir = mkdtempSync(join(tmpdir(), 'app-mcp-e2e-chrome-'))
    const child = spawn(
      bin,
      [
        '--headless',
        '--no-sandbox',
        '--disable-gpu',
        '--no-first-run',
        '--no-default-browser-check',
        // 后台标签页也照常运行定时器（两个标签页的路由测试）
        '--disable-background-timer-throttling',
        '--disable-renderer-backgrounding',
        '--disable-backgrounding-occluded-windows',
        '--remote-debugging-port=0',
        `--user-data-dir=${profileDir}`,
        'about:blank',
      ],
      { stdio: ['ignore', 'ignore', 'pipe'] },
    )
    const wsUrl = await new Promise<string>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Chromium 启动超时')), 30_000)
      child.stderr!.on(
        'data',
        lineSplitter((line) => {
          const m = /DevTools listening on (ws:\/\/\S+)/.exec(line)
          if (m) {
            clearTimeout(timer)
            resolve(m[1]!)
          }
        }),
      )
      child.once('exit', (code) => {
        clearTimeout(timer)
        reject(new Error(`Chromium 退出（code=${code}）`))
      })
    })
    const ws = new WebSocket(wsUrl)
    await new Promise<void>((resolve, reject) => {
      ws.addEventListener('open', () => resolve(), { once: true })
      ws.addEventListener('error', () => reject(new Error('无法连接 DevTools')), { once: true })
    })
    return new Browser(child, ws, profileDir)
  }

  send<T = any>(method: string, params: Record<string, unknown> = {}, sessionId?: string): Promise<T> {
    const id = this.nextId++
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve, reject })
      this.ws.send(JSON.stringify({ id, method, params, ...(sessionId && { sessionId }) }))
    })
  }

  on(listener: (msg: { method: string; params: any; sessionId?: string }) => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  /** 新建标签页并打开 `url`，等待 load 事件。 */
  async newPage(url: string): Promise<Page> {
    const { targetId } = await this.send<{ targetId: string }>('Target.createTarget', { url: 'about:blank' })
    const { sessionId } = await this.send<{ sessionId: string }>('Target.attachToTarget', { targetId, flatten: true })
    const page = new Page(this, targetId, sessionId)
    await page.send('Page.enable')
    await page.send('Runtime.enable')
    await page.goto(url)
    this.pages.add(page)
    return page
  }

  /**
   * 像操作系统交给浏览器一个地址那样直接以 `url` 新建标签页（不先打开 about:blank，会话历史只有一条），
   * 返回 targetId；不附加会话、不等待加载。
   */
  async openTarget(url: string): Promise<string> {
    const { targetId } = await this.send<{ targetId: string }>('Target.createTarget', { url })
    return targetId
  }

  /** 标签页是否仍存在（页面自己 `window.close()` 后不再存在）。 */
  async targetExists(targetId: string): Promise<boolean> {
    const { targetInfos } = await this.send<{ targetInfos: Array<{ targetId: string }> }>('Target.getTargets')
    return targetInfos.some((t) => t.targetId === targetId)
  }

  /** 附加到已存在的标签页。 */
  async attach(targetId: string): Promise<Page> {
    const { sessionId } = await this.send<{ sessionId: string }>('Target.attachToTarget', { targetId, flatten: true })
    const page = new Page(this, targetId, sessionId)
    await page.send('Runtime.enable')
    this.pages.add(page)
    return page
  }

  async close(): Promise<void> {
    try {
      await Promise.race([this.send('Browser.close'), new Promise((r) => setTimeout(r, 2000))])
    } catch {
      // 已关闭
    }
    this.ws.close()
    if (this.child.exitCode === null) {
      const exited = new Promise((r) => this.child.once('exit', r))
      this.child.kill('SIGKILL')
      await exited
    }
    rmSync(this.profileDir, { recursive: true, force: true })
  }
}

export class Page {
  readonly consoleLines: string[] = []
  private readonly off: () => void

  constructor(
    readonly browser: Browser,
    readonly targetId: string,
    readonly sessionId: string,
  ) {
    this.off = browser.on((msg) => {
      if (msg.sessionId !== sessionId) return
      if (msg.method === 'Runtime.consoleAPICalled') {
        const text = (msg.params.args as Array<{ value?: unknown; description?: string }>)
          .map((a) => (a.value !== undefined ? String(a.value) : (a.description ?? '')))
          .join(' ')
        this.consoleLines.push(`[${msg.params.type}] ${text}`)
      } else if (msg.method === 'Runtime.exceptionThrown') {
        this.consoleLines.push(`[exception] ${msg.params.exceptionDetails?.exception?.description ?? msg.params.exceptionDetails?.text}`)
      }
    })
  }

  send<T = any>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    return this.browser.send<T>(method, params, this.sessionId)
  }

  async goto(url: string): Promise<void> {
    const loaded = new Promise<void>((resolve) => {
      const off = this.browser.on((msg) => {
        if (msg.sessionId === this.sessionId && msg.method === 'Page.loadEventFired') {
          off()
          resolve()
        }
      })
    })
    await this.send('Page.navigate', { url })
    await Promise.race([loaded, new Promise((r) => setTimeout(r, 30_000))])
  }

  /** 在页面中求值（表达式或 async 函数体字符串），返回可 JSON 化的结果。 */
  async eval<T = any>(expression: string): Promise<T> {
    const r = await this.send<{ result: { value?: T }; exceptionDetails?: { text: string; exception?: { description?: string } } }>(
      'Runtime.evaluate',
      { expression, awaitPromise: true, returnByValue: true },
    )
    if (r.exceptionDetails) throw new Error(`页面脚本异常：${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`)
    return r.result.value as T
  }

  text(): Promise<string> {
    return this.eval<string>('document.body.innerText')
  }

  /** 页头的连接状态（ConnectionBadge 文本，如「MCP 已连接」）。 */
  badge(): Promise<string> {
    return this.eval<string>(`document.querySelector('.badge')?.textContent ?? ''`)
  }

  waitForBadge(text: string, timeoutMs = 20_000): Promise<string> {
    return waitFor(async () => {
      const b = await this.badge()
      return b.includes(text) ? b : undefined
    }, `页面状态变为「${text}」`, timeoutMs)
  }

  /** 点击文本为 `label` 的页签按钮。 */
  clickTab(label: string): Promise<void> {
    return this.eval(
      `(() => { const b = [...document.querySelectorAll('[role=tab]')].find((e) => e.textContent.trim() === ${JSON.stringify(label)}); if (!b) throw new Error('没有页签 ${label}'); b.click() })()`,
    )
  }

  /**
   * 模拟窗口焦点变化：覆盖 `document.hasFocus()` 并派发 focus / blur。
   * （无头模式下各标签页都报告可见且有焦点，路由测试需要显式区分。）
   */
  setFocused(focused: boolean): Promise<void> {
    return this.eval(
      `(() => { Object.defineProperty(document, 'hasFocus', { configurable: true, value: () => ${focused} }); window.dispatchEvent(new Event(${JSON.stringify(focused ? 'focus' : 'blur')})) })()`,
    )
  }

  /** 模拟标签页隐藏 / 重新可见：覆盖 `document.visibilityState` 并派发 visibilitychange。 */
  setVisible(visible: boolean): Promise<void> {
    return this.eval(
      `(() => { Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => ${JSON.stringify(visible ? 'visible' : 'hidden')} }); document.dispatchEvent(new Event('visibilitychange')) })()`,
    )
  }

  async close(): Promise<void> {
    this.off()
    this.browser.pages.delete(this)
    await this.browser.send('Target.closeTarget', { targetId: this.targetId })
  }
}
