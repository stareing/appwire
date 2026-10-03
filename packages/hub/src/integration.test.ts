/**
 * 集成测试：真实原生模块（native/*.node）起嵌入式 Hub（WebSocket 随机端口），
 * 同进程用 @app-mcp/node（App 端 Node SDK）注册工具并连上。
 *
 * 前置条件：`pnpm --filter @app-mcp/hub build:native` 与 `pnpm --filter @app-mcp/node build:native`
 * （或环境变量 APP_MCP_HUB_NATIVE / APP_MCP_NODE_NATIVE 指定路径）。缺少任一时跳过。
 *
 * @app-mcp/node 未链接进本包的 node_modules（并行开发期间不运行 pnpm install），这里用相对路径导入其源码。
 */
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it } from 'vitest'
import { createAppMcp, type AppMcp } from '../../node/src/index.js'
import { nativeFileName as appNativeFileName } from '../../node/src/native.js'
import {
  handleAnthropicToolUses,
  handleOpenAiToolCalls,
  Hub,
  HubError,
  HubToolCallError,
  toAnthropicTools,
  toOpenAiTools,
  toVercelAiTools,
  type ApprovalRequest,
  type CallPriority,
  type HubEvent,
  type HubStartOptions,
  type McpProtocolMode,
  type PairingRequest,
  type ToolExposure,
  type WakeRequest,
} from './index.js'
import { nativeFileName } from './native.js'

const pkgDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const hubNative = process.env.APP_MCP_HUB_NATIVE ?? join(pkgDir, 'native', nativeFileName())
const appNative = process.env.APP_MCP_NODE_NATIVE ?? join(pkgDir, '..', 'node', 'native', appNativeFileName())
const ready = existsSync(hubNative) && existsSync(appNative)

const hubs: Hub[] = []
const apps: AppMcp[] = []

afterEach(async () => {
  for (const app of apps.splice(0)) app.dispose()
  for (const hub of hubs.splice(0)) await hub.shutdown()
})

async function until<T>(f: () => T | undefined | null | false, what: string, timeoutMs = 10_000): Promise<T> {
  const start = Date.now()
  for (;;) {
    const v = f()
    if (v) return v
    if (Date.now() - start > timeoutMs) throw new Error(`等待超时：${what}`)
    await new Promise((r) => setTimeout(r, 20))
  }
}

async function startHub(options: HubStartOptions = {}): Promise<{ hub: Hub; events: HubEvent[] }> {
  // 不占用本机常驻 Host 的默认 IPC 端点；IPC 用临时端点单独测试。
  const hub = await Hub.start({
    listen: '127.0.0.1:0',
    ipcEndpoint: null,
    keepAlive: false,
    listChangedDebounceMs: 20,
    ...options,
  })
  hubs.push(hub)
  const events: HubEvent[] = []
  hub.onEvent((e) => events.push(e))
  return { hub, events }
}

interface ShopLog {
  calls: string[]
}

/** 同进程 App：shop（cart.add / cart.clear / order.pay + 资源 cart）。 */
async function startShop(hub: Hub): Promise<ShopLog> {
  const log: ShopLog = { calls: [] }
  const app = createAppMcp({
    appId: 'shop',
    appName: '测试商店',
    hostUrl: hub.wsUrl!,
    autoStart: false,
    keepAlive: false,
    overview: { summary: '测试用商店 App', body: '加购物车用 cart.add。' },
  })
  apps.push(app)
  let count = 0
  app.tool('cart.add', {
    description: '加入购物车',
    risk: 'write',
    input: {
      type: 'object',
      properties: { sku: { type: 'string' }, qty: { type: 'number' } },
      required: ['sku', 'qty'],
    },
    handler: async ({ sku, qty }: { sku: string; qty: number }) => {
      log.calls.push(`cart.add:${sku}`)
      count += qty
      return { data: { count }, stateHints: ['cart'] }
    },
  })
  app.tool('cart.clear', {
    description: '清空购物车',
    risk: 'destructive',
    handler: () => {
      log.calls.push('cart.clear')
      count = 0
      return { cleared: true }
    },
  })
  app.tool('order.pay', {
    description: '付款',
    risk: 'payment',
    handler: () => {
      log.calls.push('order.pay')
      return { paid: true }
    },
  })
  app.resource('cart', { description: '购物车', read: () => ({ count }) })
  app.start()
  await until(
    // 显式指定 App：不受渐进暴露影响
    () => hub.tools({ apps: ['shop'], onlyAvailable: true }).some((t) => t.name === 'shop.order.pay'),
    'shop 工具登记',
  )
  return log
}

