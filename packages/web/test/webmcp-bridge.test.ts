import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installWebMcp, type ModelContext, type WebMcpUninstall } from '../src/webmcp'
import { connected, type Harness, settle } from './fakes'
import { coreTool, defineNative, FakeLegacyModelContext, FakeNativeModelContext, hostCall } from './webmcp-fakes'

const docMc = (): ModelContext => (document as unknown as { modelContext: ModelContext }).modelContext

let h: Harness
let uninstall: WebMcpUninstall | undefined
const cleanup: (() => void)[] = []

beforeEach(async () => {
  sessionStorage.clear()
  localStorage.clear()
  h = await connected()
})

afterEach(() => {
  uninstall?.()
  uninstall = undefined
  for (const f of cleanup.splice(0).reverse()) f()
  h.app.dispose()
})

function installWith<T extends object>(native: T, where: 'document' | 'navigator' = 'document', opts = {}): T {
  cleanup.push(defineNative(where === 'document' ? document : navigator, native))
  uninstall = installWebMcp(h.app, opts)
  return native
}

describe('桥接：原生 document.modelContext（当前标准）', () => {
  it('不替换原生对象，只覆盖实例方法；页面注册同时进入原生与 appMcp', async () => {
    const native = installWith(new FakeNativeModelContext())
    expect(uninstall?.mode).toBe('bridge')
    expect(docMc()).toBe(native)
    // 另一个入口补上别名
    expect((navigator as unknown as { modelContext: unknown }).modelContext).toBe(native)

    const execute = vi.fn(async ({ text }: { text: string }) => ({ content: [{ type: 'text', text: `Added: ${text}` }] }))
    await docMc().registerTool({
      name: 'add_todo',
      description: '添加待办',
      inputSchema: { type: 'object', properties: { text: { type: 'string' } } },
      annotations: { readOnlyHint: true },
      execute,
    })
    await settle()
    expect(native.tools.has('add_todo')).toBe(true)
    expect(native.tools.get('add_todo')?.annotations).toEqual({ readOnlyHint: true })
    expect(coreTool(h, 'add_todo')).toMatchObject({ description: '添加待办', risk: 'read' })

    // Host 调用
    expect(await hostCall(h, 'add_todo', { text: 'a' })).toEqual({ data: { text: 'Added: a' } })
    // 浏览器内置 AI 调用：结果原样，第二个参数补上 requestUserInteraction
    expect(await native.invoke('add_todo', { text: 'b' })).toEqual({ content: [{ type: 'text', text: 'Added: b' }] })
    const client = execute.mock.calls[1]?.[1 as never] as unknown as { signal: AbortSignal; requestUserInteraction: unknown }
    expect(client.signal).toBeInstanceOf(AbortSignal)
    expect(typeof client.requestUserInteraction).toBe('function')
  })

  it('Host 调用时在原生对象上派发 toolactivated', async () => {
    const native = installWith(new FakeNativeModelContext())
    const activated = vi.fn()
    native.addEventListener('toolactivated', activated)
    await docMc().registerTool({ name: 't', description: 'T', execute: () => 1 })
    await settle()
    await hostCall(h, 't')
    expect(activated).toHaveBeenCalledTimes(1)
    expect(activated.mock.calls[0]?.[0]).toMatchObject({ toolName: 't' })
  })

  it('标准写法的 signal 注销：两边同步', async () => {
    const native = installWith(new FakeNativeModelContext())
    const ac = new AbortController()
    await docMc().registerTool({ name: 't', description: 'T', execute: () => 1 }, { signal: ac.signal })
    await settle()
    ac.abort()
    await settle()
    expect(native.tools.has('t')).toBe(false)
    expect(coreTool(h, 't')).toBeUndefined()
  })

  it('原生没有的旧接口 unregisterTool / provideContext / clearContext 由 SDK 补上并同步两边', async () => {
    const native = installWith(new FakeNativeModelContext())
    h.app.tool('own', { description: '自有', handler: () => 1 })
    await docMc().registerTool({ name: 'a', description: 'A', execute: () => 1 })
    docMc().unregisterTool('a')
    await settle()
    expect(native.tools.has('a')).toBe(false)
    expect(coreTool(h, 'a')).toBeUndefined()

    docMc().provideContext({
      tools: [
        { name: 'b', description: 'B', execute: () => 1 },
        { name: 'c', description: 'C', execute: () => 1 },
      ],
    })
    await settle()
    expect([...native.tools.keys()].sort()).toEqual(['b', 'c', 'own'])
    docMc().provideContext({ tools: [{ name: 'd', description: 'D', execute: () => 1 }] })
    await settle()
    expect([...native.tools.keys()].sort()).toEqual(['d', 'own'])
    expect(coreTool(h, 'b')).toBeUndefined()
    expect(coreTool(h, 'd')).toBeDefined()

    docMc().clearContext()
    await settle()
    expect([...native.tools.keys()]).toEqual(['own'])
    expect(coreTool(h, 'd')).toBeUndefined()
    expect(coreTool(h, 'own')).toBeDefined()
  })

  it('原生拒绝（名字已被安装前的原生注册占用）：回滚 SDK 侧，且不误删原生已有工具', async () => {
    const native = new FakeNativeModelContext()
    await native.registerTool({ name: 'taken', description: 'pre', execute: () => 0 })
    installWith(native)
    await expect(docMc().registerTool({ name: 'taken', description: 'x', execute: () => 1 })).rejects.toMatchObject({
      name: 'InvalidStateError',
    })
    await settle()
    expect(coreTool(h, 'taken')).toBeUndefined()
    expect(native.tools.get('taken')?.description).toBe('pre')
  })

  it('同名冲突：appMcp.tool() 已有时标准侧 InvalidStateError', async () => {
    installWith(new FakeNativeModelContext())
    h.app.tool('x', { description: 'own', handler: () => 1 })
    await expect(docMc().registerTool({ name: 'x', description: 'std', execute: () => 2 })).rejects.toMatchObject({
      name: 'InvalidStateError',
    })
  })

  it('同名冲突：标准侧先注册，appMcp.tool() 后注册 → 原生也换成 appMcp 的版本', async () => {
    const native = installWith(new FakeNativeModelContext())
    await docMc().registerTool({ name: 'x', description: 'std', execute: () => 2 })
    await settle()
    h.app.tool('x', { description: 'own', handler: () => 1 })
    await settle()
    expect(native.tools.get('x')?.description).toBe('own')
    expect(await native.invoke('x')).toEqual({ content: [{ type: 'text', text: '1' }] })
    expect(await hostCall(h, 'x')).toEqual({ data: 1 })
  })
})

