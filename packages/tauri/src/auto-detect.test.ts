/**
 * 页面直接用 @app-mcp/web 的 `createAppMcp`（不依赖本包）：插件注入脚本暴露的桥接被自动检测，
 * 工具经 Tauri IPC 登记，不加载 WASM。（单独文件：脚本以 globalThis 为 window，桥接不可删除。）
 */
import { expect, it, vi } from 'vitest'
import { createAppMcp, type RendererOp } from '@app-mcp/web'
import { createFakeTauri } from './fake-tauri'

it('@app-mcp/web 的 createAppMcp 自动改走 Tauri 桥接', async () => {
  const fake = createFakeTauri({ window: globalThis as unknown as Record<string, any> })
  const appMcp = createAppMcp({
    appId: 'shop',
    appName: '示例商城',
    logger: { debug() {}, warn: vi.fn(), error: vi.fn() },
  })
  appMcp.tool('cart.clear', { description: '清空购物车', handler: () => ({ data: 'ok', stateHints: ['cart'] }) })
  const register = (await fake.waitFor((op) => op.op === 'tool.register')) as Extract<RendererOp, { op: 'tool.register' }>
  expect(register.name).toBe('cart.clear')

  fake.emit({ type: 'call', callId: 'c1', toolId: register.id, input: {} })
  await expect(fake.waitFor((op) => op.op === 'call.result')).resolves.toEqual({
    op: 'call.result',
    callId: 'c1',
    ok: true,
    data: 'ok',
    stateHints: ['cart'],
  })
  expect(appMcp.state).toEqual({ status: 'connected' })
  appMcp.dispose()
})
