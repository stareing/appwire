/**
 * 最小 MCP 客户端（Streamable HTTP）：连接常驻的 `app-mcp-host serve`。两代协议：
 *
 * - legacy（默认，协议 2025-06-18）：`initialize` 建会话（`Mcp-Session-Id`），服务器主动消息经 GET /mcp 的 SSE 流接收；
 * - modern（协议 2026-07-28，docs/plans/12-mcp-stateless.md S7）：不握手，`server/discover` 后每个请求在 `_meta` 自带协议版本 /
 *   客户端能力 / 客户端信息，并带 `MCP-Protocol-Version`、`Mcp-Method`、`Mcp-Name` 头；通知经 `subscriptions/listen` 的长响应流接收。
 *
 * 请求：POST /mcp，响应为 JSON 或 SSE（从中取出对应 id 的响应，顺带收集通知）；服务器发起的请求（ping / roots/list）以 POST 回复最小应答。
 * 只实现 e2e 需要的部分：initialize / server/discover、tools/list、tools/call、resources/list、resources/read 与通知。
 */

export interface Tool {
  name: string
  title?: string
  description?: string
  inputSchema: Record<string, unknown>
  annotations?: Record<string, unknown>
}

export interface CallResult {
  content: Array<{ type: string; text?: string }>
  structuredContent?: any
  isError?: boolean
}

export interface InitializeResult {
  protocolVersion: string
  serverInfo: { name: string; version: string }
  capabilities: Record<string, unknown>
  instructions?: string
}

export interface DiscoverResult {
  supportedVersions: string[]
  capabilities: Record<string, unknown>
  instructions?: string
}

/** 协议代际：`legacy` = initialize + 会话；`modern` = 2026-07-28 无会话。 */
export type McpEra = 'legacy' | 'modern'

/** modern 模式协商的协议版本。 */
export const MODERN_PROTOCOL_VERSION = '2026-07-28'

export interface Notification {
  method: string
  params?: any
}

export interface McpClientOptions {
  /** MCP 端点，如 `http://127.0.0.1:7717/mcp`。 */
  url: string
  /** 附加请求头（如 `Authorization: Bearer <令牌>`）。 */
  headers?: Record<string, string>
  /** 失败时附带的诊断信息（如 Host 最近的日志）。 */
  diagnostics?: () => string
  /** 协议代际，默认 `legacy`。 */
  era?: McpEra
}

/** 逐个产出 SSE 事件的 data（多行 data 以换行拼接；空 data 跳过）。 */
async function* sseData(body: ReadableStream<Uint8Array>): AsyncGenerator<string> {
  const reader = body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  try {
    for (;;) {
      const { value, done } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true }).replace(/\r\n?/g, '\n')
      let i: number
      while ((i = buffer.indexOf('\n\n')) >= 0) {
        const block = buffer.slice(0, i)
        buffer = buffer.slice(i + 2)
        const data = block
          .split('\n')
          .filter((l) => l.startsWith('data:'))
          .map((l) => l.slice(5).replace(/^ /, ''))
          .join('\n')
        if (data.trim()) yield data
      }
    }
  } finally {
    reader.releaseLock()
  }
}

export class McpClient {
  private readonly url: string
  private readonly extraHeaders: Record<string, string>
  private readonly diagnostics: () => string
  private nextId = 1
  private sessionId: string | undefined
  private protocolVersion: string | undefined
  private readonly streamAbort = new AbortController()
  private closed = false
  readonly era: McpEra
  readonly notifications: Notification[] = []
  initializeResult: InitializeResult | undefined
  discoverResult: DiscoverResult | undefined
  /** 收到过的 `Mcp-Session-Id`（modern 模式下应始终为空）。 */
  readonly sessionIdsSeen: string[] = []
  /** modern：`subscriptions/listen` 的订阅 ID（listen 请求的 JSON-RPC id）。 */
  subscriptionId: number | undefined

  constructor(options: McpClientOptions) {
    this.url = options.url
    this.extraHeaders = options.headers ?? {}
    this.diagnostics = options.diagnostics ?? (() => '')
    this.era = options.era ?? 'legacy'
  }

  private headers(extra: Record<string, string> = {}): Record<string, string> {
    return {
      ...this.extraHeaders,
      ...(this.sessionId && { 'mcp-session-id': this.sessionId }),
      ...(this.protocolVersion && { 'mcp-protocol-version': this.protocolVersion }),
      ...extra,
    }
  }

  /** modern：每个请求自带的协议 `_meta`（SEP-2575）。 */
  private requestMeta(): Record<string, unknown> {
    return {
      'io.modelcontextprotocol/protocolVersion': MODERN_PROTOCOL_VERSION,
      'io.modelcontextprotocol/clientCapabilities': {},
      'io.modelcontextprotocol/clientInfo': { name: 'app-mcp-e2e', version: '0.1.0' },
    }
  }