describe('mirrorOwnTools', () => {
  it('appMcp.tool() 的工具注册到原生（含安装前注册的），结果按标准转换，dispose 时注销', async () => {
    h.app.tool('before', { description: '安装前', risk: 'read', handler: () => 'x' })
    const native = installWith(new FakeNativeModelContext())
    const handler = vi.fn((input: { id: string }, _ctx: unknown) => ({ removed: input.id }))
    const handle = h.app.tool('cart.remove', {
      title: '移除商品',
      description: '从购物车移除',
      risk: 'destructive',
      input: { type: 'object', properties: { id: { type: 'string' } } },
      handler,
    })
    await settle()
    expect(native.tools.get('before')?.annotations).toEqual({ readOnlyHint: true })
    const tool = native.tools.get('cart.remove')
    expect(tool).toMatchObject({
      name: 'cart.remove',
      title: '移除商品',
      description: '从购物车移除',
      inputSchema: { type: 'object', properties: { id: { type: 'string' } } },
      annotations: { readOnlyHint: false, consequentialHint: true, destructiveHint: true },
    })
    expect(await native.invoke('cart.remove', { id: 'p1' })).toEqual({
      content: [{ type: 'text', text: '{"removed":"p1"}' }],
    })
    expect(await native.invoke('before')).toEqual({ content: [{ type: 'text', text: 'x' }] })
    expect(handler.mock.calls[0]?.[1]).toMatchObject({ signal: expect.any(AbortSignal) })

    // 声明的注解覆盖按 risk 推导的值
    h.app.tool('cart.reset', {
      description: '重置',
      risk: 'destructive',
      annotations: { destructiveHint: false, idempotentHint: true },
      handler: () => null,
    })
    await settle()
    expect(native.tools.get('cart.reset')?.annotations).toEqual({
      readOnlyHint: false,
      consequentialHint: true,
      destructiveHint: false,
      idempotentHint: true,
    })

    handle.dispose()
    await settle()
    expect(native.tools.has('cart.remove')).toBe(false)
  })

  it('handler 出错 → isError 结果', async () => {
    const native = installWith(new FakeNativeModelContext())
    h.app.tool('bad', {
      description: 'x',
      handler: () => {
        throw new Error('没库存')
      },
    })
    await settle()
    expect(await native.invoke('bad')).toEqual({ content: [{ type: 'text', text: 'HANDLER_ERROR: 没库存' }], isError: true })
  })

  it('update：描述变化时重新注册；enabled: false 时从原生注销；scope dispose 时注销', async () => {
    const native = installWith(new FakeNativeModelContext())
    const scope = h.app.scope('page')
    const handle = scope.tool('t', { description: 'v1', handler: () => 1 })
    await settle()
    handle.update({ description: 'v2' })
    await settle()
    expect(native.tools.get('t')?.description).toBe('v2')
    handle.update({ enabled: false })
    await settle()
    expect(native.tools.has('t')).toBe(false)
    handle.update({ enabled: true })
    await settle()
    expect(native.tools.has('t')).toBe(true)
    scope.dispose()
    await settle()
    expect(native.tools.has('t')).toBe(false)
  })

  it('标准侧注册的工具不会被重复镜像', async () => {
    const native = installWith(new FakeNativeModelContext())
    await docMc().registerTool({ name: 'std', description: 'S', execute: () => 1 })
    await settle()
    expect(native.log.filter(([m, n]) => m === 'registerTool' && n === 'std')).toHaveLength(1)
  })

  it('mirrorOwnTools: false 时不镜像', async () => {
    const native = installWith(new FakeNativeModelContext(), 'document', { mirrorOwnTools: false })
    h.app.tool('own', { description: '自有', handler: () => 1 })
    await settle()
    expect(native.tools.size).toBe(0)
  })

  it('appMcp.dispose() 时从原生注销', async () => {
    const native = installWith(new FakeNativeModelContext())
    h.app.tool('own', { description: '自有', handler: () => 1 })
    await settle()
    h.app.dispose()
    expect(native.tools.size).toBe(0)
  })
})

