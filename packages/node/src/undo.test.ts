/**
 * 撤销（spec/protocol.md 3.8）：工具声明 `undoable` 与 handler 结果的 `undo` 经封装层透传给原生（假原生模块）；`undo` 的校验在
 * 原生核心（真实原生模块，未构建时跳过；到达 Host 的形态见一致性用例 result-undo）。
 */
import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createAppMcp, type AppMcp, type AppMcpOptions } from './index.js'
import { loadNativeBinding, nativeFileName } from './native.js'
import { FakeNativeClient, fakeBinding } from './testing/fake-native.js'

const created: AppMcp[] = []
const handler = (): null => null
const undo = { tool: 'todo.remove', arguments: { id: 3 }, label: '删除刚添加的待办' }

function setup() {
  const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const app = createAppMcp({ appId: 'todo', appName: 'Todo', binding: fakeBinding, logger, keepAlive: false })
  created.push(app)
  return { app, native: FakeNativeClient.last as FakeNativeClient, logger }
}

afterEach(() => {
  for (const app of created.splice(0)) app.dispose()
})

describe('undo（假原生模块）', () => {
  it('注册时带 undoable；未声明不带；update 给值替换、显式 undefined 取消、未出现时保持', () => {
    const { app, native } = setup()
    const t = app.tool('todo.add', { description: '添加', undoable: true, handler })
    app.tool('plain', { description: '普通', handler })
    app.scope('s').tool('scoped', { description: 'S', undoable: true, handler })
    expect(native.tools.get('todo.add')?.spec.undoable).toBe(true)
    expect(native.tools.get('plain')?.spec).not.toHaveProperty('undoable')
    expect(native.tools.get('scoped')?.spec.undoable).toBe(true)
    t.update({ description: '添加 2' })
    expect(native.tools.get('todo.add')?.spec.undoable).toBe(true)
    t.update({ undoable: false })
    expect(native.tools.get('todo.add')?.spec.undoable).toBe(false)
    t.update({ undoable: undefined })
    expect(native.tools.get('todo.add')?.spec).not.toHaveProperty('undoable')
  })

  it('handler 结果的 undo 经 completeWith 交给原生（参数序列化为 JSON 文本）；内容不在 JS 层校验', async () => {
    const { app, native, logger } = setup()
    app.tool('todo.add', { description: '添加', handler: () => ({ data: { id: 3 }, summary: '已添加', undo }) })
    app.tool('toggle', { description: '开关', handler: () => ({ data: true, undo: { tool: 'toggle' } }) })
    app.tool('bad', { description: '坏', handler: () => ({ data: 1, undo: { tool: 'bad name', arguments: [1] as never } }) })
    app.tool<unknown, unknown>('plain', { description: 'p', handler: () => ({ data: 1, undo: 'todo.remove' }) })
    expect(await native.call('todo.add')).toEqual({
      ok: true,
      data: { id: 3 },
      stateHints: [],
      summary: '已添加',
      undo: { tool: 'todo.remove', argumentsJson: '{"id":3}', label: '删除刚添加的待办' },
    })
    expect(await native.call('toggle')).toEqual({ ok: true, data: true, stateHints: [], undo: { tool: 'toggle' } })
    expect(await native.call('bad')).toEqual({ ok: true, data: 1, stateHints: [], undo: { tool: 'bad name', argumentsJson: '[1]' } })
    // undo 不是对象：不视为信封，整体作为 data（与其他信封字段同一规则）
    expect(await native.call('plain')).toEqual({ ok: true, data: { data: 1, undo: 'todo.remove' }, stateHints: [] })
    expect(logger.error).not.toHaveBeenCalled()
  })
})

const pkgDir = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const nativePath = process.env.APP_MCP_NODE_NATIVE ?? join(pkgDir, 'native', nativeFileName())

describe.skipIf(!existsSync(nativePath))('undo（真实原生模块）', () => {
  it('undoable 注册 / 更新被原生接受', () => {
    const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
    const options: AppMcpOptions = { appId: 'todo', appName: 'Todo', binding: loadNativeBinding(), logger, autoStart: false }
    const app = createAppMcp(options)
    created.push(app)
    const t = app.tool('todo.add', { description: '添加', undoable: true, handler })
    expect(() => t.update({ undoable: false })).not.toThrow()
    expect(() => t.update({ undoable: undefined })).not.toThrow()
  })
})
