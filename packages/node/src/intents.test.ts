/** 标准意图声明 `implements`（spec/intents.md 第 1、5 节）与原生客户端（假原生模块）的对接；格式校验在原生核心（见 @app-mcp/hub 集成测试）。 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { createAppMcp, type AppMcp } from './index.js'
import { FakeNativeClient, fakeBinding } from './testing/fake-native.js'

const created: AppMcp[] = []

function setup() {
  const logger = { debug: vi.fn(), warn: vi.fn(), error: vi.fn() }
  const app = createAppMcp({ appId: 'mail', appName: 'Mail', binding: fakeBinding, logger, keepAlive: false })
  created.push(app)
  return { app, native: FakeNativeClient.last as FakeNativeClient }
}

afterEach(() => {
  for (const app of created.splice(0)) app.dispose()
})

describe('implements', () => {
  it('注册时交给原生；缺省或空数组不带', () => {
    const { app, native } = setup()
    app.tool('compose.send', { description: '发信', implements: ['message.send@1'], handler: () => null })
    app.tool('plain', { description: '普通', handler: () => null })
    app.tool('empty', { description: '空', implements: [], handler: () => null })
    app.scope('s').tool('scoped', { description: 'S', implements: ['link.open@1'], handler: () => null })
    expect(native.tools.get('compose.send')?.spec).toMatchObject({ implements: ['message.send@1'] })
    expect(native.tools.get('plain')?.spec).not.toHaveProperty('implements')
    expect(native.tools.get('empty')?.spec).not.toHaveProperty('implements')
    expect(native.tools.get('scoped')?.spec).toMatchObject({ implements: ['link.open@1'] })
  })

  it('update 整体替换；显式 undefined 清除；未出现时保持', () => {
    const { app, native } = setup()
    const t = app.tool('compose.send', { description: '发信', implements: ['message.send@1'], handler: () => null })
    t.update({ description: '发信 2' })
    expect(native.tools.get('compose.send')?.spec).toMatchObject({ implements: ['message.send@1'] })
    t.update({ implements: ['message.send@1', 'file.share@1'] })
    expect(native.tools.get('compose.send')?.spec).toMatchObject({ implements: ['message.send@1', 'file.share@1'] })
    t.update({ implements: undefined })
    expect(native.tools.get('compose.send')?.spec).not.toHaveProperty('implements')
  })
})
