/**
 * 结果缓存声明 `cache`（spec/protocol.md 3.6）：封装层透传给原生（假原生模块），范围由原生核心校验（真实原生模块，
 * 未构建时跳过；Hub 侧的命中见 @app-mcp/hub 集成测试）。
 */
import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createAppMcp, type AppMcp } from './index.js'
import { loadNativeBinding, nativeFileName } from './native.js'
import { FakeNativeClient, fakeBinding } from './testing/fake-native.js'

const created: AppMcp[] = []
const handler = (): null => null

function setup() {
  const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const app = createAppMcp({ appId: 'feed', appName: 'Feed', binding: fakeBinding, logger, keepAlive: false })
  created.push(app)
  return { app, native: FakeNativeClient.last as FakeNativeClient }
}

afterEach(() => {
  for (const app of created.splice(0)) app.dispose()
})

describe('cache（假原生模块）', () => {
  it('工具与资源注册时交给原生；未声明不带', () => {
    const { app, native } = setup()
    app.tool('feed.list', { description: '列表', risk: 'read', cache: { ttlMs: 5000, scope: 'shared' }, handler })
    app.tool('plain', { description: '普通', handler })
    app.scope('s').tool('scoped', { description: 'S', risk: 'read', cache: { ttlMs: 10 }, handler })
    app.resource('feed', { description: '订阅', cache: { ttlMs: 30000, scope: 'shared' }, read: () => [] })
    app.resource('cart', { description: '购物车', read: () => [] })
    expect(native.tools.get('feed.list')?.spec).toMatchObject({ cache: { ttlMs: 5000, scope: 'shared' } })
    expect(native.tools.get('plain')?.spec).not.toHaveProperty('cache')
    expect(native.tools.get('scoped')?.spec).toMatchObject({ cache: { ttlMs: 10 } })
    expect(native.resources.get('feed')?.spec).toMatchObject({ cache: { ttlMs: 30000, scope: 'shared' } })
    expect(native.resources.get('cart')?.spec).not.toHaveProperty('cache')
  })

  it('update 整体替换；显式 undefined 清除；未出现时保持', () => {
    const { app, native } = setup()
    const t = app.tool('feed.list', { description: '列表', risk: 'read', cache: { ttlMs: 5000 }, handler })
    t.update({ description: '列表 2' })
    expect(native.tools.get('feed.list')?.spec).toMatchObject({ cache: { ttlMs: 5000 } })
    t.update({ cache: { ttlMs: 1000, scope: 'shared' } })
    expect(native.tools.get('feed.list')?.spec).toMatchObject({ cache: { ttlMs: 1000, scope: 'shared' } })
    t.update({ cache: undefined })
    expect(native.tools.get('feed.list')?.spec).not.toHaveProperty('cache')
  })
})

const pkgDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const nativePath = process.env.APP_MCP_NODE_NATIVE ?? join(pkgDir, 'native', nativeFileName())

describe.skipIf(!existsSync(nativePath))('cache（真实原生模块）', () => {
  function real(): AppMcp {
    const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
    const app = createAppMcp({ appId: 'feed', appName: 'Feed', binding: loadNativeBinding(), logger, autoStart: false })
    created.push(app)
    return app
  }
  const code = (f: () => unknown): unknown => {
    try {
      f()
    } catch (e) {
      return (e as { code?: unknown }).code
    }
    return 'ok'
  }

  it('越界、非整数或未知 scope 注册抛 INVALID_CONFIG；更新越界同样抛错', () => {
    const app = real()
    const read = (cache: unknown) => () =>
      app.tool(`t${Math.random()}`.replace('.', ''), { description: 'x', risk: 'read', cache: cache as never, handler })
    expect(code(read({ ttlMs: 0 }))).toBe('INVALID_CONFIG')
    expect(code(read({ ttlMs: 86_400_001 }))).toBe('INVALID_CONFIG')
    expect(code(read({ ttlMs: 1.5 }))).toBe('INVALID_CONFIG')
    expect(code(read({ ttlMs: 1, scope: 'public' }))).toBe('INVALID_CONFIG')
    expect(code(read({ ttlMs: 86_400_000, scope: 'shared' }))).toBe('ok')
    expect(code(() => app.resource('r0', { description: 'r', cache: { ttlMs: 0 }, read: () => 1 }))).toBe('INVALID_CONFIG')
    expect(code(() => app.resource('r1', { description: 'r', cache: { ttlMs: 1 }, read: () => 1 }))).toBe('ok')
    const t = app.tool('ok', { description: 'x', risk: 'read', cache: { ttlMs: 5 }, handler })
    expect(code(() => t.update({ cache: { ttlMs: 0 } }))).toBe('INVALID_CONFIG')
  })
})