describe.skipIf(!ready)('嵌入式 Hub + @app-mcp/node', () => {
  it('渐进暴露：apps.tools 展开后按会话导出；waker / toolExposure 配置透传', async () => {
    const { hub } = await startHub({ toolExposure: 'progressive', toolExposureThreshold: 1, waker: 'none' })
    const log = await startShop(hub)
    const builtins = [
      'apps.list', 'apps.select', 'apps.overview', 'apps.tools', 'apps.activate', 'apps.release', 'apps.lock', 'apps.unlock',
      'apps.calls', 'apps.cancel',
    ]
    expect(hub.tools().map((t) => t.name)).toEqual(builtins)
    expect(toAnthropicTools(hub, { session: 'c1' }).map((t) => t.name)).toHaveLength(builtins.length)

    const [r] = await handleAnthropicToolUses(
      hub,
      [{ type: 'tool_use', id: 'tu1', name: 'apps__tools', input: { appId: 'shop' } }],
      { session: 'c1' },
    )
    expect(r?.is_error).toBeFalsy()
    const listed = JSON.parse(String(r?.content)) as { tools: { name: string; inputSchema: unknown }[] }
    expect(listed.tools.find((t) => t.name === 'shop.cart.add')?.inputSchema).toMatchObject({ required: ['sku', 'qty'] })

    // 会话 c1 的导出包含 shop；默认会话不包含
    expect(toAnthropicTools(hub, { session: 'c1' }).length).toBe(builtins.length + 3)
    expect(hub.tools({ session: 'c1' }).map((t) => t.name)).toContain('shop.cart.add')
    expect(hub.tools().map((t) => t.name)).toEqual(builtins)

    // 未列出的工具按导出名仍可调用
    const [r2] = await handleAnthropicToolUses(hub, [
      { type: 'tool_use', id: 'tu2', name: toAnthropicTools(hub, { session: 'c1' }).find((t) => t.name.includes('cart') && t.name.endsWith('add'))!.name, input: { sku: 'a', qty: 1 } },
    ])
    expect(r2?.is_error).toBeFalsy()
    expect(log.calls).toEqual(['cart.add:a'])
    expect(hub.tools().map((t) => t.name)).toContain('shop.cart.add')
  })

  it('列工具、App、资源与事件', async () => {
    const { hub, events } = await startHub()
    expect(hub.listenAddr).toMatch(/^127\.0\.0\.1:\d+$/)
    await startShop(hub)

    const tools = hub.tools()
    const add = tools.find((t) => t.name === 'shop.cart.add')
    expect(add).toMatchObject({
      appId: 'shop',
      tool: 'cart.add',
      description: '加入购物车',
      risk: 'write',
      availability: 'available',
    })
    expect(add?.inputSchema).toMatchObject({ type: 'object', required: ['sku', 'qty'] })
    expect(tools.some((t) => t.name === 'apps.list')).toBe(true)

    const filtered = hub.tools({ maxRisk: 'write', includeBuiltin: false }).map((t) => t.name)
    expect(filtered).toEqual(['shop.cart.add'])

    const shop = hub.apps().find((a) => a.appId === 'shop')
    expect(shop).toMatchObject({ kind: 'app', connected: true, name: '测试商店', summary: '测试用商店 App' })
    expect(shop?.instances[0]).toMatchObject({ clientKind: 'native', visibility: 'visible' })

    await until(() => hub.resources().length > 0, '资源登记')
    expect(hub.resources()).toEqual([
      expect.objectContaining({ uri: 'app-mcp://shop/cart', name: 'shop.cart', available: true }),
    ])
    expect(hub.overview('shop')).toMatchObject({ appId: 'shop', summary: '测试用商店 App', source: 'runtime' })
    expect(hub.overview('nope')).toBeNull()

    await until(() => events.some((e) => e.type === 'toolsChanged'), 'toolsChanged 事件')
    expect(events).toContainEqual({ type: 'appConnected', appId: 'shop', instanceId: shop!.instances[0]!.instanceId })

    // 断开 → appDisconnected
    apps.splice(0).forEach((a) => a.dispose())
    await until(() => events.some((e) => e.type === 'appDisconnected' && e.appId === 'shop'), 'appDisconnected 事件')
  })

  it('callTool 的 onProgress：App 报告的进度经 Hub 逐条到达，先于结果', async () => {
    const { hub } = await startHub({ progressIntervalMs: 0 })
    const app = createAppMcp({ appId: 'job', appName: '任务', hostUrl: hub.wsUrl!, autoStart: false, keepAlive: false })
    apps.push(app)
    app.tool('run', {
      description: '分三步执行',
      risk: 'read',
      handler: async (_input, ctx) => {
        for (const step of [1, 2, 3]) {
          ctx.progress(step, 3, `第 ${step} 步`)
          await new Promise((r) => setTimeout(r, 30))
        }
        return { done: true }
      },
    })
    app.start()
    await until(() => hub.tools({ apps: ['job'], onlyAvailable: true }).some((t) => t.name === 'job.run'), 'job 工具登记')
    const seen: { progress: number; total?: number | null; message?: string | null }[] = []
    const out = await hub.callTool({ name: 'job.run' }, { onProgress: (p) => seen.push(p) })
    expect(out.result).toEqual({ ok: { done: true } })
    expect(seen.map((p) => p.progress)).toEqual([1, 2, 3])
    expect(seen[0]).toMatchObject({ total: 3, message: '第 1 步' })
    // 不请求进度时照常调用
    expect((await hub.callTool({ name: 'job.run' })).result).toEqual({ ok: { done: true } })
  })

  it('callTool：结果、stateHints、按会话首次附带总览、错误', async () => {
    const { hub } = await startHub()
    const log = await startShop(hub)

    const first = await hub.callTool({ name: 'shop.cart.add', arguments: { sku: 'A1', qty: 2 }, session: 's1' })
    expect(first.result).toEqual({ ok: { count: 2 } })
    expect(first.stateHints).toEqual(['cart'])
    expect(first.instanceId).toBeTruthy()
    expect(first.overview?.text).toContain('测试用商店 App')

    const second = await hub.callTool({ name: 'shop.cart.add', arguments: { sku: 'B', qty: 1 }, session: 's1' })
    expect(second.result.ok).toEqual({ count: 3 })
    expect(second.overview).toBeNull()
    const otherSession = await hub.callTool({ name: 'shop.cart.add', arguments: { sku: 'C', qty: 1 }, session: 's2' })
    expect(otherSession.overview).not.toBeNull()

    const invalid = await hub.callTool({ name: 'shop.cart.add', arguments: { sku: 1 } })
    expect(invalid.result.error?.kind).toBe('INVALID_INPUT')

    await expect(hub.callTool({ name: 'nope.tool' })).rejects.toSatisfy(
      (e: unknown) => e instanceof HubError && e.kind === 'TOOL_NOT_FOUND' && e.code === 'TOOL_NOT_FOUND',
    )

    const content = await hub.readResource('app-mcp://shop/cart')
    expect(content.uri).toBe('app-mcp://shop/cart')
    expect(JSON.parse(content.text!)).toEqual({ count: 4 })
    await expect(hub.readResource('app-mcp://nope/x')).rejects.toBeInstanceOf(HubError)
    expect(log.calls).toEqual(['cart.add:A1', 'cart.add:B', 'cart.add:C'])
  })

  it('exportTools + dispatch：openai-chat', async () => {
    const { hub } = await startHub()
    await startShop(hub)

    const tools = toOpenAiTools(hub, { includeBuiltin: false })
    expect(tools).toEqual(hub.exportTools('openai-chat', { includeBuiltin: false }))
    const add = tools.find((t) => t.function.name === 'shop__cart__add')
    expect(add).toMatchObject({ type: 'function', function: { description: '加入购物车' } })
    expect(tools.find((t) => t.function.name === 'shop__order__pay')?.function.description).toContain('payment')

    const msg = await hub.dispatch('openai-chat', {
      id: 'call_1',
      type: 'function',
      function: { name: 'shop__cart__add', arguments: JSON.stringify({ sku: 'A', qty: 1 }) },
    })
    expect(msg).toMatchObject({ role: 'tool', tool_call_id: 'call_1' })
    expect(msg.content).toContain('"count":1')

    const results = await handleOpenAiToolCalls(hub, [
      { id: 'c2', type: 'function', function: { name: 'shop__cart__add', arguments: '{"sku":"B","qty":2}' } },
      { id: 'c3', type: 'function', function: { name: 'shop__cart__add', arguments: '{}' } },
    ])
    expect(results.map((r) => r.tool_call_id)).toEqual(['c2', 'c3'])
    expect(results[0]?.content).toContain('"count":3')
    expect(results[1]?.content).toMatch(/^INVALID_INPUT: /)
    expect(await handleOpenAiToolCalls(hub, undefined)).toEqual([])
  })

  it('exportTools + dispatch：anthropic 与 Vercel AI 适配', async () => {
    const { hub } = await startHub()
    await startShop(hub)

    const tools = toAnthropicTools(hub, { apps: ['shop'], maxRisk: 'write', includeBuiltin: false })
    expect(tools).toEqual([
      expect.objectContaining({ name: 'shop__cart__add', input_schema: expect.objectContaining({ type: 'object' }) }),
    ])

    const results = await handleAnthropicToolUses(
      hub,
      [
        { type: 'text', text: '好的，我来加购物车' },
        { type: 'tool_use', id: 'toolu_1', name: 'shop__cart__add', input: { sku: 'A', qty: 5 } },
        { type: 'tool_use', id: 'toolu_2', name: 'shop.nope', input: {} },
      ],
      { session: 'conv-1', sequential: true },
    )
    expect(results).toHaveLength(2)
    expect(results[0]).toMatchObject({ type: 'tool_result', tool_use_id: 'toolu_1' })
    expect(results[0]?.is_error).toBeUndefined()
    expect(results[0]?.content).toContain('"count":5')
    expect(results[0]?.content).toContain('app-overview') // 该会话首次接触 shop
    expect(results[1]).toMatchObject({ tool_use_id: 'toolu_2', is_error: true })

    // Vercel AI：jsonSchema 包装 + execute
    const wrapped: unknown[] = []
    const vtools = toVercelAiTools(hub, { includeBuiltin: false }, {
      session: 'conv-1',
      jsonSchema: (s) => {
        wrapped.push(s)
        return { wrapped: s }
      },
    })
    expect(Object.keys(vtools).sort()).toEqual(['shop__cart__add', 'shop__cart__clear', 'shop__order__pay'])
    expect(wrapped).toHaveLength(3)
    const text = await vtools.shop__cart__add!.execute({ sku: 'Z', qty: 1 }, { toolCallId: 'v1' })
    expect(text).toContain('"count":6')
    await expect(vtools.shop__cart__add!.execute({ sku: 'Z' })).rejects.toBeInstanceOf(HubToolCallError)
  })

  it('审批：拒绝 → USER_REJECTED；JS 回调返回 Promise；抛错视为拒绝；低于阈值不询问', async () => {
    const { hub } = await startHub({ approval: { requireAtOrAbove: 'destructive' } })
    const log = await startShop(hub)

    // 未设置回调 → 拒绝
    const noHandler = await hub.callTool({ name: 'shop.cart.clear' })
    expect(noHandler.result.error?.kind).toBe('USER_REJECTED')

    const asked: ApprovalRequest[] = []
    let answer: 'yes' | 'no' | 'throw' | 'reject' = 'no'
    hub.setApprovalHandler(async (req) => {
      asked.push(req)
      await new Promise((r) => setTimeout(r, 30)) // 真正异步：Rust 侧须等待 Promise
      if (answer === 'throw') throw new Error('UI 崩了')
      if (answer === 'reject') return Promise.reject(new Error('reject'))
      return answer === 'yes'
    })

    const rejected = await hub.callTool({ name: 'shop.cart.clear', arguments: {}, session: 'conv-9' })
    expect(rejected.result.error?.kind).toBe('USER_REJECTED')
    expect(asked[0]).toMatchObject({
      appId: 'shop',
      appName: '测试商店',
      tool: 'cart.clear',
      description: '清空购物车',
      risk: 'destructive',
      // 未声明注解：按 risk 推导
      annotations: { readOnlyHint: false, destructiveHint: true },
      session: 'conv-9',
      callId: rejected.callId,
    })
    // principal / clientName 只在 MCP 出口发起的审批中出现
    expect(asked[0]).not.toHaveProperty('principal')
    expect(asked[0]).not.toHaveProperty('clientName')

    answer = 'yes'
    const approved = await hub.callTool({ name: 'shop.order.pay' })
    expect(approved.result).toEqual({ ok: { paid: true } })

    answer = 'throw'
    expect((await hub.callTool({ name: 'shop.order.pay' })).result.error?.kind).toBe('USER_REJECTED')
    answer = 'reject'
    const viaDispatch = await hub.dispatch('anthropic', { type: 'tool_use', id: 't9', name: 'shop__cart__clear', input: {} })
    expect(viaDispatch.is_error).toBe(true)
    expect(viaDispatch.content).toMatch(/^USER_REJECTED: /)

    // 同步返回 true 也可以
    hub.setApprovalHandler(() => true)
    expect((await hub.callTool({ name: 'shop.cart.clear' })).result.ok).toEqual({ cleared: true })

    const before = asked.length
    await hub.callTool({ name: 'shop.cart.add', arguments: { sku: 'x', qty: 1 } }) // write < destructive
    expect(asked.length).toBe(before)
    expect(log.calls).toEqual(['order.pay', 'cart.clear', 'cart.add:x'])
  })

  it('配对回调：返回 Promise<true> 后 App 连上', async () => {
    const { hub } = await startHub()
    const asked: PairingRequest[] = []
    hub.setPairingHandler(async (req) => {
      asked.push(req)
      await new Promise((r) => setTimeout(r, 20))
      return true
    })
    await startShop(hub)
    expect(asked).toEqual([expect.objectContaining({ appId: 'shop', appName: '测试商店', clientKind: 'native' })])
  })

  it('休眠：列出 dormant → 调用触发自定义 Waker → App handleWake 后调用成功', async () => {
    const { hub, events } = await startHub({ leaseTtlMs: 0, wakeTimeoutMs: 10_000 })
    const app = createAppMcp({
      appId: 'sleepy',
      appName: '会睡觉的 App',
      instanceId: 's1',
      hostUrl: hub.wsUrl!,
      autoStart: false,
      keepAlive: false,
      lifecycle: {
        mode: 'idle',
        idleTimeoutMs: 300,
        wake: { kind: 'android-intent', target: 'dev.example/.WakeReceiver', background: true },
      },
    })
    apps.push(app)
    app.tool('ping', { description: '回显', handler: (args: unknown) => ({ echo: args }) })
    const wakes: WakeRequest[] = []
    hub.setWaker(async (req) => {
      wakes.push(req)
      // 厂商在这里发送广播 / 打开 URI；测试中直接让同进程 App 处理激活参数。
      if (!app.handleWake(req.activationArg)) throw new HubError('LAUNCH_FAILED', '不认识的激活参数')
    })
    app.start()

    const dormant = await hub.waitForEvent((e) => e.type === 'appDormant' && e.appId === 'sleepy')
    expect(dormant).toEqual({ type: 'appDormant', appId: 'sleepy', instanceId: 's1' })
    const info = hub.apps().find((a) => a.appId === 'sleepy')!
    expect(info.connected).toBe(false)
    expect(info.dormantInstances.map((i) => i.instanceId)).toEqual(['s1'])
    expect(hub.tools({ apps: ['sleepy'], includeBuiltin: false })).toEqual([
      expect.objectContaining({ name: 'sleepy.ping', availability: 'dormant' }),
    ])
    expect(hub.tools({ apps: ['sleepy'], includeBuiltin: false, onlyAvailable: true })).toEqual([])

    const out = await hub.callTool({ name: 'sleepy.ping', arguments: { x: 1 } })
    expect(out.result.ok).toEqual({ echo: { x: 1 } })
    expect(out.instanceId).toBe('s1')
    expect(wakes).toHaveLength(1)
    expect(wakes[0]).toMatchObject({
      appId: 'sleepy',
      instanceId: 's1',
      descriptor: { kind: 'android-intent', target: 'dev.example/.WakeReceiver', background: true },
    })
    expect(wakes[0]!.activationArg).toBe(`app-mcp-wake:${wakes[0]!.token}`)
    expect(events).toContainEqual({ type: 'appWaking', appId: 'sleepy', instanceId: 's1' })

    // Waker 失败：协议类别透传
    await hub.waitForEvent((e) => e.type === 'appDormant' && e.appId === 'sleepy')
    hub.setWaker(() => Promise.reject(new HubError('APP_NOT_INSTALLED', '没装')))
    const failed = await hub.callTool({ name: 'sleepy.ping' })
    expect(failed.result.error).toMatchObject({ kind: 'APP_NOT_INSTALLED' })
    hub.setWaker(null)
  })

  it('原生 App 经本地 IPC 连接，实例带进程号', async () => {
    const endpoint =
      process.platform === 'win32'
        ? `pipe:\\\\.\\pipe\\app-mcp-hub-ts-test-${process.pid}`
        : `unix:${join(tmpdir(), `app-mcp-hub-ts-ipc-${process.pid}`, 'hub.sock')}`
    const { hub } = await startHub({ listen: null, ipcEndpoint: endpoint })
    expect(hub.ipcEndpoint).toBe(endpoint)
    const app = createAppMcp({
      appId: 'notes',
      appName: '笔记',
      hostUrl: hub.ipcEndpoint!,
      autoStart: false,
      keepAlive: false,
    })
    apps.push(app)
    app.tool('add', { description: '添加', handler: (args: unknown) => ({ saved: args }) })
    app.start()
    const inst = await until(() => hub.apps().find((a) => a.appId === 'notes')?.instances[0], 'App 经 IPC 连上')
    expect(inst.pid).toBe(process.pid)
    const out = await hub.callTool({ name: 'notes.add', arguments: { text: 'x' } })
    expect(out.result.ok).toEqual({ saved: { text: 'x' } })
  })

  it('status：运行状态与实例连接 ID（与 apps() 一致）', async () => {
    const { hub } = await startHub()
    const before = hub.status()
    expect(before).toMatchObject({
      service: 'app-mcp',
      listen: hub.listenAddr,
      mcpSessions: 0,
      mcpListenStreams: 0,
      auth: { tokenConfigured: false, tokenRequiredWithoutOrigin: false },
      apps: [],
      reports: [],
      tasks: [],
    })
    expect(before.pid).toBe(process.pid)
    expect(before.startedAtMs).toBeGreaterThan(0)
    expect(before.ipcEndpoint).toBeUndefined()
    expect(before.lease).toMatchObject({ mode: 'adaptive', defaultMs: 60000, maxMs: 60000, window: 20, pairs: [] })
    expect(before.dormantStore).toBeUndefined()

    await startShop(hub)
    const shop = hub.status().apps.find((a) => a.appId === 'shop')
    expect(shop).toMatchObject({ kind: 'app', state: 'connected', name: '测试商店' })
    const inst = shop!.instances[0]!
    expect(inst.state).toBe('connected')
    expect(inst.connectionId).toMatch(/^[0-9a-f]+-\d+$/)
    expect(hub.apps().find((a) => a.appId === 'shop')?.instances[0]?.connectionId).toBe(inst.connectionId)

    // Hub API 会话的 Agent 任务（spec/hub-api.md 3.6）
    await hub.callTool({ name: 'shop.cart.add', arguments: { sku: 'x', qty: 1 }, session: 'conv-1' })
    const task = hub.status().tasks?.find((t) => t.caller === 'api:conv-1')
    expect(task).toMatchObject({ kind: 'api', inflight: 0, selections: [] })
    expect(task?.id).toMatch(/^task-[0-9a-f]+$/)
    // 第 16 项 P3：Hub API 的调用记在主体 api 名下
    const api = hub.status().usage?.find((u) => u.subject === 'api')
    expect(api).toMatchObject({ calls: 1, wakes: 0, rateLimited: 0, apps: [{ appId: 'shop', calls: 1 }] })
    expect(api?.agent).toBeUndefined()
    expect(hub.status().agents).toEqual([])
  })

  it('无会话 MCP 请求的配置：合法取值透传，非法取值启动失败', async () => {
    const { hub } = await startHub({
      taskIdleTtlMs: 0,
      statelessToolExposure: 'progressive',
      principalSelectTtlMs: 1500,
      statelessListTtlMs: 750,
    })
    expect(hub.status().tasks).toEqual([])
    const bad = [
      { taskIdleTtlMs: -1 },
      { principalSelectTtlMs: 1.5 },
      { statelessListTtlMs: -5 },
      { statelessToolExposure: 'some' as unknown as ToolExposure },
    ]
    for (const options of bad) {
      await expect(Hub.start({ listen: null, ipcEndpoint: null, ...options })).rejects.toBeInstanceOf(HubError)
    }
  })

  it('MCP 出口协议版本与 listen 上限：合法取值透传，非法取值启动失败', async () => {
    const { hub } = await startHub({ mcpProtocolMode: 'legacyOnly', maxListenStreams: 0, maxListenResources: 8 })
    expect(hub.status().mcpListenStreams).toBe(0)
    const bad = [
      { mcpProtocolMode: 'modern' as unknown as McpProtocolMode },
      { maxListenStreams: -1 },
      { maxListenResources: 1.5 },
    ]
    for (const options of bad) {
      await expect(Hub.start({ listen: null, ipcEndpoint: null, ...options })).rejects.toBeInstanceOf(HubError)
    }
  })

  it('任务句柄上限：0 时无会话列表不含 apps.task.*，非法取值启动失败', async () => {
    // 无会话（MCP 2026-07-28）列表：缺省列出 apps.task.*，0 时不列出（spec/hub-api.md 3.6「任务句柄」）
    const modernToolNames = async (hub: Hub): Promise<string[]> => {
      const res = await fetch(`http://${hub.listenAddr}/mcp`, {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          accept: 'application/json, text/event-stream',
          'mcp-protocol-version': '2026-07-28',
          'mcp-method': 'tools/list',
        },
        body: JSON.stringify({
          jsonrpc: '2.0',
          id: 1,
          method: 'tools/list',
          params: {
            _meta: {
              'io.modelcontextprotocol/protocolVersion': '2026-07-28',
              'io.modelcontextprotocol/clientCapabilities': {},
              'io.modelcontextprotocol/clientInfo': { name: 'hub-test', version: '0' },
            },
          },
        }),
      })
      const text = await res.text()
      expect(res.status, text).toBe(200)
      const json = text.trimStart().startsWith('{')
        ? text
        : text.split('\n').filter((l) => l.startsWith('data:')).map((l) => l.slice(5).trim()).find((d) => d.includes('"id":1'))
      const msg = JSON.parse(json ?? '{}') as { result?: { tools: Array<{ name: string }> } }
      expect(msg.result, text).toBeDefined()
      return msg.result!.tools.map((t) => t.name)
    }
    const { hub: defaults } = await startHub({ mcpHttp: true })
    expect(await modernToolNames(defaults)).toContain('apps.task.begin')
    const { hub: disabled } = await startHub({ mcpHttp: true, maxTaskHandles: 0 })
    const names = await modernToolNames(disabled)
    expect(names).toContain('apps.list')
    expect(names).not.toContain('apps.task.begin')
    expect(names).not.toContain('apps.task.end')
    await startHub({ maxTaskHandles: 5 })
    for (const maxTaskHandles of [-1, 1.5, '8' as unknown as number]) {
      await expect(Hub.start({ listen: null, ipcEndpoint: null, maxTaskHandles })).rejects.toBeInstanceOf(HubError)
    }
  })

  it('stateDir：启动时读回休眠记录目录，问题文件记入 status().dormantStore.issues', async () => {
    const stateDir = mkdtempSync(join(tmpdir(), 'app-mcp-hub-ts-state-'))
    try {
      mkdirSync(join(stateDir, 'dormant'))
      writeFileSync(join(stateDir, 'dormant', 'broken.json'), '{')
      const { hub } = await startHub({ stateDir })
      const store = hub.status().dormantStore
      expect(store).toMatchObject({ dir: join(stateDir, 'dormant'), loadedInstances: 0, expiredInstances: 0, writes: 0 })
      expect(store?.issues).toHaveLength(1)
      expect(store?.issues[0]?.file).toBe('broken.json')
      expect(store?.lastError).toBeUndefined()
    } finally {
      for (const hub of hubs.splice(0)) await hub.shutdown()
      rmSync(stateDir, { recursive: true, force: true })
    }
  })

  it('注解、outputSchema 与结构化结果经 Hub 原样到达；status 列出工具声明', async () => {
    const { hub } = await startHub()
    const app = createAppMcp({ appId: 'orders', appName: '订单', hostUrl: hub.wsUrl!, autoStart: false, keepAlive: false })
    apps.push(app)
    app.tool('order.submit', {
      description: '提交订单',
      risk: 'payment',
      annotations: { idempotentHint: false, openWorldHint: true, title: '提交' },
      outputSchema: { type: 'object', properties: { orderId: { type: 'string' } } },
      handler: () => ({
        data: { orderId: 'o1' },
        status: 'pending' as const,
        stateResource: 'order.state',
        summary: '已提交，等待用户付款',
        annotations: { audience: ['user' as const], priority: 0.9 },
      }),
    })
    app.tool('order.list', { description: '订单列表', risk: 'read', handler: () => [] })
    app.resource('order.state', { description: '订单状态', read: () => ({ paid: false }) })
    app.start()
    const tools = await until(() => {
      const list = hub.tools({ apps: ['orders'], onlyAvailable: true, includeBuiltin: false })
      return list.length === 2 ? list : undefined
    }, 'orders 工具登记')
    const submit = tools.find((t) => t.name === 'orders.order.submit')!
    expect(submit.annotations).toEqual({
      title: '提交',
      readOnlyHint: false,
      destructiveHint: true,
      idempotentHint: false,
      openWorldHint: true,
    })
    expect(submit.outputSchema).toEqual({ type: 'object', properties: { orderId: { type: 'string' } } })
    const list = tools.find((t) => t.name === 'orders.order.list')!
    expect(list.annotations).toEqual({ readOnlyHint: true })
    expect(list.outputSchema).toBeUndefined()
    // 导出名可能经过转写（如 `.` → `_`），按描述查找
    expect(hub.exportTools('mcp').find((t) => t.description.includes('提交订单'))?.outputSchema).toMatchObject({
      type: 'object',
    })

    const out = await hub.callTool({ name: 'orders.order.submit' })
    expect(out.result.ok).toEqual({ orderId: 'o1' })
    expect(out).toMatchObject({
      status: 'pending',
      stateResource: 'app-mcp://orders/order.state',
      summary: '已提交，等待用户付款',
      annotations: { audience: ['user'], priority: 0.9 },
    })
    const plain = await hub.callTool({ name: 'orders.order.list' })
    expect(plain.status).toBe('done')
    expect(plain.summary).toBeUndefined()

    const declared = hub.status().apps.find((a) => a.appId === 'orders')?.tools ?? []
    expect(declared.find((t) => t.name === 'order.submit')).toMatchObject({
      risk: 'payment',
      annotations: { idempotentHint: false, openWorldHint: true, title: '提交' },
      effective: { destructiveHint: true, readOnlyHint: false },
      outputSchema: true,
    })
    expect(declared.find((t) => t.name === 'order.list')).toMatchObject({ risk: 'read', outputSchema: false })
    expect(declared.find((t) => t.name === 'order.list')).not.toHaveProperty('annotations')
  })

  it('surface / page、idempotencyKey 原样转交、navigateTimeoutMs 配置与新内置工具（spec/hub-api.md 3.14 / 3.15）', async () => {
    const { hub } = await startHub({ navigateTimeoutMs: 800 })
    const app = createAppMcp({ appId: 'cafe', appName: '咖啡', hostUrl: hub.wsUrl!, autoStart: false, keepAlive: false })
    apps.push(app)
    const key = (_: unknown, ctx: { idempotencyKey?: string }) => ({ key: ctx.idempotencyKey ?? null })
    app.tool('cart.checkout', { description: '结算', surface: 'view', page: 'cart', handler: key })
    app.tool('order.submit', { description: '下单', handler: key })
    app.start()
    const tools = await until(() => {
      const list = hub.tools({ apps: ['cafe'], onlyAvailable: true, includeBuiltin: false })
      return list.length === 2 ? list : undefined
    }, 'cafe 工具登记')
    expect(tools.find((t) => t.tool === 'cart.checkout')).toMatchObject({ surface: 'view', page: 'cart' })
    const submit = tools.find((t) => t.tool === 'order.submit')!
    expect(submit.surface).toBe('app')
    expect(submit).not.toHaveProperty('page')

    const all = hub.tools()
    for (const n of ['apps.list', 'apps.select', 'apps.overview', 'apps.activate', 'apps.release', 'apps.page', 'apps.navigate']) {
      const t = all.find((x) => x.name === n)
      expect(t, n).toBeDefined()
      expect(t).not.toHaveProperty('surface')
      expect(t).not.toHaveProperty('page')
    }

    const out = await hub.callTool({ name: 'cafe.order.submit', idempotencyKey: 'order-7' })
    expect(out.result.ok).toEqual({ key: 'order-7' })
    expect(out).not.toHaveProperty('routedTo')
    // 调用元信息（第 19 项 R4）：App 已连接，不经过唤醒
    expect(out.woke).toBe(false)
    expect(Number.isInteger(out.durationMs)).toBe(true)
    const none = await hub.callTool({ name: 'cafe.order.submit' })
    expect(none.result.ok).toEqual({ key: null })
    const bad = await hub.callTool({ name: 'cafe.order.submit', idempotencyKey: '' })
    expect(bad.result.error?.kind).toBe('INVALID_INPUT')
  })

  it('priority 经 tools/invoke 到达 App：调用队列先执行交互调用、后执行后台调用（第 16 项 P6，spec/hub-api.md 3.15）', async () => {
    const { hub } = await startHub()
    const app = createAppMcp({ appId: 'jobs', appName: '作业', hostUrl: hub.wsUrl!, autoStart: false, keepAlive: false, maxConcurrentCalls: 1 })
    apps.push(app)
    const started: string[] = []
    app.tool('job.run', {
      description: '执行',
      handler: async (input: { tag?: string; delayMs?: number }) => {
        if (input.tag !== undefined) started.push(input.tag)
        if (input.delayMs) await new Promise((r) => setTimeout(r, input.delayMs))
        return { ok: true }
      },
    })
    app.start()
    await until(() => hub.tools({ apps: ['jobs'], onlyAvailable: true, includeBuiltin: false }).length === 1, 'jobs 工具登记')
    // 预热：首次调用的总览附带等不计入排队顺序
    expect((await hub.callTool({ name: 'jobs.job.run' })).result.ok).toEqual({ ok: true })

    const call = (tag: string, priority: CallPriority, delayMs: number) =>
      hub.callTool({ name: 'jobs.job.run', arguments: { tag, delayMs }, priority })
    const slow = call('slow', 'normal', 600)
    await new Promise((r) => setTimeout(r, 200))
    const background = call('background', 'background', 0)
    await new Promise((r) => setTimeout(r, 50))
    const interactive = call('interactive', 'interactive', 0)
    for (const out of await Promise.all([slow, background, interactive])) expect(out.result.ok).toEqual({ ok: true })
    expect(started).toEqual(['slow', 'interactive', 'background'])

    // 不合法的取值：请求 JSON 无法解析，抛 HubError
    await expect(hub.callTool({ name: 'jobs.job.run', priority: 'urgent' as CallPriority })).rejects.toBeInstanceOf(HubError)
  })

  it('调用对象：进行中的调用出现在 status().calls 与 apps.calls；apps.cancel 后发起方得到 CANCELLED（第 16 项 P5）', async () => {
    const { hub } = await startHub()
    const app = createAppMcp({ appId: 'jobs', appName: '作业', hostUrl: hub.wsUrl!, autoStart: false, keepAlive: false })
    apps.push(app)
    app.tool('job.run', {
      description: '慢作业',
      handler: (_: unknown, { signal }) =>
        new Promise((resolve, reject) => {
          const timer = setTimeout(() => resolve({ ok: true }), 5000)
          signal.addEventListener('abort', () => {
            clearTimeout(timer)
            reject(new Error('aborted'))
          })
        }),
    })
    app.start()
    await until(() => hub.tools({ apps: ['jobs'], onlyAvailable: true, includeBuiltin: false }).length === 1, 'jobs 工具登记')

    const slow = hub.callTool({ name: 'jobs.job.run', callId: 'slow-1', session: 's1' })
    const status = await until(() => hub.status().calls?.find((c) => c.callId === 'slow-1' && c.state === 'running'), 'slow-1 running')
    expect(status).toMatchObject({ name: 'jobs.job.run', caller: 'api:s1', subject: 'api', state: 'running' })
    expect(status.instanceId).toBeTruthy()
    expect(status.elapsedMs).toBeGreaterThanOrEqual(0)

    // 只见自己的调用，且不含 caller
    const listed = await hub.callTool({ name: 'apps.calls', session: 's1' })
    const own = (listed.result.ok as { calls: Array<Record<string, unknown>> }).calls
    expect(own.map((c) => c.callId)).toEqual(['slow-1'])
    expect(own[0]).not.toHaveProperty('caller')
    const other = await hub.callTool({ name: 'apps.calls', session: 's2' })
    expect((other.result.ok as { calls: unknown[] }).calls).toEqual([])
    const foreign = await hub.callTool({ name: 'apps.cancel', arguments: { callId: 'slow-1' }, session: 's2' })
    expect(foreign.result.error?.kind).toBe('TOOL_NOT_FOUND')

    const cancelled = await hub.callTool({ name: 'apps.cancel', arguments: { callId: 'slow-1' }, session: 's1' })
    expect(cancelled.result.ok).toMatchObject({ callId: 'slow-1', cancelled: true })
    expect((await slow).result.error?.kind).toBe('CANCELLED')
    await until(() => !hub.status().calls?.some((c) => c.callId === 'slow-1'), 'slow-1 释放')
  })

  it('navigateTimeoutMs 必须是非负整数', async () => {
    await expect(Hub.start({ listen: null, ipcEndpoint: null, navigateTimeoutMs: -1 })).rejects.toBeInstanceOf(HubError)
  })

  it('资源保护：limits / outputValidation 配置透传，超出时 RATE_LIMITED / PAYLOAD_TOO_LARGE 并计数', async () => {
    const { hub } = await startHub({
      limits: { toolRatePerMinute: 1, toolRateBurst: 1, maxArgumentsBytes: 64 },
      outputValidation: 'reject',
    })
    expect(hub.status()).toMatchObject({
      limits: {
        toolRatePerMinute: 1,
        toolRateBurst: 1,
        appRatePerMinute: 600,
        appRateBurst: 60,
        maxArgumentsBytes: 64,
        maxResultBytes: 4 * 1024 * 1024,
        maxResourceBytes: 4 * 1024 * 1024,
      },
      outputValidation: 'reject',
    })
    await startShop(hub)
    const big = await hub.callTool({ name: 'shop.cart.add', arguments: { sku: 'x'.repeat(100), qty: 1 } })
    expect(big.result.error).toMatchObject({ kind: 'PAYLOAD_TOO_LARGE', details: { part: 'arguments', limitBytes: 64 } })
    const first = await hub.callTool({ name: 'shop.cart.clear' })
    expect(first.result.ok).toEqual({ cleared: true })
    const second = await hub.callTool({ name: 'shop.cart.clear' })
    expect(second.result.error).toMatchObject({ kind: 'RATE_LIMITED', details: { scope: 'tool', tool: 'cart.clear' } })
    expect(hub.status().apps.find((a) => a.appId === 'shop')).toMatchObject({ rateLimited: 1, tooLarge: 1 })

    // 默认值：status 给出全部字段
    const { hub: plain } = await startHub({ listen: null })
    expect(plain.status()).toMatchObject({ limits: { toolRatePerMinute: 120, toolRateBurst: 30 }, outputValidation: 'log' })
  })

  it('策略挂点：hide 不在列表且调用为 TOOL_NOT_FOUND；deny → POLICY_DENIED；setPolicy 不合法时保留旧规则', async () => {
    const { hub } = await startHub({
      policy: {
        rules: [
          { id: 'hide-clear', action: 'hide', app: 'shop', tool: 'cart.clear' },
          { id: 'no-pay', action: 'deny', app: 'shop', tool: 'order.pay' },
        ],
      },
    })
    await startShop(hub)
    const names = () => hub.tools({ apps: ['shop'], includeBuiltin: false }).map((t) => t.name).sort()
    expect(names()).toEqual(['shop.cart.add', 'shop.order.pay'])
    const hidden = await hub.callTool({ name: 'shop.cart.clear' })
    expect(hidden.result.error).toMatchObject({ kind: 'TOOL_NOT_FOUND' })
    const denied = await hub.callTool({ name: 'shop.order.pay' })
    expect(denied.result.error).toMatchObject({
      kind: 'POLICY_DENIED',
      details: { ruleId: 'no-pay', hook: 'call', appId: 'shop', tool: 'order.pay' },
    })
    expect(hub.policy().rules.map((r) => [r.id, r.hits])).toEqual([
      ['hide-clear', 1],
      ['no-pay', 1],
    ])
    expect(hub.status().policy?.rules).toHaveLength(2)

    // hide 不能写 hooks → INVALID_INPUT，旧规则继续生效，原因记入 lastError
    expect(() => hub.setPolicy({ rules: [{ id: 'x', action: 'hide', app: 'shop', hooks: ['call'] }] })).toThrow(
      expect.objectContaining({ kind: 'INVALID_INPUT' }),
    )
    expect(() => hub.setPolicy({ rules: [], bogus: 1 } as never)).toThrow(expect.objectContaining({ kind: 'INVALID_ARG' }))
    expect((await hub.callTool({ name: 'shop.order.pay' })).result.error?.kind).toBe('POLICY_DENIED')
    expect(hub.policy().lastError?.message).toContain('hooks')

    // 清空：恢复原行为
    hub.setPolicy({})
    expect(names()).toEqual(['shop.cart.add', 'shop.cart.clear', 'shop.order.pay'])
    expect((await hub.callTool({ name: 'shop.order.pay' })).result.ok).toEqual({ paid: true })
    expect(hub.policy()).toMatchObject({ rules: [] })
  })

  it('shutdown 后调用抛 SHUTDOWN', async () => {
    const { hub } = await startHub({ listen: null })
    expect(hub.listenAddr).toBeNull()
    expect(hub.wsUrl).toBeNull()
    expect(hub.ipcEndpoint).toBeNull()
    await hub.shutdown()
    await hub.shutdown()
    expect(hub.isShutdown).toBe(true)
    expect(() => hub.tools()).toThrow(expect.objectContaining({ kind: 'SHUTDOWN' }))
    expect(() => hub.status()).toThrow(expect.objectContaining({ kind: 'SHUTDOWN' }))
    await expect(hub.callTool({ name: 'a.b' })).rejects.toMatchObject({ kind: 'SHUTDOWN' })
  })

  it('配置不合法 → INVALID_ARG', async () => {
    await expect(Hub.start({ keepAlive: false, bogus: 1 } as HubStartOptions)).rejects.toMatchObject({
      kind: 'INVALID_ARG',
    })
    await expect(Hub.start({ keepAlive: false, listen: 'not-an-addr', ipcEndpoint: null })).rejects.toMatchObject({
      kind: 'START_FAILED',
    })
    await expect(
      Hub.start({ keepAlive: false, listen: null, ipcEndpoint: null, lease: { minMs: 9000, maxMs: 1000 } }),
    ).rejects.toMatchObject({ kind: 'START_FAILED' })
    await expect(
      Hub.start({ keepAlive: false, listen: null, ipcEndpoint: null, lease: { bogus: 1 } as never }),
    ).rejects.toMatchObject({ kind: 'INVALID_ARG' })
    // 限流 burst = 0（且未关闭限流）→ 启动失败；未知字段 / 非法取值 → INVALID_ARG
    await expect(
      Hub.start({ keepAlive: false, listen: null, ipcEndpoint: null, limits: { appRateBurst: 0 } }),
    ).rejects.toMatchObject({ kind: 'START_FAILED' })
    await expect(
      Hub.start({ keepAlive: false, listen: null, ipcEndpoint: null, limits: { bogus: 1 } as never }),
    ).rejects.toMatchObject({ kind: 'INVALID_ARG' })
    await expect(
      Hub.start({ keepAlive: false, listen: null, ipcEndpoint: null, outputValidation: 'strict' as never }),
    ).rejects.toMatchObject({ kind: 'INVALID_ARG' })
    // 策略规则不合法（通配符不在末尾）→ 启动失败；未知字段 → INVALID_ARG
    await expect(
      Hub.start({ keepAlive: false, listen: null, ipcEndpoint: null, policy: { rules: [{ id: 'x', action: 'hide', app: 'a*b' }] } }),
    ).rejects.toMatchObject({ kind: 'START_FAILED' })
    await expect(
      Hub.start({ keepAlive: false, listen: null, ipcEndpoint: null, policy: { bogus: 1 } as never }),
    ).rejects.toMatchObject({ kind: 'INVALID_ARG' })
  })
})