describe('桥接：旧版原生 navigator.modelContext', () => {
  it('同步注册、unregisterTool 注销；不调用原生 provideContext / clearContext', async () => {
    const native = installWith(new FakeLegacyModelContext(), 'navigator')
    expect(uninstall?.mode).toBe('bridge')
    // document 入口补上别名（Chrome 已迁移到 document.modelContext）
    expect(docMc()).toBe(native)
    const nav = (navigator as unknown as { modelContext: ModelContext }).modelContext
    await nav.registerTool({ name: 'a', description: 'A', execute: () => 1 })
    h.app.tool('own', { description: '自有', handler: () => 1 })
    await settle()
    expect([...native.tools.keys()].sort()).toEqual(['a', 'own'])
    expect(coreTool(h, 'a')).toBeDefined()

    nav.provideContext({ tools: [{ name: 'b', description: 'B', execute: () => 1 }] })
    await settle()
    expect([...native.tools.keys()].sort()).toEqual(['b', 'own'])
    nav.clearContext()
    await settle()
    expect([...native.tools.keys()]).toEqual(['own'])
    expect(native.log).toContainEqual(['unregisterTool', 'a'])
  })

  it('原生同步抛错时 registerTool 返回 rejected promise 并回滚', async () => {
    const native = new FakeLegacyModelContext()
    native.registerTool({ name: 'taken', description: 'pre', execute: () => 0 })
    installWith(native, 'navigator')
    await expect(docMc().registerTool({ name: 'taken', description: 'x', execute: () => 1 })).rejects.toMatchObject({
      name: 'InvalidStateError',
    })
    expect(coreTool(h, 'taken')).toBeUndefined()
    expect(native.tools.get('taken')?.description).toBe('pre')
  })
})

describe('原生对象不可写', () => {
  it('退化为 native-readonly：只镜像 appMcp.tool() 的工具', async () => {
    const native = Object.freeze(new FakeNativeModelContext())
    // Object.freeze 不冻结 Map 内容，镜像注册仍可进行
    installWith(native)
    expect(uninstall?.mode).toBe('native-readonly')
    expect(h.logger.warn).toHaveBeenCalledWith(expect.stringContaining('无法覆盖'))
    h.app.tool('own', { description: '自有', handler: () => 1 })
    await settle()
    expect(native.tools.has('own')).toBe(true)
    // 页面经原生接口注册：只进入原生
    await docMc().registerTool({ name: 'std', description: 'S', execute: () => 1 })
    await settle()
    expect(coreTool(h, 'std')).toBeUndefined()
    expect(native.tools.has('std')).toBe(true)
  })
})

describe('卸载', () => {
  it('恢复原生方法、注销镜像工具，页面注册到原生的工具保留', async () => {
    const native = installWith(new FakeNativeModelContext())
    h.app.tool('own', { description: '自有', handler: () => 1 })
    const ac = new AbortController()
    await docMc().registerTool({ name: 'std', description: 'S', execute: () => 1 }, { signal: ac.signal })
    await settle()
    uninstall?.()
    uninstall = undefined
    await settle()
    expect(Object.getOwnPropertyNames(native)).not.toContain('registerTool')
    expect(docMc().registerTool).toBe(FakeNativeModelContext.prototype.registerTool)
    expect((docMc() as unknown as Record<string, unknown>).unregisterTool).toBeUndefined()
    expect('modelContext' in navigator).toBe(false)
    expect([...native.tools.keys()]).toEqual(['std'])
    expect(coreTool(h, 'std')).toBeUndefined()
    // 页面的 signal 仍能注销原生注册
    ac.abort()
    expect(native.tools.size).toBe(0)
  })
})