  /** modern：请求体加 `_meta`，请求头加 `Mcp-Method` / `Mcp-Name`（SEP-2243）。 */
  private modernize(method: string, params: any): { params: any; headers: Record<string, string> } {
    if (this.era !== 'modern') return { params, headers: {} }
    const p = { ...(params ?? {}), _meta: { ...(params?._meta ?? {}), ...this.requestMeta() } }
    const name = method === 'tools/call' || method === 'prompts/get' ? p.name : method === 'resources/read' ? p.uri : undefined
    return {
      params: p,
      headers: {
        'mcp-protocol-version': MODERN_PROTOCOL_VERSION,
        'mcp-method': method,
        ...(name !== undefined && { 'mcp-name': String(name) }),
      },
    }
  }

  private fail(message: string): Error {
    const diag = this.diagnostics()
    return new Error(diag ? `${message}\n${diag}` : message)
  }

  /** 处理一条来自服务器的消息；是 `id` 的响应时返回它。 */
  private handle(msg: any, id?: number): any | undefined {
    if (msg.method === undefined && msg.id !== undefined) {
      return msg.id === id ? msg : undefined
    }
    if (msg.method !== undefined && msg.id !== undefined) {
      // 服务器发起的请求（ping / roots/list）：最小应答
      const result = msg.method === 'roots/list' ? { roots: [] } : {}
      void this.post({ jsonrpc: '2.0', id: msg.id, result }).catch(() => undefined)
      return undefined
    }
    if (msg.method) this.notifications.push({ method: msg.method, params: msg.params })
    return undefined
  }

  private post(body: unknown, signal?: AbortSignal, extra: Record<string, string> = {}): Promise<Response> {
    return fetch(this.url, {
      method: 'POST',
      headers: this.headers({ 'content-type': 'application/json', accept: 'application/json, text/event-stream', ...extra }),
      body: JSON.stringify(body),
      signal,
    })
  }

  async request<T = any>(method: string, params?: unknown, timeoutMs = 30_000): Promise<T> {
    if (this.closed) throw new Error('MCP 客户端已关闭')
    const id = this.nextId++
    const abort = new AbortController()
    const timer = setTimeout(() => abort.abort(), timeoutMs)
    const modern = this.modernize(method, params)
    params = modern.params
    try {
      const res = await this.post({ jsonrpc: '2.0', id, method, ...(params !== undefined && { params }) }, abort.signal, modern.headers)
      if (!res.ok) throw this.fail(`MCP 请求 ${method} 失败：HTTP ${res.status} ${await res.text()}`)
      const sid = res.headers.get('mcp-session-id')
      if (sid) {
        this.sessionIdsSeen.push(sid)
        this.sessionId = sid
      }
      let response: any
      const type = res.headers.get('content-type') ?? ''
      if (type.includes('text/event-stream') && res.body) {
        for await (const data of sseData(res.body)) {
          response = this.handle(JSON.parse(data), id)
          if (response) break
        }
        void res.body.cancel().catch(() => undefined)
      } else {
        response = this.handle(await res.json(), id)
      }
      if (!response) throw this.fail(`MCP 请求 ${method} 没有响应`)
      if (response.error) {
        throw Object.assign(new Error(`${response.error.code}: ${response.error.message}`), { rpc: response.error })
      }
      return response.result as T
    } catch (error) {
      if (abort.signal.aborted) throw this.fail(`MCP 请求超时：${method}`)
      throw error
    } finally {
      clearTimeout(timer)
    }
  }

  async notify(method: string, params?: unknown): Promise<void> {
    const res = await this.post({ jsonrpc: '2.0', method, ...(params !== undefined && { params }) })
    await res.body?.cancel()
    if (!res.ok) throw this.fail(`MCP 通知 ${method} 失败：HTTP ${res.status}`)
  }

  /** GET /mcp：接收服务器主动发送的通知，直到关闭。 */
  private async listen(ready: () => void): Promise<void> {
    let res: Response
    try {
      res = await fetch(this.url, {
        method: 'GET',
        headers: this.headers({ accept: 'text/event-stream' }),
        signal: this.streamAbort.signal,
      })
    } catch {
      ready()
      return
    }
    ready()
    if (!res.ok || !res.body) return
    try {
      for await (const data of sseData(res.body)) this.handle(JSON.parse(data))
    } catch {
      // 关闭时中止
    }
  }

  async initialize(): Promise<InitializeResult> {
    const result = await this.request<InitializeResult>('initialize', {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: { name: 'app-mcp-e2e', version: '0.1.0' },
    })
    this.protocolVersion = result.protocolVersion
    await this.notify('notifications/initialized')
    await new Promise<void>((ready) => void this.listen(ready))
    this.initializeResult = result
    return result
  }

