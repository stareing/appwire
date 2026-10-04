/**
 * 撤销（第 15 项 X2，spec/hub-api.md 3.23）经 @app-mcp/hub 的 JSON 透传：App（@app-mcp/node）返回 undo → `callTool` 结果的 `undo`；
 * `apps.undo` 结果的 `undoOf`；`tools()` 中声明的 `undoable`；`status().undo`。
 *
 * 前置条件同 integration.test.ts（两个真实原生模块；缺少任一时跳过）。
 */
import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it } from 'vitest'
import { createAppMcp, type AppMcp } from '../../node/src/index.js'
import { nativeFileName as appNativeFileName } from '../../node/src/native.js'
import { Hub } from './index.js'
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

describe.skipIf(!ready)('撤销（嵌入式 Hub + @app-mcp/node）', () => {
  it('undo / undoOf / undoable / status().undo 原样到达', async () => {
    // 不占用本机常驻 Host 的默认 IPC 端点。
    const hub = await Hub.start({ listen: '127.0.0.1:0', ipcEndpoint: null, keepAlive: false, listChangedDebounceMs: 20 })
    hubs.push(hub)
    const app = createAppMcp({ appId: 'todo', appName: 'Todo', hostUrl: hub.wsUrl!, autoStart: false, keepAlive: false })
    apps.push(app)
    const removed: unknown[] = []
    app.tool('add', {
      description: '添加',
      undoable: true,
      handler: () => ({ data: { id: 3 }, undo: { tool: 'remove', arguments: { id: 3 }, label: '删除刚添加的待办' } }),
    })
    app.tool('remove', {
      description: '删除',
      handler: (input) => {
        removed.push(input)
        return { removed: true }
      },
    })
    app.start()
    const tool = (name: string) => hub.tools({ apps: ['todo'], includeBuiltin: false }).find((x) => x.name === `todo.${name}`)
    await until(() => tool('add') && tool('remove'), 'todo 工具登记')
    expect(tool('add')?.undoable).toBe(true)
    expect(tool('remove')).not.toHaveProperty('undoable')
    expect(hub.tools().find((t) => t.name === 'apps.undo')).not.toHaveProperty('undoable')
    expect(hub.status().undo).toEqual({ ttlMs: 30 * 60 * 1000, maxPerTask: 32, records: 0 })

    const added = await hub.callTool({ name: 'todo.add' })
    expect(added.result.ok).toEqual({ id: 3 })
    expect(added.undo).toEqual({ label: '删除刚添加的待办', expiresInMs: expect.any(Number) })
    expect(added.undo!.expiresInMs).toBeGreaterThan(0)
    expect(added.undo!.expiresInMs).toBeLessThanOrEqual(30 * 60 * 1000)
    expect(added).not.toHaveProperty('undoOf')
    expect(hub.status().undo?.records).toBe(1)

    const undone = await hub.callTool({ name: 'apps.undo', arguments: { callId: added.callId } })
    expect(undone.result.ok).toEqual({ removed: true })
    expect(undone.undoOf).toBe(added.callId)
    expect(undone).not.toHaveProperty('undo')
    expect(removed).toEqual([{ id: 3 }])
    expect(hub.status().undo?.records).toBe(0)
    // 只能撤销一次
    const again = await hub.callTool({ name: 'apps.undo', arguments: { callId: added.callId } })
    expect(again.result.error?.kind).toBe('TOOL_NOT_FOUND')
  })

  it('配置 undo：覆盖上限、maxPerTask 0 关闭、开启时 ttlMs 0 启动失败、未知字段启动失败', async () => {
    const base = { listen: '127.0.0.1:0', ipcEndpoint: null, keepAlive: false } as const
    const tuned = await Hub.start({ ...base, undo: { ttlMs: 5000 } })
    hubs.push(tuned)
    expect(tuned.status().undo).toEqual({ ttlMs: 5000, maxPerTask: 32, records: 0 })
    const off = await Hub.start({ ...base, undo: { maxPerTask: 0 } })
    hubs.push(off)
    expect(off.status().undo?.maxPerTask).toBe(0)
    expect(off.tools().some((t) => t.name === 'apps.undo')).toBe(false)
    await expect(Hub.start({ ...base, undo: { ttlMs: 0 } })).rejects.toThrow(/ttlMs/)
    await expect(Hub.start({ ...base, undo: { ttl: 1 } as never })).rejects.toThrow()
  })
})
