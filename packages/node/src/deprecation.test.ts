/**
 * 工具弃用声明 `deprecated`（spec/protocol.md 3.7）：封装层透传给原生（假原生模块），格式由原生核心校验（真实原生模块，
 * 未构建时跳过；到达 Host 的形态见一致性用例 tool-deprecated）。
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
const deprecated = { message: '改用 orders.search：支持分页', replacement: 'orders.search', until: '2027-06-30' }

function setup() {
  const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const app = createAppMcp({ appId: 'shop', appName: 'Shop', binding: fakeBinding, logger, keepAlive: false })
  created.push(app)
  return { app, native: FakeNativeClient.last as FakeNativeClient }
}

afterEach(() => {
  for (const app of created.splice(0)) app.dispose()
})

describe('deprecated（假原生模块）', () => {
  it('注册时交给原生（复制）；未声明不带；scope 内同样透传', () => {
    const { app, native } = setup()
    const dep = { ...deprecated }
    app.tool('orders.list', { description: '列表', deprecated: dep, handler })
    app.tool('plain', { description: '普通', handler })
    app.scope('s').tool('scoped', { description: 'S', deprecated: { message: '即将移除' }, handler })
    expect(native.tools.get('orders.list')?.spec.deprecated).toEqual(deprecated)
    expect(native.tools.get('orders.list')?.spec.deprecated).not.toBe(dep)
    expect(native.tools.get('plain')?.spec).not.toHaveProperty('deprecated')
    expect(native.tools.get('scoped')?.spec).toMatchObject({ deprecated: { message: '即将移除' } })
  })

  it('update 整体替换；显式 undefined 清除；未出现时保持', () => {
    const { app, native } = setup()
    const t = app.tool('orders.list', { description: '列表', deprecated, handler })
    t.update({ description: '列表 2' })
    expect(native.tools.get('orders.list')?.spec.deprecated).toEqual(deprecated)
    t.update({ deprecated: { message: '改用 orders.v3' } })
    expect(native.tools.get('orders.list')?.spec.deprecated).toEqual({ message: '改用 orders.v3' })
    t.update({ deprecated: undefined })
    expect(native.tools.get('orders.list')?.spec).not.toHaveProperty('deprecated')
  })
})

const pkgDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const nativePath = process.env.APP_MCP_NODE_NATIVE ?? join(pkgDir, 'native', nativeFileName())

describe.skipIf(!existsSync(nativePath))('deprecated（真实原生模块）', () => {
  function real(): AppMcp {
    const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
    const app = createAppMcp({ appId: 'shop', appName: 'Shop', binding: loadNativeBinding(), logger, autoStart: false })
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

  it('格式不合法（每条规则一例）注册抛 INVALID_CONFIG；更新同样抛错；合法声明照常注册', () => {
    const app = real()
    let n = 0
    const reg = (dep: unknown) => () => app.tool(`t${n++}`, { description: 'x', deprecated: dep as never, handler })
    expect(code(reg({ message: '' }))).toBe('INVALID_CONFIG')
    expect(code(reg({ message: '   ' }))).toBe('INVALID_CONFIG')
    expect(code(reg({ message: 'x'.repeat(501) }))).toBe('INVALID_CONFIG')
    expect(code(reg({ message: 'm', replacement: 'bad name' }))).toBe('INVALID_CONFIG')
    expect(code(() => app.tool('self', { description: 'x', deprecated: { message: 'm', replacement: 'self' }, handler }))).toBe(
      'INVALID_CONFIG',
    )
    expect(code(reg({ message: 'm', until: '2027-02-29' }))).toBe('INVALID_CONFIG')
    expect(code(reg({ message: 'x'.repeat(500), replacement: 'other', until: '2028-02-29' }))).toBe('ok')
    const t = app.tool('ok', { description: 'x', deprecated, handler })
    expect(code(() => t.update({ deprecated: { message: 'm', until: '2027-13-01' } }))).toBe('INVALID_CONFIG')
    expect(code(() => t.update({ deprecated: undefined }))).toBe('ok')
  })
})