  /** modern：`server/discover`，再打开 `subscriptions/listen`（工具 / 资源列表变化）并等到确认。 */
  async discover(): Promise<DiscoverResult> {
    const result = await this.request<DiscoverResult>('server/discover', {})
    this.discoverResult = result
    await this.openListen()
    return result
  }

  /** legacy：`initialize`；modern：`server/discover`。 */
  connect(): Promise<unknown> {
    return this.era === 'modern' ? this.discover() : this.initialize()
  }

  /** modern：`subscriptions/listen` 长响应流，确认（`notifications/subscriptions/acknowledged`）到达后返回；之后的通知收集到 `notifications`。 */
  private async openListen(): Promise<void> {
    const id = this.nextId++
    this.subscriptionId = id
    const filter = { toolsListChanged: true, resourcesListChanged: true }
    const modern = this.modernize('subscriptions/listen', { notifications: filter })
    const res = await this.post(
      { jsonrpc: '2.0', id, method: 'subscriptions/listen', params: modern.params },
      this.streamAbort.signal,
      modern.headers,
    )
    if (!res.ok || !res.body) throw this.fail(`subscriptions/listen 失败：HTTP ${res.status} ${await res.text()}`)
    const sid = res.headers.get('mcp-session-id')
    if (sid) this.sessionIdsSeen.push(sid)
    const stream = sseData(res.body)
    const first = await stream.next()
    const ack = first.done ? undefined : JSON.parse(first.value)
    if (ack?.method !== 'notifications/subscriptions/acknowledged') {
      throw this.fail(`subscriptions/listen 没有确认：${first.done ? '流已结束' : first.value}`)
    }
    void (async () => {
      try {
        for await (const data of stream) this.handle(JSON.parse(data))
      } catch {
        // 关闭时中止
      }
    })()
  }

  async listTools(): Promise<Tool[]> {
    const tools: Tool[] = []
    let cursor: string | undefined
    do {
      const page: { tools: Tool[]; nextCursor?: string } = await this.request('tools/list', cursor ? { cursor } : {})
      tools.push(...page.tools)
      cursor = page.nextCursor
    } while (cursor)
    return tools
  }

  async toolNames(): Promise<string[]> {
    return (await this.listTools()).map((t) => t.name).sort()
  }

  callTool(name: string, args: Record<string, unknown> = {}, timeoutMs?: number): Promise<CallResult> {
    return this.request<CallResult>('tools/call', { name, arguments: args }, timeoutMs)
  }

  async listResources(): Promise<Array<{ uri: string; name: string }>> {
    const r: { resources: Array<{ uri: string; name: string }> } = await this.request('resources/list', {})
    return r.resources
  }

  async readResource(uri: string): Promise<any> {
    const r: { contents: Array<{ uri: string; text?: string; mimeType?: string }> } = await this.request('resources/read', { uri })
    const text = r.contents[0]?.text
    return text === undefined ? undefined : JSON.parse(text)
  }

  /** 等到连续 `quietMs` 内没有新通知（让防抖中的 list_changed 先到达）。 */
  async quiet(quietMs = 500, timeoutMs = 10_000): Promise<void> {
    const deadline = Date.now() + timeoutMs
    let count = this.notifications.length
    let since = Date.now()
    while (Date.now() - since < quietMs) {
      if (Date.now() > deadline) return
      await new Promise((r) => setTimeout(r, 50))
      if (this.notifications.length !== count) {
        count = this.notifications.length
        since = Date.now()
      }
    }
  }

  /** 自 `since`（`notifications.length` 快照）之后收到的指定通知数。 */
  countSince(method: string, since: number): number {
    return this.notifications.slice(since).filter((n) => n.method === method).length
  }

  /** 结束会话（DELETE /mcp）并关闭通知流。 */
  async close(): Promise<void> {
    if (this.closed) return
    this.closed = true
    this.streamAbort.abort()
    if (this.sessionId) {
      await fetch(this.url, { method: 'DELETE', headers: this.headers() })
        .then((r) => r.body?.cancel())
        .catch(() => undefined)
    }
  }
}

/** 结果中的文本块。 */
export function texts(r: CallResult): string[] {
  return r.content.filter((c) => c.type === 'text').map((c) => c.text ?? '')
}

/** 结果中的数据（structuredContent，缺省时解析非总览的第一段文本）。 */
export function data(r: CallResult): any {
  if (r.structuredContent !== undefined) return r.structuredContent
  const t = texts(r).find((s) => !s.startsWith('[app-mcp]'))
  return t === undefined ? undefined : JSON.parse(t)
}

/** 断言调用成功并返回数据。 */
export function ok(r: CallResult): any {
  if (r.isError) throw new Error(`工具调用失败：${texts(r).join('\n')}`)
  return data(r)
}
